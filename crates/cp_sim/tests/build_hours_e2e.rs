//! BUG-021 acceptance: construction takes `build_hours` of game time, not a day, so the first
//! building of a freshly painted flat road is standing within a few hundred ticks.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec3;
use cp_building::{BuildingCommand, BuildingGrid, BuildingState, build_ticks};
use cp_content::{ContentDb, ZoneType, default_build_hours};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_HOUR;
use cp_core::{CommandOutcome, Tick};
use cp_net::{Elevation, NetGraph};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::TileOwnership;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn road(sim: &mut SimRunner, content: &ContentDb) {
    let id = content.road_id("basic_2l").unwrap();
    let def = content.road(id).clone();
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            id,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3200.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
}

#[test]
fn default_hours_follow_the_lot_area() {
    assert_eq!(default_build_hours((1, 1)), 2.0);
    assert_eq!(default_build_hours((2, 1)), 3.0);
    assert_eq!(default_build_hours((2, 2)), 3.0);
    assert_eq!(default_build_hours((3, 2)), 4.0);
    assert_eq!(default_build_hours((3, 3)), 4.0);
    assert_eq!(default_build_hours((4, 3)), 6.0);
    assert_eq!(default_build_hours((4, 4)), 6.0);
}

#[test]
fn first_building_is_active_within_700_ticks_of_painting() {
    let content = content();
    let mut sim = SimRunner::new(7);
    sim.set_content(Arc::clone(&content));
    road(&mut sim, &content);
    sim.tick_once();
    {
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = grid.blocks().map(|(id, b)| (id, b.cell_count())).collect();
        for (id, cells) in ids {
            for cell in 0..cells {
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    ZoneType::ResLow,
                );
            }
        }
    }
    let painted = sim.tick().0;
    let mut first = None;
    while sim.tick().0 < painted + 700 {
        sim.tick_once();
        let g = sim.world().resource::<BuildingGrid>();
        if g.iter().any(|(_, b)| b.state == BuildingState::Active) {
            first = Some(sim.tick().0 - painted);
            break;
        }
    }
    assert!(first.is_some(), "no Active building within 700 ticks");
}

#[test]
fn a_two_by_two_building_takes_round_three_hours() {
    let content = content();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(&content));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    road(&mut sim, &content);
    sim.tick_once();
    let def = content.building_id("wind_turbine").unwrap();
    assert_eq!(content.building(def).lot, (2, 2));
    let at = {
        let zones = sim.world().resource::<ZoneGrid>();
        let (id, _) = zones.blocks().next().unwrap();
        zones
            .cell_footprint(ZoneCellRef {
                block: id,
                cell: 5 * 4 + 1,
            })
            .unwrap()
            .c
    };
    let start = sim.tick();
    sim.push_command(
        SimCommand::Building(BuildingCommand::Place {
            def,
            pos: at,
            rot: 0.0,
        }),
        start,
    );
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    assert!(out.last().unwrap().result.is_ok(), "{out:?}");
    let want = (3.0 * TICKS_PER_HOUR).round() as u64;
    assert_eq!(build_ticks(content.building(def)), want);
    let b = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .next()
        .unwrap()
        .1
        .clone();
    assert_eq!(b.ready_at, Tick(start.0 + want));
}
