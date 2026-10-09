//! Generates `fixtures/saves/v1-basic.sav`, the committed version-1 fixture (the last
//! format without a body CRC).
//!
//! Run once from the repo root: `cargo run -p cp_save --example gen_v1_fixture`.
//! The committed file is **never edited or regenerated** afterwards (AGENTS.md).

use cp_core::{CommandId, CommandLog, CommandQueue, SimRng, SimSpeed, Tick};
use cp_save::migrate::SaveDataV2;
use cp_save::migrate::encode_v1;
use cp_sim::SimCommand;
use cp_terrain::TileOwnership;

const SEED: u64 = 11;
const TICK: u64 = 500;
const HASH: u64 = 0xC17F_0000_0000_0002;

fn main() {
    let mut rng = SimRng::seed_from_u64(SEED);
    for _ in 0..20 {
        rng.next_u64();
    }
    let mut queue = CommandQueue::new();
    queue.push(
        SimCommand::SetSpeed {
            speed: SimSpeed::X2,
        },
        Tick(900),
    );
    let mut log = CommandLog::new();
    log.push(
        Tick(40),
        CommandId(0),
        SimCommand::SetSpeed {
            speed: SimSpeed::Pause,
        },
    );
    let data = SaveDataV2 {
        tick: Tick(TICK),
        rng,
        speed: SimSpeed::X3,
        queue,
        log,
        tiles: TileOwnership::m1_default(),
    };
    let bytes = encode_v1(&data, HASH, "river_valley").unwrap();
    std::fs::create_dir_all("fixtures/saves").unwrap();
    std::fs::write("fixtures/saves/v1-basic.sav", &bytes).unwrap();
    println!("wrote {} bytes", bytes.len());
}
