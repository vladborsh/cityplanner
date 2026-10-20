//! T-282 probe: a town of ~700 and one of ~4000 residents with ONE police station, ONE fire
//! station and ONE landfill (placed near the middle), ten game days. Prints the problems and the
//! abandoned buildings, and asserts the towns keep out of trouble (a police station covered a
//! 4000 town in C:S; problems escalate five times slower than they used to).

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_building::{BuildingCommand, BuildingGrid};
use cp_content::ContentDb;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{CommandOutcome, Cost, Treasury};
use cp_service::ServiceState;
use cp_sim::scenario::{GridCityConfig, grid_city, population};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_zone::{ZoneCellRef, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Places `def` on the free lot nearest the middle of the lattice.
fn place_central(sim: &mut SimRunner, db: &ContentDb, def: &str) -> bool {
    let def = db.building_id(def).unwrap();
    let mut spots: Vec<Vec2> = {
        let zones = sim.world().resource::<ZoneGrid>();
        zones
            .blocks()
            .flat_map(|(block, _)| (0..200u16).map(move |cell| ZoneCellRef { block, cell }))
            .filter_map(|r| zones.cell_footprint(r).map(|o| o.c))
            .collect()
    };
    let mid = spots.iter().fold(Vec2::ZERO, |a, p| a + *p) / spots.len().max(1) as f32;
    spots.sort_by(|a, b| a.distance(mid).total_cmp(&b.distance(mid)));
    for pos in spots {
        sim.push_command(
            SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 }),
            sim.tick(),
        );
        sim.apply_due_commands();
        let out: Vec<CommandOutcome> = sim
            .world_mut()
            .resource_mut::<Messages<CommandOutcome>>()
            .drain()
            .collect();
        if out.last().is_some_and(|o| o.result.is_ok()) {
            return true;
        }
    }
    false
}

/// (population, abandoned, [garbage, sick, dead, fire, crime, mail] at the end, worst crime
/// problem count over the ten days).
fn town(target: usize) -> (usize, u32, [u32; 6], u32) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let (mut sim, _) = grid_city(
        target,
        1,
        terrain,
        Arc::clone(&db),
        &GridCityConfig::default(),
    );
    sim.world_mut().resource_mut::<Treasury>().balance = Cost(50_000_000_000);
    for def in ["police_station", "fire_station", "landfill"] {
        assert!(place_central(&mut sim, &db, def), "no lot for {def}");
    }
    let mut worst_crime = 0;
    for _ in 0..10 {
        for _ in 0..TICKS_PER_DAY {
            sim.tick_once();
        }
        let s = sim.world().resource::<ServiceState>().summary();
        worst_crime = worst_crime.max(s.counts[4]);
    }
    // Income per day of the first full week, to see what a town earns (T-282).
    for _ in 0..4 * TICKS_PER_DAY {
        sim.tick_once();
    }
    let week = sim.world().resource::<cp_economy::EconomyState>().last_week;
    let per_day = |c: Cost| c.0 as f64 / 100.0 / 7.0;
    println!(
        "  income/day {:.0} upkeep/day {:.0} -> {:.2} income per resident per day",
        per_day(week.income()),
        per_day(week.upkeep()),
        per_day(week.income()) / population(&sim).max(1) as f64
    );
    let abandoned = sim.view().city.buildings.abandoned;
    let counts = sim.world().resource::<ServiceState>().summary().counts;
    let _ = sim.world().resource::<BuildingGrid>();
    (population(&sim), abandoned, counts, worst_crime)
}

#[test]
fn one_police_fire_and_garbage_service_keep_a_town_out_of_trouble() {
    for target in [700, 4000] {
        let (pop, abandoned, counts, worst_crime) = town(target);
        println!(
            "target {target}: pop {pop} abandoned {abandoned} problems [garbage sick dead fire crime mail] {counts:?} worst crime over 10 days {worst_crime}"
        );
        assert_eq!(abandoned, 0, "town {target}");
        assert!(
            counts[4] * 20 <= pop as u32 && counts[3] * 20 <= pop as u32,
            "town {target}: crime/fire out of hand"
        );
    }
}
