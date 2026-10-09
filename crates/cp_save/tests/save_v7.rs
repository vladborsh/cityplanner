//! Format 7: citizens. The committed v6 fixture migrates to "nobody lives here yet" and
//! keeps its buildings; v7 saves round-trip households, jobs and the derived occupancy.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{BuildingGrid, BuildingState};
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_save::migrate::{SaveDataV6, encode_v6};
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
fn v6_fixture_migrates_to_no_citizens_and_keeps_its_buildings() {
    let bytes = fixture("v6-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 6);
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert!(l.data.citizens.citizen_count() == 0 && l.data.citizens.household_count() == 0);
    assert!(l.data.buildings.len() >= 3);

    // The frozen v6 writer reproduces the committed file byte for byte.
    let v6 = SaveDataV6 {
        tick: l.data.tick,
        rng: l.data.rng.clone(),
        speed: l.data.speed,
        queue: l.data.queue.clone(),
        log: l.data.log.clone(),
        tiles: l.data.tiles,
        net: l.data.net.clone(),
        zones: l.data.zones.clone(),
        buildings: l.data.buildings.clone(),
    };
    assert_eq!(
        encode_v6(&v6, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        bytes
    );

    // Loaded into a runner: residents move into the finished buildings.
    let mut sim = runner(1);
    l.data.restore(sim.world_mut());
    for _ in 0..(3 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    assert!(sim.world().resource::<CitizenGrid>().household_count() > 0);

    // Re-saving writes v7 and round-trips the citizens.
    let v7 = save(&SaveData::capture(sim.world()), h.content_hash, &h.map_id).unwrap();
    assert_eq!(read_header(&v7).unwrap().0.format_version, FORMAT_VERSION);
    assert_eq!(
        load(&v7, h.content_hash).unwrap().data.citizens,
        *sim.world().resource::<CitizenGrid>()
    );
}

fn town() -> SimRunner {
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
        for (n, (id, cells)) in ids.into_iter().enumerate() {
            let zone = if n % 2 == 0 {
                ZoneType::ResLow
            } else {
                ZoneType::Industrial
            };
            for cell in 0..cells {
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    zone,
                );
            }
        }
    }
    for _ in 0..(3 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    sim
}

#[test]
fn citizens_survive_save_load_with_jobs_checksum_and_identical_future() {
    let mut a = town();
    let cg = a.world().resource::<CitizenGrid>();
    assert!(cg.citizen_count() > 5, "{} citizens", cg.citizen_count());
    assert!(
        a.world()
            .resource::<BuildingGrid>()
            .iter()
            .any(|(_, b)| b.state == BuildingState::Active)
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
    assert_eq!(b.checksum(), a.checksum(), "citizens are checksummed");
    for _ in 0..TICKS_PER_DAY {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(b.checksum(), a.checksum());
    assert_eq!(
        a.world().resource::<CitizenGrid>(),
        b.world().resource::<CitizenGrid>()
    );
}
