//! Generates `fixtures/saves/v41-basic.sav`, the committed version-41 fixture: a grid city on
//! flatlands with a painted district and the ore and oil deposits of the map (some ore spent),
//! written by the real v41 writer (before the T-206 bump). Run from the repo root:
//! `cargo run -p cp_save --example gen_v41_fixture`. The committed file is **never edited or
//! regenerated** afterwards (AGENTS.md).

use std::path::Path;
use std::sync::Arc;

use cp_content::{ContentDb, ResourceKind};
use cp_district::{DistrictCommand, DistrictId};
use cp_save::save_runner;
use cp_sim::SimCommand;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_terrain::{RES_GRID, ResourceDeposits, Terrain};

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
    // Spend some ore in the first cell that has any.
    let mut d = sim.world_mut().resource_mut::<ResourceDeposits>();
    let (cx, cz) = (0..RES_GRID * RES_GRID)
        .map(|i| (i % RES_GRID, i / RES_GRID))
        .find(|&(x, z)| d.remaining(ResourceKind::Ore, x, z) > 1000)
        .expect("ore");
    println!("spent 1000 ore in cell {cx},{cz}");
    d.take(ResourceKind::Ore, cx, cz, 1000);
    for _ in 0..300 {
        sim.tick_once();
    }
    let bytes = save_runner(&sim, content.content_hash(), "flatlands").expect("save");
    std::fs::write("fixtures/saves/v41-basic.sav", &bytes).unwrap();
    println!("v41-basic: {} bytes, tick {}", bytes.len(), sim.tick().0);
}
