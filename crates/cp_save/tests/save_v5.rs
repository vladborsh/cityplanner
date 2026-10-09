//! Format 5: zone state. The v4 fixture migrates to an empty grid that the `Network` phase
//! fills from the loaded roads; v5 saves round-trip with their zones.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_save::migrate::{SaveDataV4, encode_v4};
use cp_save::*;
use cp_sim::SimRunner;
use cp_zone::ZoneGrid;

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
fn v4_fixture_migrates_to_empty_zones_and_the_network_phase_builds_them() {
    let bytes = fixture("v4-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!((h.format_version, h.tick), (4, 1000));
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert_eq!(l.data.net.segment_count(), 2);
    assert_eq!(l.data.zones, ZoneGrid::default());

    // The frozen v4 writer reproduces the committed file byte for byte.
    let v4 = SaveDataV4 {
        tick: l.data.tick,
        rng: l.data.rng.clone(),
        speed: l.data.speed,
        queue: l.data.queue.clone(),
        log: l.data.log.clone(),
        tiles: l.data.tiles,
        net: l.data.net.clone(),
    };
    assert_eq!(
        encode_v4(&v4, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        bytes
    );

    // Restored into a runner, the first tick derives blocks for both segments.
    let mut sim = runner(1);
    l.data.clone().restore(sim.world_mut());
    assert_eq!(sim.world().resource::<ZoneGrid>().block_count(), 0);
    sim.tick_once();
    let grid = sim.world().resource::<ZoneGrid>();
    assert_eq!(grid.block_count(), 4, "two segments, two sides each");
    assert!(grid.valid_cells() > 0);

    // Re-saving writes v5 and round-trips including the zones.
    let v5 = save(&SaveData::capture(sim.world()), h.content_hash, &h.map_id).unwrap();
    assert_eq!(read_header(&v5).unwrap().0.format_version, 5);
    assert_eq!(
        load(&v5, h.content_hash).unwrap().data.zones,
        *sim.world().resource::<ZoneGrid>()
    );
}

fn busy() -> SimRunner {
    let db = content();
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let mut sim = runner(3);
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        g.add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3090.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
        g.add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(3045.0, 0.0, 2940.0),
                Vec3::new(3045.0, 0.0, 3060.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    }
    sim.tick_once();
    sim
}

#[test]
fn zones_survive_save_load_save_byte_identically_and_keep_the_checksum() {
    let mut a = busy();
    assert!(a.world().resource::<ZoneGrid>().block_count() > 0);
    let bytes = save_runner(&a, 7, "m").unwrap();
    let loaded = load(&bytes, 7).unwrap();
    assert_eq!(save(&loaded.data, 7, "m").unwrap(), bytes);

    let mut b = runner(99);
    load_into_runner(&mut b, &bytes, 7).unwrap();
    assert_eq!(
        b.checksum(),
        a.checksum(),
        "zone state is part of the checksum"
    );
    for _ in 0..40 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(b.checksum(), a.checksum());
    assert_eq!(
        a.world().resource::<ZoneGrid>(),
        b.world().resource::<ZoneGrid>()
    );
}

#[test]
fn zone_state_changes_the_checksum() {
    let mut a = busy();
    let mut empty = runner(3);
    empty.tick_once();
    assert_ne!(a.checksum(), empty.checksum());
}

#[test]
fn zone_system_follows_net_changes_and_is_deterministic() {
    let (mut a, mut b) = (busy(), busy());
    assert_eq!(a.checksum(), b.checksum());
    let blocks = a.world().resource::<ZoneGrid>().block_count();
    assert_eq!(blocks, 4, "two segments, two sides each");

    // Removing the crossing road drops its blocks on the next tick, in both runs alike.
    for sim in [&mut a, &mut b] {
        let victim = sim
            .world()
            .resource::<NetGraph>()
            .segments()
            .map(|(id, _)| id)
            .nth(1)
            .unwrap();
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .remove_segment(victim)
            .unwrap();
        sim.tick_once();
    }
    assert_eq!(a.world().resource::<ZoneGrid>().block_count(), 2);
    assert_eq!(a.checksum(), b.checksum());
}
