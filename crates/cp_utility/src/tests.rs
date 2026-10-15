use bevy_math::Vec2;
use cp_content::{ContentDb, UtilityKind};
use cp_core::curve::to_xz;
use cp_net::segment_cost;
use cp_terrain::TileOwnership;

use super::*;

fn content() -> ContentDb {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
    ContentDb::load_dir(root).unwrap_or_else(|e| panic!("{e}"))
}

fn def(db: &ContentDb, kind: UtilityKind) -> cp_content::UtilityDef {
    db.utility_for_kind(kind).expect("utility def").1.clone()
}

fn v(x: f32, z: f32) -> Vec2 {
    Vec2::new(5000.0 + x, 5000.0 + z)
}

struct T {
    db: ContentDb,
    tiles: TileOwnership,
    graph: UtilityGraph,
}

impl T {
    fn new() -> Self {
        Self {
            db: content(),
            tiles: TileOwnership::only((2, 2)),
            graph: UtilityGraph::new(),
        }
    }
    fn plan(&self, kind: UtilityKind, path: &[Vec2]) -> Result<LinkPlan, Vec<PathProblem>> {
        let inputs = PlanInputs {
            tiles: &self.tiles,
            terrain: None,
            buildings: None,
        };
        plan_link_path(&self.graph, &inputs, &def(&self.db, kind), path)
    }
    fn build(&mut self, kind: UtilityKind, path: &[Vec2]) -> LinkPlan {
        let plan = self.plan(kind, path).expect("buildable");
        apply_plan(&mut self.graph, &plan).unwrap();
        plan
    }
}

#[test]
fn content_has_a_definition_per_network_and_sewage_uses_the_pipe() {
    let db = content();
    for k in [UtilityKind::Water, UtilityKind::Power] {
        let (_, d) = db.utility_for_kind(k).unwrap();
        assert_eq!(d.kind, k);
        assert!(d.cost_per_m > 0.0 && d.max_link_m >= 2.0 * d.min_link_m);
    }
    // One pipe: `Sewage` asks for the same definition as `Water`.
    assert_eq!(
        db.utility_for_kind(UtilityKind::Sewage).map(|(id, _)| id),
        db.utility_for_kind(UtilityKind::Water).map(|(id, _)| id)
    );
    assert_eq!(db.utilities().count(), 2, "one pipe and one power line");
}

#[test]
fn a_pipe_is_one_link_with_per_metre_cost() {
    let mut t = T::new();
    let plan = t.build(UtilityKind::Water, &[v(0.0, 0.0), v(50.0, 0.0)]);
    assert_eq!((t.graph.node_count(), t.graph.link_count()), (2, 1));
    let per_m = def(&t.db, UtilityKind::Water).cost_per_m;
    assert_eq!(plan.cost, segment_cost(50.0, per_m));
    assert_eq!(plan.net, UtilityNet::Pipe);
    let (_, l) = t.graph.links().next().unwrap();
    assert!((l.length - 50.0).abs() < 0.01);
}

#[test]
fn water_and_sewage_share_one_pipe_network() {
    let mut t = T::new();
    t.build(UtilityKind::Water, &[v(0.0, 0.0), v(50.0, 0.0)]);
    // A sewage pipe starting at the water pipe's end connects to it.
    t.build(UtilityKind::Sewage, &[v(50.0, 0.0), v(50.0, 40.0)]);
    assert_eq!(t.graph.node_count(), 3);
    assert_eq!(t.graph.components(UtilityNet::Pipe).count, 1);
}

#[test]
fn long_legs_are_cut_into_equal_pieces() {
    let mut t = T::new();
    let plan = t.build(UtilityKind::Water, &[v(0.0, 0.0), v(250.0, 0.0)]);
    assert_eq!(plan.links.len(), 3);
    for l in &plan.links {
        assert!((l.length - 250.0 / 3.0).abs() < 0.1 && l.length <= 100.0);
    }
    assert_eq!(t.graph.components(UtilityNet::Pipe).count, 1);
    // Power lines may be longer: 150 m is a single link.
    let plan = t
        .plan(UtilityKind::Power, &[v(0.0, 100.0), v(150.0, 100.0)])
        .unwrap();
    assert_eq!(plan.links.len(), 1);
}

#[test]
fn snaps_to_nodes_and_splits_links() {
    let mut t = T::new();
    t.build(UtilityKind::Water, &[v(0.0, 0.0), v(80.0, 0.0)]);
    // 4 m from the end node: joins it (no new node there).
    let plan = t.build(UtilityKind::Water, &[v(84.0, 3.0), v(84.0, 60.0)]);
    assert_eq!(plan.new_nodes.len(), 1);
    assert_eq!(t.graph.node_count(), 3);
    // 2 m from the middle of the first link: the link is split and the networks connect.
    let before_links = t.graph.link_count();
    let plan = t.build(UtilityKind::Water, &[v(40.0, 2.0), v(40.0, -50.0)]);
    assert_eq!(plan.splits.len(), 1);
    assert!((plan.splits[0].t - 0.5).abs() < 0.02);
    assert_eq!(t.graph.link_count(), before_links + 2); // +1 split, +1 new
    assert_eq!(t.graph.components(UtilityNet::Pipe).count, 1);
    // Beyond the snap radii nothing connects.
    t.build(UtilityKind::Water, &[v(200.0, 200.0), v(260.0, 200.0)]);
    assert_eq!(t.graph.components(UtilityNet::Pipe).count, 2);
}

#[test]
fn pipes_and_power_lines_never_connect() {
    let mut t = T::new();
    t.build(UtilityKind::Water, &[v(0.0, 0.0), v(50.0, 0.0)]);
    let plan = t.build(UtilityKind::Power, &[v(50.0, 0.0), v(50.0, 60.0)]);
    assert_eq!(plan.new_nodes.len(), 2, "no snap onto a pipe node");
    assert!(plan.splits.is_empty());
    assert_eq!(t.graph.components(UtilityNet::Pipe).count, 1);
    assert_eq!(t.graph.components(UtilityNet::Power).count, 1);
    // Crossing links of different networks stay separate too.
    t.build(UtilityKind::Power, &[v(25.0, -30.0), v(25.0, 30.0)]);
    assert_eq!(t.graph.components(UtilityNet::Pipe).count, 1);
}

#[test]
fn rejects_each_problem_and_reports_all() {
    let t = T::new();
    assert_eq!(
        t.plan(UtilityKind::Water, &[v(0.0, 0.0)]).unwrap_err(),
        vec![PathProblem::PathTooShort]
    );
    assert_eq!(
        t.plan(UtilityKind::Water, &[v(0.0, 0.0), v(0.0, 0.0)])
            .unwrap_err(),
        vec![PathProblem::PathTooShort]
    );
    let e = t
        .plan(UtilityKind::Water, &[v(0.0, 0.0), v(2.0, 0.0)])
        .unwrap_err();
    assert!(matches!(e[..], [PathProblem::LinkTooShort { .. }]));
    // Power min is 8 m: 6 m is too short for a power line but fine for a pipe.
    assert!(
        t.plan(UtilityKind::Power, &[v(0.0, 0.0), v(6.0, 0.0)])
            .is_err()
    );
    assert!(
        t.plan(UtilityKind::Water, &[v(0.0, 0.0), v(6.0, 0.0)])
            .is_ok()
    );
    // Leaves the owned tile (x up to 6144): ownership problem; a short leg and a foreign
    // tile are both reported.
    let e = t
        .plan(
            UtilityKind::Water,
            &[
                Vec2::new(6100.0, 5000.0),
                Vec2::new(6300.0, 5000.0),
                Vec2::new(6301.0, 5000.0),
            ],
        )
        .unwrap_err();
    assert!(
        e.iter()
            .any(|p| matches!(p, PathProblem::TileNotOwned { .. }))
    );
    assert!(
        e.iter()
            .any(|p| matches!(p, PathProblem::LinkTooShort { .. }))
    );
}

#[test]
fn removal_prunes_nodes_and_splits_components() {
    let mut t = T::new();
    // A line of three links: a - b - c - d.
    t.build(
        UtilityKind::Water,
        &[v(0.0, 0.0), v(60.0, 0.0), v(120.0, 0.0), v(180.0, 0.0)],
    );
    assert_eq!((t.graph.node_count(), t.graph.link_count()), (4, 3));
    let nodes: Vec<_> = t.graph.nodes().map(|(id, _)| id).collect();
    assert!(t.graph.connected(nodes[0], nodes[3]));

    // Remove the middle link: two components, ends keep their nodes.
    let mid = t
        .graph
        .links()
        .find(|(_, l)| (to_xz(t.graph.node(l.a).unwrap().pos).x - 5060.0).abs() < 1.0)
        .map(|(id, _)| id)
        .unwrap();
    t.graph.remove_link(mid).unwrap();
    let comps = t.graph.components(UtilityNet::Pipe);
    assert_eq!(comps.count, 2);
    assert!(!t.graph.connected(nodes[0], nodes[3]));
    assert!(t.graph.connected(nodes[0], nodes[1]));
    assert!(t.graph.connected(nodes[2], nodes[3]));
    // Component ids are assigned by lowest node, deterministically.
    assert_eq!(comps.of(nodes[0]), Some(0));
    assert_eq!(comps.of(nodes[3]), Some(1));

    // Removing an end link prunes the dangling node.
    let first = t.graph.links().next().unwrap().0;
    t.graph.remove_link(first).unwrap();
    assert_eq!(t.graph.node_count(), 2);
    assert_eq!(
        t.graph.remove_link(first),
        Err(GraphError::UnknownLink(first))
    );
    let last = t.graph.links().next().unwrap().0;
    t.graph.remove_link(last).unwrap();
    assert_eq!((t.graph.node_count(), t.graph.link_count()), (0, 0));
}

#[test]
fn a_loop_survives_removal_of_one_link() {
    let mut t = T::new();
    t.build(
        UtilityKind::Water,
        &[
            v(0.0, 0.0),
            v(60.0, 0.0),
            v(60.0, 60.0),
            v(0.0, 60.0),
            v(0.0, 0.0),
        ],
    );
    assert_eq!((t.graph.node_count(), t.graph.link_count()), (4, 4));
    assert_eq!(t.graph.components(UtilityNet::Pipe).count, 1);
    let l = t.graph.links().next().unwrap().0;
    t.graph.remove_link(l).unwrap();
    assert_eq!(
        t.graph.components(UtilityNet::Pipe).count,
        1,
        "a ring minus one link is a path"
    );
    assert_eq!(t.graph.node_count(), 4);
}

#[test]
fn building_the_same_path_twice_adds_nothing() {
    let mut t = T::new();
    t.build(UtilityKind::Water, &[v(0.0, 0.0), v(50.0, 0.0)]);
    let before = t.graph.clone();
    t.build(UtilityKind::Water, &[v(0.0, 0.0), v(50.0, 0.0)]);
    assert_eq!(t.graph, before);
}

#[test]
fn graph_serde_round_trip_is_byte_stable() {
    let mut t = T::new();
    t.build(
        UtilityKind::Water,
        &[v(0.0, 0.0), v(120.0, 0.0), v(120.0, 90.0)],
    );
    t.build(UtilityKind::Power, &[v(10.0, 10.0), v(200.0, 10.0)]);
    let bytes = postcard::to_stdvec(&t.graph).unwrap();
    let back: UtilityGraph = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(back, t.graph);
    assert_eq!(postcard::to_stdvec(&back).unwrap(), bytes);
}

#[test]
fn planning_is_pure() {
    let mut t = T::new();
    t.build(UtilityKind::Water, &[v(0.0, 0.0), v(80.0, 0.0)]);
    let before = t.graph.clone();
    let a = t.plan(UtilityKind::Water, &[v(40.0, 2.0), v(40.0, -50.0)]);
    let b = t.plan(UtilityKind::Water, &[v(40.0, 2.0), v(40.0, -50.0)]);
    assert_eq!(a, b);
    assert_eq!(t.graph, before);
}

#[test]
fn non_finite_link_paths_are_rejected_next_to_existing_links() {
    let t = T::new();
    let r = t
        .plan(UtilityKind::Water, &[v(0.0, 0.0), v(60.0, 0.0)])
        .unwrap();
    let _ = r;
    for bad in [f32::NAN, f32::INFINITY, 1e30] {
        let e = t
            .plan(UtilityKind::Water, &[v(0.0, 0.0), Vec2::new(bad, 5000.0)])
            .unwrap_err();
        assert_eq!(e, vec![PathProblem::BadCoordinates]);
    }
}
