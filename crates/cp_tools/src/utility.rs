//! The pipe and power-line tools: click state machine, ghost preview, link picking.
//!
//! The preview runs [`plan_link_path`] — the function `UtilityCommand::BuildLink` runs — on
//! the same inputs the command reads from the world (graph, tile ownership, terrain, and the
//! building and zone grids), so the ghost and the command agree. A click that completes a
//! drawing produces `SimCommand::Utility(BuildLink { kind, path })`.

use bevy_math::{Vec2, Vec3};
use cp_content::UtilityKind;
use cp_core::curve::{CubicBezier3, to_xz};
use cp_net::build::{GUIDE_RADIUS, ZONE_CELL};
use cp_net::{RoadPath, SnapReport};
use cp_sim::{SimCommand, SimView};

use crate::lattice::{
    EXACT_REACH_M, GuideLine, Joined, LatticeSource, Lx, SECTION_M, WEAK_M, quantize_with,
};
use cp_utility::{
    LinkPlan, PathProblem, PlanInputs, PlanNode, UtilityCommand, UtilityGraph, UtilityLinkId,
    UtilityNet, UtilityNodeId, plan_link_path,
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

/// The cursor `p` of the tool's drawing, on the utility lattice (T-286, [`UtilityLattice`]): the
/// clicked points with the cursor go through the road tool's rules ([`quantize_with`]), the
/// cursor's vertex is where a click puts the point. `alt` switches every snap off (a direct hit on
/// a node or link stays: [`plan_link_path`] snaps it).
fn resolve_cursor(
    view: &SimView,
    kind: UtilityKind,
    points: &[Vec2],
    p: Vec2,
    alt: bool,
) -> (Vec2, SnapReport) {
    // Alt as in the road tool: no snap at all (a direct hit is `plan_link_path`'s).
    if alt || view.content.is_none() {
        return (p, SnapReport::default());
    }
    let src = UtilityLattice {
        view,
        net: net_of(kind),
    };
    let mut pts = points.to_vec();
    pts.push(p);
    let (path, report) = quantize_with(&src, &Lx::default(), &RoadPath::Free(pts), true, true);
    let at = match path {
        RoadPath::Free(v) => v.last().copied().unwrap_or(p),
        _ => p,
    };
    (at, report)
}

/// The cursor `p` of a leg from `prev`, snapped on the utility lattice ([`resolve_cursor`]).
pub fn snap_cursor(
    view: &SimView,
    kind: UtilityKind,
    _tool: &UtilityTool,
    prev: Vec2,
    p: Vec2,
    alt: bool,
) -> (Vec2, SnapReport) {
    resolve_cursor(view, kind, &[prev], p, alt)
}

/// `p` snapped for the tool's current drawing; before the first click the cursor still joins a
/// node or a whole cell of a link of its network.
pub fn snapped_cursor(
    view: &SimView,
    tool: &UtilityTool,
    p: Vec2,
    alt: bool,
) -> (Vec2, SnapReport) {
    resolve_cursor(view, tool.kind, &tool.points, p, alt)
}

/// How far (metres) from one of its links a point joins the network being drawn: half a cell,
/// so a pipe drawn a cell beside its own pipe stays a parallel pipe.
pub const LINK_JOIN_M: f32 = 0.5 * ZONE_CELL;

/// The lattice of the pipe and power-line tools (T-286, §13.5 v7.1): the road tool's rules on
/// one utility network. Only that network joins (a node within a cell, a whole cell of a chain
/// of its links); the roads, the pipes and the power lines guide (their extensions and the
/// squares through their nodes) but never join, so a pipe along a street runs beside it.
pub struct UtilityLattice<'a> {
    pub view: &'a SimView,
    pub net: UtilityNet,
}

impl UtilityLattice<'_> {
    fn graph(&self) -> &UtilityGraph {
        &self.view.utilities
    }

    /// The directions of the links of any network leaving `node`.
    fn link_dirs(&self, node: UtilityNodeId) -> Vec<Vec2> {
        let g = self.graph();
        let Some(n) = g.node(node) else {
            return Vec::new();
        };
        let at = to_xz(n.pos);
        n.links
            .iter()
            .filter_map(|l| g.link(*l))
            .filter_map(|l| {
                let (a, b) = (to_xz(l.curve.p0), to_xz(l.curve.p3));
                let other = if a.distance(at) < 0.5 { b } else { a };
                (other - at).try_normalize()
            })
            .collect()
    }

    /// The chain of links of this network through `link` (across nodes with two links), in
    /// order, as curves running one way.
    fn chain(&self, link: UtilityLinkId) -> Vec<CubicBezier3> {
        let g = self.graph();
        let Some(first) = g.link(link) else {
            return Vec::new();
        };
        let mut seen = vec![link];
        // Walks from `node` away from `from`, returning the curves met (each oriented onward).
        let walk =
            |seen: &mut Vec<UtilityLinkId>, mut node: UtilityNodeId, mut from: UtilityLinkId| {
                let mut out = Vec::new();
                while let Some(n) = g.node(node)
                    && n.links.len() == 2
                {
                    let Some(&next) = n.links.iter().find(|l| **l != from) else {
                        break;
                    };
                    if seen.contains(&next) {
                        break;
                    }
                    let Some(l) = g.link(next).filter(|l| l.net == self.net) else {
                        break;
                    };
                    seen.push(next);
                    let (c, far) = if l.a == node {
                        (l.curve, l.b)
                    } else {
                        (reversed(&l.curve), l.a)
                    };
                    out.push(c);
                    (node, from) = (far, next);
                }
                out
            };
        let ahead = walk(&mut seen, first.b, link);
        let behind = walk(&mut seen, first.a, link);
        let mut curves: Vec<CubicBezier3> = behind.iter().rev().map(reversed).collect();
        curves.push(first.curve);
        curves.extend(ahead);
        curves
    }
}

fn reversed(c: &CubicBezier3) -> CubicBezier3 {
    CubicBezier3 {
        p0: c.p3,
        p1: c.p2,
        p2: c.p1,
        p3: c.p0,
    }
}

/// The whole cell of a chain of straight pieces nearest its point `along` metres from its
/// start (as `lattice::street_cell` for a street): whole cells from the nearer end, a cell clear
/// of both, unless a 10-cell mark from either end is within `WEAK_M`. The point and the chain's
/// direction there; `None` on a chain shorter than two cells.
fn chain_cell(curves: &[CubicBezier3], along: f32) -> Option<(Vec2, Vec2)> {
    let lens: Vec<f32> = curves
        .iter()
        .map(|c| to_xz(c.p0).distance(to_xz(c.p3)))
        .collect();
    let total: f32 = lens.iter().sum();
    if total < 2.0 * ZONE_CELL {
        return None;
    }
    let mut marks = Vec::new();
    let mut k = SECTION_M;
    while k < total - 1.0 {
        marks.extend([k, total - k]);
        k += SECTION_M;
    }
    let at = match marks
        .iter()
        .copied()
        .filter(|m| (m - along).abs() <= WEAK_M)
        .min_by(|a, b| (a - along).abs().total_cmp(&(b - along).abs()))
    {
        Some(m) => m,
        None => {
            let cells = |k: f32| (k / ZONE_CELL).round() * ZONE_CELL;
            if along <= total / 2.0 {
                cells(along)
            } else {
                total - cells(total - along)
            }
            .clamp(ZONE_CELL, total - ZONE_CELL)
        }
    };
    let mut walked = 0.0;
    for (c, len) in curves.iter().zip(&lens) {
        if at <= walked + len + 1e-3 {
            let (a, b) = (to_xz(c.p0), to_xz(c.p3));
            let d = (b - a).try_normalize()?;
            return Some((a + d * (at - walked).clamp(0.0, *len), d));
        }
        walked += len;
    }
    None
}

impl LatticeSource for UtilityLattice<'_> {
    type Line = UtilityLinkId;

    fn join(&self, p: Vec2) -> Option<Joined<UtilityLinkId>> {
        let g = self.graph();
        if let Some(id) = g.nearest_node(self.net, p, ZONE_CELL) {
            let n = g.node(id)?;
            return Some(Joined {
                pos: to_xz(n.pos),
                node: Some(self.link_dirs(id)),
                along: None,
                line: None,
            });
        }
        let (link, _, _) = g.nearest_link(self.net, p, LINK_JOIN_M)?;
        let curves = self.chain(link);
        // The cursor's distance along the chain.
        let mut walked = 0.0;
        let mut along = None;
        let mut best = f32::MAX;
        for c in &curves {
            let (a, b) = (to_xz(c.p0), to_xz(c.p3));
            let len = a.distance(b);
            let d = (b - a).normalize_or_zero();
            let k = (p - a).dot(d).clamp(0.0, len);
            let miss = (a + d * k).distance(p);
            if miss < best {
                best = miss;
                along = Some(walked + k);
            }
            walked += len;
        }
        let (pos, dir) = chain_cell(&curves, along?)?;
        Some(Joined {
            pos,
            node: None,
            along: Some(dir),
            line: Some(link),
        })
    }

    fn square_on_line(
        &self,
        link: UtilityLinkId,
        from: Vec2,
        refs: &[Vec2],
        p: Vec2,
    ) -> Option<(Vec2, Vec2)> {
        crate::lattice::square_on_curves(&self.chain(link), ZONE_CELL, from, refs, p)
    }

    fn guide_nodes(&self, near: Vec2, radius: f32) -> Vec<(Vec2, Vec<Vec2>)> {
        let roads = crate::lattice::RoadLattice {
            graph: &self.view.net,
            content: self
                .view
                .content
                .as_deref()
                .expect("a lattice needs content"),
        };
        let mut out = roads.guide_nodes(near, radius);
        out.extend(
            self.graph()
                .nodes()
                .filter(|(_, n)| to_xz(n.pos).distance(near) <= radius)
                .map(|(id, n)| (to_xz(n.pos), self.link_dirs(id))),
        );
        out
    }

    fn guide_lines(&self, near: Vec2, radius: f32) -> Vec<GuideLine> {
        let roads = crate::lattice::RoadLattice {
            graph: &self.view.net,
            content: self
                .view
                .content
                .as_deref()
                .expect("a lattice needs content"),
        };
        // Straight streets only: a curve's chord is no line a pipe follows.
        let mut out: Vec<GuideLine> = roads
            .guide_lines(near, radius)
            .into_iter()
            .filter(|g| g.straight)
            .collect();
        out.extend(
            self.graph()
                .links()
                .filter(|(_, l)| l.curve.bounds_distance_xz(near) <= radius)
                .filter_map(|(_, l)| {
                    let a = to_xz(l.curve.p0);
                    Some(GuideLine {
                        at: a,
                        dir: (to_xz(l.curve.p3) - a).try_normalize()?,
                        straight: true,
                    })
                }),
        );
        out
    }

    fn exact_dirs(&self, at: Vec2) -> Vec<Vec2> {
        let roads = crate::lattice::RoadLattice {
            graph: &self.view.net,
            content: self
                .view
                .content
                .as_deref()
                .expect("a lattice needs content"),
        };
        let mut out = roads.exact_dirs(at);
        out.extend(
            self.graph()
                .links()
                .filter(|(_, l)| l.curve.bounds_distance_xz(at) <= EXACT_REACH_M)
                .filter_map(|(_, l)| (to_xz(l.curve.p3) - to_xz(l.curve.p0)).try_normalize()),
        );
        out
    }

    fn free_refs(&self, p: Vec2) -> Vec<Vec2> {
        // The direction of the nearest straight ground street within the guide radius.
        self.view
            .net
            .segments()
            .filter(|(_, s)| {
                s.elevation == cp_net::Elevation::Ground
                    && crate::lattice::is_straight(&s.curve)
                    && s.curve.bounds_distance_xz(p) <= GUIDE_RADIUS
            })
            .map(|(_, s)| (s.curve.nearest_xz(p).1, s))
            .filter(|(d, _)| *d <= GUIDE_RADIUS)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .and_then(|(_, s)| (to_xz(s.curve.p3) - to_xz(s.curve.p0)).try_normalize())
            .into_iter()
            .collect()
    }
}

/// The inputs [`plan_link_path`] reads from the view, as the command reads them from the world.
fn plan_inputs(view: &SimView) -> PlanInputs<'_> {
    PlanInputs {
        tiles: &view.tiles,
        terrain: view.terrain.as_deref(),
        buildings: Some((&view.buildings, &view.zones)),
    }
}

/// The cursor of the pipe and power-line tools before the first click (T-286, §13.5 v7.1), at
/// the point a click puts down: a pipe as a 1-cell piece along [`crate::lattice::probe_dir`]
/// (its reach band around it shows the reach), a line as one pole. Green when it can be built:
/// the piece validates; a pole stands on owned land and on no building.
pub fn cursor_piece(
    view: &SimView,
    tool: &UtilityTool,
    hover: Vec2,
    alt: bool,
) -> Option<crate::Ghost> {
    view.content.as_deref()?;
    let (at, snap) = snapped_cursor(view, tool, hover, alt);
    let lift = |p: Vec2| {
        let y = view.terrain.as_deref().map_or(0.0, |t| t.height(p));
        Vec3::new(p.x, y + 0.5, p.y)
    };
    let (polyline, valid) = if tool.kind == UtilityKind::Power {
        let free = view.tiles.contains_point(at)
            && cp_utility::plan::building_on_leg(&plan_inputs(view), at, at).is_none();
        (vec![lift(at), lift(at)], free)
    } else {
        let src = UtilityLattice {
            view,
            net: net_of(tool.kind),
        };
        let b = at + crate::lattice::probe_dir(&src, at, hover) * ZONE_CELL;
        let ok = matches!(preview(view, tool.kind, &[at, b]), Some(Ok(_)));
        (vec![lift(at), lift(b)], ok)
    };
    Some(crate::Ghost {
        polylines: vec![polyline],
        valid,
        snap,
        hover: Some(hover),
        resolved: Some(at),
        cursor_piece: true,
        ..Default::default()
    })
}

/// The buildings a link along `path` of `kind` would connect (T-286): newly in the reach of its
/// network ([`cp_utility::newly_in_reach`]).
pub fn connects(view: &SimView, kind: UtilityKind, path: &[Vec2]) -> Vec<cp_building::BuildingId> {
    let Some(db) = view.content.as_deref() else {
        return Vec::new();
    };
    cp_utility::newly_in_reach(
        &view.utilities,
        net_of(kind),
        &view.buildings,
        db,
        &db.balance().networks,
        path,
    )
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
    Some(plan_link_path(
        &view.utilities,
        &plan_inputs(view),
        def,
        path,
    ))
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
        // The bulldozer in the water layer (T-286): the pipes stay in sight.
        ActiveTool::Bulldozer => state.bulldoze_layer == crate::BulldozeLayer::Water,
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
        // The bulldozer in the water layer, not the power layer (T-286).
        st.bulldoze_in(crate::BulldozeLayer::Water);
        assert!(vis(&st, Overlay::Off));
        st.bulldoze_in(crate::BulldozeLayer::Power);
        assert!(!vis(&st, Overlay::Off));
        st.bulldoze_layer = crate::BulldozeLayer::All;
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
