//! The pipe and power-line tools: click state machine, ghost preview, link picking.
//!
//! The preview runs [`plan_link_path`] — the function `UtilityCommand::BuildLink` runs — on
//! the same inputs the command reads from the world (graph, tile ownership, terrain, and the
//! building and zone grids), so the ghost and the command agree. A click that completes a
//! drawing produces `SimCommand::Utility(BuildLink { kind, path })`.

use bevy_math::{Vec2, Vec3};
use cp_content::UtilityKind;
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
            return Some(std::mem::take(&mut self.points));
        }
        None
    }

    /// Ends a polyline drawing (needs at least two points).
    pub fn finish(&mut self) -> Option<Vec<Vec2>> {
        if self.mode != UtilityMode::Polyline || self.points.len() < 2 {
            return None;
        }
        Some(std::mem::take(&mut self.points))
    }

    /// The command a finished `path` becomes.
    pub fn command(&self, path: Vec<Vec2>) -> SimCommand {
        SimCommand::Utility(UtilityCommand::BuildLink {
            kind: self.kind,
            path,
        })
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
        assert!(t.points.is_empty(), "ready for the next line");
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
        t.click(Vec2::ZERO);
        assert!(t.finish().is_none());
        t.click(Vec2::new(20.0, 0.0));
        assert_eq!(t.finish().map(|p| p.len()), Some(2));
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
}
