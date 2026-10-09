//! Generates `fixtures/saves/v4-basic.sav`, the committed version-4 fixture (the last format
//! without zone state; it carries a small road network).
//!
//! The committed file was produced by the real v4 writer (`save_with_map` at format 4,
//! before the v5 bump) with exactly this data. Run from the repo root:
//! `cargo run -p cp_save --example gen_v4_fixture`. The committed file is **never edited or
//! regenerated** afterwards (AGENTS.md).

use bevy_math::Vec3;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::{CommandId, CommandLog, CommandQueue, SimRng, SimSpeed, Tick};
use cp_net::{Elevation, NetGraph};
use cp_save::{SaveData, save_with_map};
use cp_sim::SimCommand;
use cp_terrain::TileOwnership;

fn fixture_data() -> SaveData {
    let mut rng = SimRng::seed_from_u64(41);
    for _ in 0..8 {
        rng.next_u64();
    }
    let mut queue = CommandQueue::new();
    queue.push(
        SimCommand::SetSpeed {
            speed: SimSpeed::X3,
        },
        Tick(2000),
    );
    let mut log = CommandLog::new();
    log.push(
        Tick(60),
        CommandId(0),
        SimCommand::SetSpeed {
            speed: SimSpeed::X2,
        },
    );
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
                Vec3::new(5030.0, 100.0, 5010.0),
                Vec3::new(5060.0, 100.0, 4990.0),
                Vec3::new(5090.0, 100.0, 5000.0),
            ),
            Elevation::Ground,
        )
        .expect("segment");
    net.split_segment(&def, seg, 0.4).expect("split");
    SaveData {
        tick: Tick(1000),
        rng,
        speed: SimSpeed::X3,
        queue,
        log,
        tiles: TileOwnership::m1_around((2, 2)),
        net,
    }
}

fn main() {
    let bytes = save_with_map(
        &fixture_data(),
        0xC17F_0000_0000_0005,
        "flatlands",
        Some(0xABCD_0000_0000_0001),
    )
    .unwrap();
    std::fs::create_dir_all("fixtures/saves").unwrap();
    std::fs::write("fixtures/saves/v4-basic.sav", &bytes).unwrap();
    println!("wrote {} bytes", bytes.len());
}
