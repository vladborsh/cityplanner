//! Generates `fixtures/saves/v18-basic.sav`, the committed version-18 fixture: a small town
//! with zoned buildings that grew by themselves and two placed service buildings (a wind
//! turbine and a water pump), so the building arena holds lots, access points and service
//! defs as format 18 wrote them.
//!
//! The committed file was produced by the real v18 writer (`save_with_map` at format 18,
//! before the v19 bump of T-106, the `Footprint` change) with exactly this data. Run from the
//! repo root: `cargo run -p cp_save --example gen_v18_fixture`. The committed file is
//! **never edited or regenerated** afterwards (AGENTS.md).

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
    let mut sim = SimRunner::new(118);
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
        0xC18F_0000_0000_0012,
        "flatlands",
        Some(0xABCD_0000_0000_0012),
    )
    .unwrap();
    std::fs::write("fixtures/saves/v18-basic.sav", &bytes).unwrap();
    println!(
        "wrote {} bytes, {n} buildings, {services} service",
        bytes.len()
    );
}
