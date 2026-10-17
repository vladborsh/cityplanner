//! The length step of the road tool (`architecture/system-design.md` §6.7): a pure function the
//! tool calls on the cursor path **before** [`validate_road_path`](super::validate_road_path).
//! `BuildRoad` receives the quantized coordinates and applies them as they are, so nothing here
//! changes the command, the snaps of the validator or any replay.
//!
//! Per vertex, the first rule that fires wins (`Alt` switches rules 1 and 3 off):
//! 1. **Join within one cell**: a point at most `half_width + ZONE_CELL` from the axis of a
//!    ground segment goes to the nearest 8 m tick of that segment's stretch, and to a node of
//!    the stretch when the tick is closer than `ZONE_CELL` to it. A joined vertex is not
//!    stepped (rule 4).
//! 2. **Direction**: the validator's own key-angle / guide / 15° snap, unchanged.
//! 3. **Length** along that direction: up to the perpendicular through a control point of a
//!    neighbouring stretch (within [`CONTROL_LINE_TOL`]), otherwise rounded to `8·k`, `k ≥ 1`.
//!    A curve is shortened or extended along the same quadratic (`t ≤ 1.25`).

use std::collections::BTreeSet;

use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::curve::to_xz;

use super::{
    ElevationMode, GUIDE_HALF_LENGTH, GUIDE_RADIUS, RoadPath, RoadPathRequest, SnapKind,
    SnapReport, SnapReportKind, ZONE_CELL, angle_snap, dir_xz, flat_curve,
};
use crate::graph::NetGraph;
use crate::model::{Elevation, NodeId, SegmentId};
use crate::stretches::{control_points, locate, stretch, unlocate};

/// How far (metres, along the ray) the cursor may be from a control-point perpendicular for the
/// length to jump onto it.
pub const CONTROL_LINE_TOL: f32 = 4.0;
/// A curve may be extended past its end up to this parameter.
const MAX_CURVE_T: f32 = 1.25;

/// What `quantize_path` reads: the graph and content to find neighbours, and the `Alt` switch.
#[derive(Clone, Copy)]
pub struct StepContext<'a> {
    pub graph: &'a NetGraph,
    pub content: &'a ContentDb,
    /// `Alt` held: no joining and no length step (the direction snaps stay).
    pub alt: bool,
    /// Where the cells of the streets near the path end every 8th column (the zone layout,
    /// BUG-047). Empty: the control points 64 m from the anchor of each stretch stand in.
    pub marks: &'a [ControlMark],
    /// Half the width of the road being drawn: a street joined to a boundary is placed so that
    /// its near EDGE stands on it, its axis `edge_offset` further on (BUG-067). 0: the axis.
    pub edge_offset: f32,
}

/// A column boundary of the zoning along a street, a candidate for the length step to jump to:
/// the perpendicular to `dir` through `pos`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlMark {
    /// On the street axis.
    pub pos: Vec2,
    /// Unit direction of the street there.
    pub dir: Vec2,
    /// Metres from the start of the cells, for the label.
    pub along: f32,
    /// Every 8th column boundary: the length step jumps only to these; a join (rule 1) goes to
    /// the nearest boundary of any kind.
    pub major: bool,
    /// Unit vector along the street in the direction the cells are counted (away from the cross
    /// street the front started at).
    pub onward: Vec2,
}

/// Where a joined vertex landed, with what the next leg takes as direction references.
struct Joined {
    pos: Vec2,
    node: Option<NodeId>,
    /// For a tick on a segment: the directions along it at that point.
    along: Option<Vec2>,
    /// How far from the boundary the vertex was placed (the edge offset), zero otherwise.
    shift: Vec2,
}

/// Rule 1 for the point `p`.
fn join(cx: &StepContext, p: Vec2) -> Option<Joined> {
    let mut best: Option<(f32, SegmentId, f32)> = None;
    for (id, s) in cx.graph.segments() {
        if s.elevation != Elevation::Ground {
            continue;
        }
        let reach = cx.content.road(s.road).width * 0.5 + ZONE_CELL;
        if s.curve.bounds_distance_xz(p) > reach + 1e-3 {
            continue;
        }
        let (t, d) = s.curve.nearest_xz(p);
        if d <= reach && best.is_none_or(|(bd, ..)| d < bd) {
            best = Some((d, id, t));
        }
    }
    let (_, seg, t) = best?;
    let s = cx.graph.segment(seg)?;
    // Where the zone cells beside the street have their column boundaries (BUG-067): a street
    // joined there meets the cells on a boundary, and, with the facing street's boundaries on
    // the same block lattice, crosses the block at a right angle.
    let foot = to_xz(s.curve.point(t));
    if let Some(m) = cx
        .marks
        .iter()
        .filter(|m| m.pos.distance(foot) <= 6.0)
        .min_by(|a, b| a.pos.distance(foot).total_cmp(&b.pos.distance(foot)))
    {
        let shift = m.onward * cx.edge_offset;
        return Some(Joined {
            pos: m.pos + shift,
            node: None,
            along: Some(m.dir),
            shift,
        });
    }
    let st = stretch(cx.graph, seg)?;
    let along = locate(cx.graph, seg, s.curve.arc_table().t_to_s(t))?;
    let tick = ((along / ZONE_CELL).round() * ZONE_CELL).clamp(0.0, st.length);
    let (tseg, ts) = unlocate(cx.graph, &st, tick)?;
    let tc = cx.graph.segment(tseg)?;
    let tt = tc.curve.arc_table().s_to_t(ts);
    let pos = to_xz(tc.curve.point(tt));
    // The nearest node of the stretch closer than one cell takes the tick.
    let mut nodes = BTreeSet::new();
    for sid in &st.segments {
        if let Some(s) = cx.graph.segment(*sid) {
            nodes.insert(s.start);
            nodes.insert(s.end);
        }
    }
    let near = nodes
        .into_iter()
        .filter_map(|n| cx.graph.node(n).map(|node| (n, to_xz(node.pos))))
        .map(|(n, q)| (q.distance(pos), n, q))
        .filter(|(d, ..)| *d < ZONE_CELL)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    Some(match near {
        Some((_, n, q)) => Joined {
            pos: q,
            node: Some(n),
            along: None,
            shift: Vec2::ZERO,
        },
        None => Joined {
            pos,
            node: None,
            along: Some(dir_xz(tc.curve.tangent(tt))),
            shift: Vec2::ZERO,
        },
    })
}

/// Directions leaving the node, as the validator takes them for the angle snap.
fn node_dirs(graph: &NetGraph, n: NodeId) -> Vec<Vec2> {
    let Some(node) = graph.node(n) else {
        return Vec::new();
    };
    node.segments
        .iter()
        .filter_map(|sid| graph.segment(*sid).map(|s| (sid, s)))
        .map(|(_, s)| {
            if s.start == n {
                dir_xz(s.curve.tangent(0.0))
            } else {
                -dir_xz(s.curve.tangent(1.0))
            }
        })
        .collect()
}

/// The perpendicular through a control point of a stretch near `p` that the ray `prev + u·dir`
/// meets closest to `u = len`: `(u, control point, its distance along its stretch, the unit
/// direction of the perpendicular)`.
fn control_line(
    cx: &StepContext,
    p: Vec2,
    prev: Vec2,
    dir: Vec2,
    len: f32,
) -> Option<(f32, Vec2, f32, Vec2)> {
    let mut seen: BTreeSet<SegmentId> = BTreeSet::new();
    let mut best: Option<(f32, f32, Vec2, f32, Vec2)> = None;
    let mut consider = |c: Vec2, tau: Vec2, along: f32| {
        let denom = dir.dot(tau);
        if tau == Vec2::ZERO || denom.abs() < 0.2 {
            return;
        }
        let u = (c - prev).dot(tau) / denom;
        let miss = (u - len).abs();
        if u >= ZONE_CELL && miss <= CONTROL_LINE_TOL && best.is_none_or(|(bm, ..)| miss < bm) {
            best = Some((miss, u, c, along, Vec2::new(-tau.y, tau.x)));
        }
    };
    if !cx.marks.is_empty() {
        for m in cx.marks.iter().filter(|m| m.major) {
            consider(m.pos, m.dir, m.along);
        }
    } else {
        for (id, s) in cx.graph.segments() {
            if s.elevation != Elevation::Ground || seen.contains(&id) {
                continue;
            }
            if s.curve.bounds_distance_xz(p) > GUIDE_RADIUS + 1e-3
                || s.curve.nearest_xz(p).1 > GUIDE_RADIUS
            {
                continue;
            }
            let Some(st) = stretch(cx.graph, id) else {
                continue;
            };
            seen.extend(st.segments.iter().copied());
            for cp in control_points(cx.graph, &st) {
                let Some(seg) = cx.graph.segment(cp.seg) else {
                    continue;
                };
                let tau = dir_xz(seg.curve.tangent(seg.curve.arc_table().s_to_t(cp.s)));
                consider(to_xz(cp.pos), tau, cp.along);
            }
        }
    }
    best.map(|(_, u, c, along, n)| (u, c, along, n))
}

/// The point of the quadratic `a → c → b` extended to parameter `t`, and the quadratic cut at `t`.
fn cut_quad(a: Vec2, c: Vec2, b: Vec2, t: f32) -> (Vec2, Vec2) {
    let ctrl = a.lerp(c, t);
    let u = 1.0 - t;
    (ctrl, a * (u * u) + c * (2.0 * u * t) + b * (t * t))
}

fn quad_len(a: Vec2, c: Vec2, b: Vec2) -> f32 {
    flat_curve(a, b, Some(c)).arc_table().length()
}

/// The closest 8 m multiple of the length of the curve (never below 8), as a cut curve.
fn step_curve(a: Vec2, c: Vec2, b: Vec2) -> (Vec2, Vec2) {
    let len = quad_len(a, c, b);
    let want = ((len / ZONE_CELL).round() * ZONE_CELL).max(ZONE_CELL);
    let len_at = |t: f32| {
        let (ctrl, to) = cut_quad(a, c, b, t);
        quad_len(a, ctrl, to)
    };
    if len_at(MAX_CURVE_T) < want {
        return cut_quad(a, c, b, MAX_CURVE_T);
    }
    let (mut lo, mut hi) = (0.0_f32, MAX_CURVE_T);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if len_at(mid) < want {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    cut_quad(a, c, b, 0.5 * (lo + hi))
}

/// One leg from `prev` to the cursor `p`: rule 2, then rule 3 (unless `alt`). Returns the end
/// and what snapped it.
fn leg(
    cx: &StepContext,
    snap_angle: bool,
    prev: Vec2,
    refs: &[Vec2],
    p: Vec2,
) -> (Vec2, SnapReport) {
    let mut report = SnapReport::default();
    let mut end = p;
    if snap_angle && let Some(a) = angle_snap(cx.graph, p, prev, refs) {
        end = a.pos;
        report = SnapReport {
            kind: match a.kind {
                SnapKind::KeyAngle(d) => SnapReportKind::KeyAngle(d),
                SnapKind::Guide { perpendicular } => SnapReportKind::Guide { perpendicular },
                SnapKind::Angle(d) => SnapReportKind::Step(d),
                _ => SnapReportKind::None,
            },
            guide: a.guide,
        };
    }
    let d = end - prev;
    let len = d.length();
    if cx.alt || len < 1e-3 {
        return (end, report);
    }
    let dir = d / len;
    if let Some((u, c, along, n)) = control_line(cx, p, prev, dir, len) {
        // The guide is the perpendicular itself, centred on the control point.
        return (
            prev + dir * u,
            SnapReport {
                kind: SnapReportKind::ControlLine(along.round() as i32),
                guide: Some((c - n * GUIDE_HALF_LENGTH, c + n * GUIDE_HALF_LENGTH)),
            },
        );
    }
    let stepped = ((len / ZONE_CELL).round() * ZONE_CELL).max(ZONE_CELL);
    (
        prev + dir * stepped,
        SnapReport {
            kind: SnapReportKind::CellStep(stepped as i32),
            guide: report.guide,
        },
    )
}

/// Applies the rules above to `req.path`, returning the quantized path and the snap at its last
/// vertex. Pure: same inputs, same output; applying it to its own output changes nothing.
pub fn quantize_path(req: &RoadPathRequest, cx: &StepContext) -> (RoadPath, SnapReport) {
    let ground = matches!(req.elevation, ElevationMode::Ground);
    let joins = ground && !cx.alt && req.snap.segment;
    let (pts, control): (Vec<Vec2>, Option<Vec2>) = match &req.path {
        RoadPath::Straight { from, to } => (vec![*from, *to], None),
        RoadPath::Curve { from, control, to } => (vec![*from, *to], Some(*control)),
        RoadPath::Free(p) => (p.clone(), None),
    };
    let angle = req.snap.angle && control.is_none();
    let mut out: Vec<Vec2> = Vec::with_capacity(pts.len());
    // Direction references of the vertex just placed.
    let mut refs: Vec<Vec2> = Vec::new();
    let mut report = SnapReport::default();
    let mut stepped_curve = None;
    let mut last_shift = Vec2::ZERO;
    for (i, &p) in pts.iter().enumerate() {
        report = SnapReport::default();
        let mut joined = if joins { join(cx, p) } else { None };
        // A street across a block (BUG-067): when the vertex before it stands on a boundary of a
        // street (its direction known) and a boundary of this street lies on the perpendicular
        // from there, join that one, so the new street crosses at a right angle instead of at
        // the boundary nearest to the cursor.
        if i > 0
            && let Some(j) = joined.as_mut()
            && j.node.is_none()
            && !cx.alt
        {
            let from = out[i - 1];
            let perp = refs
                .iter()
                .map(|r| Vec2::new(-r.y, r.x))
                .filter(|n| n.length() > 0.5);
            let mut best: Option<(f32, Vec2, Vec2)> = None;
            for n in perp {
                for m in cx.marks.iter().filter(|m| m.pos.distance(j.pos) <= 12.0) {
                    // The same shift as the first vertex got: the street stays perpendicular.
                    let pos = m.pos + last_shift;
                    let off = (pos - from).perp_dot(n).abs();
                    if off <= 0.3 && best.is_none_or(|(bo, ..)| off < bo) {
                        best = Some((off, pos, m.dir));
                    }
                }
            }
            if let Some((_, pos, dir)) = best {
                j.pos = pos;
                j.along = Some(dir);
                j.shift = last_shift;
            }
        }
        last_shift = joined.as_ref().map_or(Vec2::ZERO, |j| j.shift);
        let mut next_refs = Vec::new();
        let pos = if let Some(j) = joined {
            report.kind = if j.node.is_some() {
                SnapReportKind::Node
            } else {
                SnapReportKind::Segment
            };
            if let Some(n) = j.node {
                next_refs = node_dirs(cx.graph, n);
            }
            if let Some(t) = j.along {
                next_refs = vec![t, -t];
            }
            j.pos
        } else if i == 0 {
            p
        } else if let (Some(c), true) = (control, i == 1) {
            if cx.alt {
                p
            } else {
                let (ctrl, to) = step_curve(out[0], c, p);
                report.kind = SnapReportKind::CellStep(quad_len(out[0], ctrl, to).round() as i32);
                stepped_curve = Some(ctrl);
                to
            }
        } else {
            let (q, r) = leg(cx, angle, out[i - 1], &refs, p);
            report = r;
            q
        };
        if i > 0 {
            let back = out[i - 1] - pos;
            if back.length() > 1e-3 {
                next_refs.push(-back.normalize());
            }
        }
        if next_refs.is_empty() {
            next_refs.push(Vec2::X);
        }
        refs = next_refs;
        out.push(pos);
    }
    let path = match &req.path {
        RoadPath::Straight { .. } => RoadPath::Straight {
            from: out[0],
            to: out[1],
        },
        RoadPath::Curve { control, .. } => RoadPath::Curve {
            from: out[0],
            control: stepped_curve.unwrap_or(*control),
            to: out[1],
        },
        RoadPath::Free(_) => RoadPath::Free(out),
    };
    (path, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::SnapOptions;
    use bevy_math::Vec3;
    use cp_core::curve::CubicBezier3;

    fn db() -> ContentDb {
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap_or_else(|e| panic!("{e}"))
    }

    fn v(x: f32, z: f32) -> Vec2 {
        Vec2::new(x, z)
    }

    fn road(db: &ContentDb, graph: &mut NetGraph, a: Vec2, b: Vec2) -> SegmentId {
        let id = db.road_id("basic_2l").unwrap();
        let p = |q: Vec2| Vec3::new(q.x, 0.0, q.y);
        graph
            .add_segment(
                db.road(id),
                id,
                CubicBezier3::line(p(a), p(b)),
                Elevation::Ground,
            )
            .unwrap()
    }

    fn req(db: &ContentDb, path: RoadPath) -> RoadPathRequest {
        RoadPathRequest {
            road: db.road_id("basic_2l").unwrap(),
            path,
            elevation: ElevationMode::Ground,
            snap: SnapOptions::default(),
        }
    }

    fn run(db: &ContentDb, g: &NetGraph, alt: bool, path: RoadPath) -> (RoadPath, SnapReport) {
        quantize_path(
            &req(db, path),
            &StepContext {
                graph: g,
                content: db,
                alt,
                marks: &[],
                edge_offset: 0.0,
            },
        )
    }

    fn straight_to(p: &RoadPath) -> Vec2 {
        match p {
            RoadPath::Straight { to, .. } => *to,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_straight_length_goes_to_the_nearest_multiple_of_8() {
        let (db, g) = (db(), NetGraph::new());
        for (len, want) in [(101.0, 104.0), (99.0, 96.0), (3.0, 8.0), (96.0, 96.0)] {
            let (p, r) = run(
                &db,
                &g,
                false,
                RoadPath::Straight {
                    from: v(1000.0, 1000.0),
                    to: v(1000.0 + len, 1000.0),
                },
            );
            assert!((straight_to(&p).x - 1000.0 - want).abs() < 1e-3, "{len}");
            assert_eq!(r.kind, SnapReportKind::CellStep(want as i32));
            // Quantizing the result again changes nothing.
            assert_eq!(run(&db, &g, false, p.clone()).0, p);
        }
    }

    #[test]
    fn alt_leaves_the_path_alone() {
        let (db, mut g) = (db(), NetGraph::new());
        road(&db, &mut g, v(0.0, 0.0), v(200.0, 0.0));
        let path = RoadPath::Straight {
            from: v(51.0, 6.0),
            to: v(51.0, 107.0),
        };
        // Only the direction snap of the validator remains (it may move the end by rounding).
        let (out, r) = run(&db, &g, true, path);
        let RoadPath::Straight { from, to } = out else {
            panic!()
        };
        assert_eq!(from, v(51.0, 6.0), "no join on the road");
        assert!(to.distance(v(51.0, 107.0)) < 1e-3, "no length step: {to:?}");
        assert!(!matches!(
            r.kind,
            SnapReportKind::CellStep(_) | SnapReportKind::ControlLine(_)
        ));
    }

    #[test]
    fn a_curve_ends_with_an_arc_length_that_is_a_multiple_of_8() {
        let (db, g) = (db(), NetGraph::new());
        for to in [v(1130.0, 1100.0), v(1180.0, 1060.0), v(1100.0, 1105.0)] {
            let (p, _) = run(
                &db,
                &g,
                false,
                RoadPath::Curve {
                    from: v(1000.0, 1000.0),
                    control: v(1100.0, 1000.0),
                    to,
                },
            );
            let RoadPath::Curve { from, control, to } = p else {
                panic!()
            };
            let len = flat_curve(from, to, Some(control)).arc_table().length();
            assert!(
                (len / 8.0 - (len / 8.0).round()).abs() * 8.0 < 0.05,
                "{len}"
            );
            assert!(len >= 8.0);
        }
    }

    #[test]
    fn an_end_within_a_cell_of_a_road_edge_goes_to_a_tick_and_beyond_is_left() {
        let (db, mut g) = (db(), NetGraph::new());
        let seg = road(&db, &mut g, v(0.0, 0.0), v(200.0, 0.0));
        let half = db.road(g.segment(seg).unwrap().road).width * 0.5;
        let far = v(400.0, 400.0);
        let (p, r) = run(
            &db,
            &g,
            false,
            RoadPath::Straight {
                from: far,
                to: v(51.0, half + 7.0),
            },
        );
        let to = straight_to(&p);
        assert!(to.y.abs() < 0.01, "on the axis: {to:?}");
        assert!(
            (to.x - 48.0).abs() < 0.01,
            "tick 48 m from the anchor: {to:?}"
        );
        assert_eq!(r.kind, SnapReportKind::Segment);

        let (p, _) = run(
            &db,
            &g,
            false,
            RoadPath::Straight {
                from: far,
                to: v(51.0, half + 9.0),
            },
        );
        assert!(
            straight_to(&p).y > half + 1.0,
            "9 m from the edge is not joined: {:?}",
            straight_to(&p)
        );
    }

    #[test]
    fn a_tick_within_a_cell_of_a_node_goes_to_the_node() {
        let (db, mut g) = (db(), NetGraph::new());
        road(&db, &mut g, v(0.0, 0.0), v(70.0, 0.0));
        let (p, r) = run(
            &db,
            &g,
            false,
            RoadPath::Straight {
                from: v(400.0, 400.0),
                to: v(66.0, 2.0),
            },
        );
        assert!(straight_to(&p).distance(v(70.0, 0.0)) < 0.01);
        assert_eq!(r.kind, SnapReportKind::Node);
    }

    #[test]
    fn given_marks_stand_in_for_the_control_points() {
        let (db, mut g) = (db(), NetGraph::new());
        road(&db, &mut g, v(3.0, 30.0), v(200.0, 30.0));
        // Cells of 8.75 m: the 8th boundary is at x = 73, not at the control point 67.
        let marks = [ControlMark {
            pos: v(73.0, 30.0),
            dir: v(1.0, 0.0),
            along: 70.0,
            major: true,
            onward: v(1.0, 0.0),
        }];
        let path = RoadPath::Straight {
            from: v(0.0, 0.0),
            to: v(71.0, 0.0),
        };
        let r = req(&db, path);
        let (p, rep) = quantize_path(
            &r,
            &StepContext {
                graph: &g,
                content: &db,
                alt: false,
                marks: &marks,
                edge_offset: 0.0,
            },
        );
        assert!((straight_to(&p).x - 73.0).abs() < 0.05, "{p:?}");
        assert_eq!(rep.kind, SnapReportKind::ControlLine(70));
    }

    #[test]
    fn the_length_jumps_to_the_perpendicular_through_a_neighbours_control_point() {
        let (db, mut g) = (db(), NetGraph::new());
        // A parallel street 30 m away whose anchor is at x = 3: control points at 67, 131.
        road(&db, &mut g, v(3.0, 30.0), v(200.0, 30.0));
        let (p, r) = run(
            &db,
            &g,
            false,
            RoadPath::Straight {
                from: v(0.0, 0.0),
                to: v(69.0, 0.0),
            },
        );
        assert!(
            (straight_to(&p).x - 67.0).abs() < 0.05,
            "{:?}",
            straight_to(&p)
        );
        assert_eq!(r.kind, SnapReportKind::ControlLine(64));
        let (a, b) = r.guide.unwrap();
        assert!(
            a.x.abs().max(b.x.abs()) > 60.0
                && (a.x - 67.0).abs() < 1e-3
                && (b.x - 67.0).abs() < 1e-3
        );
        // Further than the tolerance: the plain 8 m step.
        let (p, r) = run(
            &db,
            &g,
            false,
            RoadPath::Straight {
                from: v(0.0, 0.0),
                to: v(73.0, 0.0),
            },
        );
        assert!((straight_to(&p).x - 72.0).abs() < 0.05);
        assert_eq!(r.kind, SnapReportKind::CellStep(72));
    }

    #[test]
    fn free_legs_are_stepped_each_from_the_previous_vertex() {
        let (db, g) = (db(), NetGraph::new());
        let (p, _) = run(
            &db,
            &g,
            false,
            RoadPath::Free(vec![
                v(1000.0, 1000.0),
                v(1050.0, 1000.0),
                v(1050.0, 1021.0),
            ]),
        );
        let RoadPath::Free(pts) = p else { panic!() };
        assert!((pts[1].x - 1048.0).abs() < 0.01);
        assert!((pts[2].distance(pts[1]) - 24.0).abs() < 0.01, "{pts:?}");
    }
}
