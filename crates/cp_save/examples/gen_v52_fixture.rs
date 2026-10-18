//! Generates `fixtures/saves/v52-basic.sav`, the committed version-52 fixture: a grid city on
//! flatlands with its garbage, crime and service state after some ticks, written by the real v52
//! writer (before the T-247 bump, which adds the mail). Run from the repo root in a checkout of
//! the v52 code: `cargo run -p cp_save --example gen_v52_fixture`. The committed file is
//! **never edited or regenerated** afterwards (AGENTS.md).

use std::path::Path;
use std::sync::Arc;

use cp_content::ContentDb;
use cp_save::save_runner;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_terrain::Terrain;

fn main() {
    let terrain = Arc::new(Terrain::load(Path::new("assets/maps/flatlands")).expect("map"));
    let content = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let (mut sim, report) = grid_city(250, 1, terrain, content.clone(), &GridCityConfig::default());
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    for _ in 0..600 {
        sim.tick_once();
    }
    let bytes = save_runner(&sim, content.content_hash(), "flatlands").expect("save");
    std::fs::write("fixtures/saves/v52-basic.sav", &bytes).unwrap();
    println!("v52-basic: {} bytes, tick {}", bytes.len(), sim.tick().0);
}
