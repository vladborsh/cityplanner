//! BUG-003: trailing bytes, header/body tick disagreement, over-long map ids and body
//! corruption. Also the committed v1 fixture (the last format without a body CRC).

use cp_core::{SimSpeed, Tick};
use cp_save::*;
use cp_sim::{SimCommand, SimRunner};
use std::path::Path;

const HASH: u64 = 0x1234;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/saves")
            .join(name),
    )
    .unwrap()
}

fn good() -> Vec<u8> {
    let mut s = SimRunner::new(4);
    s.push_command(
        SimCommand::SetSpeed {
            speed: SimSpeed::X2,
        },
        Tick(3),
    );
    for _ in 0..20 {
        s.tick_once();
    }
    save_runner(&s, HASH, "m").unwrap()
}

#[test]
fn crc32_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
}

#[test]
fn trailing_bytes_are_rejected() {
    let mut b = good();
    b.extend_from_slice(b"GARBAGE");
    assert!(load(&b, HASH).is_err());
    // Without a CRC (v0/v1) the frame-end check catches it.
    for name in ["v0-basic.sav", "v1-basic.sav"] {
        let mut b = fixture(name);
        let (h, _) = read_header(&b).unwrap();
        assert!(load(&b, h.content_hash).is_ok());
        b.extend_from_slice(b"GARBAGE");
        assert_eq!(
            load(&b, h.content_hash).unwrap_err(),
            SaveError::TrailingBytes,
            "{name}"
        );
    }
}

#[test]
fn header_tick_must_match_body() {
    let mut b = good();
    b[16..24].copy_from_slice(&999u64.to_le_bytes());
    assert_eq!(
        load(&b, HASH).unwrap_err(),
        SaveError::TickMismatch {
            header: 999,
            body: 20
        }
    );
    for name in ["v0-basic.sav", "v1-basic.sav"] {
        let mut b = fixture(name);
        let (h, _) = read_header(&b).unwrap();
        b[16..24].copy_from_slice(&1u64.to_le_bytes());
        assert!(matches!(
            load(&b, h.content_hash).unwrap_err(),
            SaveError::TickMismatch { header: 1, .. }
        ));
    }
}

#[test]
fn over_long_map_id_is_an_error_not_a_panic() {
    let s = SimRunner::new(1);
    assert_eq!(
        save_runner(&s, HASH, &"a".repeat(65_536)).unwrap_err(),
        SaveError::MapIdTooLong(65_536)
    );
    assert!(save_runner(&s, HASH, &"a".repeat(65_535)).is_ok());
}

#[test]
fn every_single_bit_flip_in_the_body_is_detected() {
    let b = good();
    let (_, body) = read_header(&b).unwrap();
    let start = b.len() - body.len();
    for i in start..b.len() {
        for bit in 0..8 {
            let mut c = b.clone();
            c[i] ^= 1 << bit;
            assert_eq!(
                load(&c, HASH).unwrap_err(),
                SaveError::Corrupt,
                "byte {i} bit {bit}"
            );
        }
    }
}

#[test]
fn new_saves_are_version_2_with_a_crc() {
    let b = good();
    let (h, body) = read_header(&b).unwrap();
    assert_eq!(h.format_version, 2);
    assert_eq!(h.body_crc, Some(crc32(body)));
}

#[test]
fn v1_fixture_loads_without_crc_and_resaves_as_v2() {
    let bytes = fixture("v1-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!((h.format_version, h.body_crc, h.tick), (1, None, 500));
    assert_eq!(h.map_id, "river_valley");

    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert_eq!(l.data.tick, Tick(500));
    assert_eq!(l.data.speed, SimSpeed::X3);
    assert_eq!(l.data.queue.len(), 1);
    assert_eq!(l.data.log.len(), 1);

    let v2 = save(&l.data, h.content_hash, &h.map_id).unwrap();
    assert_eq!(read_header(&v2).unwrap().0.format_version, 2);
    assert_eq!(load(&v2, h.content_hash).unwrap().data, l.data);
    assert_eq!(
        v2,
        save(
            &load(&v2, h.content_hash).unwrap().data,
            h.content_hash,
            &h.map_id
        )
        .unwrap()
    );
}
