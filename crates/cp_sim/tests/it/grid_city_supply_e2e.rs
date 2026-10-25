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

/// The game content with the garbage and dead no longer counting towards abandonment: these
/// towns have no landfill, and this test is about the supply (T-146).
fn content_without_service_decay(dir: &Path) -> ContentDb {
    fn walk(dir: &Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "ron") {
                let mut text = std::fs::read_to_string(&path).unwrap();
                if path.file_name().is_some_and(|n| n == "balance.ron") {
                    assert!(text.contains("decay_service_mask: 5,"));
                    text = text.replace("decay_service_mask: 5,", "decay_service_mask: 0,");
                }
                out.push((path.display().to_string(), text));
            }
        }
    }
    let mut sources = Vec::new();
    walk(dir, &mut sources);
    ContentDb::from_sources(sources).unwrap()
}

fn check(target: usize) {
    let root = Path::new(ROOT).join("../..");
    let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
    let content = Arc::new(content_without_service_decay(&root.join("assets/content")));
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
