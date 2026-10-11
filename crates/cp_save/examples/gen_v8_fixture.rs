//! Generates `fixtures/saves/v8-basic.sav`, the committed version-8 fixture (the last format
//! without utility networks; it carries a small road network, its zone blocks, and empty
//! building, citizen and path state).
//!
//! The committed file was produced by the real v8 writer (`save_with_map` at format 8,
//! before the v9 bump) with exactly this data; `encode_v8` is the frozen equivalent kept for
//! regeneration and a test asserts they agree byte for byte. Run from the repo root:
//! `cargo run -p cp_save --example gen_v8_fixture`. The committed file is **never edited or
//! regenerated** afterwards (AGENTS.md).

use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_save::SaveData;
use cp_save::migrate::{SaveDataV8, encode_v8};
use cp_sim::SimRunner;

fn fixture_data() -> SaveData {
    let db = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let road = db.road_id("medium_4l").expect("road");
    let def = db.road(road).clone();
    let mut sim = SimRunner::new(81);
    sim.set_content(Arc::clone(&db));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        for (a, b) in [
            ((5000.0, 5000.0), (5090.0, 5000.0)),
            ((5090.0, 5000.0), (5090.0, 5090.0)),
            ((5090.0, 5000.0), (5180.0, 5000.0)),
        ] {
            g.add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(a.0, 100.0, a.1), Vec3::new(b.0, 100.0, b.1)),
                Elevation::Ground,
            )
            .expect("segment");
        }
    }
    for _ in 0..3 {
        sim.tick_once();
    }
    SaveData::capture(sim.world())
}

fn main() {
    let d = fixture_data();
    let v8 = SaveDataV8 {
        tick: d.tick,
        rng: d.rng,
        speed: d.speed,
        queue: d.queue,
        log: d.log,
        tiles: d.tiles,
        net: d.net,
        zones: d.zones,
        buildings: cp_save::migrate::BuildingGridV12::from(&d.buildings),
        citizens: d.citizens,
        paths: d.paths,
    };
    let bytes = encode_v8(
        &v8,
        0xC17F_0000_0000_0009,
        "flatlands",
        Some(0xABCD_0000_0000_0003),
    )
    .unwrap();
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "fixtures/saves/v8-basic.sav".into());
    std::fs::write(&out, &bytes).unwrap();
    println!("wrote {} bytes to {out}", bytes.len());
}
