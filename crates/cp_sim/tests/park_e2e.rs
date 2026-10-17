//! T-221: the city parks are service buildings (ServiceKind::Park) that stand ready at once, lift
//! the land value around them inside their radius and pollute nothing.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingCommand, BuildingGrid, BuildingState};
use cp_content::{ContentDb, ServiceKind};
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandOutcome};
use cp_economy::LandGrids;
use cp_net::{Elevation, NetGraph};
use cp_sim::{Progress, SimCommand, SimRunner};
use cp_terrain::TileOwnership;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn world(content: &Arc<ContentDb>) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(content));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3200.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();
    sim.world_mut().insert_resource(Progress::ALL);
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance =
        cp_core::Cost::from_units(50_000_000.0);
    sim
}

fn spot(sim: &SimRunner, col: u16) -> Vec2 {
    let zones = sim.world().resource::<ZoneGrid>();
    let (id, _) = zones.blocks().next().unwrap();
    zones
        .cell_footprint(ZoneCellRef {
            block: id,
            cell: col * 4 + 1,
        })
        .unwrap()
        .c
}

fn place(sim: &mut SimRunner, name: &str, col: u16) -> Result<(), CommandError> {
    let def = sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .building_id(name)
        .unwrap();
    let pos = spot(sim, col);
    sim.push_command(
        SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 }),
        sim.tick(),
    );
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last().unwrap().result.clone().map(|_| ())
}

#[test]
fn the_four_parks_are_park_service_buildings_with_a_land_effect() {
    let c = content();
    for name in ["small_park", "playground", "plaza_fountain", "sports_field"] {
        let d = c.building(c.building_id(name).unwrap());
        let svc = c.service(c.service_id(d.service.as_deref().unwrap()).unwrap());
        assert_eq!(svc.kind, ServiceKind::Park, "{name}");
        assert!(
            d.land_effect
                .is_some_and(|e| e.add > 0.0 && e.radius_m > 0.0),
            "{name}"
        );
    }
}

#[test]
fn a_park_stands_ready_lifts_the_land_value_inside_its_radius_only_and_does_not_pollute() {
    let c = content();
    let run = |with: bool| {
        let mut sim = world(&c);
        if with {
            place(&mut sim, "small_park", 2).unwrap();
            let g = sim.world().resource::<BuildingGrid>();
            assert!(g.iter().all(|(_, b)| b.state == BuildingState::Active));
        }
        for _ in 0..300 {
            sim.tick_once();
        }
        let p = spot(&sim, 2);
        let g = sim.world().resource::<LandGrids>();
        (
            g.value_at(p),
            g.value_at(p + Vec2::new(0.0, 600.0)),
            g.pollution_at(p),
        )
    };
    let (near_with, far_with, poll_with) = run(true);
    let (near_without, far_without, poll_without) = run(false);
    assert!(near_with > near_without, "{near_with} vs {near_without}");
    assert_eq!(far_with, far_without);
    assert_eq!(poll_with, poll_without);
}
