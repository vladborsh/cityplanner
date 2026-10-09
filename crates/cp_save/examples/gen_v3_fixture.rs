//! Generates `fixtures/saves/v3-basic.sav`, the committed version-3 fixture (the last
//! format with a body-only CRC and no map hash; it carries a small road network).
//!
//! The committed file was produced by the real v3 writer (`save` at format 3, before the
//! v4 bump) with exactly this data; `encode_v3` is the frozen equivalent kept for
//! regeneration and a test asserts they agree byte for byte. Run from the repo root:
//! `cargo run -p cp_save --example gen_v3_fixture`. The committed file is **never edited
//! or regenerated** afterwards (AGENTS.md).

use bevy_math::Vec3;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::{CommandId, CommandLog, CommandQueue, SimRng, SimSpeed, Tick};
use cp_net::{Elevation, NetGraph};
use cp_save::migrate::{SaveDataV3, encode_v3};
use cp_sim::SimCommand;
use cp_terrain::TileOwnership;

fn fixture_data() -> SaveDataV3 {
    let mut rng = SimRng::seed_from_u64(31);
    for _ in 0..6 {
        rng.next_u64();
    }
    let mut queue = CommandQueue::new();
    queue.push(
        SimCommand::SetSpeed {
            speed: SimSpeed::X2,
        },
        Tick(1200),
    );
    let mut log = CommandLog::new();
    log.push(
        Tick(50),
        CommandId(0),
        SimCommand::SetSpeed {
            speed: SimSpeed::X3,
        },
    );
    // A tiny network: one medium street split in two, from the shipped road content.
    let db = ContentDb::load_dir("assets/content").expect("content");
    let road = db.road_id("medium_4l").expect("road");
    let def = db.road(road).clone();
    let mut net = NetGraph::new();
    let seg = net
        .add_segment(
            &def,
            road,
            CubicBezier3::new(
                Vec3::new(5000.0, 100.0, 5000.0),
                Vec3::new(5030.0, 100.0, 5008.0),
                Vec3::new(5060.0, 100.0, 4992.0),
                Vec3::new(5090.0, 100.0, 5000.0),
            ),
            Elevation::Ground,
        )
        .expect("segment");
    net.split_segment(&def, seg, 0.5).expect("split");
    SaveDataV3 {
        tick: Tick(900),
        rng,
        speed: SimSpeed::X2,
        queue,
        log,
        tiles: TileOwnership::m1_around((2, 2)),
        net,
    }
}

fn main() {
    let bytes = encode_v3(&fixture_data(), 0xC17F_0000_0000_0004, "flatlands").unwrap();
    std::fs::create_dir_all("fixtures/saves").unwrap();
    std::fs::write("fixtures/saves/v3-basic.sav", &bytes).unwrap();
    println!("wrote {} bytes", bytes.len());
}
