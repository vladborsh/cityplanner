//! About `fixtures/saves/v9-basic.sav`, the committed version-9 fixture (the last format
//! without trips; a small road network with its zone blocks and a pipe and a power line).
//!
//! The committed file was produced **once** by the real v9 writer (`save_with_map` at format
//! 9, before the v10 bump) and is **never edited or regenerated** (AGENTS.md). This tool
//! loads it, re-encodes it with the frozen v9 writer (`encode_v9`) and writes the result to
//! the path given as the first argument, so the writer can be checked against the original
//! byte for byte:
//! `cargo run -p cp_save --example gen_v9_fixture -- /tmp/v9.sav && cmp /tmp/v9.sav fixtures/saves/v9-basic.sav`.

use cp_save::migrate::{SaveDataV9, encode_v9};
use cp_save::{load, read_header};

fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("usage: gen_v9_fixture <output path>");
    let bytes = std::fs::read("fixtures/saves/v9-basic.sav").expect("committed fixture");
    let (h, _) = read_header(&bytes).expect("header");
    let d = load(&bytes, h.content_hash).expect("load").data;
    let v9 = SaveDataV9 {
        tick: d.tick,
        rng: d.rng,
        speed: d.speed,
        queue: d.queue,
        log: d.log,
        tiles: d.tiles,
        net: cp_save::migrate::NetGraphV54::from(&d.net),
        zones: d.zones.to_v34(),
        buildings: cp_save::migrate::BuildingGridV12::from(&d.buildings),
        citizens: (&d.citizens).into(),
        paths: d.paths,
        utilities: d.utilities,
    };
    let again = encode_v9(&v9, h.content_hash, &h.map_id, h.map_hash).unwrap();
    std::fs::write(&out, &again).unwrap();
    println!("wrote {} bytes to {out}", again.len());
}
