//! Generates `fixtures/saves/v35-basic.sav`, the committed version-35 fixture: a new game on
//! flatlands (entry highway and entry railway) with a train station joined to the railway,
//! written by the real v35 writer (before the T-167 bump). Run from the repo root:
//! `cargo run -p cp_save --example gen_v35_fixture`. The committed file is **never edited or
//! regenerated** afterwards (AGENTS.md).

use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::BuildingCommand;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::treasury::Treasury;
use cp_core::{Cost, SimLimits};
use cp_net::{Elevation, NetGraph};
use cp_save::save_runner;
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};

fn main() {
    let terrain = Arc::new(Terrain::load(Path::new("assets/maps/flatlands")).expect("map"));
    let content = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(Arc::clone(&terrain));
    sim.set_content(Arc::clone(&content));
    sim.new_game().expect("new game");
    sim.world_mut().insert_resource(TileOwnership::only((2, 2)));
    sim.world_mut().resource_mut::<Treasury>().balance = Cost(1_000_000_000);
    let end = terrain.meta().entry_rail.as_ref().unwrap().dead_end();
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let y = |x: f32, z: f32| Vec3::new(x, terrain.height(Vec2::new(x, z)), z);
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(
                y(end.x - 150.0, end.y + 22.0),
                y(end.x - 30.0, end.y + 22.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();
    sim.push_command(
        SimCommand::Building(BuildingCommand::Place {
            def: content.building_id("train_station").unwrap(),
            pos: Vec2::new(end.x - 64.0, end.y),
            rot: 0.0,
        }),
        sim.tick(),
    );
    for _ in 0..300 {
        sim.tick_once();
    }
    let bytes = save_runner(&sim, content.content_hash(), "flatlands").expect("save");
    std::fs::write("fixtures/saves/v35-basic.sav", &bytes).unwrap();
    println!("v35-basic: {} bytes, tick {}", bytes.len(), sim.tick().0);
}
