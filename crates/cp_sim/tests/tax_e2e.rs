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
            budgets: w.resource::<cp_content::Budgets>(),
            district_map: w.resource::<cp_district::DistrictMap>(),
            districts: w.resource::<cp_district::Districts>(),
            city_policies: w.resource::<cp_district::CityPolicies>(),
            tax_collection: 1.0,
            institution_upkeep: 1.0,
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

#[test]
fn a_painted_district_reports_its_residents_and_tax() {
    use cp_district::{DistrictCommand, DistrictId};
    let (mut sim, _) = city(700);
    // Cover the whole lattice area with one district.
    let cells: Vec<(u16, u16)> = (0..320u16)
        .flat_map(|c| (0..320u16).map(move |r| (c, r)))
        .filter(|&(c, r)| (80..240).contains(&c) && (80..240).contains(&r))
        .collect();
    sim.push_command(
        cp_sim::SimCommand::District(DistrictCommand::Create { name: "All".into() }),
        sim.tick(),
    );
    sim.tick_once();
    for chunk in cells.chunks(4096) {
        let cmd = DistrictCommand::Paint {
            cells: chunk.to_vec(),
            district: DistrictId(1),
        };
        sim.push_command(cp_sim::SimCommand::District(cmd), sim.tick());
    }
    // Past the first week boundary, at the publish tick of the next summary. (This scenario
    // has no water or sewage, so its buildings are abandoned a few thousand ticks later.)
    while sim.tick().0 < cp_core::time::TICKS_PER_WEEK + 6
        || sim.tick().0 % cp_sim::stats::STATS_PERIOD != 5
    {
        sim.tick_once();
    }
    let v = sim.view();
    let d = &v.city.districts;
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].population >= 700 && d[0].buildings > 0,
        "{d:?} city {} real {} tick {}",
        v.city.population,
        sim.world()
            .resource::<cp_citizen::CitizenGrid>()
            .citizen_count(),
        sim.tick().0
    );
    assert_eq!(d[0].income_last_week, v.economy.last_week.income());
}

/// T-187: taxes are worth it. A mid town earns clearly more than its upkeep, but not by a
/// factor that makes money meaningless (measured ~15x at T-282, taxes x5).
#[test]
// T-295: taxes x1.6 (7.9 measured), band 5..12. T-282: taxes x5 (the 700 town earns a basic service in ~2 days), and the basic services cost half the upkeep: the ratio was 2..12 and is 15..30 (23 measured).
fn a_mid_town_earns_between_five_and_twelve_times_its_upkeep() {
    let (mut sim, pop) = city(1_000);
    assert!(pop >= 1_000);
    // Half the arrival rate (MOVE_IN_ONE_IN 4): when the town reaches its population many
    // households are still on the way in and pay nothing yet (4.2 measured). A day later they
    // live there (8.0 measured).
    for _ in 0..cp_core::time::TICKS_PER_DAY {
        sim.tick_once();
    }
    let (income, upkeep) = week(&mut sim, 9);
    let ratio = income.0 as f64 / upkeep.0 as f64;
    assert!((5.0..12.0).contains(&ratio), "{ratio:.2}");
}

/// §20.6 item 6: the upkeep lines are linear in the budget: roads by the Roads budget, pipes
/// and power lines by Water and Power, each service's buildings by its own (`Flow.services`).
#[test]
fn upkeep_lines_are_linear_in_the_budgets() {
    use cp_content::{Budgets, ServiceKind, budget::BUDGETED};
    let (mut sim, _) = city(1_000);
    let lines = |sim: &mut SimRunner, pct: u8| {
        let w = sim.world_mut();
        let mut b = Budgets::default();
        for k in BUDGETED {
            b.set(k, pct);
        }
        w.insert_resource(b);
        w.resource_mut::<cp_citizen::CitizenGrid>().ensure_derived();
        let content = Arc::clone(&w.resource::<SimContent>().0);
        let (r, _, services) = cp_economy::week::week_lines(
            &WeekInputs {
                content: &content,
                net: w.resource::<NetGraph>(),
                buildings: w.resource::<cp_building::BuildingGrid>(),
                citizens: w.resource::<cp_citizen::CitizenGrid>(),
                utilities: w.resource::<cp_utility::UtilityGraph>(),
                state: w.resource::<EconomyState>(),
                budgets: w.resource::<Budgets>(),
                district_map: w.resource::<cp_district::DistrictMap>(),
                districts: w.resource::<cp_district::Districts>(),
                city_policies: w.resource::<cp_district::CityPolicies>(),
                tax_collection: 1.0,
                institution_upkeep: 1.0,
            },
            1,
        );
        (r, services)
    };
    let (full, full_s) = lines(&mut sim, 100);
    assert!(full.road_upkeep > Cost::ZERO && full.utility_upkeep > Cost::ZERO);
    for pct in [50u8, 150] {
        let (r, s) = lines(&mut sim, pct);
        let k = f64::from(pct) / 100.0;
        let near = |got: Cost, want: Cost, what: &str| {
            let (g, w) = (got.0 as f64, want.0 as f64 * k);
            // Each item is rounded to the cent on its own.
            assert!(
                (g - w).abs() <= 1.0 + w.abs() * 5e-3,
                "{what} at {pct} %: {g} vs {w}"
            );
        };
        near(r.road_upkeep, full.road_upkeep, "roads");
        near(r.utility_upkeep, full.utility_upkeep, "utilities");
        near(r.building_upkeep, full.building_upkeep, "buildings");
        for kind in [ServiceKind::Water, ServiceKind::Power, ServiceKind::Health] {
            near(
                s[kind as usize],
                full_s[kind as usize],
                &format!("{kind:?}"),
            );
        }
        // The taxes do not move.
        assert_eq!(r.income(), full.income());
    }
}
