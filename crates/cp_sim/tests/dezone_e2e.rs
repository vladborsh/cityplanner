//! BUG-053: zoning over a building always works, and the building that no longer matches its
//! zone is demolished in the same tick.

use std::path::Path;
use std::sync::Arc;

use cp_building::{BuildingGrid, BuildingId, held_cells};
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, ZoneType};
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_zone::{ZoneCellRef, ZoneCommand, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn city() -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let (sim, report) = grid_city(120, 1, terrain, content, &GridCityConfig::default());
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    sim
}

/// A zoned building people live in, and the cells of its lot.
fn a_house(sim: &SimRunner) -> (BuildingId, Vec<ZoneCellRef>) {
    let w = sim.world();
    let db = &w.resource::<cp_content::SimContent>().0;
    let zones = w.resource::<ZoneGrid>();
    let buildings = w.resource::<BuildingGrid>();
    let citizens = w.resource::<CitizenGrid>();
    citizens
        .citizens()
        .filter_map(|(_, c)| c.home)
        .find_map(|id| {
            let b = buildings.get(id)?;
            let held = held_cells(zones, b);
            (db.building(b.def).service.is_none() && !held.is_empty()).then_some((id, held))
        })
        .expect("a lived-in house")
}

fn paint(sim: &mut SimRunner, cells: Vec<ZoneCellRef>, zone: ZoneType) {
    sim.push_command(
        SimCommand::Zone(ZoneCommand::PaintZones { cells, zone }),
        sim.tick(),
    );
    sim.tick_once();
}

#[test]
fn erasing_the_zone_under_a_house_works_and_the_house_goes() {
    let mut sim = city();
    let (house, cells) = a_house(&sim);
    paint(&mut sim, cells.clone(), ZoneType::None);
    assert!(sim.world().resource::<BuildingGrid>().get(house).is_none());
    let zones = sim.world().resource::<ZoneGrid>();
    for c in &cells {
        assert_eq!(zones.zone_of(*c), Some(ZoneType::None));
        assert!(!zones.is_occupied(*c), "the cells are free again");
    }
    // The people have left their home a couple of ticks later.
    for _ in 0..4 {
        sim.tick_once();
    }
    let g = sim.world().resource::<CitizenGrid>();
    assert!(g.citizens().all(|(_, c)| c.home != Some(house)));
}

#[test]
fn painting_another_zone_over_a_house_replaces_it_and_the_cells_can_grow_the_new_zone() {
    let mut sim = city();
    let (house, cells) = a_house(&sim);
    paint(&mut sim, cells.clone(), ZoneType::ComLow);
    assert!(sim.world().resource::<BuildingGrid>().get(house).is_none());
    let zones = sim.world().resource::<ZoneGrid>();
    for c in &cells {
        assert_eq!(zones.zone_of(*c), Some(ZoneType::ComLow));
        assert!(
            zones.is_valid(*c) && !zones.is_occupied(*c),
            "free to build on"
        );
    }
}

#[test]
fn zoning_next_to_a_house_leaves_it_and_services_are_never_demolished() {
    let mut sim = city();
    let (house, cells) = a_house(&sim);
    // The cells of the same block that are not under the house.
    let outside: Vec<ZoneCellRef> = {
        let zones = sim.world().resource::<ZoneGrid>();
        zones
            .block_cells(cells[0].block)
            .into_iter()
            .filter(|c| !cells.contains(c) && zones.is_valid(*c))
            .take(6)
            .collect()
    };
    assert!(!outside.is_empty());
    paint(&mut sim, outside, ZoneType::None);
    assert!(sim.world().resource::<BuildingGrid>().get(house).is_some());

    // A service building on zoned cells stays, whatever is painted under it.
    let svc = {
        let w = sim.world();
        let db = &w.resource::<cp_content::SimContent>().0;
        let zones = w.resource::<ZoneGrid>();
        w.resource::<BuildingGrid>()
            .iter()
            .find(|(_, b)| db.building(b.def).service.is_some() && !held_cells(zones, b).is_empty())
            .map(|(id, b)| (id, held_cells(zones, b)))
    };
    if let Some((id, held)) = svc {
        paint(&mut sim, held, ZoneType::None);
        assert!(sim.world().resource::<BuildingGrid>().get(id).is_some());
    }
}

#[test]
fn dezoning_is_deterministic() {
    let play = || {
        let mut sim = city();
        let (_, cells) = a_house(&sim);
        paint(&mut sim, cells, ZoneType::None);
        for _ in 0..300 {
            sim.tick_once();
        }
        sim
    };
    let (mut a, mut b) = (play(), play());
    assert_eq!(a.checksum(), b.checksum());
}
