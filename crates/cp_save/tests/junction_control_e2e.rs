//! T-021: junction control through `SimRunner`: auto-installed lights in the Network
//! phase, `ToggleTrafficLight`, and persistence of the player's choice.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::{ContentDb, RoadDefId};
use cp_core::{CommandError, CommandOutcome, Tick};
use cp_net::{
    ElevationMode, JunctionControl, JunctionGeometry, NetCommand, NetGraph, NodeId, RoadPath,
};
use cp_save::{load_into_runner, read_header, save_runner};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const HASH: u64 = 7;

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

fn sim_with(content: &Arc<ContentDb>) -> SimRunner {
    let terrain = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
    let mut sim = SimRunner::new(5);
    sim.set_terrain(Arc::new(terrain));
    sim.set_content(Arc::clone(content));
    sim
}

fn build(road: RoadDefId, from: Vec2, to: Vec2) -> SimCommand {
    SimCommand::Net(NetCommand::BuildRoad {
        road,
        path: RoadPath::Straight { from, to },
        elevation: ElevationMode::Ground,
    })
}

fn run(sim: &mut SimRunner, cmds: Vec<SimCommand>) -> Vec<CommandOutcome> {
    let start = sim.tick().0;
    let n = cmds.len() as u64;
    for (i, c) in cmds.into_iter().enumerate() {
        sim.push_command(c, Tick(start + i as u64));
    }
    let mut out = Vec::new();
    for _ in 0..n + 1 {
        sim.tick_once();
        out.extend(
            sim.world_mut()
                .resource_mut::<Messages<CommandOutcome>>()
                .drain(),
        );
    }
    out
}

/// A plus of two 4-lane streets crossing at (5000, 5000), built by commands.
fn plus(sim: &mut SimRunner, db: &ContentDb, name: &str) -> NodeId {
    let road = db.road_id(name).unwrap();
    let o = run(
        sim,
        vec![
            build(road, Vec2::new(4900.0, 5000.0), Vec2::new(5100.0, 5000.0)),
            build(road, Vec2::new(5000.0, 4900.0), Vec2::new(5000.0, 5100.0)),
        ],
    );
    assert!(o.iter().all(|o| o.result.is_ok()), "{o:?}");
    let g = sim.world().resource::<NetGraph>();
    g.nodes()
        .find(|(_, n)| n.segments.len() == 4)
        .map(|(id, _)| id)
        .expect("a 4-way node")
}

fn control(sim: &SimRunner, n: NodeId) -> JunctionControl {
    sim.world().resource::<NetGraph>().node(n).unwrap().control
}

fn has_plan(sim: &SimRunner, n: NodeId) -> bool {
    sim.world()
        .resource::<JunctionGeometry>()
        .get(n)
        .unwrap()
        .control
        .light
        .is_some()
}

#[test]
fn auto_light_then_toggle_off_and_on() {
    let db = content();
    let mut sim = sim_with(&db);
    let n = plus(&mut sim, &db, "medium_4l");
    // The Network phase installed a light by the rule.
    assert_eq!(control(&sim, n), JunctionControl::TrafficLight);
    assert!(has_plan(&sim, n));

    let toggle = || vec![SimCommand::Net(NetCommand::ToggleTrafficLight { node: n })];
    let o = run(&mut sim, toggle());
    assert!(o[0].result.is_ok());
    assert_eq!(control(&sim, n), JunctionControl::ManualPriority);
    assert!(!has_plan(&sim, n));
    // Recomputes do not bring it back.
    for _ in 0..3 {
        sim.tick_once();
    }
    assert_eq!(control(&sim, n), JunctionControl::ManualPriority);

    let o = run(&mut sim, toggle());
    assert!(o[0].result.is_ok());
    assert_eq!(control(&sim, n), JunctionControl::ManualTrafficLight);
    assert!(has_plan(&sim, n));
}

#[test]
fn toggle_is_rejected_where_a_light_makes_no_sense() {
    let db = content();
    let mut sim = sim_with(&db);
    let road = db.road_id("medium_4l").unwrap();
    let o = run(
        &mut sim,
        vec![build(
            road,
            Vec2::new(4900.0, 5000.0),
            Vec2::new(5100.0, 5000.0),
        )],
    );
    assert!(o[0].result.is_ok());
    // A dead end: rejected, state untouched.
    let end = sim
        .world()
        .resource::<NetGraph>()
        .nodes()
        .next()
        .map(|(id, _)| id)
        .unwrap();
    let before = sim.world().resource::<NetGraph>().clone();
    let o = run(
        &mut sim,
        vec![SimCommand::Net(NetCommand::ToggleTrafficLight {
            node: end,
        })],
    );
    assert!(matches!(
        o[0].result,
        Err(CommandError::Rejected { domain: "net", .. })
    ));
    assert_eq!(sim.world().resource::<NetGraph>(), &before);

    // A node id that does not exist.
    let ghost: NodeId = cp_core::Id::new(9999, 0);
    let o = run(
        &mut sim,
        vec![SimCommand::Net(NetCommand::ToggleTrafficLight {
            node: ghost,
        })],
    );
    assert!(matches!(
        o[0].result,
        Err(CommandError::Rejected { domain: "net", .. })
    ));
}

#[test]
fn players_choice_survives_save_load_and_the_plan_is_rebuilt() {
    let db = content();
    let mut sim = sim_with(&db);
    let n = plus(&mut sim, &db, "medium_4l");
    run(
        &mut sim,
        vec![SimCommand::Net(NetCommand::ToggleTrafficLight { node: n })],
    );
    assert_eq!(control(&sim, n), JunctionControl::ManualPriority);
    let bytes = save_runner(&sim, HASH, "flatlands").unwrap();

    let mut other = sim_with(&db);
    load_into_runner(&mut other, &bytes, HASH).unwrap();
    assert_eq!(control(&other, n), JunctionControl::ManualPriority);
    other.tick_once();
    assert_eq!(control(&other, n), JunctionControl::ManualPriority);
    assert!(!has_plan(&other, n));

    // Same save with a light: the plan comes back after the Network phase runs.
    run(
        &mut sim,
        vec![SimCommand::Net(NetCommand::ToggleTrafficLight { node: n })],
    );
    let bytes = save_runner(&sim, HASH, "flatlands").unwrap();
    let mut other = sim_with(&db);
    load_into_runner(&mut other, &bytes, HASH).unwrap();
    other.tick_once();
    assert_eq!(control(&other, n), JunctionControl::ManualTrafficLight);
    assert!(has_plan(&other, n));
    assert_eq!(
        other
            .world()
            .resource::<JunctionGeometry>()
            .get(n)
            .unwrap()
            .control,
        sim.world()
            .resource::<JunctionGeometry>()
            .get(n)
            .unwrap()
            .control
    );
}

#[test]
fn old_fixtures_with_priority_nodes_still_load_and_get_the_rule_applied() {
    // v3 and later fixtures carry a small network whose nodes were saved as `Priority`
    // (variant 0): appended enum variants keep old files readable.
    let db = content();
    for name in ["v3-basic.sav", "v4-basic.sav", "v5-basic.sav"] {
        let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves").join(name)).unwrap();
        let h = read_header(&bytes).unwrap().0;
        let mut sim = sim_with(&db);
        load_into_runner(&mut sim, &bytes, h.content_hash).unwrap();
        let g = sim.world().resource::<NetGraph>().clone();
        assert!(
            g.nodes()
                .all(|(_, n)| n.control == JunctionControl::Priority)
        );
        sim.tick_once();
        if name == "v3-basic.sav" {
            assert!(g.node_count() > 0, "the v3 fixture carries a network");
        }
    }
}
