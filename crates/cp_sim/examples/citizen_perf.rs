//! Growth workload of T-031: a 14x14 street lattice grown to ~10k citizens, then 2000 more
//! ticks; prints what the city looks like. No clock here (sim crates have none, even in
//! examples): time it from outside, e.g. `time cargo run --release -p cp_sim --example
//! citizen_perf`, or use `cp_bench` (`--scenario grid_city`, `scripts/bench.sh`) for ms per phase.

use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::BuildingGrid;
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn main() {
    let content = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(&content));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let p = |x: f32, z: f32| Vec3::new(2000.0 + x, 0.0, 2000.0 + z);
        for line in 0..14 {
            let o = line as f32 * 90.0;
            for piece in 0..13 {
                let a = piece as f32 * 90.0;
                for (from, to) in [(p(a, o), p(a + 90.0, o)), (p(o, a), p(o, a + 90.0))] {
                    g.add_segment(&def, road, CubicBezier3::line(from, to), Elevation::Ground)
                        .unwrap();
                }
            }
        }
    }
    sim.tick_once();
    {
        let kinds = [
            ZoneType::ResHigh,
            ZoneType::ResHigh,
            ZoneType::Industrial,
            ZoneType::ComHigh,
        ];
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = grid.blocks().map(|(id, b)| (id, b.cell_count())).collect();
        for (n, (id, cells)) in ids.into_iter().enumerate() {
            for cell in 0..cells {
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    kinds[n % 4],
                );
            }
        }
    }
    let mut ticks = 0u64;
    while sim.world().resource::<CitizenGrid>().citizen_count() < 10_000 && ticks < 200_000 {
        sim.tick_once();
        ticks += 1;
    }
    let (c, h, b) = {
        let w = sim.world();
        (
            w.resource::<CitizenGrid>().citizen_count(),
            w.resource::<CitizenGrid>().household_count(),
            w.resource::<BuildingGrid>().len(),
        )
    };
    println!("growth: {ticks} ticks -> {c} citizens, {h} households, {b} buildings");
    for _ in 0..2000 {
        sim.tick_once();
    }
    let w = sim.world();
    println!(
        "steady: 2000 more ticks, {} citizens ({} adults, {} unemployed)",
        w.resource::<CitizenGrid>().citizen_count(),
        w.resource::<CitizenGrid>().adults(),
        w.resource::<CitizenGrid>().unemployed(),
    );
}
