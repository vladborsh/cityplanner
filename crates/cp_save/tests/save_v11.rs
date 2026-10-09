//! Format 11: agents. The committed v10 fixture migrates to "no agents" and keeps its trips;
//! v11 saves round-trip pedestrians and vehicles and resume them identically.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_citizen::TripBook;
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_save::migrate::{SaveDataV10, encode_v10};
use cp_save::*;
use cp_sim::SimRunner;
use cp_traffic::Agents;
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
fn v10_fixture_migrates_to_no_agents_and_keeps_its_trips() {
    let bytes = fixture("v10-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 10);
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert!(!l.data.trips.is_empty(), "the fixture has trips in flight");
    assert_eq!(
        l.data.agents.pedestrian_count() + l.data.agents.vehicle_count(),
        0
    );

    // The frozen v10 writer reproduces the committed file byte for byte.
    let v10 = SaveDataV10 {
        tick: l.data.tick,
        rng: l.data.rng.clone(),
        speed: l.data.speed,
        queue: l.data.queue.clone(),
        log: l.data.log.clone(),
        tiles: l.data.tiles,
        net: l.data.net.clone(),
        zones: l.data.zones.clone(),
        buildings: l.data.buildings.clone(),
        citizens: l.data.citizens.clone(),
        paths: l.data.paths.clone(),
        utilities: l.data.utilities.clone(),
        trips: l.data.trips.clone(),
    };
    assert_eq!(
        encode_v10(&v10, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        bytes
    );

    // Loaded into a runner it keeps running; old-format trips still finish.
    let before = l.data.trips.len();
    let mut sim = runner(1);
    l.data.restore(sim.world_mut());
    for _ in 0..TICKS_PER_DAY {
        sim.tick_once();
    }
    assert!(sim.world().resource::<TripBook>().completed > 0 || before == 0);
    let v11 = save(&SaveData::capture(sim.world()), h.content_hash, &h.map_id).unwrap();
    assert_eq!(read_header(&v11).unwrap().0.format_version, FORMAT_VERSION);
    assert_eq!(
        load(&v11, h.content_hash).unwrap().data.agents,
        *sim.world().resource::<Agents>()
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
fn agents_survive_save_load_and_resume_identically() {
    let mut a = town();
    // Run on until someone is walking.
    for _ in 0..(3 * TICKS_PER_DAY) {
        a.tick_once();
        if a.world().resource::<Agents>().pedestrian_count() > 0 {
            break;
        }
    }
    assert!(
        a.world().resource::<Agents>().pedestrian_count() > 0,
        "somebody walks"
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
    assert_eq!(b.checksum(), a.checksum(), "agents are checksummed");
    for _ in 0..TICKS_PER_DAY {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(b.checksum(), a.checksum());
    assert_eq!(
        a.world().resource::<Agents>(),
        b.world().resource::<Agents>()
    );
}
