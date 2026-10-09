//! Generates `fixtures/saves/v2-basic.sav`, the committed version-2 fixture (the last
//! format without the road network).
//!
//! The committed file was produced by the real v2 writer (`save` at format 2, before the
//! v3 bump) with this data; `encode_v2` is the frozen equivalent kept for regeneration and
//! a test asserts they agree byte for byte. Run from the repo root:
//! `cargo run -p cp_save --example gen_v2_fixture`. The committed file is **never edited
//! or regenerated** afterwards (AGENTS.md).

use cp_core::{CommandId, CommandLog, CommandQueue, SimRng, SimSpeed, Tick};
use cp_save::migrate::{SaveDataV2, encode_v2};
use cp_sim::SimCommand;
use cp_terrain::TileOwnership;

fn main() {
    let mut rng = SimRng::seed_from_u64(21);
    for _ in 0..8 {
        rng.next_u64();
    }
    let mut queue = CommandQueue::new();
    queue.push(
        SimCommand::SetSpeed {
            speed: SimSpeed::X1,
        },
        Tick(800),
    );
    let mut log = CommandLog::new();
    log.push(
        Tick(30),
        CommandId(0),
        SimCommand::SetSpeed {
            speed: SimSpeed::X3,
        },
    );
    let data = SaveDataV2 {
        tick: Tick(700),
        rng,
        speed: SimSpeed::X3,
        queue,
        log,
        tiles: TileOwnership::m1_default(),
    };
    let bytes = encode_v2(&data, 0xC17F_0000_0000_0003, "flatlands").unwrap();
    std::fs::create_dir_all("fixtures/saves").unwrap();
    std::fs::write("fixtures/saves/v2-basic.sav", &bytes).unwrap();
    println!("wrote {} bytes", bytes.len());
}
