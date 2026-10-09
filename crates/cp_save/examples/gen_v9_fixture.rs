//! Generates `fixtures/saves/v9-basic.sav`, the committed version-9 fixture (the last format
//! without trips; it carries a small road network, its zone blocks, and a pipe and a power
//! line).
//!
//! The committed file was produced by the real v9 writer (`save_with_map` at format 9, before
//! the v10 bump) with exactly this data. Run from the repo root:
//! `cargo run -p cp_save --example gen_v9_fixture`. The committed file is **never edited or
//! regenerated** afterwards (AGENTS.md).

use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_save::{SaveData, save_with_map};
use cp_sim::SimRunner;
use cp_utility::{UtilityGraph, UtilityNet};

fn main() {
    let db = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let road = db.road_id("medium_4l").expect("road");
    let def = db.road(road).clone();
    let mut sim = SimRunner::new(91);
    sim.set_content(Arc::clone(&db));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        for (a, b) in [
            ((5000.0, 5000.0), (5090.0, 5000.0)),
            ((5090.0, 5000.0), (5090.0, 5090.0)),
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
    {
        let mut u = sim.world_mut().resource_mut::<UtilityGraph>();
        for (net, y) in [(UtilityNet::Pipe, 99.0), (UtilityNet::Power, 101.0)] {
            let a = u.add_node(net, Vec3::new(5000.0, y, 5010.0));
            let b = u.add_node(net, Vec3::new(5060.0, y, 5010.0));
            u.add_link(
                a,
                b,
                CubicBezier3::line(Vec3::new(5000.0, y, 5010.0), Vec3::new(5060.0, y, 5010.0)),
            )
            .expect("link");
        }
    }
    for _ in 0..3 {
        sim.tick_once();
    }
    let bytes = save_with_map(
        &SaveData::capture(sim.world()),
        0xC17F_0000_0000_000A,
        "flatlands",
        Some(0xABCD_0000_0000_0004),
    )
    .unwrap();
    std::fs::create_dir_all("fixtures/saves").unwrap();
    std::fs::write("fixtures/saves/v9-basic.sav", &bytes).unwrap();
    println!("wrote {} bytes", bytes.len());
}
