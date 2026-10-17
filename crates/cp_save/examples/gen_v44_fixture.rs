//! Generates `fixtures/saves/v44-basic.sav`, the committed version-44 fixture: a grid city on
//! flatlands with a forestry area, written by the real v44 writer (before the T-214 bump). Run
//! from the repo root: `cargo run -p cp_save --example gen_v44_fixture`. The committed file is
//! **never edited or regenerated** afterwards (AGENTS.md).

use std::path::Path;
use std::sync::Arc;

use cp_content::{ContentDb, ResourceKind};
use cp_district::{AreaId, AreaKind, DistrictCommand};
use cp_save::save_runner;
use cp_sim::SimCommand;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_terrain::Terrain;

fn main() {
    let terrain = Arc::new(Terrain::load(Path::new("assets/maps/flatlands")).expect("map"));
    let content = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let (mut sim, report) = grid_city(250, 1, terrain, content.clone(), &GridCityConfig::default());
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    cp_sim::new_game::fill_deposits(sim.world_mut());
    let cells: Vec<_> = (150..170)
        .flat_map(|c| (150..170).map(move |r| (c, r)))
        .collect();
    for cmd in [
        DistrictCommand::CreateArea {
            kind: AreaKind::Industry(ResourceKind::Forestry),
            name: "Forest 1".into(),
        },
        DistrictCommand::PaintArea {
            cells,
            area: AreaId(1),
        },
    ] {
        sim.push_command(SimCommand::District(cmd), sim.tick());
        sim.tick_once();
    }
    for _ in 0..300 {
        sim.tick_once();
    }
    let bytes = save_runner(&sim, content.content_hash(), "flatlands").expect("save");
    std::fs::write("fixtures/saves/v44-basic.sav", &bytes).unwrap();
    println!("v44-basic: {} bytes, tick {}", bytes.len(), sim.tick().0);
}
