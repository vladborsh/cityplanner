//! The weekly settlement against a hand calculation, the cadence and the commands.

use std::sync::Arc;

use bevy_ecs::schedule::Schedule;
use bevy_ecs::world::World;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Lot,
};
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, ServiceKind, SimContent, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_WEEK;
use cp_core::{Cost, Id, Tick, Treasury};
use cp_net::{Elevation, NetGraph};
use cp_utility::{UtilityGraph, UtilityNet};

use super::*;

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

fn building(id: &str, zone: &str, extra: &str, service: &str) -> (String, String) {
    (
        format!("b/{id}.ron"),
        format!(
            r#"Building((id: "{id}", name: "{id}", zone: {zone}, level: 1, lot: (1, 1), {extra}
            style: (floors: 1, roof: Flat, palette: "p"), service: {service}))"#
        ),
    )
}

fn utility(id: &str, kind: &str, upkeep: f32) -> (String, String) {
    (
        format!("u/{id}.ron"),
        format!(
            r#"Utility((id: "{id}", name: "{id}", kind: {kind}, cost_per_m: 1.0,
            upkeep_per_m_week: {upkeep}, min_link_m: 1.0, max_link_m: 200.0, mesh: "m"))"#
        ),
    )
}

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            building("house", "ResLow", "households: 4, tax_base: 40.0,", "None"),
            building(
                "shop",
                "ComLow",
                "jobs: (4, 0, 0, 0), tax_base: 30.0,",
                "None",
            ),
            building(
                "pump",
                "Industrial",
                "tax_base: 0.0, cost: 12000.0, upkeep_week: 300.0,",
                "Some(\"pump\")",
            ),
            (
                "s/pump.ron".into(),
                r#"Service((id: "pump", name: "pump", kind: Water, radius_m: 100.0,
                capacity: 10, site: OnWater, vehicles: []))"#
                    .into(),
            ),
            utility("water_pipe", "Water", 0.02),
            utility("power_line", "Power", 0.03),
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

fn add(world: &mut World, db: &ContentDb, def: &str, state: BuildingState) -> BuildingId {
    world.resource_mut::<BuildingGrid>().insert(Building {
        def: db.building_id(def).unwrap(),
        lot: Lot {
            block: Id::new(0, 0),
            col: 0,
            w: 1,
            d: 1,
        },
        level: 1,
        state,
        ready_at: Tick(0),
        pos: Vec2::new(5000.0, 5000.0),
        rot: 0.0,
        access: AccessPoint {
            segment: Id::new(0, 0),
            t: 0.0,
        },
        flags: BuildingFlags::default(),
        level_progress: 0.0,
    })
}

/// The fixed city of the hand calculation below.
fn city() -> (World, Schedule) {
    let db = content();
    let mut world = World::new();
    world.insert_resource(Tick(0));
    world.insert_resource(SimContent(Arc::clone(&db)));
    world.insert_resource(Treasury::default());
    world.insert_resource(EconomyState::default());
    world.init_resource::<NetGraph>();
    world.init_resource::<BuildingGrid>();
    world.init_resource::<CitizenGrid>();
    world.init_resource::<UtilityGraph>();

    // Two straight streets, 100 m and 60 m.
    let road = db.road_id("street").unwrap();
    let def = db.road(road).clone();
    for (a, b) in [((0.0, 0.0), (100.0, 0.0)), ((0.0, 50.0), (60.0, 50.0))] {
        world
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1)),
                Elevation::Ground,
            )
            .unwrap();
    }
    // A 50 m pipe and a 40 m power line.
    {
        let mut u = world.resource_mut::<UtilityGraph>();
        for (net, len) in [(UtilityNet::Pipe, 50.0), (UtilityNet::Power, 40.0)] {
            let a = u.add_node(net, Vec3::new(0.0, 0.0, 100.0));
            let b = u.add_node(net, Vec3::new(len, 0.0, 100.0));
            u.add_link(
                a,
                b,
                CubicBezier3::line(Vec3::new(0.0, 0.0, 100.0), Vec3::new(len, 0.0, 100.0)),
            )
            .unwrap();
        }
    }
    // Two houses (3 and 1 of 4 flats let), an empty one, a house still under construction
    // (pays nothing), a shop with 2 of 4 jobs filled, and a pump.
    let h1 = add(&mut world, &db, "house", BuildingState::Active);
    let h2 = add(&mut world, &db, "house", BuildingState::Active);
    add(&mut world, &db, "house", BuildingState::Active);
    add(&mut world, &db, "house", BuildingState::UnderConstruction);
    let shop = add(&mut world, &db, "shop", BuildingState::Active);
    add(&mut world, &db, "pump", BuildingState::Active);
    {
        let mut c = world.resource_mut::<CitizenGrid>();
        c.ensure_derived();
        let adult = (30 * 365, 0, false);
        for _ in 0..3 {
            c.add_household(h1, 0, Tick(0), &[adult]);
        }
        c.add_household(h2, 0, Tick(0), &[adult]);
        let workers: Vec<_> = c.citizens().map(|(id, _)| id).take(2).collect();
        for w in workers {
            c.set_job(w, shop, 0);
        }
    }
    let mut schedule = Schedule::default();
    schedule.add_systems(settle_week);
    (world, schedule)
}

fn run_at(world: &mut World, schedule: &mut Schedule, tick: u64) {
    world.insert_resource(Tick(tick));
    schedule.run(world);
}

fn balance(world: &World) -> Cost {
    world.resource::<Treasury>().balance
}

/// Hand calculation, all in currency units (the test compares cents).
///
/// **Week 1**, default settings (taxes 9%, budgets 100%):
/// * tax: house 1 has 3 of 4 flats let: 40 × 0.09 × 3/4 = 2.70; house 2: 40 × 0.09 × 1/4 =
///   0.90; the empty house 0; the one under construction 0; the shop has 2 of 4 jobs:
///   30 × 0.09 × 2/4 = 1.35; the pump has `tax_base` 0. Income 4.95.
/// * roads: (100 + 60) m × 0.5 = 80.00
/// * buildings: the pump 300.00
/// * utilities: pipe 50 m × 0.02 = 1.00, power line 40 m × 0.03 = 1.20, total 2.20
/// * week: 4.95 − 80.00 − 300.00 − 2.20 = −377.25, so 70 000.00 → 69 622.75
///
/// **Week 2**, after commercial tax 12%, roads 120%, water 50%, power 150%:
/// * tax: 2.70 + 0.90 + 30 × 0.12 × 2/4 (1.80) = 5.40
/// * roads: 80.00 × 1.2 = 96.00; the pump (water) 300.00 × 0.5 = 150.00
/// * utilities: the pipe (water) 1.00 × 0.5 = 0.50, the line (power) 1.20 × 1.5 = 1.80
/// * week: 5.40 − 96.00 − 150.00 − 0.50 − 1.80 = −242.90, so 69 622.75 → 69 379.85
#[test]
fn weekly_balance_matches_the_hand_calculation() {
    let (mut world, mut schedule) = city();
    run_at(&mut world, &mut schedule, TICKS_PER_WEEK);
    let r = world.resource::<EconomyState>().last_week;
    assert_eq!(r.week, 1);
    assert_eq!(r.income(), Cost(495));
    assert_eq!(r.income_by_zone[0], Cost(360)); // ResLow
    assert_eq!(r.income_by_zone[2], Cost(135)); // ComLow
    assert_eq!(r.road_upkeep, Cost(8_000));
    assert_eq!(r.building_upkeep, Cost(30_000));
    assert_eq!(r.utility_upkeep, Cost(220));
    assert_eq!(r.balance(), Cost(-37_725));
    assert_eq!(balance(&world), Cost(6_962_275));

    for cmd in [
        EconomyCommand::SetTaxRate {
            zone: ZoneType::ComLow,
            pct: 12,
        },
        EconomyCommand::SetBudget {
            service: ServiceKind::Roads,
            pct: 120,
        },
        EconomyCommand::SetBudget {
            service: ServiceKind::Water,
            pct: 50,
        },
        EconomyCommand::SetBudget {
            service: ServiceKind::Power,
            pct: 150,
        },
    ] {
        assert_eq!(apply(&mut world, &cmd), Ok(Cost::ZERO));
    }
    run_at(&mut world, &mut schedule, 2 * TICKS_PER_WEEK);
    let r = world.resource::<EconomyState>().last_week;
    assert_eq!(r.week, 2);
    assert_eq!(r.income(), Cost(540));
    assert_eq!(r.road_upkeep, Cost(9_600));
    assert_eq!(r.building_upkeep, Cost(15_000));
    assert_eq!(r.utility_upkeep, Cost(50 + 180));
    assert_eq!(r.balance(), Cost(-24_290));
    assert_eq!(balance(&world), Cost(6_937_985));
}

#[test]
fn settles_once_per_week_boundary_and_never_at_tick_zero() {
    let (mut world, mut schedule) = city();
    for t in [0, 1, TICKS_PER_WEEK - 1] {
        run_at(&mut world, &mut schedule, t);
        assert_eq!(balance(&world), Treasury::default().balance, "tick {t}");
    }
    run_at(&mut world, &mut schedule, TICKS_PER_WEEK);
    let after_one = balance(&world);
    assert_ne!(after_one, Treasury::default().balance);
    run_at(&mut world, &mut schedule, TICKS_PER_WEEK + 1);
    assert_eq!(balance(&world), after_one);
}

#[test]
fn upkeep_may_push_the_treasury_below_the_debt_floor() {
    let (mut world, mut schedule) = city();
    world.resource_mut::<Treasury>().balance = cp_core::treasury::DEBT_FLOOR;
    run_at(&mut world, &mut schedule, TICKS_PER_WEEK);
    assert_eq!(
        balance(&world),
        Cost(cp_core::treasury::DEBT_FLOOR.0 - 37_725)
    );
}

#[test]
fn commands_validate_their_ranges_and_change_nothing_on_error() {
    let (mut world, _) = city();
    let before = *world.resource::<EconomyState>();
    for (cmd, err) in [
        (
            EconomyCommand::SetTaxRate {
                zone: ZoneType::ResLow,
                pct: 0,
            },
            EconomyError::TaxOutOfRange(0),
        ),
        (
            EconomyCommand::SetTaxRate {
                zone: ZoneType::ResLow,
                pct: 30,
            },
            EconomyError::TaxOutOfRange(30),
        ),
        (
            EconomyCommand::SetTaxRate {
                zone: ZoneType::None,
                pct: 10,
            },
            EconomyError::NoTaxForZone,
        ),
        (
            EconomyCommand::SetBudget {
                service: ServiceKind::Water,
                pct: 49,
            },
            EconomyError::BudgetOutOfRange(49),
        ),
        (
            EconomyCommand::SetBudget {
                service: ServiceKind::Water,
                pct: 151,
            },
            EconomyError::BudgetOutOfRange(151),
        ),
    ] {
        assert_eq!(apply(&mut world, &cmd), Err(err));
        assert_eq!(*world.resource::<EconomyState>(), before);
    }
    // The edges are allowed.
    for pct in [1, 29] {
        let cmd = EconomyCommand::SetTaxRate {
            zone: ZoneType::Office,
            pct,
        };
        assert_eq!(apply(&mut world, &cmd), Ok(Cost::ZERO));
        assert_eq!(
            world.resource::<EconomyState>().taxes.get(ZoneType::Office),
            Some(pct)
        );
    }
    for pct in [50, 150] {
        let cmd = EconomyCommand::SetBudget {
            service: ServiceKind::Sewage,
            pct,
        };
        assert_eq!(apply(&mut world, &cmd), Ok(Cost::ZERO));
        assert_eq!(
            world
                .resource::<EconomyState>()
                .budgets
                .get(ServiceKind::Sewage),
            pct
        );
    }
}

#[test]
fn defaults_are_nine_percent_taxes_and_full_budgets() {
    let s = EconomyState::default();
    for z in TAXED_ZONES {
        assert_eq!(s.taxes.get(z), Some(9));
    }
    assert_eq!(s.taxes.get(ZoneType::None), None);
    for k in BUDGETED {
        assert_eq!(s.budgets.get(k), 100);
    }
    assert_eq!(s.last_week, WeekReport::default());
}
