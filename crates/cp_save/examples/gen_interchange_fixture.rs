//! Generates `fixtures/saves/interchange-v37.sav` (T-064): a ground street with two raised roads
//! over it that cross each other in a node (one level) — an interchange — written by the real
//! writer of its time. Run from the repo root:
//! `cargo run -p cp_save --example gen_interchange_fixture`. The committed file is **never
//! edited or regenerated** afterwards (AGENTS.md).

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::Cost;
use cp_core::SimLimits;
use cp_core::treasury::Treasury;
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_save::save_runner;
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};

fn main() {
    let terrain = Arc::new(Terrain::load(Path::new("assets/maps/flatlands")).expect("map"));
    let content = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(Arc::clone(&terrain));
    sim.set_content(Arc::clone(&content));
    sim.world_mut().insert_resource(TileOwnership::only((2, 2)));
    sim.world_mut().resource_mut::<Treasury>().balance = Cost(1_000_000_000);
    sim.tick_once();
    let road = content.road_id("basic_2l").unwrap();
    let c = Vec2::new(5120.0, 5120.0);
    let roads = [
        // The street on the ground.
        (
            Vec2::new(-250.0, 0.0),
            Vec2::new(250.0, 0.0),
            ElevationMode::Ground,
        ),
        // Two decks over it, at one level, crossing each other.
        (
            Vec2::new(0.0, -300.0),
            Vec2::new(0.0, 300.0),
            ElevationMode::Elevated(10.0),
        ),
        (
            Vec2::new(-300.0, 120.0),
            Vec2::new(300.0, 120.0),
            ElevationMode::Elevated(10.0),
        ),
    ];
    for (a, b, elevation) in roads {
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road,
                path: RoadPath::Straight {
                    from: c + a,
                    to: c + b,
                },
                elevation,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
    for _ in 0..10 {
        sim.tick_once();
    }
    let bytes = save_runner(&sim, content.content_hash(), "flatlands").expect("save");
    std::fs::write("fixtures/saves/interchange-v37.sav", &bytes).unwrap();
    println!(
        "interchange-v37: {} bytes, tick {}",
        bytes.len(),
        sim.tick().0
    );
}
