//! Acceptance tests for the `m0-data` save flows: only the public API, plus the committed
//! v0 fixture. Covers what the unit tests do not: stability of re-saves across a
//! migration, hostile/corrupt input, and header edge values.

use cp_core::{SimSpeed, Tick};
use cp_save::*;
use cp_sim::{SimCommand, SimRunner};
use std::panic::catch_unwind;
use std::path::Path;

const HASH: u64 = 0x1234_5678_9ABC_DEF0;

fn runner(seed: u64, ticks: u64) -> SimRunner {
    let mut s = SimRunner::new(seed);
    s.push_command(
        SimCommand::SetSpeed {
            speed: SimSpeed::X3,
        },
        Tick(5),
    );
    s.push_command(
        SimCommand::SetSpeed {
            speed: SimSpeed::X2,
        },
        Tick(1_000_000),
    );
    for _ in 0..ticks {
        s.tick_once();
    }
    s
}

fn v0() -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/saves/v0-basic.sav"))
        .unwrap()
}

#[test]
fn migrated_v0_fixture_resaves_stably_and_resumes() {
    let old = v0();
    let (h, _) = read_header(&old).unwrap();
    assert_eq!(h.format_version, 0);
    let l1 = load(&old, h.content_hash).unwrap();
    let a = save(&l1.data, h.content_hash, &h.map_id).unwrap();
    let l2 = load(&a, h.content_hash).unwrap();
    assert_eq!(l1.data, l2.data);
    assert_eq!(
        a,
        save(&l2.data, h.content_hash, &h.map_id).unwrap(),
        "re-save is a fixed point"
    );
    // Migrated state resumes like a continuous run: load twice, step both, same checksum.
    let (mut x, mut y) = (SimRunner::new(0), SimRunner::new(1));
    l1.data.clone().restore(x.world_mut());
    l2.data.clone().restore(y.world_mut());
    for _ in 0..600 {
        x.tick_once();
        y.tick_once();
    }
    assert_eq!(x.checksum(), y.checksum());
    assert_eq!(x.tick(), Tick(900));
}

#[test]
fn save_at_tick_zero_and_large_tick_round_trip() {
    for (ticks, big) in [(0u64, None), (1, None), (0, Some(u64::MAX / 2))] {
        let mut s = runner(11, ticks);
        if let Some(t) = big {
            s.world_mut().insert_resource(Tick(t));
        }
        let a = save_runner(&s, HASH, "flatlands").unwrap();
        let l = load(&a, HASH).unwrap();
        assert_eq!(l.header.tick, big.unwrap_or(ticks));
        assert_eq!(l.data.tick.0, l.header.tick);
        assert_eq!(a, save(&l.data, HASH, "flatlands").unwrap());
        let mut r = SimRunner::new(0);
        load_into_runner(&mut r, &a, HASH).unwrap();
        assert_eq!(r.checksum(), s.checksum());
    }
}

#[test]
fn map_id_edge_values_round_trip() {
    for id in ["", "a", "река-долина", &"x".repeat(65_535)] {
        let a = save_runner(&runner(1, 2), HASH, id).unwrap();
        assert_eq!(load(&a, HASH).unwrap().header.map_id, id);
    }
}

#[test]
fn every_truncation_is_rejected_without_panicking() {
    for good in [save_runner(&runner(3, 40), HASH, "m").unwrap(), v0()] {
        for n in 0..good.len() {
            let r = catch_unwind(|| load(&good[..n], HASH));
            assert!(
                matches!(r, Ok(Err(_))),
                "prefix {n}/{} must be a clean Err",
                good.len()
            );
        }
    }
}

#[test]
fn corrupt_bytes_never_panic() {
    for good in [save_runner(&runner(3, 40), HASH, "m").unwrap(), v0()] {
        for i in 0..good.len() {
            for mask in [0x01u8, 0x80, 0xFF] {
                let mut b = good.clone();
                b[i] ^= mask;
                assert!(
                    catch_unwind(|| drop(load(&b, HASH))).is_ok(),
                    "byte {i} ^ {mask:#x}"
                );
            }
        }
    }
}

#[test]
fn garbage_and_foreign_files_are_rejected() {
    assert_eq!(load(&[], HASH).unwrap_err(), SaveError::Truncated);
    assert_eq!(
        load(b"PK\x03\x04 not a save at all", HASH).unwrap_err(),
        SaveError::BadMagic
    );
    // Valid header, body that is not zstd.
    let good = save_runner(&runner(1, 1), HASH, "m").unwrap();
    let (_, body) = read_header(&good).unwrap();
    let mut b = good[..good.len() - body.len()].to_vec();
    let garbage = [0xDE, 0xAD, 0xBE, 0xEF, 1, 2, 3];
    b.extend_from_slice(&garbage);
    // v2 headers end with the body CRC; make it match so the zstd error path is reached
    // (changed from the original test, which predates the CRC).
    let n = good.len() - body.len();
    // (v4+: the CRC covers the header too.)
    let crc = crc32_parts(&[&b[..n - 4], &garbage]);
    b[n - 4..n].copy_from_slice(&crc.to_le_bytes());
    assert!(matches!(
        load(&b, HASH).unwrap_err(),
        SaveError::Compression(_)
    ));
    // Header claims a longer map_id than the file holds.
    let mut b = good.clone();
    // (v4 layout: the map_id length follows the 8-byte map hash, at offset 32.)
    b[32..34].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(load(&b, HASH).unwrap_err(), SaveError::Truncated);
    // Newer and far-future versions are refused with the version in the error.
    for v in [FORMAT_VERSION + 1, u32::MAX] {
        let mut b = good.clone();
        b[4..8].copy_from_slice(&v.to_le_bytes());
        assert_eq!(
            load(&b, HASH).unwrap_err(),
            SaveError::UnsupportedVersion(v)
        );
    }
    // Invalid UTF-8 map_id (with a matching CRC, so the decode error is what fires; without
    // it the CRC mismatch is reported first).
    let mut b = good.clone();
    b[34] = 0xFF;
    let n = good.len() - body.len();
    let crc = crc32_parts(&[&b[..n - 4], body]);
    b[n - 4..n].copy_from_slice(&crc.to_le_bytes());
    assert!(matches!(load(&b, HASH).unwrap_err(), SaveError::Decode(_)));
    let mut b = good.clone();
    b[34] = 0xFF;
    assert_eq!(load(&b, HASH).unwrap_err(), SaveError::CrcMismatch);
}

#[test]
fn failed_load_leaves_the_runner_untouched() {
    let mut r = runner(5, 30);
    let before = r.checksum();
    let mut bad = save_runner(&runner(9, 10), HASH, "m").unwrap();
    let n = bad.len();
    bad.truncate(n - 3);
    assert!(load_into_runner(&mut r, &bad, HASH).is_err());
    assert_eq!(r.checksum(), before);
    assert_eq!(r.tick(), Tick(30));
}

#[test]
fn hash_mismatch_warns_exactly_once_and_data_is_intact() {
    let s = runner(2, 15);
    let a = save_runner(&s, HASH, "m").unwrap();
    let l = load(&a, !HASH).unwrap();
    assert_eq!(l.warnings.len(), 1);
    assert_eq!(l.data, SaveData::capture(s.world()));
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/saves")
            .join(name),
    )
    .unwrap()
}

#[test]
fn committed_fixtures_of_every_old_version_load_and_resume() {
    for (name, version) in [("v0-basic.sav", 0), ("v1-basic.sav", 1)] {
        let bytes = fixture(name);
        let (h, _) = read_header(&bytes).unwrap();
        assert_eq!(h.format_version, version, "{name}");
        let l = load(&bytes, h.content_hash).unwrap();
        assert!(l.warnings.is_empty());
        let mut r = SimRunner::new(0);
        l.data.clone().restore(r.world_mut());
        let t = r.tick();
        r.tick_once();
        assert_eq!(r.tick().0, t.0 + 1);
        // Migrated re-save is current-version and a fixed point.
        let a = save(&l.data, h.content_hash, &h.map_id).unwrap();
        assert_eq!(read_header(&a).unwrap().0.format_version, FORMAT_VERSION);
        assert_eq!(
            a,
            save(
                &load(&a, h.content_hash).unwrap().data,
                h.content_hash,
                &h.map_id
            )
            .unwrap()
        );
    }
}

#[test]
fn trailing_bytes_are_rejected_in_every_version() {
    let current = save_runner(&runner(1, 5), HASH, "m").unwrap();
    for mut b in [current, fixture("v0-basic.sav"), fixture("v1-basic.sav")] {
        b.extend_from_slice(b"GARBAGE");
        assert!(load(&b, HASH).is_err());
    }
}

#[test]
fn header_tick_must_agree_with_the_body_in_every_version() {
    let current = save_runner(&runner(1, 5), HASH, "m").unwrap();
    for good in [current, fixture("v0-basic.sav"), fixture("v1-basic.sav")] {
        let mut b = good.clone();
        let real = read_header(&good).unwrap().0.tick;
        b[16..24].copy_from_slice(&(real + 999).to_le_bytes());
        let (h, body) = read_header(&good).unwrap();
        if h.format_version >= 4 {
            // The CRC covers the header: recompute it so the tick check is what fires.
            let n = good.len() - body.len();
            let crc = crc32_parts(&[&b[..n - 4], body]);
            b[n - 4..n].copy_from_slice(&crc.to_le_bytes());
        }
        assert!(
            matches!(load(&b, HASH), Err(SaveError::TickMismatch { .. })),
            "v{}",
            read_header(&good).unwrap().0.format_version
        );
    }
}

#[test]
fn oversized_map_id_is_an_error_not_a_panic() {
    let r = catch_unwind(|| save_runner(&runner(1, 1), HASH, &"x".repeat(65_536)));
    assert!(matches!(r, Ok(Err(SaveError::MapIdTooLong(65_536)))));
}

#[test]
fn every_single_bit_flip_in_a_current_body_is_detected() {
    let good = save_runner(&runner(3, 40), HASH, "m").unwrap();
    let (_, body) = read_header(&good).unwrap();
    for i in good.len() - body.len()..good.len() {
        for bit in 0..8 {
            let mut b = good.clone();
            b[i] ^= 1 << bit;
            assert_eq!(
                load(&b, HASH).unwrap_err(),
                SaveError::CrcMismatch,
                "byte {i} bit {bit}"
            );
        }
    }
}

#[test]
fn checksum_covers_tiles_speed_and_pending_queue() {
    use cp_terrain::TileOwnership;
    let base = || runner(7, 10);
    let c0 = base().checksum();
    assert_eq!(c0, base().checksum());

    let mut s = base();
    s.world_mut().insert_resource(TileOwnership::only((1, 1)));
    assert_ne!(c0, s.checksum(), "tiles");

    let mut s = base();
    s.world_mut().insert_resource(SimSpeed::X1);
    assert_ne!(c0, s.checksum(), "speed");

    let mut s = base();
    s.push_command(
        SimCommand::SetSpeed {
            speed: SimSpeed::X1,
        },
        Tick(5_000_000),
    );
    assert_ne!(c0, s.checksum(), "pending queue");

    // A save/load round trip preserves the checksum, queue included.
    let mut r = SimRunner::new(0);
    load_into_runner(&mut r, &save_runner(&base(), HASH, "m").unwrap(), HASH).unwrap();
    assert_eq!(c0, r.checksum());
}

#[test]
fn set_terrain_after_loading_a_fixture_keeps_the_saved_ownership() {
    use cp_terrain::{GRID, MapMeta, Terrain, TileOwnership};
    use std::sync::Arc;
    let n = GRID * GRID;
    let meta = MapMeta {
        name: "t".into(),
        sea_level: 1.0,
        start_tile: (0, 0),
        external_connections: vec![],
    };
    let terrain = Arc::new(Terrain::from_samples(meta, &vec![1000; n], &vec![0; n]).unwrap());
    for name in ["v0-basic.sav", "v1-basic.sav", "v2-basic.sav"] {
        let bytes = fixture(name);
        let (h, _) = read_header(&bytes).unwrap();
        let mut r = SimRunner::new(0);
        load_into_runner(&mut r, &bytes, h.content_hash).unwrap();
        // The fixtures store the 3x3 centre, whatever the map's start tile says.
        assert_eq!(r.view().tiles, TileOwnership::m1_around((2, 2)), "{name}");
        r.set_terrain(Arc::clone(&terrain));
        assert_eq!(r.view().tiles, TileOwnership::m1_around((2, 2)), "{name}");
    }
}

/// A runner with `n` SetSpeed commands queued at ticks 0..n, run for `ticks` ticks.
fn used_runner(seed: u64, n: u64, ticks: u64) -> SimRunner {
    let mut s = SimRunner::new(seed);
    for i in 0..n {
        s.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::X2,
            },
            Tick(i),
        );
    }
    for _ in 0..ticks {
        s.tick_once();
    }
    s
}

/// Loading into a runner that already ran (and already cached its log hash) must give
/// the checksum of a continuous run from the saved state (BUG-006).
#[test]
fn load_into_a_used_runner_matches_a_continuous_run() {
    let mut saved = SimRunner::new(4);
    for t in [2, 40] {
        saved.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::X3,
            },
            Tick(t),
        );
    }
    for _ in 0..10 {
        saved.tick_once();
    }
    let bytes = save_runner(&saved, HASH, "m").unwrap();
    for _ in 0..50 {
        saved.tick_once();
    }
    let continuous = saved.checksum();

    // Old log longer than the saved one (used to panic), shorter, and equal length.
    // The 300 ticks cross the periodic checksum at tick 255 as well.
    for (n, ticks) in [(5, 300), (1, 20), (2, 20)] {
        let mut used = used_runner(9, n, ticks);
        used.checksum();
        load_into_runner(&mut used, &bytes, HASH).unwrap();
        for _ in 0..50 {
            used.tick_once();
        }
        assert_eq!(used.checksum(), continuous, "n={n} ticks={ticks}");
    }
}

#[test]
fn loading_twice_in_a_row_into_the_same_runner_is_stable() {
    let mut saved = used_runner(4, 3, 10);
    let bytes = save_runner(&saved, HASH, "m").unwrap();
    for _ in 0..30 {
        saved.tick_once();
    }
    let continuous = saved.checksum();

    let mut used = used_runner(9, 8, 300);
    for _ in 0..2 {
        used.checksum();
        load_into_runner(&mut used, &bytes, HASH).unwrap();
        for _ in 0..30 {
            used.tick_once();
        }
        assert_eq!(used.checksum(), continuous);
    }
}

// ---- m0-data-2: save v4 (CRC over header+body, map_hash) and tile ownership ----

use cp_sim::SimTerrain;
use cp_terrain::{GRID, MapMeta, Terrain, TileOwnership};
use std::sync::Arc;

fn map(name: &str) -> Arc<Terrain> {
    Arc::new(
        Terrain::load(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/maps")
                .join(name),
        )
        .unwrap(),
    )
}

fn flat_at(start: (u8, u8), tag: u16) -> Arc<Terrain> {
    let meta = MapMeta {
        name: format!("flat{tag}"),
        sea_level: 0.0,
        start_tile: start,
        external_connections: vec![],
    };
    Arc::new(Terrain::from_samples(meta, &vec![tag; GRID * GRID], &vec![0; GRID * GRID]).unwrap())
}

fn on_map(t: Arc<Terrain>) -> SimRunner {
    let mut s = runner(3, 30);
    s.set_terrain(t);
    s
}

#[test]
fn every_fixture_v0_to_v3_loads_keeps_its_ownership_and_resaves_as_v4() {
    let tm = map("flatlands");
    for v in 0..=3u32 {
        let name = format!("v{v}-basic.sav");
        let bytes = fixture(&name);
        let (h, _) = read_header(&bytes).unwrap();
        assert_eq!(h.format_version, v, "{name}");
        // With and without a terrain present, an old save never warns about the map.
        let plain = load(&bytes, h.content_hash).unwrap();
        let with_map = load_with_map(&bytes, h.content_hash, Some(tm.map_hash())).unwrap();
        assert_eq!(plain.data, with_map.data);
        assert!(
            with_map
                .warnings
                .iter()
                .all(|w| !matches!(w, LoadWarning::MapMismatch { .. })),
            "{name}"
        );
        // Ownership stored in the file is what the sim gets, even after set_terrain.
        let stored = plain.data.tiles;
        let mut r = SimRunner::new(0);
        r.set_terrain(flat_at((0, 0), 10));
        load_into_runner(&mut r, &bytes, h.content_hash).unwrap();
        assert_eq!(*r.world().resource::<TileOwnership>(), stored, "{name}");
        r.set_terrain(flat_at((4, 4), 11));
        assert_eq!(
            *r.world().resource::<TileOwnership>(),
            stored,
            "{name}: set_terrain after load"
        );
        // Re-save is the current version (v5 since T-026) and a fixed point.
        let a = save(&plain.data, h.content_hash, &h.map_id).unwrap();
        assert_eq!(read_header(&a).unwrap().0.format_version, FORMAT_VERSION);
        assert_eq!(FORMAT_VERSION, 5); // bumped with the v5 (zones) format
        let b = save(
            &load(&a, h.content_hash).unwrap().data,
            h.content_hash,
            &h.map_id,
        )
        .unwrap();
        assert_eq!(a, b, "{name}");
    }
}

#[test]
fn any_bit_flip_anywhere_in_a_v4_file_is_an_error() {
    let s = on_map(map("river_valley"));
    let good = save_runner(&s, HASH, "river_valley").unwrap();
    assert!(read_header(&good).unwrap().0.map_hash.is_some());
    assert!(load(&good, HASH).is_ok());
    for i in 0..good.len() {
        for bit in 0..8 {
            let mut b = good.clone();
            b[i] ^= 1 << bit;
            let r = std::panic::catch_unwind(|| load(&b, HASH));
            assert!(
                matches!(r, Ok(Err(_))),
                "byte {i} bit {bit} was not rejected: {r:?}"
            );
        }
    }
}

#[test]
fn a_different_map_warns_but_loads_and_the_same_map_is_silent() {
    let a = on_map(map("flatlands"));
    let bytes = save_runner(&a, HASH, "flatlands").unwrap();

    let mut same = SimRunner::new(0);
    same.set_terrain(map("flatlands"));
    let (_, w) = load_into_runner(&mut same, &bytes, HASH).unwrap();
    assert!(w.is_empty(), "{w:?}");

    let mut other = SimRunner::new(0);
    other.set_terrain(map("river_valley"));
    let (h, w) = load_into_runner(&mut other, &bytes, HASH).unwrap();
    assert_eq!(h.map_id, "flatlands");
    assert!(
        matches!(w.as_slice(), [LoadWarning::MapMismatch { .. }]),
        "{w:?}"
    );
    assert_eq!(other.tick(), a.tick(), "state restored despite the warning");

    // No terrain on the loading side: nothing to compare.
    let mut bare = SimRunner::new(0);
    let (_, w) = load_into_runner(&mut bare, &bytes, HASH).unwrap();
    assert!(w.is_empty());

    // Two maps differing only in one height sample hash differently.
    assert_ne!(
        flat_at((2, 2), 100).map_hash(),
        flat_at((2, 2), 101).map_hash()
    );
    assert_ne!(
        flat_at((2, 2), 100).map_hash(),
        flat_at((2, 3), 100).map_hash()
    );
    // A content-hash and a map-hash mismatch are reported separately.
    let (_, w) = load_into_runner(&mut other, &bytes, !HASH).unwrap();
    assert_eq!(w.len(), 2);
}

#[test]
fn map_hash_zero_and_none_in_a_v4_header() {
    let s = runner(3, 5);
    for mh in [None, Some(0u64)] {
        let bytes = save_with_map(&SaveData::capture(s.world()), HASH, "m", mh).unwrap();
        let l = load_with_map(&bytes, HASH, Some(12345)).unwrap();
        println!(
            "map_hash {mh:?} -> header {:?}, warnings {:?}",
            l.header.map_hash, l.warnings
        );
        assert!(
            l.warnings
                .iter()
                .all(|w| !matches!(w, LoadWarning::MapMismatch { .. })),
            "unknown map hash ({mh:?}) must not warn: {:?}",
            l.warnings
        );
    }
}

#[test]
fn set_terrain_and_load_orderings_on_every_start_tile() {
    for (start, open) in [
        ((0, 0), 4),
        ((4, 0), 4),
        ((0, 4), 4),
        ((4, 4), 4),
        ((2, 0), 6),
        ((0, 2), 6),
        ((4, 2), 6),
        ((2, 4), 6),
        ((2, 2), 9),
    ] {
        let mut r = SimRunner::new(1);
        assert_eq!(
            *r.world().resource::<TileOwnership>(),
            TileOwnership::none()
        );
        r.set_terrain(flat_at(start, 5));
        let t = *r.world().resource::<TileOwnership>();
        assert_eq!(t.open_count(), open, "{start:?}");
        assert!(t.is_open(start.0 as usize, start.1 as usize));
        // Idempotent and not re-centred by a second, different map.
        r.set_terrain(flat_at(start, 5));
        assert_eq!(*r.world().resource::<TileOwnership>(), t);
        r.set_terrain(flat_at((2, 2), 6));
        assert_eq!(*r.world().resource::<TileOwnership>(), t);
    }
    // Load into a used runner that already has a map: the save wins, state matches a
    // continuous run (BUG-006 re-check, now with terrain).
    let mut saved = on_map(map("flatlands"));
    let bytes = save_runner(&saved, HASH, "flatlands").unwrap();
    let mut used = SimRunner::new(9);
    used.set_terrain(map("river_valley"));
    for _ in 0..300 {
        used.tick_once();
    }
    let _ = used.checksum();
    load_into_runner(&mut used, &bytes, HASH).unwrap();
    for _ in 0..40 {
        used.tick_once();
        saved.tick_once();
    }
    assert_eq!(used.checksum(), saved.checksum());
    assert!(used.world().get_resource::<SimTerrain>().is_some());
}

#[test]
fn saving_while_paused_with_a_pending_unpause_resumes_identically() {
    use std::time::Duration;
    let f = Duration::from_millis(50);
    let mut live = runner(21, 0);
    for _ in 0..20 {
        live.step(f);
    }
    live.push_command(
        SimCommand::SetSpeed {
            speed: SimSpeed::Pause,
        },
        live.tick(),
    );
    live.step(f);
    live.step(f);
    assert_eq!(live.speed(), SimSpeed::Pause);
    // Unpause is queued but not yet stepped when the game is saved.
    live.push_command(
        SimCommand::SetSpeed {
            speed: SimSpeed::X2,
        },
        live.tick(),
    );
    let bytes = save_runner(&live, HASH, "m").unwrap();

    // Load into a used runner (longer log than the save), then keep playing both.
    let mut used = used_runner(9, 8, 300);
    used.checksum();
    load_into_runner(&mut used, &bytes, HASH).unwrap();
    for _ in 0..50 {
        assert_eq!(used.step(f), live.step(f));
    }
    assert_eq!(used.speed(), SimSpeed::X2);
    assert_eq!(used.tick(), live.tick());
    assert_eq!(used.checksum(), live.checksum());
}
