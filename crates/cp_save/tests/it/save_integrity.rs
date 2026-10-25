//! BUG-003: trailing bytes, header/body tick disagreement, over-long map ids and body
//! corruption. (Old formats are refused since T-275.)

use cp_core::{SimSpeed, Tick};
use cp_save::*;
use cp_sim::{SimCommand, SimRunner};

const HASH: u64 = 0x1234;

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
}

#[test]
fn header_tick_must_match_body() {
    let mut b = good();
    b[16..24].copy_from_slice(&999u64.to_le_bytes());
    // The v4 CRC covers the header, so a bare edit is a CRC error; recompute it so the
    // tick check is what fires.
    assert_eq!(load(&b, HASH).unwrap_err(), SaveError::CrcMismatch);
    let (_, body) = read_header(&b).unwrap();
    let n = b.len() - body.len();
    let crc = crc32_parts(&[&b[..n - 4], body]);
    b[n - 4..n].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(
        load(&b, HASH).unwrap_err(),
        SaveError::TickMismatch {
            header: 999,
            body: 20
        }
    );
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
                SaveError::CrcMismatch,
                "byte {i} bit {bit}"
            );
        }
    }
}

#[test]
fn new_saves_are_current_version_with_a_crc() {
    let b = good();
    let (h, body) = read_header(&b).unwrap();
    assert_eq!(h.format_version, cp_save::FORMAT_VERSION);
    assert!(h.format_version >= 2);
    let prefix = &b[..b.len() - body.len() - 4];
    assert_eq!(h.body_crc, Some(crc32_parts(&[prefix, body])));
}
