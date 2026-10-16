//! T-177 acceptance: `BuildingCommand::Move` carries a service building to another place under
//! the same rules as `Place` — same entity, state kept, coverage recomputed; a bad spot is
//! refused and the building stays; the replay has the same checksum.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingCommand, BuildingGrid, BuildingId};
use cp_content::{ContentDb, ServiceKind};
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandOutcome};
use cp_net::{Elevation, NetGraph};
use cp_service::{COVERAGE_PERIOD, ServiceCoverage, ServiceState};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::TileOwnership;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

/// Two separate 200 m roads, z = 3000 and z = 3300, on one owned tile.
fn world(c: &Arc<ContentDb>) -> SimRunner {
    let road = c.road_id("basic_2l").unwrap();
    let def = c.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(c));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    for z in [3000.0, 3300.0] {
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(3000.0, 0.0, z), Vec3::new(3200.0, 0.0, z)),
                Elevation::Ground,
            )
            .unwrap();
    }
    sim.tick_once();
    sim
}

fn run(sim: &mut SimRunner, cmd: BuildingCommand) -> Result<(), String> {
    sim.push_command(SimCommand::Building(cmd), sim.tick());
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    match out.last().unwrap().result.clone() {
        Ok(_) => Ok(()),
        Err(CommandError::Rejected { reason, .. }) => Err(reason),
        Err(e) => panic!("{e:?}"),
    }
}

/// Centre of a valid cell (row 1) of the block beside the road at `z`, column `col`.
fn spot(sim: &SimRunner, z: f32, col: u16) -> Vec2 {
    let zones = sim.world().resource::<ZoneGrid>();
    zones
        .blocks()
        .filter_map(|(id, _)| {
            zones.cell_footprint(ZoneCellRef {
                block: id,
                cell: col * 4 + 1,
            })
        })
        .map(|o| o.c)
        .find(|c| (c.y - z).abs() < 40.0)
        .unwrap()
}

fn run_ticks(sim: &mut SimRunner, n: u64) {
    for _ in 0..n {
        sim.tick_once();
    }
}

fn station(sim: &SimRunner) -> BuildingId {
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .next()
        .unwrap()
        .0
}

fn fire_on(sim: &SimRunner, z: f32) -> f32 {
    let net = sim.world().resource::<NetGraph>();
    let cov = sim.world().resource::<ServiceCoverage>();
    net.segments()
        .filter(|(_, s)| (s.curve.p0.z - z).abs() < 1.0)
        .map(|(id, _)| cov.on_segment(ServiceKind::Fire, id))
        .fold(0.0, f32::max)
}

fn scenario() -> (SimRunner, BuildingId) {
    let c = content();
    let fire = c.building_id("fire_station").unwrap();
    let mut sim = world(&c);
    let a = spot(&sim, 3000.0, 5);
    run(
        &mut sim,
        BuildingCommand::Place {
            def: fire,
            pos: a,
            rot: 0.0,
        },
    )
    .unwrap();
    let id = station(&sim);
    {
        let mut st = sim.world_mut().resource_mut::<ServiceState>();
        let f = st.facility_mut(id);
        f.used_today = 2;
        f.stored = 1;
    }
    run_ticks(&mut sim, COVERAGE_PERIOD + 1);
    (sim, id)
}

#[test]
fn a_moved_fire_station_keeps_its_state_and_covers_the_other_road() {
    let (mut sim, id) = scenario();
    assert!(fire_on(&sim, 3000.0) > 0.0 && fire_on(&sim, 3300.0) <= 0.0);

    let to = spot(&sim, 3300.0, 5);
    run(
        &mut sim,
        BuildingCommand::Move {
            building: id,
            pos: to,
            rot: 0.0,
        },
    )
    .unwrap();
    let g = sim.world().resource::<BuildingGrid>();
    assert_eq!(g.len(), 1);
    let b = g.get(id).expect("same entity");
    assert!((b.pos.y - 3300.0).abs() < 40.0, "{:?}", b.pos);
    let f = sim.world().resource::<ServiceState>().facility(id).copied();
    assert_eq!(f.map(|f| (f.used_today, f.stored)), Some((2, 1)));
    // The old cells are free again: a new station fits where the old one stood.
    let c = content();
    let fire = c.building_id("fire_station").unwrap();
    let a = spot(&sim, 3000.0, 5);
    run(
        &mut sim,
        BuildingCommand::Place {
            def: fire,
            pos: a,
            rot: 0.0,
        },
    )
    .unwrap();

    run_ticks(&mut sim, COVERAGE_PERIOD + 1);
    assert!(fire_on(&sim, 3300.0) > 0.0, "covers the new road");
}

#[test]
fn a_move_to_a_bad_spot_is_refused_and_changes_nothing() {
    let (mut sim, id) = scenario();
    let before = sim
        .world()
        .resource::<BuildingGrid>()
        .get(id)
        .unwrap()
        .clone();
    // On the road itself.
    let err = run(
        &mut sim,
        BuildingCommand::Move {
            building: id,
            pos: Vec2::new(3100.0, 3300.0),
            rot: 0.0,
        },
    );
    assert!(err.is_err());
    // Onto another building.
    let c = content();
    let fire = c.building_id("fire_station").unwrap();
    let b = spot(&sim, 3300.0, 10);
    run(
        &mut sim,
        BuildingCommand::Place {
            def: fire,
            pos: b,
            rot: 0.0,
        },
    )
    .unwrap();
    assert!(
        run(
            &mut sim,
            BuildingCommand::Move {
                building: id,
                pos: b,
                rot: 0.0,
            },
        )
        .is_err()
    );
    assert_eq!(
        sim.world().resource::<BuildingGrid>().get(id).unwrap(),
        &before
    );
    // A building that is gone cannot be moved.
    run(&mut sim, BuildingCommand::Demolish { building: id }).unwrap();
    assert!(
        run(
            &mut sim,
            BuildingCommand::Move {
                building: id,
                pos: b,
                rot: 0.0,
            },
        )
        .is_err()
    );
}

#[test]
fn a_free_standing_building_moves_across_the_field_but_not_onto_a_road() {
    let c = content();
    let tower = c.building_id("water_tower").unwrap();
    let mut sim = world(&c);
    let start = Vec2::new(3100.0, 3150.0);
    run(
        &mut sim,
        BuildingCommand::Place {
            def: tower,
            pos: start,
            rot: 0.0,
        },
    )
    .unwrap();
    let id = station(&sim);
    // Overlapping its own old rectangle is fine.
    run(
        &mut sim,
        BuildingCommand::Move {
            building: id,
            pos: start + Vec2::new(4.0, 0.0),
            rot: 0.3,
        },
    )
    .unwrap();
    let b = sim
        .world()
        .resource::<BuildingGrid>()
        .get(id)
        .unwrap()
        .clone();
    assert_eq!((b.pos, b.rot), (start + Vec2::new(4.0, 0.0), 0.3));
    assert!(
        run(
            &mut sim,
            BuildingCommand::Move {
                building: id,
                pos: Vec2::new(3100.0, 3003.0),
                rot: 0.0,
            },
        )
        .is_err()
    );
    assert_eq!(sim.world().resource::<BuildingGrid>().get(id).unwrap(), &b);
}

#[test]
fn replaying_a_move_gives_the_same_checksum() {
    let sum = || {
        let (mut sim, id) = scenario();
        let to = spot(&sim, 3300.0, 5);
        run(
            &mut sim,
            BuildingCommand::Move {
                building: id,
                pos: to,
                rot: 0.0,
            },
        )
        .unwrap();
        run_ticks(&mut sim, 200);
        sim.checksum()
    };
    assert_eq!(sum(), sum());
}
