//! Generates `fixtures/saves/v55-fitted.sav`, the committed version-55 fixture: a closed block
//! whose edges stand 70 m apart (four `basic_2l` streets 82 m apart, centre to centre), so the
//! v2 layout fitted its cells to 8.75 m, painted low-density residential everywhere and grown
//! until houses stand on the fitted cells; written by the real v55 writer (before T-265, whose
//! layout v3 lays cells of 8 m without stretching). Run from the repo root in a checkout of the
//! v55 code: `cargo run -p cp_save --example gen_v55_fixture`. The committed file is **never
//! edited or regenerated** afterwards (AGENTS.md).

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
    let (x, z, d) = (4600.0f32, 4600.0f32, 82.0f32);
    let corners = [
        Vec2::new(x, z),
        Vec2::new(x + d, z),
        Vec2::new(x + d, z + d),
        Vec2::new(x, z + d),
    ];
    for i in 0..4 {
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road,
                path: RoadPath::Straight {
                    from: corners[i],
                    to: corners[(i + 1) % 4],
                },
                elevation: ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
    for _ in 0..3 {
        sim.tick_once();
    }
    let cells: Vec<_> = {
        let z = sim.world().resource::<ZoneGrid>();
        z.blocks()
            .flat_map(|(id, _)| z.block_cells(id))
            .filter(|c| z.is_valid(*c))
            .collect()
    };
    let fitted = sim
        .world()
        .resource::<ZoneGrid>()
        .blocks()
        .filter(|(_, b)| (b.cell_len - 8.0).abs() > 0.01)
        .count();
    assert!(fitted > 0, "the block's cells are fitted");
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
    assert!(buildings >= 8, "{buildings} buildings");
    let bytes = save_runner(&sim, content.content_hash(), "flatlands").expect("save");
    let out = Path::new("fixtures/saves/v55-fitted.sav");
    std::fs::write(out, &bytes).expect("write");
    println!(
        "{}: {} bytes, {fitted} fitted blocks, {buildings} buildings, checksum {:016x}",
        out.display(),
        bytes.len(),
        sim.checksum()
    );
}
