//! T-128 acceptance: tax is paid by occupied places (households by building level, filled
//! jobs by education), so a grown city is in the black. The weekly report is a pure function
//! of the sim state, so the test builds `grid_city` and reads reports at different rates.

use std::path::Path;
use std::sync::Arc;

use cp_content::{ContentDb, SimContent};
use cp_core::Cost;
use cp_economy::{EconomyState, WeekInputs, state::TAXED_ZONES, week_report};
use cp_net::NetGraph;
use cp_sim::SimRunner;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn city(pop: usize) -> (SimRunner, usize) {
    let root = Path::new(ROOT).join("../..");
    let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
    let content = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
    let (sim, r) = grid_city(pop, 1, terrain, content, &GridCityConfig::default());
    assert!(r.reached, "{r:?}");
    (sim, r.population)
}

/// `(tax income, upkeep)` of one week with every zone at `pct`.
fn week(sim: &mut SimRunner, pct: u8) -> (Cost, Cost) {
    let w = sim.world_mut();
    for z in TAXED_ZONES {
        w.resource_mut::<EconomyState>().taxes.set(z, pct);
    }
    w.resource_mut::<cp_citizen::CitizenGrid>().ensure_derived();
    let content = Arc::clone(&w.resource::<SimContent>().0);
    let r = week_report(
        &WeekInputs {
            content: &content,
            net: w.resource::<NetGraph>(),
            buildings: w.resource::<cp_building::BuildingGrid>(),
            citizens: w.resource::<cp_citizen::CitizenGrid>(),
            utilities: w.resource::<cp_utility::UtilityGraph>(),
            state: w.resource::<EconomyState>(),
        },
        1,
    );
    (
        r.income(),
        r.road_upkeep + r.building_upkeep + r.utility_upkeep,
    )
}

#[test]
fn a_city_of_a_thousand_pays_for_itself_at_nine_percent() {
    let (mut sim, pop) = city(1_000);
    assert!(pop >= 1_000);
    let (income, upkeep) = week(&mut sim, 9);
    assert!(
        income.0 as f64 >= 1.2 * upkeep.0 as f64,
        "income {income:?} upkeep {upkeep:?}"
    );
    let (higher, _) = week(&mut sim, 12);
    assert!(higher > income);
}

#[test]
fn a_city_of_seven_hundred_breaks_even_at_nine_percent() {
    let (mut sim, pop) = city(700);
    assert!(pop >= 700);
    let (income, upkeep) = week(&mut sim, 9);
    assert!(income >= upkeep, "income {income:?} upkeep {upkeep:?}");
}
