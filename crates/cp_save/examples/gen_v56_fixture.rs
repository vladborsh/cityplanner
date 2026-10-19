//! Generates `fixtures/saves/v56-dead-ends.sav`, the committed version-56 fixture: a lone
//! `basic_2l` street of 100 m (two dead ends), its cells painted low-density residential on both
//! sides and grown until houses stand; written by the real v56 writer (before T-275, whose layout
//! keeps a half width free at a dead end for the cross street that will close it: v56 laid the
//! cells right up to the dead ends). Run from the repo root in a checkout of the v56 code:
//! `cargo run -p cp_save --example gen_v56_fixture`. The committed file is **never edited or
//! regenerated** afterwards (AGENTS.md).

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_building::{BuildingGrid, Demand};
use cp_content::{ContentDb, ZoneType};
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_save::save_runner;
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_zone::{ZoneCommand, ZoneGrid};

fn main() {
    let terrain = Arc::new(Terrain::load(Path::new("assets/maps/flatlands")).expect("map"));
    let content = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let mut sim = SimRunner::new(7);
    sim.set_terrain(terrain);
    sim.set_content(content.clone());
    sim.tick_once();
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let road = content.road_id("basic_2l").expect("basic_2l");
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road,
            path: RoadPath::Straight {
                from: Vec2::new(4600.0, 4600.0),
                to: Vec2::new(4700.0, 4600.0),
            },
            elevation: ElevationMode::Ground,
        }),
        sim.tick(),
    );
    for _ in 0..4 {
        sim.tick_once();
    }
    let cells: Vec<_> = {
        let z = sim.world().resource::<ZoneGrid>();
        z.blocks()
            .flat_map(|(id, _)| z.block_cells(id))
            .filter(|c| z.is_valid(*c))
            .collect()
    };
    let cols: Vec<u16> = {
        let z = sim.world().resource::<ZoneGrid>();
        z.blocks().map(|(_, b)| b.cols).collect()
    };
    // The street is cut in two pieces; 12 columns a side in all.
    assert_eq!(
        cols.iter().map(|&c| u32::from(c)).sum::<u32>(),
        24,
        "v56: 12 columns a side, to the dead ends, {cols:?}"
    );
    sim.push_command(
        SimCommand::Zone(ZoneCommand::PaintZones {
            cells,
            zone: ZoneType::ResLow,
        }),
        sim.tick(),
    );
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(1.0);
    for _ in 0..4000 {
        sim.tick_once();
    }
    let buildings = sim.world().resource::<BuildingGrid>().iter().count();
    assert!(buildings >= 6, "{buildings} buildings");
    let bytes = save_runner(&sim, content.content_hash(), "flatlands").expect("save");
    let out = Path::new("fixtures/saves/v56-dead-ends.sav");
    std::fs::write(out, &bytes).expect("write");
    println!(
        "{}: {} bytes, {buildings} buildings, checksum {:016x}",
        out.display(),
        bytes.len(),
        sim.checksum()
    );
}
