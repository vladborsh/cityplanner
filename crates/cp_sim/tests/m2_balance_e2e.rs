//! T-148: the M2 balance harness. A scripted city with its services (`grid_city`) is run for
//! game days; the table of population, treasury and abandoned buildings per day is printed by
//! the ignored test (`cargo test -p cp_sim --release --test m2_balance_e2e -- --ignored
//! --nocapture`), and a small town is checked to live.

use std::path::Path;
use std::sync::Arc;

use cp_building::{BuildingGrid, BuildingState};
use cp_content::ContentDb;
use cp_core::Treasury;
use cp_core::time::TICKS_PER_DAY;
use cp_sim::scenario::{GridCityConfig, grid_city, population};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

struct Row {
    day: u64,
    pop: usize,
    treasury: i64,
    abandoned: usize,
    buildings: usize,
}

fn run_days(target: usize, seed: u64, days: u64) -> Vec<Row> {
    let root = Path::new(ROOT).join("../..");
    let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
    let content = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
    let cfg = GridCityConfig::default();
    let (mut sim, report) = grid_city(target, seed, terrain, content, &cfg);
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    let start = sim.world().resource::<Treasury>().balance.0;
    let mut rows = Vec::new();
    for day in 0..=days {
        if day > 0 {
            for _ in 0..TICKS_PER_DAY {
                sim.tick_once();
            }
        }
        if std::env::var("M2_WHY")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            == Some(day)
        {
            let g = sim.world().resource::<BuildingGrid>();
            let st = sim.world().resource::<cp_service::ServiceState>();
            let mut n = [0usize; 8];
            let mut p = [0usize; 5];
            for (id, b) in g.iter() {
                for (i, c) in n.iter_mut().enumerate() {
                    *c += usize::from(b.flags.0 & (1 << i) != 0);
                }
                for (i, c) in p.iter_mut().enumerate() {
                    *c += usize::from(st.problems_of(id).0 & (1 << i) != 0);
                }
            }
            let db = sim.world().resource::<cp_content::SimContent>().0.clone();
            let cov = sim.world().resource::<cp_service::ServiceCoverage>();
            let mut have: std::collections::BTreeMap<String, u32> = Default::default();
            let (mut uncovered, mut garbage) = (0, 0);
            for (id, b) in g.iter() {
                let d = db.building(b.def);
                if d.service.is_some() {
                    *have.entry(d.id.clone()).or_default() += 1;
                }
                if st.problems_of(id).has(cp_service::ServiceProblems::GARBAGE) {
                    garbage += 1;
                    uncovered +=
                        u32::from(cov.coverage_of(cp_content::ServiceKind::Garbage, b) <= 0.0);
                }
            }
            let cg = sim.world().resource::<cp_citizen::CitizenGrid>();
            let sick = cg
                .citizens()
                .filter(|(_, c)| c.sick_since.is_some())
                .count();
            let waiting: u32 = cg.dead.values().map(|&d| u32::from(d)).sum();
            println!(
                "services {have:?}; garbage {garbage}, uncovered {uncovered}; sick citizens {sick}, bodies waiting {waiting}, households {}",
                cg.household_count()
            );
            let mut dark = 0;
            let mut sick_uncov = 0;
            for (id, b) in g.iter() {
                if db.building(b.def).service.is_some()
                    && b.flags.has(cp_building::BuildingFlags::NO_POWER)
                {
                    dark += 1;
                }
                if st.problems_of(id).has(cp_service::ServiceProblems::SICK)
                    && cov.coverage_of(cp_content::ServiceKind::Health, b) <= 0.0
                {
                    sick_uncov += 1;
                }
            }
            let used: Vec<_> = st
                .facilities()
                .map(|(i, f)| (i.index, f.used_today, f.stored))
                .collect();
            println!(
                "dark services {dark}, sick homes uncovered {sick_uncov}, facilities {used:?}"
            );
            println!("flags water,power,workers,customers,goods,sewage,road,path: {n:?}");
            println!("problems garbage,sick,dead,fire,crime: {p:?}");
        }
        let g = sim.world().resource::<BuildingGrid>();
        rows.push(Row {
            day,
            pop: population(&sim),
            treasury: (sim.world().resource::<Treasury>().balance.0 - start) / 100,
            abandoned: g
                .iter()
                .filter(|(_, b)| b.state == BuildingState::Abandoned)
                .count(),
            buildings: g.iter().count(),
        });
    }
    rows
}

#[test]
#[ignore = "T-148 report: run in release with --nocapture"]
fn print_the_m2_balance_table() {
    let target: usize = std::env::var("M2_POP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);
    for seed in [1, 2, 3] {
        println!("seed {seed}, target {target}");
        println!("day  pop  treasury_delta  abandoned  buildings");
        for r in run_days(target, seed, 20) {
            println!(
                "{:>3} {:>5} {:>14} {:>9} {:>9}",
                r.day, r.pop, r.treasury, r.abandoned, r.buildings
            );
        }
    }
}

/// A scripted town with its services lives: nobody is mass-abandoned in a game week and the
/// people stay.
#[test]
fn a_town_with_its_services_keeps_its_buildings_and_people_for_a_week() {
    let rows = run_days(600, 1, 7);
    let (first, last) = (rows.first().unwrap(), rows.last().unwrap());
    assert!(
        last.pop as f64 >= 0.9 * first.pop as f64,
        "{} -> {}",
        first.pop,
        last.pop
    );
    assert!(
        last.abandoned * 20 <= last.buildings,
        "{} of {} buildings abandoned",
        last.abandoned,
        last.buildings
    );
}
