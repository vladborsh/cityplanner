//! T-323 (§24): street trees on a segment — `SetTrees` and its price, the row following the
//! pieces of a segment through the graph operations, the refusal on a bridge, the "with trees"
//! build plan priced as road plus trees, and a replay of the same log giving the same state.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_core::{CommandOutcome, Cost, Treasury};
use cp_economy::segment_upkeep;
use cp_net::{
    Anchor, BuildPlan, ElevationMode, NetCommand, NetError, NetGraph, PlanPiece, SegmentId,
};
use cp_sim::{SimCommand, SimRunner};

const C: f32 = 5100.0;

fn world() -> SimRunner {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
    let db = Arc::new(cp_content::ContentDb::load_dir(format!("{dir}/content")).unwrap());
    let terrain = Arc::new(
        cp_terrain::Terrain::load(&std::path::Path::new(dir).join("maps/flatlands")).unwrap(),
    );
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain);
    sim.set_content(db);
    sim.world_mut()
        .insert_resource(cp_terrain::TileOwnership::only((2, 2)));
    sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(1_000_000.0);
    sim
}

fn db(sim: &SimRunner) -> Arc<cp_content::ContentDb> {
    sim.world().resource::<cp_content::SimContent>().0.clone()
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

fn funds(sim: &SimRunner) -> Cost {
    sim.world().resource::<Treasury>().balance
}

fn graph(sim: &SimRunner) -> &NetGraph {
    sim.world().resource::<NetGraph>()
}

fn build(sim: &mut SimRunner, road: &str, elevation: ElevationMode, trees: bool) {
    let (a, b) = (
        Vec2::new(C, C),
        Vec2::new(
            C + if matches!(elevation, ElevationMode::Ground) {
                90.0
            } else {
                600.0
            },
            C,
        ),
    );
    let cmd = NetCommand::BuildPlan {
        plan: BuildPlan {
            road: db(sim).road_id(road).unwrap(),
            elevation,
            pieces: vec![PlanPiece::straight(Anchor::Free(a), Anchor::Free(b), a, b)],
            trees,
        },
    };
    run(sim, cmd).unwrap();
}

fn only_segment(sim: &SimRunner) -> SegmentId {
    graph(sim).segments().next().unwrap().0
}

#[test]
fn planting_costs_per_metre_removing_is_free_and_upkeep_grows() {
    let mut sim = world();
    build(&mut sim, "basic_2l", ElevationMode::Ground, false);
    let seg = only_segment(&sim);
    let bal = db(&sim).balance().clone();
    let len = graph(&sim).segment(seg).unwrap().length;
    let before = funds(&sim);
    run(&mut sim, NetCommand::SetTrees { seg, trees: true }).unwrap();
    let price = Cost::scaled(bal.trees_cost_per_m, f64::from(len));
    assert_eq!(before.0 - funds(&sim).0, price.0);
    assert!(graph(&sim).segment(seg).unwrap().trees);
    // The same again changes nothing and costs nothing.
    let paid = funds(&sim);
    run(&mut sim, NetCommand::SetTrees { seg, trees: true }).unwrap();
    assert_eq!(funds(&sim), paid);
    // Upkeep: the road's plus the row's, per metre.
    let db = db(&sim);
    let s = graph(&sim).segment(seg).unwrap().clone();
    let with = segment_upkeep(db.road(s.road), &s, 100, bal.trees_upkeep_per_m_week);
    let without = segment_upkeep(
        db.road(s.road),
        &cp_net::Segment { trees: false, ..s },
        100,
        bal.trees_upkeep_per_m_week,
    );
    assert_eq!(
        with.0 - without.0,
        Cost::scaled(bal.trees_upkeep_per_m_week, f64::from(len)).0
    );
    // Removing is free and gives nothing back.
    run(&mut sim, NetCommand::SetTrees { seg, trees: false }).unwrap();
    assert_eq!(funds(&sim), paid);
    assert!(!graph(&sim).segment(seg).unwrap().trees);
}

#[test]
fn a_bridge_takes_no_trees() {
    let mut sim = world();
    build(&mut sim, "basic_2l", ElevationMode::Elevated(4.0), false);
    let seg = graph(&sim)
        .segments()
        .find(|(_, s)| s.elevation == cp_net::Elevation::Elevated)
        .unwrap()
        .0;
    let before = funds(&sim);
    let r = run(&mut sim, NetCommand::SetTrees { seg, trees: true });
    assert_eq!(r, Err(format!("net: {}", NetError::TreesNotHere(seg))));
    assert_eq!(funds(&sim), before);
    // A plan "with trees" on an elevated road plants none and charges none.
    let mut again = world();
    build(&mut again, "basic_2l", ElevationMode::Elevated(4.0), true);
    assert!(graph(&again).segments().all(|(_, s)| !s.trees));
    assert_eq!(funds(&again), funds(&sim));
}

#[test]
fn a_cut_piece_keeps_the_row_and_an_upgrade_too() {
    let mut sim = world();
    build(&mut sim, "basic_2l", ElevationMode::Ground, true);
    let seg = only_segment(&sim);
    run(
        &mut sim,
        NetCommand::RemovePart {
            seg,
            s0: 30.0,
            s1: 60.0,
        },
    )
    .unwrap();
    // The two ends are left, both with trees.
    let left: Vec<_> = graph(&sim).segments().map(|(_, s)| s.trees).collect();
    assert_eq!(left, vec![true, true]);
    let first = graph(&sim).segments().next().unwrap().0;
    let medium = db(&sim).road_id("medium_4l").unwrap();
    run(
        &mut sim,
        NetCommand::UpgradeSegment {
            seg: first,
            road: medium,
        },
    )
    .unwrap();
    assert!(graph(&sim).segment(first).unwrap().trees);
}

#[test]
fn a_plan_with_trees_costs_the_road_plus_the_trees_and_replays_the_same() {
    let mut bare = world();
    let mut green = world();
    build(&mut bare, "basic_2l", ElevationMode::Ground, false);
    build(&mut green, "basic_2l", ElevationMode::Ground, true);
    let per_m = db(&green).balance().trees_cost_per_m;
    let trees: Cost = graph(&green)
        .segments()
        .map(|(_, s)| Cost::scaled(per_m, f64::from(s.length)))
        .fold(Cost::ZERO, |a, c| Cost(a.0 + c.0));
    assert!(graph(&green).segments().all(|(_, s)| s.trees));
    assert_eq!(
        bare.world().resource::<Treasury>().balance.0 - trees.0,
        funds(&green).0
    );
    // The same log on a second sim: the same treasury and the same checksum.
    let mut twin = world();
    build(&mut twin, "basic_2l", ElevationMode::Ground, true);
    assert_eq!(funds(&twin), funds(&green));
    assert_eq!(twin.checksum(), green.checksum());
    assert_ne!(bare.checksum(), green.checksum());
}
