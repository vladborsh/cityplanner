//! Generates `fixtures/saves/v37-basic.sav`, the committed version-37 fixture: a grid city on
//! flatlands with a painted district, written by the real v37 writer (before the T-190 bump).
//! Run from the repo root: `cargo run -p cp_save --example gen_v37_fixture`. The committed file
//! is **never edited or regenerated** afterwards (AGENTS.md).

use std::path::Path;
use std::sync::Arc;

use cp_content::ContentDb;
use cp_district::{DistrictCommand, DistrictId};
use cp_save::save_runner;
use cp_sim::SimCommand;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_terrain::Terrain;

fn main() {
    let terrain = Arc::new(Terrain::load(Path::new("assets/maps/flatlands")).expect("map"));
    let content = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let (mut sim, report) = grid_city(250, 1, terrain, content.clone(), &GridCityConfig::default());
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    let cells: Vec<_> = (150..170)
        .flat_map(|c| (150..170).map(move |r| (c, r)))
        .collect();
    for cmd in [
        DistrictCommand::Create {
            name: "Old Town".into(),
        },
        DistrictCommand::Paint {
            cells,
            district: DistrictId(1),
        },
    ] {
        sim.push_command(SimCommand::District(cmd), sim.tick());
        sim.tick_once();
    }
    for _ in 0..300 {
        sim.tick_once();
    }
    let bytes = save_runner(&sim, content.content_hash(), "flatlands").expect("save");
    std::fs::write("fixtures/saves/v37-basic.sav", &bytes).unwrap();
    println!("v37-basic: {} bytes, tick {}", bytes.len(), sim.tick().0);
}
