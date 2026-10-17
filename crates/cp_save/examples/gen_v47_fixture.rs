//! Generates `fixtures/saves/v47-basic.sav`, the committed version-47 fixture: a grid city on
//! flatlands with a city-park area holding an attraction and a gate, written by the real v47
//! writer (before the T-224 bump). Run from the repo root in a checkout of the v47 code:
//! `cargo run -p cp_save --example gen_v47_fixture`. The committed file is **never edited or
//! regenerated** afterwards (AGENTS.md).

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_building::{Building, BuildingFlags, BuildingGrid, BuildingState, Footprint};
use cp_content::{ContentDb, ParkKind};
use cp_core::Tick;
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
    let cells: Vec<_> = (150..160)
        .flat_map(|c| (150..160).map(move |r| (c, r)))
        .collect();
    for cmd in [
        DistrictCommand::CreateArea {
            kind: AreaKind::Park(ParkKind::CityPark),
            name: "Park 1".into(),
        },
        DistrictCommand::PaintArea {
            cells,
            area: AreaId(1),
        },
    ] {
        sim.push_command(SimCommand::District(cmd), sim.tick());
        sim.tick_once();
    }
    // An attraction inside the area (cell 150..160 x 32 m = 4800..5120 m), active at once.
    let def = content.building_id("city_park_attr_1").expect("def");
    let d = content.building(def);
    sim.world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
            def,
            footprint: Footprint::Free {
                w: d.lot.0,
                d: d.lot.1,
            },
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos: Vec2::new(4960.0, 4960.0),
            rot: 0.0,
            access: None,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
    for _ in 0..300 {
        sim.tick_once();
    }
    let bytes = save_runner(&sim, content.content_hash(), "flatlands").expect("save");
    std::fs::write("fixtures/saves/v47-basic.sav", &bytes).unwrap();
    println!("v47-basic: {} bytes, tick {}", bytes.len(), sim.tick().0);
}
