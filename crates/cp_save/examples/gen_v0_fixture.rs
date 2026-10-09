//! Generates `fixtures/saves/v0-basic.sav`, the committed version-0 fixture.
//!
//! Run once from the repo root: `cargo run -p cp_save --example gen_v0_fixture`.
//! The committed file is **never edited or regenerated** afterwards (AGENTS.md); if the
//! format changes, add a new fixture for the new version instead.

use cp_core::{CommandId, CommandLog, CommandQueue, SimRng, SimSpeed, Tick};
use cp_save::migrate::{SaveDataV0, encode_v0};
use cp_sim::SimCommand;
use cp_terrain::TileOwnership;

pub const FIXTURE_SEED: u64 = 7;
pub const FIXTURE_TICK: u64 = 300;
pub const FIXTURE_HASH: u64 = 0xC17F_0000_0000_0001;

fn main() {
    let mut rng = SimRng::seed_from_u64(FIXTURE_SEED);
    for _ in 0..10 {
        rng.next_u64();
    }
    let mut queue = CommandQueue::new();
    queue.push(
        SimCommand::SetSpeed {
            speed: SimSpeed::Pause,
        },
        Tick(400),
    );
    let mut log = CommandLog::new();
    log.push(
        Tick(12),
        CommandId(0),
        SimCommand::SetSpeed {
            speed: SimSpeed::X3,
        },
    );
    let v0 = SaveDataV0 {
        tick: Tick(FIXTURE_TICK),
        rng,
        queue,
        log,
        tiles: TileOwnership::m1_around((2, 2)),
    };
    let bytes = encode_v0(&v0, FIXTURE_HASH, "flatlands").unwrap();
    std::fs::create_dir_all("fixtures/saves").unwrap();
    std::fs::write("fixtures/saves/v0-basic.sav", &bytes).unwrap();
    println!("wrote {} bytes", bytes.len());
}
