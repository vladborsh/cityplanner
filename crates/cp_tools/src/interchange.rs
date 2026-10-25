//! The interchange tool (T-332, `system-design.md` §18.1 "Решения T-332" 7, 8): a template of
//! `assets/content/interchanges/` stamped at a point of a one-way highway (carriageway A) as one
//! `NetCommand::BuildBatch`. The anchor is the point of A under the cursor, the axis its lane
//! direction there; carriageway B is the one-way highway beside it that runs the other way.
//!
//! The batch is checked here the way the sim runs it, plan by plan on a copy of the net: a ramp
//! that joins A after an earlier ramp cut it names the piece of A that cut left (its
//! `OnSegment`), and a free end on a node an earlier plan made is that node (`BATCH_JOIN`).

use bevy_math::{Vec2, Vec3};
use cp_building::BuildingId;
use cp_content::{ContentDb, InterchangeDef, LaneDir, PieceElevation, TemplatePoint};
use cp_core::Cost;
use cp_core::curve::to_xz;
use cp_locale::tr;
use cp_net::{
    Anchor, BATCH_JOIN, BuildPlan, Elevation, ElevationMode, NetCommand, NetGraph, NodeId,
    PlanPiece, SegmentId, plan_applied, validate_build_plan,
};
use cp_sim::{SimCommand, SimView};

use crate::road::{road_error_text, sample_curve};

/// Why there is no stamp: A is curved, too short or changes height under the template.
pub const NEEDS_STRAIGHT: &str = "needs a straight highway";
/// Why there is no stamp: no carriageway runs the other way beside A.
pub const NO_SECOND_CARRIAGEWAY: &str = "second carriageway not found";
/// Why there is no stamp: the cursor is not on a one-way highway.
pub const NOT_ON_HIGHWAY: &str = "point at a one-way highway";

/// How far from a carriageway the cursor may be, metres.
pub const ANCHOR_RADIUS: f32 = 12.0;
/// The footprint along A is straight when it stays within this of the axis, degrees.
const STRAIGHT_DEG: f32 = 2.0;
/// B runs the other way when it is within this of the reversed axis, degrees.
const ANTIPARALLEL_DEG: f32 = 5.0;
/// Where B may be, metres to the left of A.
const B_MIN: f32 = 8.0;
const B_MAX: f32 = 48.0;
/// The footprint is probed every this many metres along the axis.
const PROBE_STEP: f32 = 10.0;
/// How far off the probed line a carriageway may be besides the 2° (rounding of its curve), m.
const PROBE_SLACK: f32 = 0.5;
/// The anchor moves along A in steps of this (the preview is computed once per step), metres.
pub const ANCHOR_STEP: f32 = 1.0;

/// State of the interchange tool: the template in hand.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InterchangeTool {
    /// The id of the [`InterchangeDef`].
    pub def: Option<String>,
}

/// The anchor frame of a stamp: `u` along A from `origin` in its direction, `v` to its left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StampFrame {
    pub origin: Vec2,
    /// Unit, A's lane direction.
    pub axis: Vec2,
    /// Distance of B to the left of A (0 for a template without B).
    pub b: f32,
    /// The height class of A under the template.
    pub elevation: Elevation,
}

impl StampFrame {
    /// The unit vector to the left of the axis (`(t.z, -t.x)`, as `CubicBezier3::offset_xz`).
    pub fn left(&self) -> Vec2 {
        Vec2::new(self.axis.y, -self.axis.x)
    }

    /// The plane point at `(u, v)`.
    pub fn at(&self, u: f32, v: f32) -> Vec2 {
        self.origin + self.axis * u + self.left() * v
    }
}

/// What a click with the interchange tool would do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InterchangePreview {
    /// The anchor frame; `None` off a highway.
    pub frame: Option<StampFrame>,
    /// The roads of the stamp as lines (world), one per template piece: the planned segments
    /// at their heights when they check, the template's points on the ground otherwise.
    pub lines: Vec<Vec<Vec3>>,
    pub valid: bool,
    /// Why it cannot be built (one line); `None` when valid.
    pub error: Option<String>,
    /// Price of the whole stamp; `None` when invalid.
    pub cost: Option<Cost>,
    /// The plans a click sends, as one `BuildBatch` (valid only).
    pub plans: Vec<BuildPlan>,
    /// The buildings the stamp would demolish (BUG-096), tinted red.
    pub demolish: Vec<BuildingId>,
}

/// The batch command of a valid preview.
pub fn interchange_command(preview: &InterchangePreview) -> Option<SimCommand> {
    (preview.valid && !preview.plans.is_empty()).then(|| {
        SimCommand::Net(NetCommand::BuildBatch {
            plans: preview.plans.clone(),
        })
    })
}

/// Whether `seg` is a carriageway an interchange stands on: a one-way highway (`highway_1w_*`).
fn is_carriageway(net: &NetGraph, content: &ContentDb, seg: SegmentId) -> bool {
    net.segment(seg)
        .is_some_and(|s| content.road(s.road).id.starts_with("highway_1w_"))
}

/// The unit direction the lanes of `seg` run at `t`, in the plane.
fn lane_direction(net: &NetGraph, seg: SegmentId, t: f32) -> Option<Vec2> {
    let s = net.segment(seg)?;
    let along = to_xz(s.curve.derivative(t)).normalize_or_zero();
    let backward = s
        .lanes
        .iter()
        .filter_map(|l| net.lane(*l))
        .any(|l| l.dir == LaneDir::Backward);
    (along != Vec2::ZERO).then_some(if backward { -along } else { along })
}

/// The carriageway point nearest to `p` within `radius`: `(segment, t, point)`.
fn nearest_carriageway(
    net: &NetGraph,
    content: &ContentDb,
    p: Vec2,
    radius: f32,
) -> Option<(SegmentId, f32, Vec2)> {
    let mut best: Option<(f32, SegmentId, f32)> = None;
    for (id, s) in net.segments() {
        if s.curve.bounds_distance_xz(p) > radius || !is_carriageway(net, content, id) {
            continue;
        }
        let (t, d) = s.curve.nearest_xz(p);
        if d <= radius && best.is_none_or(|(bd, _, _)| d < bd) {
            best = Some((d, id, t));
        }
    }
    let (_, id, t) = best?;
    Some((id, t, to_xz(net.segment(id)?.curve.point(t))))
}

/// Whether a carriageway runs along `dir` (within 2°) through the probes `origin + dir * u` for
/// `u` over `±half`, in one height class: the footprint of the template on it.
fn straight_along(
    net: &NetGraph,
    content: &ContentDb,
    origin: Vec2,
    dir: Vec2,
    half: f32,
    elevation: Elevation,
) -> bool {
    let tan = STRAIGHT_DEG.to_radians().tan();
    let cos = STRAIGHT_DEG.to_radians().cos();
    let n = (half / PROBE_STEP).ceil() as i32;
    (-n..=n).all(|i| {
        let u = (i as f32 * PROBE_STEP).clamp(-half, half);
        let slack = tan * u.abs() + PROBE_SLACK;
        nearest_carriageway(net, content, origin + dir * u, slack).is_some_and(|(seg, t, _)| {
            net.segment(seg).is_some_and(|s| s.elevation == elevation)
                && lane_direction(net, seg, t).is_some_and(|d| d.dot(dir) >= cos)
        })
    })
}

/// The anchor frame of `def` at the cursor `hover`, or why there is none.
pub fn stamp_frame(
    net: &NetGraph,
    content: &ContentDb,
    def: &InterchangeDef,
    hover: Vec2,
) -> Result<StampFrame, &'static str> {
    let (seg, t, origin) =
        nearest_carriageway(net, content, hover, ANCHOR_RADIUS).ok_or(NOT_ON_HIGHWAY)?;
    let axis = lane_direction(net, seg, t).ok_or(NOT_ON_HIGHWAY)?;
    let elevation = net.segment(seg).ok_or(NOT_ON_HIGHWAY)?.elevation;
    let half = def.length * 0.5;
    if !straight_along(net, content, origin, axis, half, elevation) {
        return Err(NEEDS_STRAIGHT);
    }
    let mut frame = StampFrame {
        origin,
        axis,
        b: 0.0,
        elevation,
    };
    if def.needs_second_carriageway {
        frame.b = second_carriageway(net, content, &frame, half).ok_or(NO_SECOND_CARRIAGEWAY)?;
    }
    Ok(frame)
}

/// The distance of B to the left of A: the nearest carriageway across from the anchor that runs
/// the other way and is straight over the footprint.
fn second_carriageway(
    net: &NetGraph,
    content: &ContentDb,
    frame: &StampFrame,
    half: f32,
) -> Option<f32> {
    let cos = ANTIPARALLEL_DEG.to_radians().cos();
    let mut v = B_MIN;
    while v <= B_MAX {
        let probe = frame.at(0.0, v);
        if let Some((seg, t, q)) = nearest_carriageway(net, content, probe, PROBE_STEP * 0.5)
            && lane_direction(net, seg, t).is_some_and(|d| d.dot(-frame.axis) >= cos)
        {
            let b = (q - frame.origin).dot(frame.left());
            let elevation = net.segment(seg)?.elevation;
            return ((B_MIN..=B_MAX).contains(&b)
                && straight_along(net, content, frame.at(0.0, b), -frame.axis, half, elevation))
            .then_some(b);
        }
        v += PROBE_STEP * 0.5;
    }
    None
}

/// The plane point of vertex `j` of piece `k`: on a carriageway, the point of it nearest to the
/// template's (A and B are straight to 2°, not exactly on the axis).
fn vertex(
    net: &NetGraph,
    content: &ContentDb,
    def: &InterchangeDef,
    frame: &StampFrame,
    k: usize,
    j: usize,
) -> Option<Vec2> {
    let (u, v) = def.point_uv(k, j, frame.b)?;
    let p = frame.at(u, v);
    Some(match resolve(def, k, j)? {
        TemplatePoint::OnA(_) | TemplatePoint::OnB(_) => {
            nearest_carriageway(net, content, p, PROBE_STEP)?.2
        }
        _ => p,
    })
}

/// The point a template point stands for: a `Piece` reference followed to its source.
fn resolve(def: &InterchangeDef, k: usize, j: usize) -> Option<TemplatePoint> {
    match *def.pieces.get(k)?.points.get(j)? {
        TemplatePoint::Piece(pk, pj) if usize::from(pk) < k => {
            resolve(def, usize::from(pk), usize::from(pj))
        }
        TemplatePoint::Piece(..) => None,
        p => Some(p),
    }
}

/// The cubic pieces through `pts`: straight legs, or with `smooth` one curve whose tangent at
/// each inner point is the chord of its neighbours (Catmull-Rom) and at the ends the end leg.
fn legs(pts: &[Vec2], smooth: bool) -> Vec<(Vec2, Vec2, Vec2, Vec2)> {
    let tangent = |i: usize| -> Vec2 {
        if i == 0 {
            pts[1] - pts[0]
        } else if i + 1 == pts.len() {
            pts[i] - pts[i - 1]
        } else {
            (pts[i + 1] - pts[i - 1]) * 0.5
        }
    };
    pts.windows(2)
        .enumerate()
        .map(|(i, w)| {
            if smooth && pts.len() > 2 {
                (
                    w[0],
                    w[0] + tangent(i) / 3.0,
                    w[1] - tangent(i + 1) / 3.0,
                    w[1],
                )
            } else {
                (
                    w[0],
                    w[0].lerp(w[1], 1.0 / 3.0),
                    w[0].lerp(w[1], 2.0 / 3.0),
                    w[1],
                )
            }
        })
        .collect()
}

fn elevation_mode(e: PieceElevation) -> ElevationMode {
    match e {
        PieceElevation::Ground => ElevationMode::Ground,
        PieceElevation::Elevated(h) => ElevationMode::Elevated(h),
        PieceElevation::Levels(from, to) => ElevationMode::Levels { from, to },
    }
}

/// The preview of `def` at the cursor `hover`.
pub fn interchange_preview(
    view: &SimView,
    def: &InterchangeDef,
    hover: Vec2,
) -> InterchangePreview {
    let (Some(content), Some(terrain)) = (view.content.as_deref(), view.terrain.as_deref()) else {
        return InterchangePreview::default();
    };
    let net: &NetGraph = &view.net;
    let ground = |p: Vec2| Vec3::new(p.x, terrain.height(p) + 0.5, p.y);
    let frame = match stamp_frame(net, content, def, hover) {
        Ok(f) => f,
        Err(why) => {
            return InterchangePreview {
                error: Some(tr(why)),
                ..Default::default()
            };
        }
    };
    // The template's points in the world; the lines drawn when the batch does not check.
    let points: Option<Vec<Vec<Vec2>>> = (0..def.pieces.len())
        .map(|k| {
            (0..def.pieces[k].points.len())
                .map(|j| vertex(net, content, def, &frame, k, j))
                .collect()
        })
        .collect();
    let Some(points) = points else {
        return InterchangePreview {
            frame: Some(frame),
            error: Some(tr(NEEDS_STRAIGHT)),
            ..Default::default()
        };
    };
    let raw_lines = || -> Vec<Vec<Vec3>> {
        points
            .iter()
            .map(|pts| pts.iter().map(|p| ground(*p)).collect())
            .collect()
    };
    match check_batch(view, content, def, &points) {
        Ok((plans, lines, cost)) => {
            let cmd = SimCommand::Net(NetCommand::BuildBatch {
                plans: plans.clone(),
            });
            InterchangePreview {
                frame: Some(frame),
                lines,
                valid: true,
                error: None,
                cost: Some(cost),
                demolish: crate::under_road(view, &[cmd]).buildings,
                plans,
            }
        }
        Err(why) => InterchangePreview {
            frame: Some(frame),
            lines: raw_lines(),
            error: Some(why),
            ..Default::default()
        },
    }
}

/// The plans of a stamp, its segments as lines (one per plan) and its price.
type CheckedBatch = (Vec<BuildPlan>, Vec<Vec<Vec3>>, Cost);

/// The plans of the stamp with the template's points at `points`, checked in order on a copy of
/// the net as `BuildBatch` runs them: the plans, their segments as lines and the price.
fn check_batch(
    view: &SimView,
    content: &ContentDb,
    def: &InterchangeDef,
    points: &[Vec<Vec2>],
) -> Result<CheckedBatch, String> {
    let terrain = view
        .terrain
        .as_deref()
        .ok_or_else(|| tr("cannot build here"))?;
    let before: &NetGraph = &view.net;
    let mut work = before.clone();
    let mut plans = Vec::with_capacity(def.pieces.len());
    let mut lines = Vec::with_capacity(def.pieces.len());
    let mut cost = Cost::ZERO;
    for (k, piece) in def.pieces.iter().enumerate() {
        let road = content
            .road_id(&piece.road)
            .ok_or_else(|| tr("cannot build here"))?;
        let pts = &points[k];
        // The anchor of each vertex on the net as the earlier plans left it; a vertex of an
        // earlier piece is free (the sim joins it to the node that piece made).
        let anchors: Vec<Anchor> = piece
            .points
            .iter()
            .enumerate()
            .map(|(j, p)| match p {
                TemplatePoint::OnA(_) | TemplatePoint::OnB(_) => {
                    nearest_carriageway(&work, content, pts[j], PROBE_SLACK)
                        .map(|(seg, t, _)| Anchor::OnSegment(seg, t))
                }
                _ => Some(Anchor::Free(pts[j])),
            })
            .collect::<Option<_>>()
            .ok_or_else(|| tr(NEEDS_STRAIGHT))?;
        let pieces: Vec<PlanPiece> = legs(pts, piece.smooth)
            .into_iter()
            .zip(anchors.windows(2))
            .map(|((_, c1, c2, _), a)| PlanPiece {
                from: a[0],
                c1,
                c2,
                to: a[1],
            })
            .collect();
        let plan = BuildPlan {
            road,
            elevation: elevation_mode(piece.elevation),
            pieces,
            trees: false,
        };
        // What the sim does first: a free end on a node made earlier in the batch is that node.
        let mut joined = plan.clone();
        for p in &mut joined.pieces {
            for a in [&mut p.from, &mut p.to] {
                if let Anchor::Free(q) = *a
                    && let Some(n) = made_node(before, &work, q)
                {
                    *a = Anchor::Node(n);
                }
            }
        }
        let built =
            validate_build_plan(&work, terrain, &view.tiles, content, &joined).map_err(|errs| {
                errs.first()
                    .map_or_else(|| tr("cannot build here"), road_error_text)
            })?;
        work = plan_applied(&work, content, &built).ok_or_else(|| tr("cannot build here"))?;
        cost = Cost(cost.0 + built.cost.0);
        lines.push(
            built
                .segments
                .iter()
                .flat_map(|s| sample_curve(&s.curve, 4.0))
                .collect(),
        );
        plans.push(plan);
    }
    Ok((plans, lines, cost))
}

/// A node of `work` that `before` does not have within `BATCH_JOIN` of `p` (as the sim joins a
/// free end of a batch plan).
fn made_node(before: &NetGraph, work: &NetGraph, p: Vec2) -> Option<NodeId> {
    work.nodes()
        .filter(|(id, n)| {
            before.node(*id).is_none() && Vec2::new(n.pos.x, n.pos.z).distance(p) <= BATCH_JOIN
        })
        .map(|(id, _)| id)
        .next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_smooth_piece_is_tangent_to_its_neighbours_chord_and_straight_legs_stay_on_the_chord() {
        let pts = [Vec2::ZERO, Vec2::new(40.0, 10.0), Vec2::new(80.0, 0.0)];
        let s = legs(&pts, true);
        assert_eq!(s.len(), 2);
        // At the inner point both legs leave along the chord of its neighbours (+x).
        let d0 = (s[0].3 - s[0].2).normalize();
        let d1 = (s[1].1 - s[1].0).normalize();
        assert!(d0.distance(Vec2::X) < 1e-4 && d1.distance(Vec2::X) < 1e-4);
        let l = legs(&pts, false);
        assert!((l[0].1 - pts[0].lerp(pts[1], 1.0 / 3.0)).length() < 1e-4);
    }

    #[test]
    fn the_frame_puts_v_to_the_left_of_the_axis() {
        let f = StampFrame {
            origin: Vec2::ZERO,
            axis: Vec2::X,
            b: 20.0,
            elevation: Elevation::Ground,
        };
        // Looking down +Y with X right and Z toward the viewer, left of +X is -Z.
        assert_eq!(f.at(0.0, 1.0), Vec2::new(0.0, -1.0));
        assert_eq!(f.at(5.0, 0.0), Vec2::new(5.0, 0.0));
    }
}
