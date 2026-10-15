//! BUG-031: a `grid_city` town has water, sewage and power. Its sources are sized for the
//! lattice and its pipes and power lines are one network, so after 14 game days nothing is
//! abandoned and nobody has left (before the fix 203 of 204 buildings lacked water and the
//! town emptied after 7 days).

use std::path::Path;
use std::sync::Arc;

use cp_building::{BuildingFlags, BuildingGrid, BuildingState};
use cp_content::ContentDb;
use cp_core::time::TICKS_PER_DAY;
use cp_sim::scenario::{GridCityConfig, grid_city, population};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn check(target: usize) {
    let root = Path::new(ROOT).join("../..");
    let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
    let content = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
    let (mut sim, report) = grid_city(target, 1, terrain, content, &GridCityConfig::default());
    assert!(report.reached && report.rejected.is_empty(), "{report:?}");
    let start = population(&sim);
    for _ in 0..14 * TICKS_PER_DAY {
        sim.tick_once();
    }
    let g = sim.world().resource::<BuildingGrid>();
    let supply = BuildingFlags::NO_WATER | BuildingFlags::NO_SEWAGE | BuildingFlags::NO_POWER;
    let (mut total, mut fine) = (0usize, 0usize);
    for (_, b) in g.iter() {
        total += 1;
        fine += usize::from(b.state == BuildingState::Active && b.flags.0 & supply == 0);
    }
    assert!(
        fine as f64 >= 0.95 * total as f64,
        "{fine} of {total} buildings are active with water, sewage and power"
    );
    let now = population(&sim);
    assert!(
        now as f64 >= 0.9 * start as f64,
        "{start} residents at the start, {now} after 14 days"
    );
}

#[test]
fn a_small_grid_city_keeps_its_supply_and_its_people_for_two_weeks() {
    check(700);
}

#[test]
#[ignore = "grid_city 10k: run in release (cargo test -p cp_sim --release --test grid_city_supply_e2e -- --ignored)"]
fn the_10k_grid_city_keeps_its_supply_and_its_people_for_two_weeks() {
    check(10_000);
}
