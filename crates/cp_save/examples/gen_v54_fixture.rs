//! Generates `fixtures/saves/v54-net.sav`, the committed version-54 fixture: a street with two
//! cross streets (one cutting it, one at its end, a bend) built with `BuildRoad`, written by the
//! real v54 writer (before T-262 gave every segment a build order). Run from the repo root in a
//! checkout of the v54 code: `cargo run -p cp_save --example gen_v54_fixture`. The committed
//! file is **never edited or regenerated** afterwards (AGENTS.md).

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_save::save_runner;
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;

fn main() {
    let terrain = Arc::new(Terrain::load(Path::new("assets/maps/flatlands")).expect("map"));
    let content = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain);
    sim.set_content(content.clone());
    sim.tick_once();
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let road = content.road_id("basic_2l").expect("basic_2l");
    let (x, z) = (4600.0f32, 4600.0f32);
    let legs = [
        // A 200 m street (cut every 64 m), a street across it, a bent road off its end.
        RoadPath::Straight {
            from: Vec2::new(x, z),
            to: Vec2::new(x + 200.0, z),
        },
        RoadPath::Straight {
            from: Vec2::new(x + 80.0, z - 90.0),
            to: Vec2::new(x + 80.0, z + 90.0),
        },
        RoadPath::Curve {
            from: Vec2::new(x + 200.0, z),
            control: Vec2::new(x + 240.0, z),
            to: Vec2::new(x + 240.0, z + 40.0),
        },
    ];
    for path in legs {
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road,
                path,
                elevation: ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
    for _ in 0..5 {
        sim.tick_once();
    }
    let bytes = save_runner(&sim, content.content_hash(), "flatlands").expect("save");
    let out = Path::new("fixtures/saves/v54-net.sav");
    std::fs::write(out, &bytes).expect("write");
    println!("wrote {} ({} bytes)", out.display(), bytes.len());
}
