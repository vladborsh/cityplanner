//! The pipe and power-line tools: click state machine, ghost preview, link picking.
//!
//! The preview runs [`plan_link_path`] — the function `UtilityCommand::BuildLink` runs — on
//! the same inputs the command reads from the world (graph, tile ownership, terrain, and the
//! building and zone grids), so the ghost and the command agree. A click that completes a
//! drawing produces `SimCommand::Utility(BuildLink { kind, path })`.

use bevy_math::{Vec2, Vec3};
use cp_content::UtilityKind;
use cp_net::build::{
    ANGLE_SNAP_TOL_DEG, ANGLE_STEP_DEG, GUIDE_HALF_LENGTH, GUIDE_RADIUS, KEY_ANGLE_TOL_DEG,
};
use cp_net::{SnapReport, SnapReportKind};
use cp_sim::{SimCommand, SimView};
use cp_utility::{
    LinkPlan, PathProblem, PlanInputs, PlanNode, UtilityCommand, UtilityGraph, UtilityLinkId,
    UtilityNet, plan_link_path,
};

/// Pick radius of the remove tool, metres.
pub const LINK_PICK_RADIUS: f32 = 8.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UtilityMode {
    /// Two clicks: start, end.
    #[default]
    Straight,
    /// A polyline; Enter (or a click on the last point) finishes it.
    Polyline,
}

/// State of the pipe / power-line tool.
#[derive(Clone, Debug, PartialEq)]
pub struct UtilityTool {
    /// `Water` (the one pipe) or `Power` lines.
    pub kind: UtilityKind,
    pub mode: UtilityMode,
    /// Points clicked so far in the current drawing.
    pub points: Vec<Vec2>,
}

impl Default for UtilityTool {
    fn default() -> Self {
        Self {
            kind: UtilityKind::Water,
            mode: UtilityMode::default(),
            points: Vec::new(),
        }
    }
}

impl UtilityTool {
    pub fn set_kind(&mut self, kind: UtilityKind) {
        // One pipe for water and sewage: the tool only builds `Water` or `Power`.
        let kind = if kind == UtilityKind::Sewage {
            UtilityKind::Water
        } else {
            kind
        };
        if self.kind != kind {
            self.kind = kind;
            self.points.clear();
        }
    }

    pub fn set_mode(&mut self, mode: UtilityMode) {
        if self.mode != mode {
            self.mode = mode;
            self.points.clear();
        }
    }

    pub fn cancel(&mut self) {
        self.points.clear();
    }

    /// The path as it would be with the cursor at `hover` (what the ghost shows); nothing to
    /// preview before the first click.
    pub fn path_with_hover(&self, hover: Vec2) -> Option<Vec<Vec2>> {
        self.points.first()?;
        let mut path = self.points.clone();
        path.push(hover);
        Some(path)
    }

    /// A click at `p`. Returns the finished path when this click completed the drawing.
    pub fn click(&mut self, p: Vec2) -> Option<Vec<Vec2>> {
        if self.mode == UtilityMode::Polyline
            && self.points.last().is_some_and(|l| l.distance(p) < 1.0)
        {
            return self.finish();
        }
        self.points.push(p);
        if self.mode == UtilityMode::Straight && self.points.len() == 2 {
            // A chain (T-176): the next piece starts where this one ends.
            return Some(std::mem::replace(&mut self.points, vec![p]));
        }
        None
    }

    /// Ends a polyline drawing (needs at least two points).
    pub fn finish(&mut self) -> Option<Vec<Vec2>> {
        if self.mode != UtilityMode::Polyline || self.points.len() < 2 {
            return None;
        }
        let last = *self.points.last()?;
        Some(std::mem::replace(&mut self.points, vec![last]))
    }

    /// The command a finished `path` becomes.
    pub fn command(&self, path: Vec<Vec2>) -> SimCommand {
        SimCommand::Utility(UtilityCommand::BuildLink {
            kind: self.kind,
            path,
        })
    }
}

/// Settles a click or Enter that completed a piece (T-176), like `road::chain_commit`: a path
/// that does not validate is not sent and `tool` returns to `before` (the chain is kept).
pub fn chain_commit(
    view: Option<&SimView>,
    tool: &mut UtilityTool,
    before: UtilityTool,
    path: Vec<Vec2>,
) -> Option<SimCommand> {
    if let Some(v) = view
        && matches!(preview(v, tool.kind, &path), Some(Err(_)))
    {
        *tool = before;
        return None;
    }
    Some(tool.command(path))
}

/// Radius within which the cursor jumps to a node of the network being drawn, metres.
pub const NODE_PULL_RADIUS: f32 = 12.0;
/// The key angles from a neighbouring direction, degrees (as the road tool's).
const KEY_ANGLES: [f32; 6] = [45.0, -45.0, 90.0, -90.0, 135.0, -135.0];

fn rotate(v: Vec2, deg: f32) -> Vec2 {
    let (s, c) = deg.to_radians().sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Signed angle in degrees from `a` to `b`, in `(-180, 180]`.
fn signed_deg(a: Vec2, b: Vec2) -> f32 {
    let ang = (b.y.atan2(b.x) - a.y.atan2(a.x)).to_degrees();
    (ang + 180.0).rem_euclid(360.0) - 180.0
}

/// The directions a leg starting at `from` is measured against: the links of the network that
/// leave a node at `from`, and the leg before `from` in the drawing, turned round (so that
/// 180° is straight on and 90° is a right angle either way).
fn direction_refs(view: &SimView, net: UtilityNet, tool: &UtilityTool, from: Vec2) -> Vec<Vec2> {
    let mut refs = Vec::new();
    if let [.., before, last] = tool.points.as_slice()
        && last.distance(from) < 0.5
        && let Some(d) = (*before - *last).try_normalize()
    {
        refs.push(d);
    }
    let g = &*view.utilities;
    for (_, n) in g.nodes().filter(|(_, n)| n.net == net) {
        if Vec2::new(n.pos.x, n.pos.z).distance(from) > 0.5 {
            continue;
        }
        for l in &n.links {
            let Some(link) = g.link(*l) else { continue };
            let (a, b) = (
                Vec2::new(link.curve.p0.x, link.curve.p0.z),
                Vec2::new(link.curve.p3.x, link.curve.p3.z),
            );
            let other = if a.distance(from) < 0.5 { b } else { a };
            if let Some(d) = (other - from).try_normalize() {
                refs.push(d);
            }
        }
    }
    refs
}

/// The lines the cursor may run parallel or perpendicular to: roads (ground) and links of the
/// same network within [`GUIDE_RADIUS`] of `p`, as unit tangents.
fn guide_dirs(view: &SimView, net: UtilityNet, p: Vec2) -> Vec<Vec2> {
    let mut out = Vec::new();
    for (_, s) in view.net.segments() {
        if s.elevation != cp_net::Elevation::Ground
            || s.curve.bounds_distance_xz(p) > GUIDE_RADIUS + 1e-3
        {
            continue;
        }
        let (t, d) = s.curve.nearest_xz(p);
        let tan = s.curve.tangent(t);
        if d <= GUIDE_RADIUS
            && let Some(u) = Vec2::new(tan.x, tan.z).try_normalize()
        {
            out.push(u);
        }
    }
    for (_, l) in view.utilities.links().filter(|(_, l)| l.net == net) {
        let (a, b) = (
            Vec2::new(l.curve.p0.x, l.curve.p0.z),
            Vec2::new(l.curve.p3.x, l.curve.p3.z),
        );
        let ab = b - a;
        let t = if ab.length_squared() > 1e-6 {
            ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if (a + ab * t).distance(p) <= GUIDE_RADIUS
            && let Some(u) = ab.try_normalize()
        {
            out.push(u);
        }
    }
    out
}

/// The cursor `p` of a leg from `prev`, snapped the way the road tool does (T-182): to a node
/// of the network within [`NODE_PULL_RADIUS`]; else the key angles (45/90/135°) from the
/// directions at `prev` and the guides parallel or perpendicular to a road or a link near the
/// cursor, within [`KEY_ANGLE_TOL_DEG`] — the smaller deviation wins, a key angle on a tie —
/// then the [`ANGLE_STEP_DEG`] step. Returns the new point and what snapped it. `alt` turns
/// every snap off.
pub fn snap_cursor(
    view: &SimView,
    kind: UtilityKind,
    tool: &UtilityTool,
    prev: Vec2,
    p: Vec2,
    alt: bool,
) -> (Vec2, SnapReport) {
    let none = (p, SnapReport::default());
    if alt {
        return none;
    }
    let net = net_of(kind);
    let nearest = view
        .utilities
        .nodes()
        .filter(|(_, n)| n.net == net)
        .map(|(_, n)| Vec2::new(n.pos.x, n.pos.z))
        .filter(|q| q.distance(p) <= NODE_PULL_RADIUS && q.distance(prev) > 1e-3)
        .min_by(|a, b| a.distance(p).total_cmp(&b.distance(p)));
    if let Some(q) = nearest {
        return (
            q,
            SnapReport {
                kind: SnapReportKind::Node,
                guide: None,
            },
        );
    }
    let d = p - prev;
    let len = d.length();
    if len < 1e-3 {
        return none;
    }
    let refs = direction_refs(view, net, tool, prev);
    // (deviation, direction, report kind)
    let mut best: Option<(f32, Vec2, SnapReportKind, bool)> = None;
    let mut consider = |dev: f32, dir: Vec2, kind: SnapReportKind, guide: bool| {
        if dev <= KEY_ANGLE_TOL_DEG && best.is_none_or(|(b, ..)| dev < b) {
            best = Some((dev, dir, kind, guide));
        }
    };
    for &r in &refs {
        let rel = signed_deg(r, d);
        for target in KEY_ANGLES {
            consider(
                (rel - target).abs(),
                rotate(r, target),
                SnapReportKind::KeyAngle(target.abs() as i32),
                false,
            );
        }
    }
    for tan in guide_dirs(view, net, p) {
        for (dir, perpendicular) in [(tan, false), (Vec2::new(-tan.y, tan.x), true)] {
            let dir = if d.dot(dir) >= 0.0 { dir } else { -dir };
            consider(
                signed_deg(dir, d).abs(),
                dir,
                SnapReportKind::Guide { perpendicular },
                true,
            );
        }
    }
    if let Some((_, dir, kind, guide)) = best {
        // A key angle keeps the leg's length; a guide takes the cursor's projection.
        let at = if guide { d.dot(dir) } else { len };
        let pos = prev + dir * at;
        return (
            pos,
            SnapReport {
                kind,
                guide: Some((pos - dir * GUIDE_HALF_LENGTH, pos + dir * GUIDE_HALF_LENGTH)),
            },
        );
    }
    // The 15° step from the best direction.
    let mut step: Option<(f32, Vec2, i32)> = None;
    for &r in &refs {
        let ang = signed_deg(r, d);
        let n = (ang / ANGLE_STEP_DEG).round();
        let dev = (ang - n * ANGLE_STEP_DEG).abs();
        if dev <= ANGLE_SNAP_TOL_DEG && step.is_none_or(|(b, ..)| dev < b) {
            step = Some((
                dev,
                rotate(r, n * ANGLE_STEP_DEG),
                (n * ANGLE_STEP_DEG) as i32,
            ));
        }
    }
    match step {
        Some((_, dir, deg)) => (
            prev + dir * len,
            SnapReport {
                kind: SnapReportKind::Step(deg),
                guide: None,
            },
        ),
        None => none,
    }
}

/// `p` snapped for the tool's current drawing; unchanged before the first click (the first
/// point may still jump to a node).
pub fn snapped_cursor(
    view: &SimView,
    tool: &UtilityTool,
    p: Vec2,
    alt: bool,
) -> (Vec2, SnapReport) {
    match tool.points.last() {
        Some(&prev) => snap_cursor(view, tool.kind, tool, prev, p, alt),
        None if alt => (p, SnapReport::default()),
        None => {
            let net = net_of(tool.kind);
            let node = view
                .utilities
                .nodes()
                .filter(|(_, n)| n.net == net)
                .map(|(_, n)| Vec2::new(n.pos.x, n.pos.z))
                .filter(|q| q.distance(p) <= NODE_PULL_RADIUS)
                .min_by(|a, b| a.distance(p).total_cmp(&b.distance(p)));
            match node {
                Some(q) => (
                    q,
                    SnapReport {
                        kind: SnapReportKind::Node,
                        guide: None,
                    },
                ),
                None => (p, SnapReport::default()),
            }
        }
    }
}

/// The network a kind builds into.
pub fn net_of(kind: UtilityKind) -> UtilityNet {
    UtilityNet::of(kind)
}

/// Validates `path` against the snapshot with the same pure function `BuildLink` uses.
/// `None` when the view has no terrain, content or definition for `kind`.
pub fn preview(
    view: &SimView,
    kind: UtilityKind,
    path: &[Vec2],
) -> Option<Result<LinkPlan, Vec<PathProblem>>> {
    view.terrain.as_deref()?;
    let (_, def) = view.content.as_deref()?.utility_for_kind(kind)?;
    let inputs = PlanInputs {
        tiles: &view.tiles,
        terrain: view.terrain.as_deref(),
        buildings: Some((&view.buildings, &view.zones)),
    };
    Some(plan_link_path(&view.utilities, &inputs, def, path))
}

/// The pieces of a plan as two-point polylines (for the ghost), at node height.
pub fn plan_polylines(graph: &UtilityGraph, plan: &LinkPlan) -> Vec<Vec<Vec3>> {
    let pos = |n: PlanNode| match n {
        PlanNode::Existing(id) => graph.node(id).map_or(Vec3::ZERO, |n| n.pos),
        PlanNode::New(i) => plan.new_nodes[i],
        PlanNode::Split(i) => plan.splits[i].pos,
    };
    plan.links
        .iter()
        .map(|l| vec![pos(l.a), pos(l.b)])
        .collect()
}

/// Whether the pipe network is drawn (T-103): while a water tool is active (the pipe tool,
/// removing pipes, placing a pump or an outflow) or the water overlay is on.
pub fn pipes_visible(
    state: &crate::ToolState,
    overlay: cp_render::Overlay,
    content: Option<&cp_content::ContentDb>,
) -> bool {
    use crate::ActiveTool;
    if overlay == cp_render::Overlay::Water {
        return true;
    }
    match state.active {
        ActiveTool::Utility => state.utility.kind == UtilityKind::Water,
        ActiveTool::UtilityRemove => state.remove_net == Some(UtilityNet::Pipe),
        ActiveTool::Place => content.is_some_and(|db| {
            state
                .building
                .def
                .and_then(|def| db.building(def).service.as_deref())
                .and_then(|name| db.service_id(name))
                .is_some_and(|svc| {
                    matches!(
                        db.service(svc).kind,
                        cp_content::ServiceKind::Water | cp_content::ServiceKind::Sewage
                    )
                })
        }),
        _ => false,
    }
}

/// The link of `net` nearest to `p` within `radius` (the remove tool's target).
pub fn pick_link(
    graph: &UtilityGraph,
    net: UtilityNet,
    p: Vec2,
    radius: f32,
) -> Option<UtilityLinkId> {
    graph.nearest_link(net, p, radius).map(|(id, _, _)| id)
}

/// A link's centre line as points (for highlighting).
pub fn link_line(graph: &UtilityGraph, id: UtilityLinkId) -> Vec<Vec3> {
    graph
        .link(id)
        .map(|l| vec![l.curve.point(0.0), l.curve.point(1.0)])
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    use cp_content::ContentDb;
    use cp_sim::SimRunner;
    use cp_terrain::{Terrain, TileOwnership};

    use crate::CommandOutbox;

    const ROOT: &str = env!("CARGO_MANIFEST_DIR");

    fn sim() -> SimRunner {
        let mut sim = SimRunner::new(1);
        sim.set_terrain(Arc::new(
            Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap(),
        ));
        sim.set_content(Arc::new(
            ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap(),
        ));
        sim
    }

    fn tool(kind: UtilityKind, mode: UtilityMode) -> UtilityTool {
        UtilityTool {
            kind,
            mode,
            points: vec![],
        }
    }

    #[test]
    fn straight_takes_two_clicks_and_becomes_a_build_link_command() {
        let mut t = tool(UtilityKind::Power, UtilityMode::Straight);
        assert!(t.path_with_hover(Vec2::ONE).is_none());
        assert!(t.click(Vec2::new(10.0, 10.0)).is_none());
        assert_eq!(
            t.path_with_hover(Vec2::new(50.0, 10.0)),
            Some(vec![Vec2::new(10.0, 10.0), Vec2::new(50.0, 10.0)])
        );
        let path = t.click(Vec2::new(90.0, 10.0)).unwrap();
        assert_eq!(t.points, [Vec2::new(90.0, 10.0)], "the chain goes on");
        assert_eq!(
            t.command(path.clone()),
            SimCommand::Utility(UtilityCommand::BuildLink {
                kind: UtilityKind::Power,
                path
            })
        );
    }

    #[test]
    fn polyline_collects_points_until_finished() {
        let mut t = tool(UtilityKind::Water, UtilityMode::Polyline);
        for p in [(0.0, 0.0), (30.0, 0.0), (30.0, 30.0)] {
            assert!(t.click(Vec2::new(p.0, p.1)).is_none());
        }
        let path = t.click(Vec2::new(30.0, 30.0)).unwrap();
        assert_eq!(path.len(), 3);
        // Enter with fewer than two points does nothing; with two it finishes.
        t.cancel();
        t.click(Vec2::ZERO);
        assert!(t.finish().is_none());
        t.click(Vec2::new(20.0, 0.0));
        assert_eq!(t.finish().map(|p| p.len()), Some(2));
    }

    /// T-176: pipes and lines chain too; Esc (`cancel`) ends the chain.
    #[test]
    fn a_chain_continues_from_the_last_point_and_cancel_ends_it() {
        let mut t = tool(UtilityKind::Power, UtilityMode::Straight);
        t.click(Vec2::new(0.0, 0.0));
        t.click(Vec2::new(40.0, 0.0)).unwrap();
        let next = t.click(Vec2::new(40.0, 40.0)).unwrap();
        assert_eq!(next, [Vec2::new(40.0, 0.0), Vec2::new(40.0, 40.0)]);
        t.cancel();
        assert!(t.click(Vec2::new(90.0, 90.0)).is_none(), "a fresh start");

        let mut p = tool(UtilityKind::Water, UtilityMode::Polyline);
        for q in [(0.0, 0.0), (30.0, 0.0)] {
            p.click(Vec2::new(q.0, q.1));
        }
        p.finish().unwrap();
        p.click(Vec2::new(30.0, 30.0));
        assert_eq!(
            p.finish().unwrap(),
            [Vec2::new(30.0, 0.0), Vec2::new(30.0, 30.0)]
        );
    }

    /// A piece that does not validate is not sent and leaves the chain as it was.
    #[test]
    fn a_red_piece_does_not_drop_the_chain() {
        let sim = sim();
        let view = sim.view();
        let mut t = tool(UtilityKind::Water, UtilityMode::Straight);
        // Far outside the bought land: the plan is refused.
        let (a, b) = (Vec2::new(100.0, 100.0), Vec2::new(190.0, 100.0));
        t.click(a);
        let before = t.clone();
        let path = t.click(b).unwrap();
        assert!(chain_commit(Some(&view), &mut t, before.clone(), path).is_none());
        assert_eq!(t, before, "still waiting for the end of the piece");
        // On owned land the same steps send a command and move the chain on.
        let (a, b) = (Vec2::new(5000.0, 5000.0), Vec2::new(5090.0, 5000.0));
        t.cancel();
        t.click(a);
        let before = t.clone();
        let path = t.click(b).unwrap();
        assert!(chain_commit(Some(&view), &mut t, before, path).is_some());
        assert_eq!(t.points, [b]);
    }

    #[test]
    fn changing_kind_mode_or_cancelling_drops_the_drawing() {
        let mut t = tool(UtilityKind::Water, UtilityMode::Polyline);
        t.click(Vec2::ZERO);
        t.set_kind(UtilityKind::Power);
        assert!(t.points.is_empty());
        t.click(Vec2::ZERO);
        t.set_mode(UtilityMode::Straight);
        assert!(t.points.is_empty());
        t.click(Vec2::ZERO);
        t.cancel();
        assert!(t.points.is_empty());
    }

    #[test]
    fn ghost_and_command_agree_on_validity_and_cost() {
        let mut sim = sim();
        let (a, b) = (Vec2::new(5000.0, 5000.0), Vec2::new(5090.0, 5000.0));
        let view = sim.view();
        let plan = preview(&view, UtilityKind::Water, &[a, b])
            .unwrap()
            .expect("owned land is buildable");
        assert!(plan.cost.0 > 0);
        let lines = plan_polylines(&view.utilities, &plan);
        assert_eq!(lines.len(), plan.links.len());

        // Clicking through the tool and flushing the outbox builds exactly the plan.
        let mut t = tool(UtilityKind::Water, UtilityMode::Straight);
        let mut outbox = CommandOutbox::default();
        t.click(a);
        let path = t.click(b).unwrap();
        outbox.push(t.command(path));
        outbox.flush_into(&mut sim);
        sim.tick_once();
        let built = sim.view();
        assert_eq!(built.utilities.link_count(), plan.links.len());
        assert!(!Arc::ptr_eq(&view.utilities, &built.utilities));
        let length: f32 = built.utilities.links().map(|(_, l)| l.length).sum();
        assert!((length - 90.0).abs() < 0.5);

        // A second view of an unchanged graph shares the snapshot.
        assert!(Arc::ptr_eq(&built.utilities, &sim.view().utilities));
    }

    #[test]
    fn invalid_ghost_matches_the_rejected_command() {
        let mut sim = sim();
        sim.world_mut().insert_resource(TileOwnership::only((0, 0)));
        let far = [Vec2::new(5000.0, 5000.0), Vec2::new(5090.0, 5000.0)];
        let view = sim.view();
        let problems = preview(&view, UtilityKind::Power, &far)
            .unwrap()
            .expect_err("not owned");
        assert!(
            problems
                .iter()
                .any(|p| matches!(p, PathProblem::TileNotOwned { .. }))
        );
        assert_eq!(problems[0].to_string(), "outside the land you own");

        let mut outbox = CommandOutbox::default();
        outbox.push(tool(UtilityKind::Power, UtilityMode::Straight).command(far.to_vec()));
        outbox.flush_into(&mut sim);
        assert_eq!(sim.view().utilities.link_count(), 0, "the sim rejected it");
        // Too short a leg is reported the same way by both.
        let short = [Vec2::new(10.0, 10.0), Vec2::new(11.0, 10.0)];
        assert!(preview(&view, UtilityKind::Power, &short).unwrap().is_err());
    }

    #[test]
    fn the_remove_tool_picks_the_nearest_link_of_its_network_and_the_command_removes_it() {
        let mut sim = sim();
        let mut outbox = CommandOutbox::default();
        let build = |kind, y| {
            tool(kind, UtilityMode::Straight)
                .command(vec![Vec2::new(5000.0, y), Vec2::new(5090.0, y)])
        };
        outbox.push(build(UtilityKind::Water, 5000.0));
        outbox.push(build(UtilityKind::Power, 5040.0));
        outbox.flush_into(&mut sim);
        let view = sim.view();
        assert_eq!(view.utilities.link_count(), 2);

        let at = Vec2::new(5045.0, 5003.0);
        assert!(pick_link(&view.utilities, UtilityNet::Power, at, LINK_PICK_RADIUS).is_none());
        let link = pick_link(&view.utilities, UtilityNet::Pipe, at, LINK_PICK_RADIUS).unwrap();
        assert_eq!(link_line(&view.utilities, link).len(), 2);
        outbox.push(SimCommand::Utility(UtilityCommand::RemoveLink { link }));
        outbox.flush_into(&mut sim);
        let after = sim.view();
        assert_eq!(after.utilities.link_count(), 1);
        assert!(after.utilities.link(link).is_none());
    }

    #[test]
    fn pipes_are_visible_with_water_tools_and_the_water_overlay_only() {
        use crate::{ActiveTool, ToolState};
        use cp_render::Overlay;
        let db = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
        let mut st = ToolState::default();
        let vis = |st: &ToolState, o| pipes_visible(st, o, Some(&db));
        // No tool, no overlay: hidden. The water overlay alone shows them.
        assert!(!vis(&st, Overlay::Off));
        assert!(vis(&st, Overlay::Water));
        assert!(!vis(&st, Overlay::Traffic) && !vis(&st, Overlay::Power));
        // The pipe tool, not the power-line tool.
        st.active = ActiveTool::Utility;
        st.utility.set_kind(UtilityKind::Water);
        assert!(vis(&st, Overlay::Off));
        st.utility.set_kind(UtilityKind::Power);
        assert!(!vis(&st, Overlay::Off));
        // Removing pipes, not lines.
        st.active = ActiveTool::UtilityRemove;
        st.remove_net = Some(UtilityNet::Pipe);
        assert!(vis(&st, Overlay::Off));
        st.remove_net = Some(UtilityNet::Power);
        assert!(!vis(&st, Overlay::Off));
        // Placing a pump or an outflow, not a power plant.
        st.active = ActiveTool::Place;
        for (name, want) in [
            ("water_pump_station", true),
            ("sewage_outflow_station", true),
            ("coal_power_plant", false),
            ("wind_turbine", false),
        ] {
            st.building.def = db.building_id(name);
            assert!(st.building.def.is_some(), "{name}");
            assert_eq!(vis(&st, Overlay::Off), want, "{name}");
        }
        // Roads and the rest.
        for tool in [
            ActiveTool::None,
            ActiveTool::Road,
            ActiveTool::Bulldozer,
            ActiveTool::Zone,
        ] {
            st.active = tool;
            assert!(!vis(&st, Overlay::Off), "{tool:?}");
        }
    }
}
