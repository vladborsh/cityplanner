//! T-225: park levels by visitors, the fee command, land value around a park by its level and
//! the week's entrance fees in the books.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingCommand, BuildingFlags, BuildingGrid, BuildingState, Footprint,
};
use cp_citizen::{LeisureArrival, TripBook};
use cp_content::{ContentDb, ParkKind};
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandOutcome, Cost, Id, Tick};
use cp_district::{AreaId, AreaKind, Areas, DistrictCommand};
use cp_economy::LandGrids;
use cp_net::{Elevation, NetGraph};
use cp_park::{ParkCommand, ParkLedger, ParkState};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{GRID, MapMeta, Terrain, TileOwnership};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn terrain() -> Terrain {
    let meta = MapMeta {
        name: "flat".into(),
        sea_level: 0.0,
        start_tile: (1, 1),
        external_connections: vec![],
        entry_road: None,
        entry_rail: None,
    };
    Terrain::from_samples(meta, &vec![1000; GRID * GRID], &vec![0; GRID * GRID]).unwrap()
}

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

fn place(sim: &mut SimRunner, c: &ContentDb, id: &str, pos: Vec2) -> Result<(), String> {
    let def = c.building_id(id).unwrap();
    run(
        sim,
        SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 }),
    )
}

/// A street on an owned tile, a city-park area over the ground south of it and a gate that
/// stands already (put in the grid, active).
fn world(c: &Arc<ContentDb>) -> (SimRunner, AreaId) {
    let road = c.road_id("basic_2l").unwrap();
    let def = c.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(c));
    sim.set_terrain(Arc::new(terrain()));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    let seg = sim
        .world_mut()
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
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance = Cost::from_units(5.0e6);
    let cells = (90..100)
        .flat_map(|x| (88..100).map(move |z| (x, z)))
        .collect();
    for cmd in [
        DistrictCommand::CreateArea {
            kind: AreaKind::Park(ParkKind::CityPark),
            name: "Park".into(),
        },
        DistrictCommand::PaintArea {
            cells,
            area: AreaId(1),
        },
    ] {
        run(&mut sim, SimCommand::District(cmd)).unwrap();
    }
    let gate = c.building_id("city_park_gate").unwrap();
    sim.world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
            def: gate,
            footprint: Footprint::Free { w: 2, d: 2 },
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos: Vec2::new(3100.0, 3010.0),
            rot: 0.0,
            access: Some(AccessPoint {
                segment: seg,
                t: 0.5,
            }),
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
    sim.tick_once();
    (sim, AreaId(1))
}

/// `n` residents arrive at the gate (as the routine would report them).
fn arrivals(sim: &mut SimRunner, n: u32) {
    let gate = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .next()
        .unwrap()
        .0;
    let mut book = sim.world_mut().resource_mut::<TripBook>();
    for i in 0..n {
        book.leisure_arrivals.push(LeisureArrival {
            citizen: Id::new(i, 0),
            building: gate,
            leave_at: Tick(1),
        });
    }
}

#[test]
fn an_area_earns_xp_by_visits_rises_at_the_threshold_and_opens_the_next_attraction() {
    let c = content();
    let (mut sim, area) = world(&c);
    let at = Vec2::new(3100.0, 3100.0);
    let e = place(&mut sim, &c, "city_park_attr_2", at).unwrap_err();
    assert!(e.contains("needs area level 2 (it is 1)"), "{e}");

    let need = c.balance().park_levels[0];
    arrivals(&mut sim, need - 1);
    sim.tick_once();
    let a = sim.world().resource::<Areas>().get(area).unwrap().clone();
    assert_eq!((a.level, a.xp), (1, need - 1));
    arrivals(&mut sim, 1);
    sim.tick_once();
    let a = sim.world().resource::<Areas>().get(area).unwrap().clone();
    assert_eq!((a.level, a.xp), (2, need));
    place(&mut sim, &c, "city_park_attr_2", at).unwrap();
}

#[test]
fn the_fee_is_set_within_its_range_for_a_park_area_only() {
    let c = content();
    let (mut sim, area) = world(&c);
    sim.tick_once();
    let max = c.balance().park_fee_max;
    let set = |sim: &mut SimRunner, area, fee| {
        run(sim, SimCommand::Park(ParkCommand::SetFee { area, fee }))
    };
    set(&mut sim, area, 7.5).unwrap();
    assert_eq!(sim.world().resource::<ParkState>().areas[&area.0].fee, 7.5);
    // The count does not lose it.
    sim.tick_once();
    assert_eq!(sim.world().resource::<ParkState>().areas[&area.0].fee, 7.5);
    assert!(
        set(&mut sim, area, max + 1.0)
            .unwrap_err()
            .contains("between 0 and")
    );
    assert!(set(&mut sim, area, -1.0).is_err());
    assert!(set(&mut sim, area, f32::NAN).is_err());
    assert!(
        set(&mut sim, AreaId(9), 1.0)
            .unwrap_err()
            .contains("no such area")
    );
    assert_eq!(sim.world().resource::<ParkState>().areas[&area.0].fee, 7.5);
    // An industrial area is not a park.
    run(
        &mut sim,
        SimCommand::District(DistrictCommand::CreateArea {
            kind: AreaKind::Industry(cp_content::ResourceKind::Farming),
            name: "Farm".into(),
        }),
    )
    .unwrap();
    assert!(
        set(&mut sim, AreaId(2), 1.0)
            .unwrap_err()
            .contains("not a park")
    );
}

#[test]
fn land_next_to_a_higher_level_park_is_worth_more() {
    let c = content();
    let (mut sim, area) = world(&c);
    let near = Vec2::new(3100.0, 2950.0);
    let value = |sim: &mut SimRunner| {
        // The city count runs on its period.
        for _ in 0..cp_economy::CITY_PERIOD {
            sim.tick_once();
        }
        sim.world().resource::<LandGrids>().value_at(near)
    };
    let one = value(&mut sim);
    sim.world_mut()
        .resource_mut::<Areas>()
        .get_mut(area)
        .unwrap()
        .level = 3;
    let three = value(&mut sim);
    assert!(three > one + 1.0, "level 3: {three}, level 1: {one}");
}

#[test]
fn the_weeks_entrance_fees_reach_the_books() {
    let c = content();
    let (mut sim, _) = world(&c);
    sim.world_mut().resource_mut::<ParkLedger>().fees = Cost::from_units(120.0);
    for _ in 0..cp_core::time::TICKS_PER_WEEK + 200 {
        sim.tick_once();
    }
    let ledger = *sim.world().resource::<ParkLedger>();
    assert_eq!(ledger.last_fees, Cost::from_units(120.0));
    assert_eq!(ledger.fees, Cost::ZERO);
    let flow = sim.view().flow;
    assert_eq!(flow.week_to_date.park_fees, Cost::ZERO, "a new week began");
    assert!(
        flow.hours
            .iter()
            .any(|(_, f)| f.park_fees == Cost::from_units(120.0)),
        "the hour that closed the week booked it"
    );
}
