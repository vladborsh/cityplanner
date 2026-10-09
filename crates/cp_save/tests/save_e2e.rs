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
    b[n - 4..n].copy_from_slice(&crc32(&garbage).to_le_bytes());
    assert!(matches!(
        load(&b, HASH).unwrap_err(),
        SaveError::Compression(_)
    ));
    // Header claims a longer map_id than the file holds.
    let mut b = good.clone();
    b[24..26].copy_from_slice(&u16::MAX.to_le_bytes());
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
    // Invalid UTF-8 map_id.
    let mut b = good.clone();
    b[26] = 0xFF;
    assert!(matches!(load(&b, HASH).unwrap_err(), SaveError::Decode(_)));
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
                SaveError::Corrupt,
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
    s.world_mut().insert_resource(TileOwnership::none());
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
