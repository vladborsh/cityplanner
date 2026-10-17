//! T-224: a game saved while residents are on their way to a park and inside it resumes like the
//! continuous run (visitors, walkers, leisure targets and the fee ledger are all in the save).

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_building::{
    Building, BuildingCommand, BuildingFlags, BuildingGrid, BuildingState, Footprint,
};
use cp_citizen::CitizenGrid;
use cp_citizen::{Location, TripBook, TripPurpose};
use cp_content::{ContentDb, ParkKind};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{CommandError, CommandOutcome, Tick};
use cp_district::{AreaId, AreaKind, DistrictCommand};
use cp_park::{ParkState, ParkVisits};
use cp_save::*;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
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

fn town(c: &Arc<ContentDb>) -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let (mut sim, _) = grid_city(500, 1, terrain, Arc::clone(c), &GridCityConfig::default());
    run(
        &mut sim,
        SimCommand::District(DistrictCommand::CreateArea {
            kind: AreaKind::Park(ParkKind::CityPark),
            name: "Park".into(),
        }),
    )
    .unwrap();
    let cells = (125..185)
        .flat_map(|x| (125..185).map(move |z| (x, z)))
        .collect();
    run(
        &mut sim,
        SimCommand::District(DistrictCommand::PaintArea {
            cells,
            area: AreaId(1),
        }),
    )
    .unwrap();
    put(&mut sim, c, "city_park_attr_1", Vec2::new(4300.0, 4300.0));
    put(&mut sim, c, "city_park_attr_2", Vec2::new(4400.0, 4300.0));
    let def = c.building_id("city_park_gate").unwrap();
    let spots: Vec<Vec2> = {
        let zones = sim.world().resource::<ZoneGrid>();
        zones
            .blocks()
            .flat_map(|(block, _)| (0..200u16).map(move |cell| ZoneCellRef { block, cell }))
            .filter_map(|r| zones.cell_footprint(r).map(|o| o.c))
            .collect()
    };
    for pos in spots {
        if run(
            &mut sim,
            SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 }),
        )
        .is_ok()
        {
            break;
        }
    }
    sim
}

#[test]
fn a_save_with_visitors_on_the_way_and_inside_resumes_like_the_continuous_run() {
    let c = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let mut sim = town(&c);
    // Run until a resident is inside the park and another is on the way to it.
    let mut ready = false;
    for _ in 0..TICKS_PER_DAY * 6 {
        sim.tick_once();
        let w = sim.world();
        let inside = !w.resource::<ParkVisits>().inside.is_empty();
        let on_way = w
            .resource::<TripBook>()
            .iter()
            .any(|(_, t)| t.purpose == TripPurpose::Leisure);
        let at_park = w
            .resource::<CitizenGrid>()
            .citizens()
            .any(|(_, c)| c.location == Location::Leisure);
        if inside && on_way && at_park {
            ready = true;
            break;
        }
    }
    assert!(ready, "a visitor inside and one on the way at once");
    assert!(sim.world().resource::<ParkState>().areas[&1].visitors_week > 0);

    let bytes = save_runner(&sim, c.content_hash(), "flatlands").unwrap();
    let mut loaded = {
        let terrain =
            Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
        let mut s = SimRunner::new(1);
        s.set_terrain(terrain);
        s.set_content(Arc::clone(&c));
        s
    };
    load_into_runner(&mut loaded, &bytes, c.content_hash()).unwrap();
    assert_eq!(loaded.checksum(), sim.checksum(), "state at load");
    for _ in 0..600 {
        sim.tick_once();
        loaded.tick_once();
    }
    assert_eq!(loaded.checksum(), sim.checksum(), "load + 600 ticks");
}
