//! Generates `fixtures/saves/v17-basic.sav`, the committed version-17 fixture (cars carry the id of their re-route
//! request; one of them is waiting for a new route).
//!
//! The committed file was produced by the real v17 writer (`save_with_map` at format 17,
//! before the v18 bump) with exactly this data. Run from the repo root:
//! `cargo run -p cp_save --example gen_v17_fixture`. The committed file is **never edited or
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
    for _ in 0..4 {
        sim.tick_once();
    }
    // Cut a street under the cars: those with it ahead wait for new routes (8 ticks). It is
    // one nobody stands on or is heading for.
    let cut = {
        let agents = sim.world().resource::<Agents>();
        let net = sim.world().resource::<NetGraph>();
        let seg_of = |l| net.lane(l).map(|l| l.segment);
        let busy: Vec<_> = agents
            .vehicles()
            .flat_map(|(_, v)| [seg_of(v.lane()), seg_of(v.path.goal)])
            .flatten()
            .collect();
        agents
            .vehicles()
            .flat_map(|(_, v)| v.path.lanes().skip(usize::from(v.hop) + 1))
            .filter_map(seg_of)
            .find(|s| !busy.contains(s))
            .expect("a street with cars still to come and nobody on it")
    };
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .remove_segment(cut)
        .unwrap();
    for _ in 0..2 {
        sim.tick_once();
    }
    let waiting = sim
        .world()
        .resource::<Agents>()
        .vehicles()
        .filter(|(_, v)| v.reroute.is_some())
        .count();
    assert!(waiting > 0, "a car waits for a new route");
    let n = sim.world().resource::<Agents>().vehicle_count();
    assert!(n > 0, "a car is in flight");
    let bytes = save_with_map(
        &SaveData::capture(sim.world()),
        0xC17F_0000_0000_000F,
        "flatlands",
        Some(0xABCD_0000_0000_0009),
    )
    .unwrap();
    std::fs::write("fixtures/saves/v17-basic.sav", &bytes).unwrap();
    println!("wrote {} bytes, {n} cars, {waiting} waiting", bytes.len());
}
