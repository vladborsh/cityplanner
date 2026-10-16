//! Format 10: trips. The committed v9 fixture migrates to "no trips in flight" and keeps
//! its citizens; v10 saves round-trip trips and resume them on schedule.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_citizen::{CitizenGrid, Location, TripBook};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_save::migrate::{SaveDataV9, encode_v9};
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
fn v9_fixture_migrates_to_no_trips_and_keeps_its_world() {
    let bytes = fixture("v9-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 9);
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert!(l.data.trips.is_empty());
    assert!(l.data.net.segment_count() >= 2 && l.data.utilities.link_count() == 2);

    // The frozen v9 writer reproduces the committed file byte for byte.
    let v9 = SaveDataV9 {
        tick: l.data.tick,
        rng: l.data.rng.clone(),
        speed: l.data.speed,
        queue: l.data.queue.clone(),
        log: l.data.log.clone(),
        tiles: l.data.tiles,
        net: l.data.net.clone(),
        zones: l.data.zones.clone().to_v34(),
        buildings: cp_save::migrate::BuildingGridV12::from(&l.data.buildings),
        citizens: (&l.data.citizens).into(),
        paths: l.data.paths.clone(),
        utilities: l.data.utilities.clone(),
    };
    assert_eq!(
        encode_v9(&v9, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        bytes
    );

    // Loaded into a runner it keeps running, with an empty trip book.
    let mut sim = runner(1);
    l.data.restore(sim.world_mut());
    assert!(sim.world().resource::<TripBook>().is_empty());
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    let v10 = save(&SaveData::capture(sim.world()), h.content_hash, &h.map_id).unwrap();
    assert_eq!(read_header(&v10).unwrap().0.format_version, FORMAT_VERSION);
    assert_eq!(
        load(&v10, h.content_hash).unwrap().data.trips,
        *sim.world().resource::<TripBook>()
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
            ((3000.0, 3090.0), (3090.0, 3090.0)),
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
        let kinds = [ZoneType::ResLow, ZoneType::Industrial, ZoneType::ComLow];
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = grid.blocks().map(|(i, b)| (i, b.cell_count())).collect();
        for (n, (id, cells)) in ids.into_iter().enumerate() {
            for cell in 0..cells {
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    kinds[n % kinds.len()],
                );
            }
        }
    }
    for _ in 0..(5 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    sim
}

#[test]
fn trips_survive_save_load_and_resume_identically() {
    let mut a = town();
    // Save in the middle of a working day with people in transit.
    while a.tick().0 % TICKS_PER_DAY < TICKS_PER_DAY / 3
        || !a
            .world()
            .resource::<CitizenGrid>()
            .citizens()
            .any(|(_, c)| {
                c.location == Location::Moving && !a.world().resource::<TripBook>().is_empty()
            })
    {
        a.tick_once();
    }
    assert!(!a.world().resource::<TripBook>().is_empty());

    let bytes = save_runner(&a, 7, "m").unwrap();
    let loaded = load(&bytes, 7).unwrap();
    assert_eq!(
        save(&loaded.data, 7, "m").unwrap(),
        bytes,
        "byte-stable re-save"
    );

    let mut b = runner(99);
    load_into_runner(&mut b, &bytes, 7).unwrap();
    assert_eq!(b.checksum(), a.checksum(), "trips are checksummed");
    for _ in 0..TICKS_PER_DAY {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(b.checksum(), a.checksum());
    assert_eq!(
        a.world().resource::<TripBook>(),
        b.world().resource::<TripBook>()
    );
}
