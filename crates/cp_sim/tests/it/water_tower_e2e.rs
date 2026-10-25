//! T-109 acceptance: on flatlands the water tower goes into a field, with no road and no water
//! nearby, and a pipe within 40 m of it supplies the houses of the pipe's component — with no
//! power plant anywhere (the tower needs power, but pumps like a pump without it).

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingCommand, BuildingFlags, BuildingGrid, BuildingState, Demand};
use cp_content::{ContentDb, UtilityKind, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandOutcome, SimLimits};
use cp_net::{Elevation, NetGraph};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_utility::UtilityCommand;
use cp_zone::{ZoneCellRef, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn run(sim: &mut SimRunner, cmd: SimCommand) -> Result<(), String> {
    sim.push_command(cmd, sim.tick());
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

/// A city: one 200 m road at z = 5000, its blocks painted for houses, a pipe along it, and
/// (when `tower`) a water tower 35 m off the pipe in the field. Returns the runner after the
/// houses have been built and the supply has run.
fn city(tower: bool) -> SimRunner {
    let c = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(Arc::new(
        Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap(),
    ));
    sim.set_content(Arc::clone(&c));
    sim.world_mut()
        .insert_resource(TileOwnership::m1_around((2, 2)));
    let road = c.road_id("basic_2l").unwrap();
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &c.road(road).clone(),
            road,
            CubicBezier3::line(
                Vec3::new(5000.0, 0.0, 5000.0),
                Vec3::new(5200.0, 0.0, 5000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();
    if tower {
        let def = c.building_id("water_tower").unwrap();
        run(
            &mut sim,
            SimCommand::Building(BuildingCommand::Place {
                def,
                pos: Vec2::new(5100.0, 5035.0),
                rot: 0.3,
            }),
        )
        .unwrap();
    }
    run(
        &mut sim,
        SimCommand::Utility(UtilityCommand::BuildLink {
            kind: UtilityKind::Water,
            path: vec![Vec2::new(5010.0, 5000.0), Vec2::new(5190.0, 5000.0)],
        }),
    )
    .unwrap();
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(1.0);
    {
        let mut zones = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = zones.blocks().map(|(id, b)| (id, b.cell_count())).collect();
        for (block, n) in ids {
            for cell in 0..n {
                zones.set_zone(
                    ZoneCellRef {
                        block,
                        cell: cell as u16,
                    },
                    ZoneType::ResLow,
                );
            }
        }
    }
    for _ in 0..3000 {
        sim.tick_once();
    }
    // Let the supply (every 64 ticks) see every house that stands now.
    for _ in 0..70 {
        sim.tick_once();
    }
    sim
}

#[test]
fn a_tower_in_a_field_waters_the_houses_on_its_pipe() {
    let sim = city(true);
    let c = sim.world().resource::<cp_content::SimContent>().0.clone();
    let tower_def = c.building_id("water_tower").unwrap();
    let g = sim.world().resource::<BuildingGrid>();
    let (_, tower) = g.iter().find(|(_, b)| b.def == tower_def).unwrap();
    assert!(tower.lot().is_none() && tower.access().is_none());
    assert_eq!(tower.state, BuildingState::Active);
    assert!(
        !tower.flags.has(BuildingFlags::NO_ROAD) && !tower.flags.has(BuildingFlags::NO_PATH),
        "free-standing: no road flags"
    );
    assert!(
        tower.flags.has(BuildingFlags::NO_POWER),
        "no plant anywhere"
    );
    assert!(!tower.flags.has(BuildingFlags::NO_WATER));

    let houses: Vec<_> = g
        .iter()
        .filter(|(_, b)| b.def != tower_def && b.state == BuildingState::Active)
        .collect();
    assert!(houses.len() >= 5, "{} houses", houses.len());
    let watered = houses
        .iter()
        .filter(|(_, b)| !b.flags.has(BuildingFlags::NO_WATER))
        .count();
    assert!(
        watered >= 5,
        "{watered} of {} houses have water",
        houses.len()
    );
}

#[test]
fn without_the_tower_the_same_houses_have_no_water() {
    let sim = city(false);
    let g = sim.world().resource::<BuildingGrid>();
    let houses: Vec<_> = g
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .collect();
    assert!(houses.len() >= 5, "{} houses", houses.len());
    assert!(
        houses
            .iter()
            .all(|(_, b)| b.flags.has(BuildingFlags::NO_WATER))
    );
}
