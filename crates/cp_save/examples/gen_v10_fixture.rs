//! About `fixtures/saves/v10-basic.sav`, the committed version-10 fixture (the last format
//! without agents; a small town with residents, jobs and trips in flight).
//!
//! The committed file was produced **once** by the real v10 writer (`save_with_map` at
//! format 10, before the v11 bump) from a ~5-day run of a three-street town and is **never
//! edited or regenerated** (AGENTS.md). The sim has moved on since, so this tool cannot
//! rebuild it from scratch: it loads the committed file, re-encodes it with the frozen v10
//! writer (`encode_v10`) and writes the result to the path given as the first argument, so
//! the writer can be checked against the original byte for byte:
//! `cargo run -p cp_save --example gen_v10_fixture -- /tmp/v10.sav && cmp /tmp/v10.sav fixtures/saves/v10-basic.sav`.

use cp_save::migrate::{SaveDataV10, encode_v10};
use cp_save::{load, read_header};

fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("usage: gen_v10_fixture <output path>");
    let bytes = std::fs::read("fixtures/saves/v10-basic.sav").expect("committed fixture");
    let (h, _) = read_header(&bytes).expect("header");
    let d = load(&bytes, h.content_hash).expect("load").data;
    let v10 = SaveDataV10 {
        tick: d.tick,
        rng: d.rng,
        speed: d.speed,
        queue: d.queue,
        log: d.log,
        tiles: d.tiles,
        net: d.net,
        zones: d.zones.to_v34(),
        buildings: cp_save::migrate::BuildingGridV12::from(&d.buildings),
        citizens: (&d.citizens).into(),
        paths: d.paths,
        utilities: d.utilities,
        trips: d.trips,
    };
    let again = encode_v10(&v10, h.content_hash, &h.map_id, h.map_hash).unwrap();
    std::fs::write(&out, &again).unwrap();
    println!("wrote {} bytes to {out}", again.len());
}
