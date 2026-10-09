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
    let a = save(&l1.data, h.content_hash, &h.map_id);
    let l2 = load(&a, h.content_hash).unwrap();
    assert_eq!(l1.data, l2.data);
    assert_eq!(
        a,
        save(&l2.data, h.content_hash, &h.map_id),
        "v1 re-save is a fixed point"
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
        let a = save_runner(&s, HASH, "flatlands");
        let l = load(&a, HASH).unwrap();
        assert_eq!(l.header.tick, big.unwrap_or(ticks));
        assert_eq!(l.data.tick.0, l.header.tick);
        assert_eq!(a, save(&l.data, HASH, "flatlands"));
        let mut r = SimRunner::new(0);
        load_into_runner(&mut r, &a, HASH).unwrap();
        assert_eq!(r.checksum(), s.checksum());
    }
}

#[test]
fn map_id_edge_values_round_trip() {
    for id in ["", "a", "река-долина", &"x".repeat(65_535)] {
        let a = save_runner(&runner(1, 2), HASH, id);
        assert_eq!(load(&a, HASH).unwrap().header.map_id, id);
    }
}

#[test]
fn every_truncation_is_rejected_without_panicking() {
    for good in [save_runner(&runner(3, 40), HASH, "m"), v0()] {
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
    for good in [save_runner(&runner(3, 40), HASH, "m"), v0()] {
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
    let good = save_runner(&runner(1, 1), HASH, "m");
    let (_, body) = read_header(&good).unwrap();
    let mut b = good[..good.len() - body.len()].to_vec();
    b.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 1, 2, 3]);
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
    let mut bad = save_runner(&runner(9, 10), HASH, "m");
    let n = bad.len();
    bad.truncate(n - 3);
    assert!(load_into_runner(&mut r, &bad, HASH).is_err());
    assert_eq!(r.checksum(), before);
    assert_eq!(r.tick(), Tick(30));
}

#[test]
fn hash_mismatch_warns_exactly_once_and_data_is_intact() {
    let s = runner(2, 15);
    let a = save_runner(&s, HASH, "m");
    let l = load(&a, !HASH).unwrap();
    assert_eq!(l.warnings.len(), 1);
    assert_eq!(l.data, SaveData::capture(s.world()));
}
