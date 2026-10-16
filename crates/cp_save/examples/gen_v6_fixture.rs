//! Generates `fixtures/saves/v6-basic.sav`, the committed version-6 fixture (the last format
//! without citizens; it carries a small road network, zoned blocks and buildings that are
//! both finished and under construction).
//!
//! The committed file was produced by the real v6 writer (`save_with_map` at format 6,
//! before the v7 bump) with exactly this data; `encode_v6` is the frozen equivalent kept
//! for regeneration and a test asserts they agree byte for byte. Run from the repo root:
//! `cargo run -p cp_save --example gen_v6_fixture`. The committed file is **never edited or
//! regenerated** afterwards (AGENTS.md).

use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{BuildingGrid, BuildingState};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_save::SaveData;
use cp_save::migrate::{SaveDataV6, encode_v6};
use cp_sim::SimRunner;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn main() {
    let db = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let road = db.road_id("basic_2l").expect("road");
    let def = db.road(road).clone();
    let mut sim = SimRunner::new(61);
    sim.set_content(Arc::clone(&db));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        for (a, b) in [
            ((3000.0, 3000.0), (3090.0, 3000.0)),
            ((3045.0, 2940.0), (3045.0, 3060.0)),
        ] {
            g.add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1)),
                Elevation::Ground,
            )
            .expect("segment");
        }
    }
    sim.tick_once();
    {
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = grid.blocks().map(|(i, b)| (i, b.cell_count())).collect();
        for (id, n) in ids {
            for cell in 0..n {
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    ZoneType::ResLow,
                );
            }
        }
    }
    // Until some buildings are finished and some are still under construction.
    for _ in 0..40_000 {
        sim.tick_once();
        let b = sim.world().resource::<BuildingGrid>();
        let active = b
            .iter()
            .filter(|(_, x)| x.state == BuildingState::Active)
            .count();
        let building = b
            .iter()
            .filter(|(_, x)| x.state == BuildingState::UnderConstruction)
            .count();
        if active >= 2 && building >= 1 {
            break;
        }
    }
    let d = SaveData::capture(sim.world());
    let v6 = SaveDataV6 {
        tick: d.tick,
        rng: d.rng,
        speed: d.speed,
        queue: d.queue,
        log: d.log,
        tiles: d.tiles,
        net: d.net,
        zones: d.zones.to_v34(),
        buildings: cp_save::migrate::BuildingGridV12::from(&d.buildings),
    };
    let bytes = encode_v6(
        &v6,
        0xC17F_0000_0000_0007,
        "flatlands",
        Some(0xABCD_0000_0000_0001),
    )
    .unwrap();
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "fixtures/saves/v6-basic.sav".into());
    std::fs::write(&out, &bytes).unwrap();
    println!("wrote {} bytes to {out}", bytes.len());
}
