//! T-168: tourists. A city with shops and an entry highway gets visits: cars appear at the
//! `Outside` node, drive to a shop, stay, and drive out again; every arrival is one
//! `tourism_income` in the ledger and so in the week report. Without the highway there are none.

use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingState, Footprint, Lot,
};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_WEEK;
use cp_core::{Cost, Id, Tick};
use cp_economy::EconomyState;
use cp_net::{Elevation, NetGraph, NodeKind};
use cp_sim::SimRunner;
use cp_traffic::{Agents, ServiceRuns};
use cp_transit::{TouristPhase, Tourists, TransitLedger};

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

/// One visit brings `tourism_income` (12.00 by default), in cents.
const VISIT: Cost = Cost(1200);

fn content() -> Arc<ContentDb> {
    content_with(Vec::new())
}

/// [`content`] and `extra` sources (a `Balance` with other knobs).
fn content_with(extra: Vec<(String, String)>) -> Arc<ContentDb> {
    Arc::new(
        ContentDb::from_sources(
            extra
                .into_iter()
                .chain([
                    ("r/street.ron".into(), ROAD.into()),
                    ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
                    (
                        "b/shop.ron".into(),
                        r#"Building((id: "shop", name: "shop", zone: ComLow, level: 1, lot: (1, 1),
                styles: ["p"], service: None))"#
                            .into(),
                    ),
                    cp_content::test_policy_source("TourismPromotion", 0.25),
                    (
                        "v/car.ron".into(),
                        r#"Vehicle((id: "car", name: "car", kind: Car, length: 4.5, width: 1.8,
                max_speed_kmh: 60.0, accel: 2.0, decel: 4.0, min_gap: 2.0, mesh: "m"))"#
                            .into(),
                    ),
                ])
                .collect(),
        )
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

/// A 1 km street with ten shops along it; its west end is an `Outside` node if `highway`.
fn town(highway: bool) -> SimRunner {
    town_with(highway, content())
}

/// [`town`] with the content `db`.
fn town_with(highway: bool, db: Arc<ContentDb>) -> SimRunner {
    let mut sim = SimRunner::new(7);
    sim.set_content(Arc::clone(&db));
    let road = db.road_id("street").unwrap();
    let def = db.road(road).clone();
    if highway {
        sim.world_mut().resource_mut::<NetGraph>().add_node(
            Vec3::ZERO,
            NodeKind::Outside,
            Elevation::Ground,
        );
    }
    let seg = sim
        .world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(Vec3::ZERO, Vec3::new(1000.0, 0.0, 0.0)),
            Elevation::Ground,
        )
        .unwrap();
    for i in 0..10 {
        let at = 150.0 + 80.0 * i as f32;
        sim.world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(Building {
                def: db.building_id("shop").unwrap(),
                footprint: Footprint::Lot(Lot {
                    block: Id::new(0, 0),
                    col: 0,
                    w: 1,
                    d: 1,
                }),
                level: 1,
                state: BuildingState::Active,
                ready_at: Tick(0),
                pos: Vec2::new(at, 5.0),
                rot: 0.0,
                access: Some(AccessPoint {
                    segment: seg,
                    t: at / 1000.0,
                }),
                flags: BuildingFlags(0),
                level_progress: 0.0,
            });
    }
    sim.tick_once();
    sim
}

fn customers(sim: &SimRunner) -> u32 {
    sim.world().resource::<Tourists>().customers.values().sum()
}

#[test]
fn cars_come_from_the_outside_node_visit_a_shop_and_drive_out_again() {
    let mut sim = town(true);
    let mut left = Vec::new();
    let mut max_cars = 0;
    for _ in 0..16_000 {
        sim.tick_once();
        let w = sim.world();
        let t = w.resource::<Tourists>();
        max_cars = max_cars.max(w.resource::<Agents>().vehicle_count());
        for (&id, v) in &t.visits {
            if v.phase == TouristPhase::Leaving && !left.contains(&id) {
                left.push(id);
            }
        }
        if left.iter().any(|id| !t.visits.contains_key(id)) {
            break;
        }
    }
    assert!(customers(&sim) > 0, "tourists visited");
    assert!(max_cars > 0, "their cars were on the road");
    let w = sim.world();
    let gone = left
        .iter()
        .find(|id| !w.resource::<Tourists>().visits.contains_key(id))
        .expect("a car that left reached the Outside node and was taken off the road");
    assert!(w.resource::<ServiceRuns>().get(*gone).is_none());
    // Nothing is lost: a record per run, a run per record (the shops here have no factory, so
    // trucks come in from the Outside node too, T-170).
    assert_eq!(
        w.resource::<Tourists>().visits.len() + w.resource::<cp_transit::Goods>().hauls.len(),
        w.resource::<ServiceRuns>().len()
    );
}

#[test]
fn the_week_report_holds_one_income_per_visit() {
    let mut sim = town(true);
    while sim.tick().0 <= TICKS_PER_WEEK + 2 {
        sim.tick_once();
    }
    let w = sim.world();
    let report = w.resource::<EconomyState>().last_week;
    assert_eq!(report.week, 1);
    assert!(report.tourism > Cost::ZERO, "tourism earned in the week");
    // The week's money plus what has been booked since is every visit that ever arrived.
    let since = w.resource::<TransitLedger>().tourism;
    assert_eq!(
        report.tourism + since,
        Cost(VISIT.0 * i64::from(customers(&sim)))
    );
    assert_eq!(report.tourism.0 % VISIT.0, 0);
}

#[test]
fn without_an_entry_highway_nobody_comes() {
    let mut sim = town(false);
    for _ in 0..9_000 {
        sim.tick_once();
    }
    let w = sim.world();
    assert!(w.resource::<Tourists>().is_empty());
    assert_eq!(w.resource::<TransitLedger>().tourism, Cost::ZERO);
}

#[test]
fn the_same_city_gets_the_same_tourists() {
    let run = || {
        let mut sim = town(true);
        for _ in 0..6_000 {
            sim.tick_once();
        }
        (sim.checksum(), customers(&sim))
    };
    let (a, na) = run();
    let (b, nb) = run();
    assert!(na > 0);
    assert_eq!((a, na), (b, nb));
}

#[test]
fn the_view_counts_the_tourists_on_the_map() {
    let mut sim = town(true);
    let mut seen = 0;
    for _ in 0..16_000 {
        sim.tick_once();
        let f = sim.view().freight;
        assert_eq!(
            f.tourists_now as usize,
            sim.world().resource::<Tourists>().visits.len()
        );
        seen = seen.max(f.tourists_week);
        if seen > 0 {
            break;
        }
    }
    assert!(seen > 0, "visits arrived and the view counts them");
}

#[test]
fn tourism_promotion_brings_a_quarter_more_visits_in_the_same_days() {
    use cp_content::PolicyKind;
    use cp_district::CityPolicies;
    let days = |on: bool| {
        let mut sim = town(true);
        if on {
            sim.world_mut().resource_mut::<CityPolicies>().mask =
                PolicyKind::TourismPromotion.bit();
        }
        for _ in 0..4 * cp_core::time::TICKS_PER_DAY {
            sim.tick_once();
        }
        // Arrivals so far: those who have been served plus those standing at a shop.
        sim.world().resource::<Tourists>().visits_week
    };
    let (plain, promoted) = (days(false), days(true));
    assert!(plain >= 40, "{plain} visits without the policy");
    // Expect 1.25 ×; the flow is random but the seed is fixed.
    assert!(
        promoted * 100 >= plain * 115 && promoted * 100 <= plain * 140,
        "{plain} visits, {promoted} promoted"
    );
}

/// BUG-101: with 15 % of the tourists in cars (the rest travel by their route's time) the city
/// gets about as many visits and as much money in the same days as with every tourist driving.
#[test]
fn hidden_tourists_still_visit_and_pay_about_as_much() {
    let run = |share: f32| {
        let balance = format!("Balance((tourist_visible_share: {share:?}))");
        let mut sim = town_with(true, content_with(vec![("balance.ron".into(), balance)]));
        let mut most_cars = 0;
        for _ in 0..4 * cp_core::time::TICKS_PER_DAY {
            sim.tick_once();
            most_cars = most_cars.max(sim.world().resource::<Agents>().vehicle_count());
        }
        let w = sim.world();
        (
            w.resource::<Tourists>().visits_week,
            w.resource::<TransitLedger>().tourism,
            most_cars,
        )
    };
    let (all, all_money, all_cars) = run(1.0);
    let (few, few_money, few_cars) = run(0.15);
    assert!(all >= 40, "{all} visits");
    assert!(
        few * 100 >= all * 80 && few * 100 <= all * 120,
        "{all} visits all driving, {few} with 15 % driving"
    );
    assert!(
        few_money.0 * 100 >= all_money.0 * 80 && few_money.0 * 100 <= all_money.0 * 120,
        "{all_money:?} vs {few_money:?}"
    );
    assert!(
        few_cars * 2 < all_cars,
        "fewer tourist cars on the road: {all_cars} vs {few_cars}"
    );
}
