//! Generates `fixtures/saves/v5-basic.sav`, the committed version-5 fixture (the last format
//! without buildings; it carries a small road network and its derived zone blocks).
//!
//! The committed file was produced by the real v5 writer (`save_with_map` at format 5,
//! before the v6 bump) with exactly this data. Run from the repo root:
//! `cargo run -p cp_save --example gen_v5_fixture`. The committed file is **never edited or
//! regenerated** afterwards (AGENTS.md).

use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_save::{SaveData, save_with_map};
use cp_sim::SimRunner;

fn main() {
    let db = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let road = db.road_id("medium_4l").expect("road");
    let def = db.road(road).clone();
    let mut sim = SimRunner::new(51);
    sim.set_content(Arc::clone(&db));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        g.add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(5000.0, 100.0, 5000.0),
                Vec3::new(5090.0, 100.0, 5000.0),
            ),
            Elevation::Ground,
        )
        .expect("segment");
    }
    // One tick derives the zone blocks from the road.
    for _ in 0..3 {
        sim.tick_once();
    }
    let bytes = save_with_map(
        &SaveData::capture(sim.world()),
        0xC17F_0000_0000_0006,
        "flatlands",
        Some(0xABCD_0000_0000_0001),
    )
    .unwrap();
    std::fs::create_dir_all("fixtures/saves").unwrap();
    std::fs::write("fixtures/saves/v5-basic.sav", &bytes).unwrap();
    println!("wrote {} bytes", bytes.len());
}
