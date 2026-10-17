//! Generates `fixtures/saves/v36-goods.sav`, the committed version-36 fixture: a grid city on
//! flatlands run until a goods truck is on the road, written by the real v36 writer (before the
//! T-170 bump). Run from the repo root: `cargo run -p cp_save --example gen_v36_fixture`. The
//! committed file is **never edited or regenerated** afterwards (AGENTS.md).

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
    let mut ticks = 0;
    while sim.world().resource::<cp_transit::Goods>().hauls.is_empty() {
        sim.tick_once();
        ticks += 1;
        assert!(ticks < 200_000, "no goods truck in {ticks} ticks");
    }
    let bytes = save_runner(&sim, content.content_hash(), "flatlands").expect("save");
    std::fs::write("fixtures/saves/v36-goods.sav", &bytes).unwrap();
    let g = sim.world().resource::<cp_transit::Goods>();
    println!(
        "v36-goods: {} bytes, tick {}, {} hauls, {} stocks",
        bytes.len(),
        sim.tick().0,
        g.hauls.len(),
        g.stock.len()
    );
}
