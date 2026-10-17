//! The road tool: modes, click state machine, preview.

use bevy_math::{Vec2, Vec3};
use cp_content::{RoadDef, RoadDefId};
use cp_core::Cost;
use cp_core::curve::CubicBezier3;
use cp_net::{
    ElevationMode, RoadBuildError, RoadBuildPlan, RoadPath, RoadPathRequest, SnapOptions,
    SnapReport, SnapReportKind, StepContext, cell_ticks, clearance_report, control_points,
    quantize_path, stretch, validate_road_path, validate_road_path_report,
};
use cp_sim::SimView;

/// Metres of road height per PgUp/PgDn step.
pub const ELEVATION_STEP: f32 = 5.0;
/// Highest elevation level (`level * ELEVATION_STEP` metres).
pub const MAX_ELEVATION_LEVEL: u8 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoadMode {
    #[default]
    Straight,
    /// Three clicks: start, control point, end.
    Curve,
    /// A polyline; Enter (or a click on the last point) finishes it.
    Free,
}

/// Depth of the axis of a road tunnel the tool draws, metres (the content's default depth).
pub const ROAD_TUNNEL_DEPTH: f32 = 12.0;

/// State of the road tool.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoadTool {
    pub mode: RoadMode,
    pub road: Option<RoadDefId>,
    /// 0 = ground, otherwise raised by `level * ELEVATION_STEP`.
    pub level: u8,
    /// Depth of the axis below the terrain for a road that is built in a tunnel only (the
    /// metro track, T-164): the tool then draws `Tunnel(depth)` and the height keys do nothing.
    pub tunnel_depth: Option<f32>,
    /// A road type that has a tunnel is built under the ground (T-173): the height keys go
    /// below ground level (PgDn from level 0) and the tool draws `Tunnel(ROAD_TUNNEL_DEPTH)`.
    pub underground: bool,
    /// Points clicked so far in the current drawing.
    pub points: Vec<Vec2>,
}

impl RoadTool {
    pub fn elevation(&self) -> ElevationMode {
        if let Some(depth) = self.tunnel_depth {
            ElevationMode::Tunnel(depth)
        } else if self.underground {
            ElevationMode::Tunnel(ROAD_TUNNEL_DEPTH)
        } else if self.level == 0 {
            ElevationMode::Ground
        } else {
            ElevationMode::Elevated(f32::from(self.level) * ELEVATION_STEP)
        }
    }

    pub fn raise(&mut self) {
        if self.tunnel_depth.is_some() {
            return;
        }
        if self.underground {
            self.underground = false;
            return;
        }
        self.level = (self.level + 1).min(MAX_ELEVATION_LEVEL);
    }

    pub fn lower(&mut self) {
        if self.tunnel_depth.is_some() {
            return;
        }
        if self.level == 0 {
            self.underground = true;
            return;
        }
        self.level = self.level.saturating_sub(1);
    }

    pub fn set_mode(&mut self, mode: RoadMode) {
        if self.mode != mode {
            self.mode = mode;
            self.points.clear();
        }
    }

    pub fn cancel(&mut self) {
        self.points.clear();
    }

    /// Clicks needed before the path is complete (`None` for free mode, which ends on demand).
    fn clicks_needed(&self) -> Option<usize> {
        match self.mode {
            RoadMode::Straight => Some(2),
            RoadMode::Curve => Some(3),
            RoadMode::Free => None,
        }
    }

    /// The path as it would be if the cursor were at `hover` (what the ghost shows). With
    /// nothing clicked yet there is nothing to preview.
    pub fn path_with_hover(&self, hover: Vec2) -> Option<RoadPath> {
        let first = *self.points.first()?;
        Some(match (self.mode, self.points.len()) {
            (RoadMode::Straight, _) | (RoadMode::Curve, 1) => RoadPath::Straight {
                from: first,
                to: hover,
            },
            (RoadMode::Curve, _) => RoadPath::Curve {
                from: first,
                control: self.points[1],
                to: hover,
            },
            (RoadMode::Free, _) => {
                let mut pts = self.points.clone();
                pts.push(hover);
                RoadPath::Free(pts)
            }
        })
    }

    /// The request for `path` with the tool's road type, elevation and the default snaps (the
    /// ones the `BuildRoad` command uses, so the preview and the command agree).
    pub fn request(&self, path: RoadPath) -> Option<RoadPathRequest> {
        Some(RoadPathRequest {
            road: self.road?,
            path,
            elevation: self.elevation(),
            snap: SnapOptions::default(),
        })
    }

    /// A click at `p`. Returns the finished request when this click completed the drawing; the
    /// tool then stays in a chain (T-176): the next piece starts at the last point (a curve's
    /// tangent is free). `Esc` ([`Self::cancel`]) ends the chain.
    pub fn click(&mut self, p: Vec2) -> Option<RoadPathRequest> {
        self.road?;
        if self.mode == RoadMode::Free && self.points.last().is_some_and(|l| l.distance(p) < 1.0) {
            return self.finish_free();
        }
        self.points.push(p);
        if self.clicks_needed() == Some(self.points.len()) {
            let pts = std::mem::replace(&mut self.points, vec![p]);
            let path = match self.mode {
                RoadMode::Straight => RoadPath::Straight {
                    from: pts[0],
                    to: pts[1],
                },
                RoadMode::Curve => RoadPath::Curve {
                    from: pts[0],
                    control: pts[1],
                    to: pts[2],
                },
                RoadMode::Free => unreachable!("free mode has no fixed length"),
            };
            return self.request(path);
        }
        None
    }

    /// Ends a free-mode drawing (needs at least two points).
    pub fn finish_free(&mut self) -> Option<RoadPathRequest> {
        if self.mode != RoadMode::Free || self.points.len() < 2 {
            return None;
        }
        let last = *self.points.last()?;
        let pts = std::mem::replace(&mut self.points, vec![last]);
        self.request(RoadPath::Free(pts))
    }
}

/// The end point of a path.
fn path_end(path: &RoadPath) -> Option<Vec2> {
    match path {
        RoadPath::Straight { to, .. } | RoadPath::Curve { to, .. } => Some(*to),
        RoadPath::Free(pts) => pts.last().copied(),
    }
}

/// Settles a click that completed a piece (T-176): `tool` already stands in the chain state
/// ([`RoadTool::click`]); `before` is the tool as it was before the click. A piece that does not
/// validate (a red ghost, or too close to a road) is not sent and `tool` goes back to `before`,
/// so the chain and its points are not lost. A good piece comes back quantized, the way
/// `BuildRoad` is sent, and the chain continues from the end the quantizing gave it.
pub fn chain_commit(
    view: Option<&SimView>,
    tool: &mut RoadTool,
    before: RoadTool,
    req: RoadPathRequest,
    alt: bool,
) -> Option<RoadPathRequest> {
    let req = match view {
        Some(v) => quantized(v, req, alt).0,
        None => req,
    };
    if let Some(v) = view
        && (matches!(preview(v, &req), Some(Err(_))) || too_close(v, &req))
    {
        *tool = before;
        return None;
    }
    if let Some(end) = path_end(&req.path) {
        tool.points = vec![end];
    }
    Some(req)
}

/// Validates `req` against the snapshot with the same pure function `BuildRoad` uses.
/// `None` when the view has no terrain or content (nothing to validate against).
pub fn preview(
    view: &SimView,
    req: &RoadPathRequest,
) -> Option<Result<RoadBuildPlan, Vec<RoadBuildError>>> {
    let terrain = view.terrain.as_deref()?;
    let content = view.content.as_deref()?;
    Some(validate_road_path(
        &view.net,
        terrain,
        &view.tiles,
        content,
        req,
    ))
}

/// `req` with its path quantized to the 8 m step and the joins of §6.7 (`Alt` switches them
/// off), plus what the quantizing snapped. The ghost and the `BuildRoad` command both use the
/// result, so what is drawn is what is built. Without content there is nothing to look up and
/// `req` comes back as it is.
pub fn quantized(view: &SimView, req: RoadPathRequest, alt: bool) -> (RoadPathRequest, SnapReport) {
    let Some(content) = view.content.as_deref() else {
        return (req, SnapReport::default());
    };
    let cx = StepContext {
        graph: &view.net,
        content,
        alt,
    };
    let (path, report) = quantize_path(&req, &cx);
    (RoadPathRequest { path, ..req }, report)
}

/// The verdict shown when a new road runs less than one cell from another (§6.7, T-124).
pub const TOO_CLOSE: &str = "too close to another road: leave a cell (8 m) or connect";

/// Does the path of `req` pass validation but run closer than one cell to an existing road? The
/// tool then draws a red ghost and sends no command; the command itself does not check this.
pub fn too_close(view: &SimView, req: &RoadPathRequest) -> bool {
    let (Some(content), Some(Ok(plan))) = (view.content.as_deref(), preview(view, req)) else {
        return false;
    };
    !clearance_report(&view.net, content, &plan).is_empty()
}

/// What a control-point marker of the road tool stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerKind {
    /// A control point: every 64 m from the anchor of a stretch (drawn as a diamond).
    Control,
    /// An 8 m tick, shown while the cursor is within a cell of a road edge (a small cross).
    Tick,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadMarker {
    pub pos: Vec2,
    pub kind: MarkerKind,
}

/// The markers the road tool draws around the cursor `hover`: the control points of every
/// ground stretch within [`GUIDE_RADIUS`] of it and, when the cursor is within one cell of a
/// road's edge, the 8 m ticks of that road's stretch. Derived from the graph; nothing is stored.
pub fn road_markers(view: &SimView, hover: Vec2) -> Vec<RoadMarker> {
    let Some(content) = view.content.as_deref() else {
        return Vec::new();
    };
    let graph = &*view.net;
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    let mut near_edge: Option<(f32, cp_net::SegmentId)> = None;
    for (id, s) in graph.segments() {
        if s.elevation != cp_net::Elevation::Ground
            || s.curve.bounds_distance_xz(hover) > cp_net::build::GUIDE_RADIUS + 1e-3
        {
            continue;
        }
        let (_, d) = s.curve.nearest_xz(hover);
        let reach = content.road(s.road).width * 0.5 + cp_net::ZONE_CELL;
        if d <= reach && near_edge.is_none_or(|(bd, _)| d < bd) {
            near_edge = Some((d, id));
        }
        if d > cp_net::build::GUIDE_RADIUS || seen.contains(&id) {
            continue;
        }
        let Some(st) = stretch(graph, id) else {
            continue;
        };
        seen.extend(st.segments.iter().copied());
        out.extend(control_points(graph, &st).into_iter().map(|p| RoadMarker {
            pos: Vec2::new(p.pos.x, p.pos.z),
            kind: MarkerKind::Control,
        }));
    }
    if let Some((_, id)) = near_edge
        && let Some(st) = stretch(graph, id)
    {
        out.extend(cell_ticks(graph, &st).into_iter().map(|p| RoadMarker {
            pos: Vec2::new(p.pos.x, p.pos.z),
            kind: MarkerKind::Tick,
        }));
    }
    out
}

/// [`preview`] plus the snap at the cursor (also for an invalid path).
pub fn preview_report(
    view: &SimView,
    req: &RoadPathRequest,
) -> Option<(Result<RoadBuildPlan, Vec<RoadBuildError>>, SnapReport)> {
    let terrain = view.terrain.as_deref()?;
    let content = view.content.as_deref()?;
    Some(validate_road_path_report(
        &view.net,
        terrain,
        &view.tiles,
        content,
        req,
    ))
}

/// Distance between direction chevrons along a one-way ghost, metres.
pub const CHEVRON_SPACING: f32 = 16.0;
/// Lift of the chevrons over the curve, metres (above the 0.3 m ghost band).
const CHEVRON_LIFT: f32 = 0.7;

/// Do all the car lanes of `def` run `Forward` (and are there any)?
pub fn road_is_one_way(def: &RoadDef) -> bool {
    def.is_one_way()
}

/// The direction chevrons of a one-way piece: one every [`CHEVRON_SPACING`] metres, so
/// `floor(length / 16)` of them, centred in their 16 m cell. Each chevron is three gizmo
/// segments: the two arms of the arrowhead and a short stem behind its tip, pointing along
/// the curve from `start` to `end`.
pub fn chevron_lines(curve: &CubicBezier3, length: f32) -> Vec<[Vec3; 2]> {
    let n = (length / CHEVRON_SPACING).floor().max(0.0) as usize;
    let table = curve.arc_table();
    let lift = Vec3::Y * CHEVRON_LIFT;
    let mut out = Vec::with_capacity(n * 3);
    for i in 0..n {
        let s = ((i as f32 + 0.5) * CHEVRON_SPACING).min(table.length());
        let t = table.s_to_t(s);
        let p = curve.point(t) + lift;
        let flat = Vec3::new(curve.tangent(t).x, 0.0, curve.tangent(t).z);
        let Some(dir) = flat.try_normalize() else {
            continue;
        };
        let side = Vec3::new(-dir.z, 0.0, dir.x);
        let tip = p + dir * 1.5;
        out.push([p - dir * 1.0 + side * 1.5, tip]);
        out.push([p - dir * 1.0 - side * 1.5, tip]);
        out.push([p - dir * 1.8, tip]);
    }
    out
}

/// The tool's label for a snap: `90°`, `45°`, `∥` or `⊥`; steps and plain snaps show none.
pub fn snap_label(kind: SnapReportKind) -> Option<String> {
    match kind {
        SnapReportKind::KeyAngle(d) => Some(format!("{d}°")),
        SnapReportKind::Guide { perpendicular } => {
            Some(if perpendicular { "⊥" } else { "∥" }.to_string())
        }
        SnapReportKind::ControlLine(d) => Some(format!("⊥ {d}")),
        _ => None,
    }
}

/// Cost as a money string: cents → `$1,234.56`.
pub fn format_money(cost: Cost) -> String {
    let cents = cost.0;
    let sign = if cents < 0 { "-" } else { "" };
    let cents = cents.unsigned_abs();
    let dollars = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in dollars.chars().enumerate() {
        if i > 0 && (dollars.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{sign}${grouped}.{:02}", cents % 100)
}

/// Points along a curve, about every `step` metres.
pub fn sample_curve(c: &CubicBezier3, step: f32) -> Vec<Vec3> {
    let len = c.arc_table().length().max(step);
    let n = (len / step).ceil() as usize;
    (0..=n).map(|i| c.point(i as f32 / n as f32)).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn lowering_from_the_ground_goes_into_a_tunnel_and_raising_comes_back() {
        let mut tool = RoadTool::default();
        assert_eq!(tool.elevation(), ElevationMode::Ground);
        tool.lower();
        assert_eq!(tool.elevation(), ElevationMode::Tunnel(ROAD_TUNNEL_DEPTH));
        tool.raise();
        assert_eq!(tool.elevation(), ElevationMode::Ground);
        tool.raise();
        assert_eq!(tool.elevation(), ElevationMode::Elevated(ELEVATION_STEP));
        tool.lower();
        tool.lower();
        assert_eq!(tool.elevation(), ElevationMode::Tunnel(ROAD_TUNNEL_DEPTH));
    }

    use super::*;

    fn tool(mode: RoadMode) -> RoadTool {
        RoadTool {
            mode,
            road: Some(RoadDefId(0)),
            ..Default::default()
        }
    }

    #[test]
    fn snap_labels_show_key_angles_and_guides_only() {
        assert_eq!(
            snap_label(SnapReportKind::KeyAngle(90)).as_deref(),
            Some("90°")
        );
        assert_eq!(
            snap_label(SnapReportKind::KeyAngle(45)).as_deref(),
            Some("45°")
        );
        assert_eq!(
            snap_label(SnapReportKind::Guide {
                perpendicular: false
            })
            .as_deref(),
            Some("∥")
        );
        assert_eq!(
            snap_label(SnapReportKind::Guide {
                perpendicular: true
            })
            .as_deref(),
            Some("⊥")
        );
        assert_eq!(
            snap_label(SnapReportKind::ControlLine(64)).as_deref(),
            Some("⊥ 64")
        );
        assert_eq!(snap_label(SnapReportKind::CellStep(48)), None);
        assert_eq!(snap_label(SnapReportKind::Step(15)), None);
        assert_eq!(snap_label(SnapReportKind::Node), None);
    }

    #[test]
    fn money_is_formatted_from_cents() {
        assert_eq!(format_money(Cost(0)), "$0.00");
        assert_eq!(format_money(Cost(5)), "$0.05");
        assert_eq!(format_money(Cost(123_456)), "$1,234.56");
        assert_eq!(format_money(Cost(100_000_000)), "$1,000,000.00");
        assert_eq!(format_money(Cost(-250)), "-$2.50");
    }

    #[test]
    fn elevation_levels_step_and_clamp() {
        let mut t = tool(RoadMode::Straight);
        assert_eq!(t.elevation(), ElevationMode::Ground);
        // Lowering from the ground goes under it (T-173), raising comes back.
        t.lower();
        assert_eq!(t.level, 0);
        assert_eq!(t.elevation(), ElevationMode::Tunnel(ROAD_TUNNEL_DEPTH));
        t.raise();
        assert_eq!(t.elevation(), ElevationMode::Ground);
        t.raise();
        assert_eq!(t.elevation(), ElevationMode::Elevated(ELEVATION_STEP));
        for _ in 0..10 {
            t.raise();
        }
        assert_eq!(t.level, MAX_ELEVATION_LEVEL);
        t.lower();
        assert_eq!(t.level, MAX_ELEVATION_LEVEL - 1);
    }

    /// T-164: the metro track tool draws in a tunnel; the height keys leave it alone.
    #[test]
    fn a_tunnel_tool_sends_tunnel_requests_and_ignores_the_height_keys() {
        let mut t = tool(RoadMode::Straight);
        t.tunnel_depth = Some(12.0);
        t.raise();
        assert_eq!(t.level, 0);
        assert_eq!(t.elevation(), ElevationMode::Tunnel(12.0));
        assert!(t.click(Vec2::new(0.0, 0.0)).is_none());
        let req = t.click(Vec2::new(64.0, 0.0)).unwrap();
        assert_eq!(req.elevation, ElevationMode::Tunnel(12.0));
    }

    #[test]
    fn straight_takes_two_clicks() {
        let mut t = tool(RoadMode::Straight);
        assert!(t.path_with_hover(Vec2::ONE).is_none());
        assert!(t.click(Vec2::new(1.0, 2.0)).is_none());
        assert_eq!(
            t.path_with_hover(Vec2::new(5.0, 2.0)),
            Some(RoadPath::Straight {
                from: Vec2::new(1.0, 2.0),
                to: Vec2::new(5.0, 2.0)
            })
        );
        let req = t.click(Vec2::new(9.0, 2.0)).unwrap();
        assert_eq!(
            req.path,
            RoadPath::Straight {
                from: Vec2::new(1.0, 2.0),
                to: Vec2::new(9.0, 2.0)
            }
        );
        assert_eq!(req.snap, SnapOptions::default());
        assert_eq!(
            t.points,
            [Vec2::new(9.0, 2.0)],
            "the chain goes on from the last node"
        );
    }

    #[test]
    fn curve_takes_three_clicks_start_control_end() {
        let mut t = tool(RoadMode::Curve);
        assert!(t.click(Vec2::new(0.0, 0.0)).is_none());
        // After the first click the ghost is a straight line; after the second a curve.
        assert!(matches!(
            t.path_with_hover(Vec2::new(10.0, 0.0)),
            Some(RoadPath::Straight { .. })
        ));
        assert!(t.click(Vec2::new(50.0, 0.0)).is_none());
        assert_eq!(
            t.path_with_hover(Vec2::new(50.0, 50.0)),
            Some(RoadPath::Curve {
                from: Vec2::new(0.0, 0.0),
                control: Vec2::new(50.0, 0.0),
                to: Vec2::new(50.0, 50.0)
            })
        );
        let req = t.click(Vec2::new(50.0, 50.0)).unwrap();
        assert!(matches!(req.path, RoadPath::Curve { .. }));
    }

    #[test]
    fn free_mode_collects_points_until_finished() {
        let mut t = tool(RoadMode::Free);
        for p in [(0.0, 0.0), (30.0, 0.0), (30.0, 30.0)] {
            assert!(t.click(Vec2::new(p.0, p.1)).is_none());
        }
        assert_eq!(
            t.path_with_hover(Vec2::new(60.0, 30.0)),
            Some(RoadPath::Free(vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(30.0, 0.0),
                Vec2::new(30.0, 30.0),
                Vec2::new(60.0, 30.0)
            ]))
        );
        // Clicking on the last point again finishes.
        let req = t.click(Vec2::new(30.0, 30.0)).unwrap();
        assert_eq!(
            req.path,
            RoadPath::Free(vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(30.0, 0.0),
                Vec2::new(30.0, 30.0)
            ])
        );
        assert_eq!(t.points, [Vec2::new(30.0, 30.0)], "the chain goes on");
        // Enter with fewer than two points does nothing.
        t.cancel();
        t.click(Vec2::ZERO);
        assert!(t.finish_free().is_none());
        t.click(Vec2::new(20.0, 0.0));
        assert!(t.finish_free().is_some());
    }

    /// T-176: after a piece the next one starts at its last point; Esc ends the chain.
    #[test]
    fn a_chain_continues_from_the_last_point_in_every_mode_and_cancel_ends_it() {
        let mut t = tool(RoadMode::Straight);
        t.click(Vec2::new(0.0, 0.0));
        t.click(Vec2::new(40.0, 0.0)).unwrap();
        let next = t.click(Vec2::new(40.0, 40.0)).unwrap();
        assert_eq!(
            next.path,
            RoadPath::Straight {
                from: Vec2::new(40.0, 0.0),
                to: Vec2::new(40.0, 40.0)
            },
            "one click: start = the last node"
        );
        t.cancel();
        assert!(t.points.is_empty(), "Esc ends the chain");
        assert!(t.click(Vec2::new(90.0, 90.0)).is_none(), "a fresh start");

        // A curve: the next piece starts at the end and takes control and end again.
        let mut c = tool(RoadMode::Curve);
        for p in [(0.0, 0.0), (30.0, 0.0), (30.0, 30.0)] {
            c.click(Vec2::new(p.0, p.1));
        }
        assert_eq!(c.points, [Vec2::new(30.0, 30.0)]);
        assert!(c.click(Vec2::new(30.0, 60.0)).is_none());
        let req = c.click(Vec2::new(60.0, 60.0)).unwrap();
        assert!(matches!(
            req.path,
            RoadPath::Curve { from, .. } if from == Vec2::new(30.0, 30.0)
        ));

        // Free: finishing keeps the last point as the start of the next polyline.
        let mut f = tool(RoadMode::Free);
        for p in [(0.0, 0.0), (30.0, 0.0)] {
            f.click(Vec2::new(p.0, p.1));
        }
        f.finish_free().unwrap();
        f.click(Vec2::new(30.0, 30.0));
        let req = f.finish_free().unwrap();
        assert_eq!(
            req.path,
            RoadPath::Free(vec![Vec2::new(30.0, 0.0), Vec2::new(30.0, 30.0)])
        );
    }

    /// A piece that does not validate is not sent and leaves the chain as it was; a good one
    /// moves the chain on to its end.
    #[test]
    fn a_red_piece_does_not_drop_the_chain() {
        use std::path::Path;
        use std::sync::Arc;
        let root = env!("CARGO_MANIFEST_DIR");
        let mut sim = cp_sim::SimRunner::new(1);
        sim.set_terrain(Arc::new(
            cp_terrain::Terrain::load(&Path::new(root).join("../../assets/maps/flatlands"))
                .unwrap(),
        ));
        let db = Arc::new(
            cp_content::ContentDb::load_dir(Path::new(root).join("../../assets/content")).unwrap(),
        );
        let road = db.road_id("basic_2l").unwrap();
        sim.set_content(db);
        let view = sim.view();
        let mut t = RoadTool {
            road: Some(road),
            ..Default::default()
        };
        // Outside the bought land: refused.
        t.click(Vec2::new(100.0, 100.0));
        let before = t.clone();
        let req = t.click(Vec2::new(200.0, 100.0)).unwrap();
        assert!(chain_commit(Some(&view), &mut t, before.clone(), req, false).is_none());
        assert_eq!(t, before);
        // On owned land: sent, and the chain goes on from the end.
        t.cancel();
        t.click(Vec2::new(5000.0, 5000.0));
        let before = t.clone();
        let req = t.click(Vec2::new(5096.0, 5000.0)).unwrap();
        let sent = chain_commit(Some(&view), &mut t, before, req, false).expect("valid");
        let RoadPath::Straight { to, .. } = sent.path else {
            panic!("straight")
        };
        assert_eq!(t.points, [to]);
    }

    #[test]
    fn changing_mode_or_cancelling_drops_the_drawing() {
        let mut t = tool(RoadMode::Curve);
        t.click(Vec2::ZERO);
        t.set_mode(RoadMode::Free);
        assert!(t.points.is_empty());
        t.click(Vec2::ZERO);
        t.cancel();
        assert!(t.points.is_empty());
    }

    #[test]
    fn nothing_is_built_without_a_road_type() {
        let mut t = RoadTool::default();
        assert!(t.click(Vec2::ZERO).is_none());
        assert!(t.points.is_empty());
    }
}
