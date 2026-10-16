//! Generates `fixtures/saves/v25-basic.sav`, the committed version-25 fixture: the town of the
//! v19 generator (grown buildings, two placed wind turbines) written by the real v25 writer
//! (`save_with_map` at format 25, before the T-118 bump, MoveIn trips). Run from the repo
//! root: `cargo run -p cp_save --example gen_v25_fixture`. The committed file is **never
//! edited or regenerated** afterwards (AGENTS.md).

use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingCommand, BuildingGrid};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_save::{SaveData, save_with_map};
use cp_sim::{SimCommand, SimRunner};
use cp_zone::{ZoneCellRef, ZoneGrid};

fn main() {
    let db = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let road = db.road_id("basic_2l").expect("road");
    let def = db.road(road).clone();
    let mut sim = SimRunner::new(123);
    sim.set_content(Arc::clone(&db));
    sim.world_mut()
        .insert_resource(cp_terrain::TileOwnership::only((1, 1)));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        // A 3x3-line lattice of 90 m streets.
        let p = |x: f32, z: f32| Vec3::new(3000.0 + x, 0.0, 3000.0 + z);
        let mut streets = Vec::new();
        for line in 0..3 {
            let o = line as f32 * 90.0;
            for piece in 0..2 {
                let a = piece as f32 * 90.0;
                streets.push((p(a, o), p(a + 90.0, o)));
                streets.push((p(o, a), p(o, a + 90.0)));
            }
        }
        for (a, b) in streets {
            g.add_segment(&def, road, CubicBezier3::line(a, b), Elevation::Ground)
                .expect("segment");
        }
    }
    sim.tick_once();
    {
        let kinds = [ZoneType::ResHigh, ZoneType::ComLow, ZoneType::Industrial];
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = grid.blocks().map(|(i, b)| (i, b.cell_count())).collect();
        for (n, (id, cells)) in ids.into_iter().enumerate() {
            for cell in 0..cells {
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    kinds[n % kinds.len()],
                );
            }
        }
    }
    // Two service buildings beside the first blocks' roads.
    let spots: Vec<Vec2> = {
        let zones = sim.world().resource::<ZoneGrid>();
        zones
            .blocks()
            .take(2)
            .filter_map(|(id, _)| {
                zones
                    .cell_footprint(ZoneCellRef {
                        block: id,
                        cell: 5 * 4 + 1,
                    })
                    .map(|f| f.c)
            })
            .collect()
    };
    for (name, at) in ["wind_turbine", "wind_turbine"].into_iter().zip(&spots) {
        sim.push_command(
            SimCommand::Building(BuildingCommand::Place {
                def: db.building_id(name).expect("service building"),
                pos: *at,
                rot: 0.0,
            }),
            sim.tick(),
        );
    }
    sim.tick_once();
    for _ in 0..(5 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    let n = sim.world().resource::<BuildingGrid>().len();
    let services = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| db.building(b.def).service.is_some())
        .count();
    assert!(
        n > services && services > 0,
        "{n} buildings, {services} service"
    );
    let bytes = save_with_map(
        &SaveData::capture(sim.world()),
        0xC25F_0000_0000_0016,
        "flatlands",
        Some(0xABDF_0000_0000_0016),
    )
    .unwrap();
    std::fs::write("fixtures/saves/v25-basic.sav", &bytes).unwrap();
    println!(
        "wrote {} bytes, {n} buildings, {services} service",
        bytes.len()
    );
}
