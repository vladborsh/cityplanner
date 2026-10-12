//! Generates `fixtures/saves/v16-basic.sav`, the committed version-16 fixture (land grids and decay timers; a
//! car is in flight, which the v17 migration gives an empty reroute slot).
//!
//! The committed file was produced by the real v16 writer (`save_with_map` at format 16,
//! before the v17 bump) with exactly this data. Run from the repo root:
//! `cargo run -p cp_save --example gen_v16_fixture`. The committed file is **never edited or
//! regenerated** afterwards (AGENTS.md).

use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_save::{SaveData, save_with_map};
use cp_sim::SimRunner;
use cp_traffic::Agents;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn main() {
    let db = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let road = db.road_id("basic_2l").expect("road");
    let def = db.road(road).clone();
    let mut sim = SimRunner::new(111);
    sim.set_content(Arc::clone(&db));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        for (a, b) in [
            ((3000.0, 3000.0), (3200.0, 3000.0)),
            ((3200.0, 3000.0), (3300.0, 3000.0)),
            ((3300.0, 3000.0), (3500.0, 3000.0)),
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
    sim.push_command(
        cp_sim::SimCommand::Economy(cp_economy::EconomyCommand::SetTaxRate {
            zone: ZoneType::ComLow,
            pct: 13,
        }),
        sim.tick(),
    );
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
    for _ in 0..(5 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    // Cars on the road: routed demo cars (the town is too small for commuters to drive).
    sim.world_mut()
        .resource_scope(|w, mut agents: bevy_ecs::change_detection::Mut<Agents>| {
            let net = w.resource::<NetGraph>();
            let graph = std::sync::Arc::clone(w.resource::<cp_traffic::PathNetwork>().graph());
            let rng = w.resource::<cp_core::SimRng>();
            cp_traffic::demo::top_up(&mut agents, &graph, net, 6, 0, rng, 0);
        });
    for _ in 0..30 {
        sim.tick_once();
    }
    let n = sim.world().resource::<Agents>().vehicle_count();
    assert!(n > 0, "a car is in flight");
    let bytes = save_with_map(
        &SaveData::capture(sim.world()),
        0xC17F_0000_0000_000E,
        "flatlands",
        Some(0xABCD_0000_0000_0008),
    )
    .unwrap();
    std::fs::write("fixtures/saves/v16-basic.sav", &bytes).unwrap();
    println!("wrote {} bytes, {n} cars", bytes.len());
}
