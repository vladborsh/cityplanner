//! T-052: format v4 — CRC over header + body, `map_hash` in the header.

use std::path::Path;
use std::sync::Arc;

use cp_core::{SimSpeed, Tick};
use cp_save::migrate::{NetGraphV54, SaveDataV3, encode_v3};
use cp_save::*;
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{GRID, MapMeta, Terrain};

const HASH: u64 = 0x1234;
const MAP: u64 = 0xFEED_FACE_0000_0001;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/saves")
            .join(name),
    )
    .unwrap()
}

fn runner(seed: u64, ticks: u64) -> SimRunner {
    let mut s = SimRunner::new(seed);
    s.push_command(
        SimCommand::SetSpeed {
            speed: SimSpeed::X2,
        },
        Tick(3),
    );
    for _ in 0..ticks {
        s.tick_once();
    }
    s
}

fn terrain(sea: f32) -> Arc<Terrain> {
    let meta = MapMeta {
        name: "t".into(),
        sea_level: sea,
        start_tile: (2, 2),
        external_connections: vec![],
        entry_road: None,
        entry_rail: None,
    };
    Arc::new(Terrain::from_samples(meta, &vec![1000; GRID * GRID], &vec![0; GRID * GRID]).unwrap())
}

fn good_with_map() -> Vec<u8> {
    let s = runner(4, 20);
    save_with_map(&SaveData::capture(s.world()), HASH, "meadow", Some(MAP)).unwrap()
}

#[test]
fn every_single_bit_flip_outside_the_crc_field_is_an_error() {
    let b = good_with_map();
    let (h, body) = read_header(&b).unwrap();
    assert_eq!(h.format_version, FORMAT_VERSION);
    assert_eq!(h.map_hash, Some(MAP));
    let crc_at = b.len() - body.len() - 4;
    let mut checked = 0;
    // Exhaustive over the whole file (header and body): the save is small.
    for i in 0..b.len() {
        if (crc_at..crc_at + 4).contains(&i) {
            continue;
        }
        for bit in 0..8 {
            let mut c = b.clone();
            c[i] ^= 1 << bit;
            let e = load(&c, HASH).expect_err(&format!("byte {i} bit {bit} loaded"));
            // Magic and version (bytes 0..8) are caught earlier, and a damaged map_id
            // length (32..34) can make the file look truncated before the CRC is
            // reached; every other byte is the CRC error.
            if i >= 8 && !(32..34).contains(&i) {
                assert_eq!(e, SaveError::CrcMismatch, "byte {i} bit {bit}");
            }
            checked += 1;
        }
    }
    assert_eq!(checked, (b.len() - 4) * 8);
    // Flipping the CRC field itself is also an error.
    for i in crc_at..crc_at + 4 {
        let mut c = b.clone();
        c[i] ^= 1;
        assert_eq!(load(&c, HASH).unwrap_err(), SaveError::CrcMismatch);
    }
}

#[test]
fn tampered_header_fields_are_crc_errors_not_silent_loads() {
    let b = good_with_map();
    // content_hash (8..16), tick (16..24), map_hash (24..32), map_id (34..).
    for at in [8usize, 16, 24, 34] {
        let mut c = b.clone();
        c[at] ^= 0x01;
        assert_eq!(load(&c, HASH).unwrap_err(), SaveError::CrcMismatch, "{at}");
    }
}

#[test]
fn map_hash_mismatch_warns_and_loads() {
    let b = good_with_map();
    let same = load_with_map(&b, HASH, Some(MAP)).unwrap();
    assert!(same.warnings.is_empty());
    let other = load_with_map(&b, HASH, Some(MAP + 1)).unwrap();
    assert_eq!(
        other.warnings,
        vec![LoadWarning::MapMismatch {
            saved: MAP,
            current: MAP + 1
        }]
    );
    assert_eq!(other.data, same.data);
    // Both mismatches are reported together.
    let both = load_with_map(&b, HASH + 1, Some(MAP + 1)).unwrap();
    assert_eq!(both.warnings.len(), 2);
    // No terrain on the loading side: nothing to compare.
    assert!(load_with_map(&b, HASH, None).unwrap().warnings.is_empty());
}

#[test]
fn unknown_map_hash_never_warns() {
    let s = runner(2, 5);
    let b = save(&SaveData::capture(s.world()), HASH, "m").unwrap();
    assert_eq!(read_header(&b).unwrap().0.map_hash, None);
    assert!(
        load_with_map(&b, HASH, Some(MAP))
            .unwrap()
            .warnings
            .is_empty()
    );
    // Old versions have no map hash either.
    for name in [
        "v0-basic.sav",
        "v1-basic.sav",
        "v2-basic.sav",
        "v3-basic.sav",
    ] {
        let bytes = fixture(name);
        let h = read_header(&bytes).unwrap().0;
        assert_eq!(h.map_hash, None, "{name}");
        let l = load_with_map(&bytes, h.content_hash, Some(MAP)).unwrap();
        assert!(l.warnings.is_empty(), "{name}");
    }
}

#[test]
fn runner_api_takes_the_map_hash_from_the_terrain() {
    let mut a = runner(5, 10);
    a.set_terrain(terrain(0.0));
    let bytes = save_runner(&a, HASH, "m").unwrap();
    let h = read_header(&bytes).unwrap().0;
    assert_eq!(h.map_hash, Some(terrain(0.0).map_hash()));

    // Same map: no warning. Different map: warning, state still loads.
    let mut same = SimRunner::new(0);
    same.set_terrain(terrain(0.0));
    assert!(
        load_into_runner(&mut same, &bytes, HASH)
            .unwrap()
            .1
            .is_empty()
    );

    let mut other = SimRunner::new(0);
    other.set_terrain(terrain(5.0));
    let (_, warnings) = load_into_runner(&mut other, &bytes, HASH).unwrap();
    assert_eq!(
        warnings,
        vec![LoadWarning::MapMismatch {
            saved: terrain(0.0).map_hash(),
            current: terrain(5.0).map_hash()
        }]
    );
    assert_eq!(other.tick(), a.tick());

    // Without a terrain on the loading runner: no warning.
    let mut bare = SimRunner::new(0);
    assert!(
        load_into_runner(&mut bare, &bytes, HASH)
            .unwrap()
            .1
            .is_empty()
    );
}

#[test]
fn save_load_save_is_byte_identical_with_a_map_hash() {
    let b = good_with_map();
    let l = load(&b, HASH).unwrap();
    let again = save_with_map(&l.data, HASH, &l.header.map_id, l.header.map_hash).unwrap();
    assert_eq!(again, b);
}

#[test]
fn v3_fixture_loads_and_the_frozen_writer_reproduces_it() {
    let bytes = fixture("v3-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!((h.format_version, h.map_hash, h.tick), (3, None, 900));
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert_eq!(l.data.net.segment_count(), 2);
    assert_eq!(l.data.speed, SimSpeed::X2);
    assert_eq!(l.data.queue.len(), 1);
    assert_eq!(l.data.log.len(), 1);

    let v3 = SaveDataV3 {
        tick: l.data.tick,
        rng: l.data.rng.clone(),
        speed: l.data.speed,
        queue: l.data.queue.clone(),
        log: l.data.log.clone(),
        tiles: l.data.tiles,
        net: NetGraphV54::from(&l.data.net),
    };
    assert_eq!(encode_v3(&v3, h.content_hash, &h.map_id).unwrap(), bytes);

    // Re-saving writes v4, and is a fixed point.
    let v4 = save(&l.data, h.content_hash, &h.map_id).unwrap();
    assert_eq!(read_header(&v4).unwrap().0.format_version, FORMAT_VERSION);
    assert_eq!(load(&v4, h.content_hash).unwrap().data, l.data);
}

#[test]
fn every_committed_fixture_loads_through_the_chain() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/saves");
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".sav"))
        .collect();
    names.sort();
    assert!(names.len() >= 4, "{names:?}");
    for name in names {
        let bytes = fixture(&name);
        let h = read_header(&bytes).unwrap().0;
        let l = load(&bytes, h.content_hash).unwrap_or_else(|e| panic!("{name}: {e}"));
        let resaved = save(&l.data, h.content_hash, &h.map_id).unwrap();
        assert_eq!(
            load(&resaved, h.content_hash).unwrap().data,
            l.data,
            "{name}"
        );
    }
}
