//! T-224: residents and tourists visit the park areas. A town of ~1700 residents with a
//! city-park area, a gate and two attractions: visits and fees in a game day, none without a
//! gate, none to a full park, a fee that sends visitors elsewhere, tourists who pick the park,
//! the same run twice.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_building::{
    Building, BuildingCommand, BuildingFlags, BuildingGrid, BuildingState, Footprint,
};
use cp_citizen::{LeisureTargets, TripBook};
use cp_content::{ContentDb, ParkKind};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{CommandError, CommandOutcome, Tick};
use cp_district::{AreaId, AreaKind, DistrictCommand};
use cp_park::{ParkLedger, ParkState, ParkVisits, Visit};
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn town(content: &Arc<ContentDb>) -> SimRunner {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
    let (sim, report) = grid_city(
        500,
        1,
        terrain,
        Arc::clone(content),
        &GridCityConfig::default(),
    );
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    sim
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

/// Places `def` on the first free lot that takes it; panics if none does.
fn place_anywhere(sim: &mut SimRunner, c: &ContentDb, def: &str) {
    let def = c.building_id(def).unwrap();
    let spots: Vec<Vec2> = {
        let zones = sim.world().resource::<ZoneGrid>();
        zones
            .blocks()
            .flat_map(|(block, _)| (0..200u16).map(move |cell| ZoneCellRef { block, cell }))
            .filter_map(|r| zones.cell_footprint(r).map(|o| o.c))
            .collect()
    };
    for pos in spots {
        let cmd = SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 });
        if run(sim, cmd).is_ok() {
            return;
        }
    }
    panic!("no lot takes {def:?}");
}

/// An active free-standing park building, put straight into the grid.
fn put(sim: &mut SimRunner, c: &ContentDb, id: &str, pos: Vec2) {
    let def = c.building_id(id).unwrap();
    let d = c.building(def);
    sim.world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
            def,
            footprint: Footprint::Free {
                w: d.lot.0,
                d: d.lot.1,
            },
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos,
            rot: 0.0,
            access: None,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
}

/// A city-park area over the town, two attractions in it and, with `gate`, a gate; runs until
/// everything is built. Returns the area.
fn add_park(sim: &mut SimRunner, c: &ContentDb, gate: bool) -> AreaId {
    run(
        sim,
        SimCommand::District(DistrictCommand::CreateArea {
            kind: AreaKind::Park(ParkKind::CityPark),
            name: "Park".into(),
        }),
    )
    .unwrap();
    let id = AreaId(sim.view().areas.iter().last().unwrap().0.0);
    let cells = (125..185)
        .flat_map(|x| (125..185).map(move |z| (x, z)))
        .collect();
    run(
        sim,
        SimCommand::District(DistrictCommand::PaintArea { cells, area: id }),
    )
    .unwrap();
    put(sim, c, "city_park_attr_1", Vec2::new(4300.0, 4300.0));
    put(sim, c, "city_park_attr_2", Vec2::new(4400.0, 4300.0));
    if gate {
        place_anywhere(sim, c, "city_park_gate");
    }
    for _ in 0..TICKS_PER_DAY * 2 {
        sim.tick_once();
    }
    id
}

fn visits(sim: &SimRunner, area: AreaId) -> u32 {
    sim.world().resource::<ParkState>().areas[&area.0].visitors_week
}

fn days(sim: &mut SimRunner, n: u64) {
    for _ in 0..TICKS_PER_DAY * n {
        sim.tick_once();
    }
}

#[test]
fn a_park_with_a_gate_and_attractions_gets_visits_and_fees_and_without_a_gate_none() {
    let c = content();
    let mut sim = town(&c);
    let area = add_park(&mut sim, &c, true);
    {
        let mut st = sim.world_mut().resource_mut::<ParkState>();
        st.areas.get_mut(&area.0).unwrap().fee = 5.0;
        assert!(st.areas[&area.0].capacity >= 120);
    }
    days(&mut sim, 2);
    assert!(
        !sim.world().resource::<LeisureTargets>().is_empty(),
        "the gate is a target"
    );
    let n = visits(&sim, area);
    assert!(n > 0, "visits in two game days");
    assert!(
        sim.world().resource::<ParkLedger>().fees.0 > 0,
        "fees came in"
    );
    let leisure = sim.world().resource::<TripBook>().completed;
    assert!(leisure > 0);

    // No gate: nobody goes.
    let mut none = town(&c);
    let area = add_park(&mut none, &c, false);
    days(&mut none, 3);
    assert!(none.world().resource::<LeisureTargets>().is_empty());
    assert_eq!(visits(&none, area), 0);
    assert!(none.world().resource::<ParkLedger>().is_empty());
}

#[test]
fn a_full_park_takes_no_new_visitors() {
    let c = content();
    let mut sim = town(&c);
    let area = add_park(&mut sim, &c, true);
    // 400 visitors inside against a capacity of 120, for a long time.
    sim.world_mut()
        .resource_mut::<ParkVisits>()
        .inside
        .extend((0..400).map(|_| Visit {
            area: area.0,
            leave_at: Tick(u64::MAX / 2),
        }));
    // The next hourly refill sees a full park: the gate draws nobody.
    for _ in 0..TICKS_PER_DAY / 24 + 2 {
        sim.tick_once();
    }
    let before = visits(&sim, area);
    days(&mut sim, 2);
    assert!(sim.world().resource::<LeisureTargets>().is_empty());
    assert_eq!(visits(&sim, area), before, "no new visit while it is full");
}

#[test]
fn the_highest_fee_sends_some_visitors_to_the_city_parks_instead() {
    let c = content();
    let count = |fee_of: fn(f32) -> f32| {
        let mut sim = town(&c);
        let area = add_park(&mut sim, &c, true);
        place_anywhere(&mut sim, &c, "small_park");
        place_anywhere(&mut sim, &c, "playground");
        days(&mut sim, 1);
        let max = c.balance().park_fee_max;
        sim.world_mut()
            .resource_mut::<ParkState>()
            .areas
            .get_mut(&area.0)
            .unwrap()
            .fee = fee_of(max);
        days(&mut sim, 4);
        visits(&sim, area)
    };
    let free = count(|_| 0.0);
    let dear = count(|m| m);
    assert!(free > 0);
    assert!(
        dear < free,
        "{dear} visits at the highest fee, {free} for free"
    );
}

#[test]
fn the_same_town_replays_the_same_visits() {
    let c = content();
    let once = || {
        let mut sim = town(&c);
        let area = add_park(&mut sim, &c, true);
        days(&mut sim, 2);
        (visits(&sim, area), sim.checksum())
    };
    assert_eq!(once(), once());
}
