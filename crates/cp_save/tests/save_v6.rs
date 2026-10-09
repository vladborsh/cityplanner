//! Format 6: buildings. The committed v5 fixture migrates to "no buildings"; v6 saves
//! round-trip buildings, keep their cells occupied and resume construction on schedule.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{BuildingGrid, BuildingState};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_save::migrate::{SaveDataV5, encode_v5};
use cp_save::*;
use cp_sim::SimRunner;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/saves")
            .join(name),
    )
    .unwrap()
}

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn runner(seed: u64) -> SimRunner {
    let mut sim = SimRunner::new(seed);
    sim.set_content(content());
    sim
}

#[test]
fn v5_fixture_migrates_to_no_buildings_and_keeps_its_zones() {
    let bytes = fixture("v5-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!((h.format_version, h.tick), (5, 3));
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert!(l.data.buildings.is_empty());
    assert!(
        l.data.zones.block_count() > 0,
        "the fixture carries zone blocks"
    );
    assert_eq!(l.data.net.segment_count(), 1);

    // The frozen v5 writer reproduces the committed file byte for byte.
    let v5 = SaveDataV5 {
        tick: l.data.tick,
        rng: l.data.rng.clone(),
        speed: l.data.speed,
        queue: l.data.queue.clone(),
        log: l.data.log.clone(),
        tiles: l.data.tiles,
        net: l.data.net.clone(),
        zones: l.data.zones.clone(),
    };
    assert_eq!(
        encode_v5(&v5, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        bytes
    );

    // Loaded into a runner it keeps going; zone it and buildings appear.
    let mut sim = runner(1);
    l.data.restore(sim.world_mut());
    {
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = grid.blocks().map(|(i, b)| (i, b.cell_count())).collect();
        for (id, n) in ids {
            for cell in 0..n {
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
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    assert!(!sim.world().resource::<BuildingGrid>().is_empty());

    // Re-saving writes the current version and round-trips.
    let v6 = save(&SaveData::capture(sim.world()), h.content_hash, &h.map_id).unwrap();
    assert_eq!(read_header(&v6).unwrap().0.format_version, FORMAT_VERSION);
    assert_eq!(
        load(&v6, h.content_hash).unwrap().data.buildings,
        *sim.world().resource::<BuildingGrid>()
    );
}

fn busy() -> SimRunner {
    let db = content();
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let mut sim = runner(3);
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        for (a, b) in [
            ((3000.0, 3000.0), (3090.0, 3000.0)),
            ((3045.0, 2940.0), (3045.0, 3060.0)),
        ] {
            g.add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1)),
                Elevation::Ground,
            )
            .unwrap();
        }
    }
    sim.tick_once();
    {
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = grid.blocks().map(|(i, b)| (i, b.cell_count())).collect();
        for (id, n) in ids {
            for cell in 0..n {
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    ZoneType::ComLow,
                );
            }
        }
    }
    // Long enough to have both finished and still-under-construction buildings.
    while sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .count()
        < 2
    {
        sim.tick_once();
    }
    sim
}

#[test]
fn buildings_survive_save_load_with_occupancy_checksum_and_construction_schedule() {
    let mut a = busy();
    let b0 = a.world().resource::<BuildingGrid>();
    assert!(b0.iter().any(|(_, b)| b.state == BuildingState::Active));
    // Make sure one is under construction at save time.
    for _ in 0..TICKS_PER_DAY / 2 {
        a.tick_once();
        if a.world()
            .resource::<BuildingGrid>()
            .iter()
            .any(|(_, b)| b.state == BuildingState::UnderConstruction)
        {
            break;
        }
    }
    assert!(
        a.world()
            .resource::<BuildingGrid>()
            .iter()
            .any(|(_, b)| b.state == BuildingState::UnderConstruction)
    );

    let bytes = save_runner(&a, 7, "m").unwrap();
    let loaded = load(&bytes, 7).unwrap();
    assert_eq!(
        save(&loaded.data, 7, "m").unwrap(),
        bytes,
        "byte-stable re-save"
    );

    let mut b = runner(99);
    load_into_runner(&mut b, &bytes, 7).unwrap();
    assert_eq!(b.checksum(), a.checksum(), "buildings are checksummed");
    // Occupancy is rebuilt: every building's cells are occupied and invalid.
    {
        let grid = b.world().resource::<ZoneGrid>();
        for (_, bd) in b.world().resource::<BuildingGrid>().iter() {
            for c in bd.lot.cells() {
                assert!(grid.is_occupied(c) && !grid.is_valid(c));
            }
        }
    }
    // Both continue identically: spawning, construction finishing, zone validity.
    for _ in 0..TICKS_PER_DAY + 100 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(b.checksum(), a.checksum());
    assert_eq!(
        a.world().resource::<BuildingGrid>(),
        b.world().resource::<BuildingGrid>()
    );
    assert_eq!(
        a.world().resource::<ZoneGrid>(),
        b.world().resource::<ZoneGrid>()
    );
}

#[test]
fn v6_fixture_has_finished_and_unfinished_buildings() {
    let bytes = fixture("v6-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 6);
    let l = load(&bytes, h.content_hash).unwrap();
    let states: Vec<_> = l.data.buildings.iter().map(|(_, b)| b.state).collect();
    assert!(
        states
            .iter()
            .filter(|s| **s == BuildingState::Active)
            .count()
            >= 2,
        "{states:?}"
    );
    assert!(
        states.contains(&BuildingState::UnderConstruction),
        "{states:?}"
    );
}
