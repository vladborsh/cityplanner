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

// --- demand ---

mod demand {
    use bevy_ecs::schedule::Schedule;
    use bevy_ecs::world::World;
    use cp_building::{BuildingGrid, BuildingState, DEMAND_PERIOD, Demand};
    use cp_citizen::CitizenGrid;
    use cp_content::{Balance, SimContent};
    use cp_core::Tick;

    use super::*;
    use crate::demand::{DemandAcc, smooth, update_demand};

    fn raw(acc: DemandAcc) -> [f32; 5] {
        acc.raw(&Balance::default())
    }

    /// Zero-based sums of a city with nothing in it: only the base values are wanted.
    #[test]
    fn an_empty_city_wants_its_base_values() {
        assert_eq!(raw(DemandAcc::default()), [10.0, 5.0, 5.0, 0.0, 10.0]);
    }

    #[test]
    fn surplus_jobs_raise_r_and_surplus_housing_lowers_it() {
        let base = raw(DemandAcc::default())[0];
        let jobs = raw(DemandAcc {
            free_jobs_weighted: 40.0,
            ..DemandAcc::default()
        });
        assert_eq!(jobs[0], base + 40.0);
        assert_eq!(jobs[4], base + 40.0, "move-in follows the jobs too");
        // People without work lower it ...
        let jobless = raw(DemandAcc {
            unemployed: 30,
            ..DemandAcc::default()
        });
        assert_eq!(jobless[0], base - 30.0);
        // ... and so do vacant homes (2 adults each), which do not hold back moving in.
        let vacant = raw(DemandAcc {
            vacant_homes: 10,
            ..DemandAcc::default()
        });
        assert_eq!(vacant[0], base - 20.0);
        assert_eq!(vacant[4], base);
    }

    #[test]
    fn a_shortage_of_goods_raises_c_and_i() {
        let base = raw(DemandAcc::default());
        // 100 residents support 20 shop jobs; there are none.
        let no_shops = raw(DemandAcc {
            citizens: 100,
            ..DemandAcc::default()
        });
        assert_eq!(no_shops[1], base[1] + 20.0);
        // With 15 of them the want is only the remaining 5.
        let some_shops = raw(DemandAcc {
            citizens: 100,
            commercial_jobs: 15,
            ..DemandAcc::default()
        });
        assert_eq!(some_shops[1], base[1] + 5.0);
        // Shops need goods: 100 shop jobs want 50 industrial jobs.
        let no_industry = raw(DemandAcc {
            commercial_jobs: 100,
            ..DemandAcc::default()
        });
        assert_eq!(no_industry[2], base[2] + 50.0);
        // Enough industry for them: back to the base.
        let supplied = raw(DemandAcc {
            commercial_jobs: 100,
            industrial_jobs: 50,
            ..DemandAcc::default()
        });
        assert_eq!(supplied[2], base[2]);
        // Jobless manual workers want industry; educated ones want offices.
        let workers = raw(DemandAcc {
            unemployed_manual: 7,
            unemployed_educated: 9,
            ..DemandAcc::default()
        });
        assert_eq!(workers[2], base[2] + 7.0);
        assert_eq!(workers[3], base[3] + 9.0);
        // A vacant office slot absorbs them.
        let offices = raw(DemandAcc {
            unemployed_educated: 9,
            free_office_jobs: 12,
            ..DemandAcc::default()
        });
        assert_eq!(offices[3], base[3] - 3.0);
    }

    #[test]
    fn raw_values_are_clamped() {
        let r = raw(DemandAcc {
            free_jobs_weighted: 1.0e6,
            unemployed: 0,
            citizens: 1_000_000,
            ..DemandAcc::default()
        });
        assert_eq!((r[0], r[1]), (100.0, 100.0));
        let r = raw(DemandAcc {
            vacant_homes: 1_000_000,
            ..DemandAcc::default()
        });
        assert_eq!(r[0], -100.0);
    }

    #[test]
    fn smoothing_closes_a_share_of_the_gap() {
        assert_eq!(smooth(0.0, 8.0, 0.25), 2.0);
        assert_eq!(smooth(10.0, -10.0, 0.5), 0.0);
        assert_eq!(smooth(3.0, 9.0, 0.0), 3.0);
        assert_eq!(smooth(3.0, 9.0, 1.0), 9.0);
        assert_eq!(smooth(3.0, 9.0, 7.0), 9.0, "the share is capped at 1");
    }

    fn world() -> (World, Schedule) {
        let mut w = World::new();
        w.insert_resource(Tick(0));
        w.insert_resource(SimContent(content()));
        w.init_resource::<BuildingGrid>();
        w.init_resource::<CitizenGrid>();
        w.init_resource::<Demand>();
        w.init_resource::<DemandAcc>();
        let mut s = Schedule::default();
        s.add_systems(update_demand);
        (w, s)
    }

    fn run_ticks(w: &mut World, s: &mut Schedule, from: u64, to: u64) {
        for t in from..to {
            w.insert_resource(Tick(t));
            s.run(w);
        }
    }

    /// A shop (4 job slots, none filled, standing) in an empty city: R and move-in get the
    /// 4 free slots, C is the base minus the 4 shop jobs.
    #[test]
    fn the_cycle_publishes_after_the_64th_bucket_and_not_before() {
        let (mut w, mut s) = world();
        let db = content();
        super::add(&mut w, &db, "shop", BuildingState::Active);
        run_ticks(&mut w, &mut s, 0, DEMAND_PERIOD - 1);
        assert_eq!(*w.resource::<Demand>(), Demand::default(), "mid-cycle");
        run_ticks(&mut w, &mut s, DEMAND_PERIOD - 1, DEMAND_PERIOD);
        let d = *w.resource::<Demand>();
        let acc = *w.resource::<DemandAcc>();
        assert_eq!((acc.seen, acc.commercial_jobs), (64, 4));
        assert_eq!(acc.free_jobs_weighted, 4.0);
        // EMA at 0.25 from the defaults (R 10, C 5) to the raw values (14, 1).
        assert_eq!(d.r, smooth(10.0, 14.0, 0.25));
        assert_eq!(d.c, smooth(5.0, 1.0, 0.25));
        assert_eq!(d.move_in, smooth(10.0, 14.0, 0.25));
    }

    #[test]
    fn a_cycle_not_scanned_from_its_start_is_dropped() {
        let (mut w, mut s) = world();
        let db = content();
        super::add(&mut w, &db, "shop", BuildingState::Active);
        // Joined at bucket 20 (e.g. a save from before demand existed): the first cycle's
        // sums are partial, so nothing is published at its end ...
        run_ticks(&mut w, &mut s, 20, DEMAND_PERIOD);
        assert_eq!(*w.resource::<Demand>(), Demand::default());
        // ... and the next full cycle is.
        run_ticks(&mut w, &mut s, DEMAND_PERIOD, 2 * DEMAND_PERIOD);
        assert_ne!(*w.resource::<Demand>(), Demand::default());
    }

    #[test]
    fn a_frozen_demand_is_left_alone() {
        let (mut w, mut s) = world();
        *w.resource_mut::<Demand>() = Demand::fixed(3.0);
        let db = content();
        super::add(&mut w, &db, "shop", BuildingState::Active);
        run_ticks(&mut w, &mut s, 0, 3 * DEMAND_PERIOD);
        assert_eq!(*w.resource::<Demand>(), Demand::fixed(3.0));
    }

    #[test]
    fn buildings_under_construction_count_as_standing() {
        let (mut w, mut s) = world();
        let db = content();
        super::add(&mut w, &db, "house", BuildingState::UnderConstruction);
        run_ticks(&mut w, &mut s, 0, DEMAND_PERIOD);
        // 4 vacant flats: 8 adults against R, but not against moving in.
        let acc = *w.resource::<DemandAcc>();
        assert_eq!(acc.vacant_homes, 4);
        let r = acc.raw(db.balance());
        assert_eq!((r[0], r[4]), (2.0, 10.0));
    }

    /// Cycles of the system on a world; returns the demand after `cycles` full cycles.
    fn after(w: &mut World, s: &mut Schedule, cycles: u64) -> Demand {
        run_ticks(w, s, 0, cycles * DEMAND_PERIOD);
        *w.resource::<Demand>()
    }

    #[test]
    fn a_city_with_free_jobs_wants_more_homes_and_one_full_of_jobless_does_not() {
        let db = content();
        // Ten shops, nobody lives there: 40 free jobs.
        let (mut jobs, mut s) = world();
        for _ in 0..10 {
            super::add(&mut jobs, &db, "shop", BuildingState::Active);
        }
        let d = after(&mut jobs, &mut s, 40);
        assert!(d.r > 40.0 && d.move_in > 40.0, "{d:?}");

        // Five full houses and no work: 20 households of jobless adults.
        let (mut homes, mut s) = world();
        let houses: Vec<_> = (0..5)
            .map(|_| super::add(&mut homes, &db, "house", BuildingState::Active))
            .collect();
        {
            let mut c = homes.resource_mut::<CitizenGrid>();
            c.ensure_derived();
            for h in houses {
                for _ in 0..4 {
                    c.add_household(h, 0, Tick(0), &[(30 * 365, 0, false)]);
                }
            }
        }
        let d = after(&mut homes, &mut s, 40);
        assert!(d.r < 0.0 && d.move_in < 0.0, "{d:?}");
        // Shops for 20 residents: 4 shop jobs wanted, none there.
        assert!(d.c > 5.0, "{d:?}");
        // The jobless are manual workers: industry is wanted.
        assert!(d.i > 5.0, "{d:?}");
    }

    #[test]
    fn spawn_gates_follow_the_zone_family() {
        let d = Demand {
            r: 1.0,
            c: 2.0,
            i: 3.0,
            o: 4.0,
            ..Demand::default()
        };
        assert_eq!(d.get(ZoneType::ResLow), 1.0);
        assert_eq!(d.get(ZoneType::ResHigh), 1.0);
        assert_eq!(d.get(ZoneType::ComLow), 2.0);
        assert_eq!(d.get(ZoneType::ComHigh), 2.0);
        assert_eq!(d.get(ZoneType::Industrial), 3.0);
        assert_eq!(d.get(ZoneType::Office), 4.0);
        assert_eq!(d.get(ZoneType::None), 0.0);
    }
}
