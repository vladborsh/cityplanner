//! T-332 (§18.1 decision 7): `NetCommand::BuildBatch` builds its plans in order as one change,
//! all of them or none, charged once; a free end on a node an earlier plan of the batch made is
//! that node.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_core::{CommandOutcome, Cost, Treasury};
use cp_net::{Anchor, BuildPlan, ElevationMode, NetCommand, NetGraph, PlanPiece, Violation};
use cp_sim::{SimCommand, SimRunner};

const C: f32 = 5100.0;

fn world() -> (SimRunner, Arc<cp_content::ContentDb>) {
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
    (sim, db)
}

fn run(sim: &mut SimRunner, c: NetCommand) -> Result<Cost, String> {
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
        .map_err(|e| e.to_string())
}

fn p(x: f32, z: f32) -> Vec2 {
    Vec2::new(C + x, C + z)
}

fn plan(road: cp_content::RoadDefId, elevation: ElevationMode, points: &[Vec2]) -> BuildPlan {
    BuildPlan {
        road,
        elevation,
        pieces: points
            .windows(2)
            .map(|w| PlanPiece::straight(Anchor::Free(w[0]), Anchor::Free(w[1]), w[0], w[1]))
            .collect(),
        trees: false,
    }
}

fn graph(sim: &SimRunner) -> &NetGraph {
    sim.world().resource::<NetGraph>()
}

fn balance(sim: &SimRunner) -> Cost {
    sim.world().resource::<Treasury>().balance
}

#[test]
fn a_batch_builds_its_plans_on_each_other_and_is_charged_once() {
    let (mut sim, db) = world();
    let street = db.road_id("basic_2l").unwrap();
    // A street, an overpass between two of its vertices' neighbours, and a road from the
    // street's middle vertex: the later plans end on nodes the earlier ones made.
    let plans = vec![
        plan(
            street,
            ElevationMode::Ground,
            &[p(0.0, 0.0), p(150.0, 0.0), p(300.0, 0.0)],
        ),
        plan(
            street,
            ElevationMode::Ground,
            &[p(150.0, 0.0), p(150.0, 160.0)],
        ),
        plan(
            street,
            ElevationMode::Elevated(6.0),
            &[p(0.0, 0.0), p(0.0, 300.0)],
        ),
    ];
    let before = balance(&sim);
    let cost = run(&mut sim, NetCommand::BuildBatch { plans }).unwrap();
    assert!(cost.0 > 0);
    assert_eq!(
        Cost(before.0 - cost.0),
        balance(&sim),
        "charged once, the sum"
    );
    let g = graph(&sim);
    assert_eq!(g.check_invariants(), Vec::<Violation>::new());
    // The joins: the middle vertex has three roads, the street's start two (street + overpass).
    let degree = |at: Vec2| {
        g.nodes()
            .find(|(_, n)| Vec2::new(n.pos.x, n.pos.z).distance(at) < 0.05)
            .map_or(0, |(_, n)| n.segments.len())
    };
    assert_eq!(degree(p(150.0, 0.0)), 3);
    assert_eq!(degree(p(0.0, 0.0)), 2);
    let log = sim.world().resource::<cp_core::CommandLog<SimCommand>>();
    assert_eq!(log.len(), 1, "one command in the log");
}

#[test]
fn a_batch_with_one_bad_plan_builds_nothing_and_charges_nothing() {
    let (mut sim, db) = world();
    let street = db.road_id("basic_2l").unwrap();
    let before = (balance(&sim), graph(&sim).clone());
    // The second plan crosses the first at 10°: a sharp angle, refused.
    let plans = vec![
        plan(street, ElevationMode::Ground, &[p(0.0, 0.0), p(300.0, 0.0)]),
        plan(
            street,
            ElevationMode::Ground,
            &[p(100.0, -10.0), p(200.0, 7.6)],
        ),
    ];
    let e = run(&mut sim, NetCommand::BuildBatch { plans }).unwrap_err();
    assert!(!e.is_empty());
    assert_eq!(graph(&sim).segments().count(), 0, "nothing built");
    assert_eq!((balance(&sim), graph(&sim).clone()), before);
    // Money for the first plan but not for both: nothing either.
    let plans = vec![
        plan(street, ElevationMode::Ground, &[p(0.0, 0.0), p(200.0, 0.0)]),
        plan(
            street,
            ElevationMode::Ground,
            &[p(0.0, 100.0), p(500.0, 100.0)],
        ),
    ];
    let first = run(
        &mut world().0,
        NetCommand::BuildPlan {
            plan: plans[0].clone(),
        },
    )
    .unwrap();
    let floor = cp_core::treasury::DEBT_FLOOR;
    sim.world_mut().resource_mut::<Treasury>().balance = Cost(floor.0 + first.0 + 1);
    let e = run(&mut sim, NetCommand::BuildBatch { plans }).unwrap_err();
    assert!(
        e.to_lowercase().contains("fund") || e.contains("money"),
        "{e}"
    );
    assert_eq!(graph(&sim).segments().count(), 0);
}

#[test]
fn an_empty_batch_is_a_bad_plan() {
    let (mut sim, _) = world();
    assert!(run(&mut sim, NetCommand::BuildBatch { plans: Vec::new() }).is_err());
}
