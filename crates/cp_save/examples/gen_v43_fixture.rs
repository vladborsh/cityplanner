//! Generates `fixtures/saves/v43-goods.sav`, the committed version-43 fixture: a grid city on
//! flatlands run until a goods truck is on the road, written by the real v43 writer (before the
//! T-210 bump). Run from the repo root: `cargo run -p cp_save --example gen_v43_fixture`. The
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
    cp_sim::new_game::fill_deposits(sim.world_mut());
    let mut ticks = 0;
    while sim.world().resource::<cp_transit::Goods>().hauls.is_empty() {
        sim.tick_once();
        ticks += 1;
        assert!(ticks < 200_000, "no goods truck in {ticks} ticks");
    }
    let bytes = save_runner(&sim, content.content_hash(), "flatlands").expect("save");
    std::fs::write("fixtures/saves/v43-goods.sav", &bytes).unwrap();
    let g = sim.world().resource::<cp_transit::Goods>();
    println!(
        "v43-goods: {} bytes, tick {}, {} hauls",
        bytes.len(),
        sim.tick().0,
        g.hauls.len()
    );
}
