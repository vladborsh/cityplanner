//! Generates `fixtures/saves/v53-corner.sav`, the committed version-53 fixture: an L with a
//! rounded corner (a street down, a quadratic bend, a street east) and the zone cells the v53
//! layout gave it (the cells follow the arc), written by the real v53 writer (before the
//! BUG-073b bump, which lays the inner side of a rounded corner from its virtual corner). Run
//! from the repo root in a checkout of the v53 code: `cargo run -p cp_save --example
//! gen_v53_fixture`. The committed file is **never edited or regenerated** afterwards (AGENTS.md).

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
    // The corner: the quadratic from (x, z + r) through (x, z) to (x + r, z) is 40 m long.
    let (x, z, r) = (4600.0f32, 4600.0f32, 24.642_298f32);
    let legs = [
        RoadPath::Straight {
            from: Vec2::new(x, z + r + 176.0),
            to: Vec2::new(x, z + r),
        },
        RoadPath::Curve {
            from: Vec2::new(x, z + r),
            control: Vec2::new(x, z),
            to: Vec2::new(x + r, z),
        },
        RoadPath::Straight {
            from: Vec2::new(x + r, z),
            to: Vec2::new(x + r + 192.0, z),
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
    std::fs::write("fixtures/saves/v53-corner.sav", &bytes).unwrap();
    println!("v53-corner: {} bytes, tick {}", bytes.len(), sim.tick().0);
}
