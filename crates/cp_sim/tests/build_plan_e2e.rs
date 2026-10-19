//! T-262: `NetCommand::BuildPlan` is applied literally — the anchors are the truth, nothing is
//! snapped again — and the graph keeps its invariants (§6.8.3): no segment under 8 m or over
//! 100 m, a cut that would leave under 8 m is a node at the near end, a crossing within 8 m of a
//! node goes through it, a road across a dead end joins it, spans are cut into `floor(L/96)+1`
//! equal pieces, no joint is merged away afterwards. The cases are the probes of the net audit.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_core::{CommandOutcome, Cost, Treasury};
use cp_net::{
    Anchor, BuildPlan, ElevationMode, NetCommand, NetGraph, NodeId, PlanPiece, RoadPath, SegmentId,
    Violation,
};
use cp_sim::{SimCommand, SimRunner};

const C: f32 = 5100.0;

fn world() -> (SimRunner, cp_content::RoadDefId) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
    let db = Arc::new(cp_content::ContentDb::load_dir(format!("{dir}/content")).unwrap());
    let terrain = Arc::new(
        cp_terrain::Terrain::load(&std::path::Path::new(dir).join("maps/flatlands")).unwrap(),
    );
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(&db));
    sim.world_mut()
        .insert_resource(cp_terrain::TileOwnership::only((2, 2)));
    sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(1_000_000_000.0);
    (sim, db.road_id("basic_2l").unwrap())
}

fn run(sim: &mut SimRunner, c: NetCommand) -> Result<(), String> {
    sim.push_command(SimCommand::Net(c), sim.tick());
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last()
        .unwrap()
        .result
        .clone()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn p(x: f32, z: f32) -> Vec2 {
    Vec2::new(C + x, C + z)
}

fn free(points: &[Vec2]) -> Vec<PlanPiece> {
    points
        .windows(2)
        .map(|w| PlanPiece::straight(Anchor::Free(w[0]), Anchor::Free(w[1]), w[0], w[1]))
        .collect()
}

fn plan(road: cp_content::RoadDefId, pieces: Vec<PlanPiece>) -> NetCommand {
    NetCommand::BuildPlan {
        plan: BuildPlan {
            road,
            elevation: ElevationMode::Ground,
            pieces,
        },
    }
}

fn build(sim: &mut SimRunner, road: cp_content::RoadDefId, points: &[Vec2]) -> Result<(), String> {
    run(sim, plan(road, free(points)))
}

fn graph(sim: &SimRunner) -> &NetGraph {
    sim.world().resource::<NetGraph>()
}

fn lengths(sim: &SimRunner) -> Vec<f32> {
    let mut v: Vec<f32> = graph(sim).segments().map(|(_, s)| s.length).collect();
    v.sort_by(f32::total_cmp);
    v
}

fn node_at(sim: &SimRunner, at: Vec2) -> Option<NodeId> {
    graph(sim)
        .nodes()
        .find(|(_, n)| Vec2::new(n.pos.x, n.pos.z).distance(at) < 0.05)
        .map(|(id, _)| id)
}

fn degree(sim: &SimRunner, at: Vec2) -> usize {
    node_at(sim, at).map_or(0, |n| graph(sim).node(n).unwrap().segments.len())
}

fn assert_invariants(sim: &SimRunner) {
    assert_eq!(graph(sim).check_invariants(), Vec::<Violation>::new());
}

#[test]
fn the_vertices_of_the_plan_are_the_vertices_of_the_graph() {
    let (mut sim, road) = world();
    // A street, and a road that starts 10 m from its end node: not pulled onto the node (the
    // old 12 m snap), not turned to a 15 degree step, not cut on a 64 m lattice.
    build(&mut sim, road, &[p(0.0, 0.0), p(200.0, 0.0)]).unwrap();
    let (start, odd) = (p(200.0, 10.0), p(263.0, 87.0));
    build(&mut sim, road, &[start, odd]).unwrap();
    assert!(node_at(&sim, start).is_some() && node_at(&sim, odd).is_some());
    assert_eq!(degree(&sim, p(200.0, 0.0)), 1);
    // 99 m: two pieces of half the length.
    assert_eq!(lengths(&sim).len(), 3 + 2);
    assert_invariants(&sim);
}

#[test]
fn a_plan_across_a_junction_arm_goes_through_the_node() {
    let (mut sim, road) = world();
    // A cross: the arms meet in (0, 0).
    build(&mut sim, road, &[p(-100.0, 0.0), p(100.0, 0.0)]).unwrap();
    build(&mut sim, road, &[p(0.0, -100.0), p(0.0, 100.0)]).unwrap();
    let junction = node_at(&sim, p(0.0, 0.0)).unwrap();
    // A street at 45 degrees that crosses the east arm 3 m from the centre (and the south arm
    // 3 m from it): through the junction node, no 3 m segment between two junctions.
    build(&mut sim, road, &[p(-37.0, -40.0), p(43.0, 40.0)]).unwrap();
    assert_eq!(node_at(&sim, p(0.0, 0.0)), Some(junction));
    assert!(lengths(&sim)[0] >= 8.0, "{:?}", lengths(&sim));
    assert_invariants(&sim);
}

#[test]
fn a_crossing_far_from_nodes_cuts_the_street_in_two_pieces_of_8_m_or_more() {
    let (mut sim, road) = world();
    build(&mut sim, road, &[p(-60.0, 0.0), p(60.0, 0.0)]).unwrap();
    build(&mut sim, road, &[p(20.0, -80.0), p(20.0, 80.0)]).unwrap();
    assert!(node_at(&sim, p(20.0, 0.0)).is_some());
    assert_eq!(degree(&sim, p(20.0, 0.0)), 4);
    assert!(lengths(&sim)[0] >= 8.0);
    assert_invariants(&sim);
}

#[test]
fn a_road_across_a_dead_end_joins_it() {
    let (mut sim, road) = world();
    // A stub ending exactly on the line of the road drawn over it.
    build(&mut sim, road, &[p(0.0, -80.0), p(0.0, 0.0)]).unwrap();
    assert_eq!(degree(&sim, p(0.0, 0.0)), 1);
    build(&mut sim, road, &[p(-80.0, 0.0), p(80.0, 0.0)]).unwrap();
    assert_eq!(degree(&sim, p(0.0, 0.0)), 3);
    assert_invariants(&sim);
}

#[test]
fn a_cut_leaving_under_8_m_is_a_node_at_the_near_end() {
    let (mut sim, road) = world();
    build(&mut sim, road, &[p(0.0, 0.0), p(60.0, 0.0)]).unwrap();
    let seg: SegmentId = graph(&sim).segments().next().unwrap().0;
    // A road starting 5 m from the end of the street, on it.
    let t = 55.0 / 60.0;
    let to = p(55.0, 70.0);
    run(
        &mut sim,
        plan(
            road,
            vec![PlanPiece::straight(
                Anchor::OnSegment(seg, t),
                Anchor::Free(to),
                p(55.0, 0.0),
                to,
            )],
        ),
    )
    .unwrap();
    assert_eq!(degree(&sim, p(60.0, 0.0)), 2);
    assert!(node_at(&sim, p(55.0, 0.0)).is_none());
    assert!(lengths(&sim)[0] >= 8.0);
    assert_invariants(&sim);
}

#[test]
fn long_spans_are_cut_into_equal_pieces_and_stay_cut() {
    let (mut sim, road) = world();
    // 300 m: floor(300 / 96) + 1 = 4 pieces of 75 m; 96 m: two of 48; 95 m: one.
    build(&mut sim, road, &[p(-150.0, 0.0), p(150.0, 0.0)]).unwrap();
    assert_eq!(graph(&sim).segment_count(), 4);
    for l in lengths(&sim) {
        assert!((l - 75.0).abs() < 0.1, "{l}");
    }
    build(&mut sim, road, &[p(-150.0, 100.0), p(-54.0, 100.0)]).unwrap();
    build(&mut sim, road, &[p(-150.0, 200.0), p(-55.0, 200.0)]).unwrap();
    assert_eq!(graph(&sim).segment_count(), 4 + 2 + 1);
    // A cross street at 38 m beyond a cut: the pieces around it stay, nothing is merged into
    // a 100 m+ segment, the cut at 75 m stays a node.
    build(
        &mut sim,
        road,
        &[p(-75.0 + 38.0, -80.0), p(-75.0 + 38.0, 80.0)],
    )
    .unwrap();
    assert_eq!(degree(&sim, p(-75.0, 0.0)), 2);
    assert_invariants(&sim);
}

#[test]
fn only_real_errors_refuse_a_plan() {
    let (mut sim, road) = world();
    build(&mut sim, road, &[p(-60.0, 0.0), p(60.0, 0.0)]).unwrap();
    // Too short.
    let e = build(&mut sim, road, &[p(0.0, 40.0), p(5.0, 40.0)]).unwrap_err();
    assert!(e.contains("SegmentTooShort"), "{e}");
    // A road 5 m beside another one lies on its asphalt: refused, whatever it crosses.
    build(&mut sim, road, &[p(-30.0, 60.0), p(-30.0, 120.0)]).unwrap();
    let e = build(&mut sim, road, &[p(-25.0, 60.0), p(-25.0, 120.0)]).unwrap_err();
    assert!(e.contains("NodeOnRoad"), "{e}");
    // A sharp angle at an existing node.
    let e = build(&mut sim, road, &[p(60.0, 0.0), p(0.0, 10.0)]).unwrap_err();
    assert!(e.contains("SharpAngle"), "{e}");
    assert_invariants(&sim);
}

#[test]
fn segments_get_a_build_order_and_a_cut_keeps_it() {
    let (mut sim, road) = world();
    build(&mut sim, road, &[p(-100.0, 0.0), p(100.0, 0.0)]).unwrap();
    let first: Vec<u32> = graph(&sim).segments().map(|(_, s)| s.order).collect();
    assert_eq!(first, vec![0, 1, 2]);
    let after = graph(&sim).next_order();
    assert_eq!(after, 3);
    build(&mut sim, road, &[p(0.0, -90.0), p(0.0, 90.0)]).unwrap();
    // The middle piece of the street was cut at 0: both halves keep its order 1; the new road
    // takes the next orders.
    let mut orders: Vec<u32> = graph(&sim).segments().map(|(_, s)| s.order).collect();
    orders.sort_unstable();
    assert_eq!(orders, vec![0, 1, 1, 2, 3, 4]);
    assert_eq!(graph(&sim).next_order(), 5);
}

#[test]
fn a_bad_plan_is_refused_and_changes_nothing() {
    let (mut sim, road) = world();
    let balance = sim.world().resource::<Treasury>().balance;
    assert!(run(&mut sim, plan(road, vec![])).is_err());
    let nan = Vec2::new(f32::NAN, 5100.0);
    assert!(build(&mut sim, road, &[p(0.0, 0.0), nan]).is_err());
    // A node that does not exist, taken from another world.
    let ghost = {
        let (mut other, r2) = world();
        build(&mut other, r2, &[p(0.0, 0.0), p(60.0, 0.0)]).unwrap();
        graph(&other).segments().next().unwrap().1.start
    };
    let q = p(60.0, 0.0);
    assert!(
        run(
            &mut sim,
            plan(
                road,
                vec![PlanPiece::straight(
                    Anchor::Node(ghost),
                    Anchor::Free(q),
                    p(0.0, 0.0),
                    q
                )]
            )
        )
        .is_err()
    );
    // The second piece is bad (under 8 m): the first is not kept.
    let mut two = free(&[p(0.0, 0.0), p(60.0, 0.0)]);
    two.extend(free(&[p(60.0, 0.0), p(64.0, 0.0)]));
    assert!(run(&mut sim, plan(road, two)).is_err());
    assert_eq!(graph(&sim).segment_count(), 0);
    assert_eq!(sim.world().resource::<Treasury>().balance, balance);
}

#[test]
fn build_road_still_builds_what_it_always_built() {
    // The older command keeps its snaps and its 64 m cuts for old logs.
    let (mut sim, road) = world();
    run(
        &mut sim,
        NetCommand::BuildRoad {
            road,
            path: RoadPath::Straight {
                from: p(0.0, 0.0),
                to: p(200.0, 0.0),
            },
            elevation: ElevationMode::Ground,
        },
    )
    .unwrap();
    let mut l = lengths(&sim);
    l.sort_by(f32::total_cmp);
    assert_eq!(l.len(), 3);
    assert!(
        (l[0] - 64.0).abs() < 0.1 || (l[0] - 72.0).abs() < 0.1,
        "{l:?}"
    );
}

#[test]
fn a_plan_whose_node_stands_on_a_road_it_does_not_join_is_refused() {
    let (mut sim, road) = world();
    build(&mut sim, road, &[p(-60.0, 0.0), p(60.0, 0.0)]).unwrap();
    let before = graph(&sim).segment_count();
    // An end 3 m off the axis of the street (on its asphalt), not joined: refused.
    let e = build(&mut sim, road, &[p(0.0, 80.0), p(10.0, 3.0)]).unwrap_err();
    assert!(e.contains("NodeOnRoad"), "{e}");
    // The same end on the axis, as an anchor of the street, is a T.
    let (seg, mid) = {
        let (id, s) = graph(&sim).segments().next().unwrap();
        let m = s.curve.point(0.5);
        (id, Vec2::new(m.x, m.z))
    };
    run(
        &mut sim,
        plan(
            road,
            vec![PlanPiece::straight(
                Anchor::Free(mid + Vec2::new(0.0, 80.0)),
                Anchor::OnSegment(seg, 0.5),
                mid + Vec2::new(0.0, 80.0),
                mid,
            )],
        ),
    )
    .unwrap();
    assert!(graph(&sim).segment_count() > before);
    // A road laid over an old dead end that it does not join: refused (the node lies under it).
    build(&mut sim, road, &[p(200.0, -80.0), p(200.0, 0.0)]).unwrap();
    let e = build(&mut sim, road, &[p(203.0, -60.0), p(203.0, 60.0)]).unwrap_err();
    assert!(e.contains("NodeOnRoad"), "{e}");
    assert_invariants(&sim);
}

#[test]
fn a_crossing_inside_the_junction_of_a_wide_road_goes_through_its_node() {
    let (mut sim, _) = world();
    let wide = sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .road_id("large_6l")
        .unwrap();
    // A cross of 6-lane streets (half width 14); a street across the east arm 12 m from the
    // centre, inside the junction, goes through the node, where one of 2 lanes (8 m) would not.
    build(&mut sim, wide, &[p(-100.0, 0.0), p(100.0, 0.0)]).unwrap();
    build(&mut sim, wide, &[p(0.0, -100.0), p(0.0, 100.0)]).unwrap();
    let node = node_at(&sim, p(0.0, 0.0)).unwrap();
    let before = degree(&sim, p(0.0, 0.0));
    build(&mut sim, wide, &[p(12.0, -90.0), p(12.0, 90.0)]).unwrap_err();
    let c45 = [p(-40.0, -52.0), p(64.0, 52.0)];
    build(&mut sim, wide, &c45).unwrap();
    assert_eq!(node_at(&sim, p(0.0, 0.0)), Some(node));
    assert_eq!(degree(&sim, p(0.0, 0.0)), before + 2);
    assert_invariants(&sim);
}

#[test]
fn a_dead_end_or_a_plain_cut_pulls_only_within_8_m_a_junction_within_its_footprint() {
    let (mut sim, _) = world();
    let wide = sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .road_id("large_6l")
        .unwrap();
    let narrow = sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .road_id("basic_2l")
        .unwrap();
    // A 6-lane street ending at the origin (half width 14): a street across 12 m before its
    // end stays where it was aimed, the dead end does not pull it (8 m is its whole reach).
    build(&mut sim, wide, &[p(-100.0, 0.0), p(0.0, 0.0)]).unwrap();
    build(&mut sim, narrow, &[p(-12.0, -60.0), p(-12.0, 60.0)]).unwrap();
    assert!(node_at(&sim, p(-12.0, 0.0)).is_some());
    assert_invariants(&sim);
}

#[test]
fn at_an_acute_junction_a_point_on_the_other_arms_asphalt_is_the_node() {
    let (mut sim, _) = world();
    let wide = sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .road_id("large_6l")
        .unwrap();
    let narrow = sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .road_id("basic_2l")
        .unwrap();
    // Two 6-lane arms from the origin at 30 degrees (half width 14). A street across the first
    // arm 13 m out lies on the second arm's asphalt (6.5 m from its axis): the node takes it, a
    // radius of 8 m would leave a junction standing on a road it does not join.
    let dir = Vec2::from_angle(30f32.to_radians());
    build(&mut sim, wide, &[p(0.0, 0.0), p(-90.0, 0.0)]).unwrap();
    build(
        &mut sim,
        wide,
        &[p(0.0, 0.0), p(dir.x * 90.0, dir.y * 90.0)],
    )
    .unwrap();
    let node = node_at(&sim, p(0.0, 0.0)).unwrap();
    let before = degree(&sim, p(0.0, 0.0));
    let on_arm = dir * 13.0;
    let across = Vec2::new(-dir.y, dir.x) * 60.0;
    build(
        &mut sim,
        narrow,
        &[
            p(on_arm.x - across.x, on_arm.y - across.y),
            p(on_arm.x + across.x, on_arm.y + across.y),
        ],
    )
    .unwrap();
    assert_eq!(node_at(&sim, p(0.0, 0.0)), Some(node));
    assert!(degree(&sim, p(0.0, 0.0)) > before);
    assert_invariants(&sim);
}

#[test]
fn a_road_whose_asphalt_lies_on_a_road_beside_it_is_refused() {
    let (mut sim, road) = world();
    build(&mut sim, road, &[p(-100.0, 0.0), p(100.0, 0.0)]).unwrap();
    // Parallel 8 m away: the half widths are 6 and 6, the asphalts overlap by 4 m, though no node
    // of either lies on the other.
    let e = build(&mut sim, road, &[p(-60.0, 8.0), p(60.0, 8.0)]).unwrap_err();
    assert!(e.contains("RoadsOverlap"), "{e}");
    // 13 m away the asphalts clear each other: built.
    build(&mut sim, road, &[p(-60.0, 13.0), p(60.0, 13.0)]).unwrap();
    assert_invariants(&sim);
}
