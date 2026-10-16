//! T-159: riders. A line between a home and a workplace 800 m apart carries those without a
//! car; a car owner keeps the car; deleting the line sends new trips back to walking; a full bus
//! leaves the rest waiting for the next one.

use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_citizen::{CitizenGrid, Location, Trip, TripBook, TripMode, TripPurpose, TripState};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::{Id, Tick};
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_sim::SimRunner;
use cp_traffic::Side;
use cp_transit::{
    Stage, StopRef, TransitCommand, TransitFleet, TransitLedger, TransitMode, TransitNet, apply,
};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

struct Town {
    sim: SimRunner,
    sides: Vec<SegmentId>,
    home: BuildingId,
    work: BuildingId,
    side: f32,
}

/// Copies the content with `bus_basic` holding `capacity` passengers.
fn content_with_bus_capacity(capacity: u16) -> std::path::PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let t = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &t);
            } else {
                std::fs::copy(e.path(), t).unwrap();
            }
        }
    }
    let dir = std::env::temp_dir().join(format!(
        "cp_riders_content_{capacity}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    copy(&Path::new(ROOT).join("../../assets/content"), &dir);
    let f = dir.join("vehicles/bus_basic.ron");
    let s = std::fs::read_to_string(&f).unwrap();
    std::fs::write(
        &f,
        s.replace("capacity: 60", &format!("capacity: {capacity}")),
    )
    .unwrap();
    dir
}

fn town(content_dir: &Path, side: f32) -> Town {
    let db = Arc::new(ContentDb::load_dir(content_dir).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(&db));
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    // An 800 m square: the home lies by the middle of the first side, the workplace by the
    // middle of the third.
    let c = [
        (0.0, 0.0),
        (side, 0.0),
        (side, side),
        (0.0, side),
        (0.0, 0.0),
    ];
    let sides: Vec<SegmentId> = c
        .windows(2)
        .map(|w| {
            sim.world_mut()
                .resource_mut::<NetGraph>()
                .add_segment(
                    &def,
                    road,
                    CubicBezier3::line(
                        Vec3::new(w[0].0, 0.0, w[0].1),
                        Vec3::new(w[1].0, 0.0, w[1].1),
                    ),
                    Elevation::Ground,
                )
                .unwrap()
        })
        .collect();
    let mut put = |name: &str, pos: Vec2, seg: SegmentId, t: f32| {
        sim.world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(Building {
                def: db.building_id(name).unwrap(),
                footprint: Footprint::Lot(Lot {
                    block: Id::new(0, 0),
                    col: 0,
                    w: 1,
                    d: 1,
                }),
                level: 1,
                state: BuildingState::Active,
                ready_at: Tick(0),
                pos,
                rot: 0.0,
                access: Some(AccessPoint { segment: seg, t }),
                flags: BuildingFlags(0),
                level_progress: 0.0,
            })
    };
    let depot = put("bus_depot", Vec2::new(200.0, 8.0), sides[0], 0.25);
    let _ = depot;
    let home = put("house_low_1", Vec2::new(side / 2.0, 8.0), sides[0], 0.5);
    let work = put(
        "com_low_1",
        Vec2::new(side / 2.0, side - 8.0),
        sides[2],
        0.5,
    );
    // The test sends the citizens itself.
    sim.world_mut()
        .insert_resource(cp_citizen::RoutineSettings { decide: false });
    sim.tick_once();
    Town {
        sim,
        sides,
        home,
        work,
        side,
    }
}

impl Town {
    fn line(&mut self) {
        let stops = [0usize, 1, 2, 3]
            .iter()
            .map(|&i| StopRef::NewRoad {
                seg: self.sides[i],
                s: self.side / 2.0,
                side: Side::Right,
            })
            .collect();
        apply(
            self.sim.world_mut(),
            &TransitCommand::CreateLine {
                mode: TransitMode::Bus,
                stops,
            },
        )
        .unwrap();
    }

    /// A household of `n` carless adults (or with cars) living at the home.
    fn household(&mut self, n: usize, has_car: bool) {
        let members = vec![(30 * 365, 0, has_car); n];
        let home = self.home;
        let mut g = self.sim.world_mut().resource_mut::<CitizenGrid>();
        g.ensure_derived();
        g.add_household(home, 0, Tick(0), &members);
        for (id, _) in g
            .citizens()
            .map(|(i, c)| (i, c.clone()))
            .collect::<Vec<_>>()
        {
            g.set_location(id, Location::Home);
        }
    }

    /// Sends every adult at home to the workplace, as a freshly planned trip by foot or car.
    fn send_all(&mut self) -> usize {
        let movers: Vec<_> = self
            .sim
            .world()
            .resource::<CitizenGrid>()
            .citizens()
            .filter(|(_, c)| c.location == Location::Home)
            .map(|(id, c)| (id, c.has_car))
            .collect();
        let (home, work) = (self.home, self.work);
        let w = self.sim.world_mut();
        w.resource_mut::<TripBook>().ensure_derived();
        let mut n = 0;
        for (citizen, car) in movers {
            let added = w.resource_mut::<TripBook>().add(Trip {
                citizen,
                from: home,
                to: work,
                purpose: TripPurpose::Work,
                mode: if car { TripMode::Car } else { TripMode::Walk },
                // Due at once: the routine turns it into a planned trip and picks its mode.
                state: TripState::Scheduled { depart_at: Tick(0) },
            });
            if added.is_some() {
                n += 1;
            }
        }
        n
    }

    fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.sim.tick_once();
        }
    }

    fn modes(&self) -> Vec<TripMode> {
        self.sim
            .world()
            .resource::<TripBook>()
            .iter()
            .map(|(_, t)| t.mode)
            .collect()
    }

    fn arrived(&self) -> usize {
        self.sim
            .world()
            .resource::<CitizenGrid>()
            .citizens()
            .filter(|(_, c)| c.location == Location::Work)
            .count()
    }
}

fn plain() -> std::path::PathBuf {
    Path::new(ROOT).join("../../assets/content")
}

#[test]
fn carless_residents_ride_the_line_and_pay_fares_and_a_deleted_line_sends_trips_back_on_foot() {
    let mut t = town(&plain(), 800.0);
    t.line();
    t.household(3, false);
    // Let the buses come out first.
    t.run(120);
    assert!(!t.sim.world().resource::<TransitFleet>().buses.is_empty());
    assert_eq!(t.send_all(), 3);
    t.run(2);
    assert_eq!(t.modes(), vec![TripMode::Transit; 3]);
    assert_eq!(t.sim.world().resource::<TransitNet>().riders.len(), 3);
    t.run(1500);
    let stats: u32 = t
        .sim
        .world()
        .resource::<TransitNet>()
        .stats
        .values()
        .map(|s| s.riders_today)
        .sum();
    assert!(stats > 0, "riders counted");
    let rs = format!(
        "{:?} {:?}",
        t.sim.world().resource::<TransitNet>().riders,
        t.sim.world().resource::<TransitFleet>()
    );
    assert_eq!(t.arrived(), 3, "all three reached the workplace {rs}");
    assert!(t.sim.world().resource::<TripBook>().is_empty());
    assert!(t.sim.world().resource::<TransitNet>().riders.is_empty());
    let fares = t.sim.world().resource::<TransitLedger>().fares;
    assert!(fares.0 > 0, "fares were taken: {fares:?}");

    // The line goes: the next trips are on foot again.
    let line = t
        .sim
        .world()
        .resource::<TransitNet>()
        .lines
        .iter()
        .next()
        .unwrap()
        .0;
    apply(t.sim.world_mut(), &TransitCommand::DeleteLine { line }).unwrap();
    t.run(2);
    {
        let mut g = t.sim.world_mut().resource_mut::<CitizenGrid>();
        let ids: Vec<_> = g.citizens().map(|(i, _)| i).collect();
        for id in ids {
            g.set_location(id, Location::Home);
        }
    }
    // The citizens stand at the work; put them home for the new trips.
    t.send_all();
    t.run(2);
    assert!(
        t.modes().iter().all(|&m| m != TripMode::Transit),
        "{:?}",
        t.modes()
    );
}

#[test]
fn a_car_owner_keeps_the_car() {
    let mut t = town(&plain(), 1200.0);
    t.line();
    t.household(2, true);
    t.run(120);
    t.send_all();
    t.run(2);
    assert!(
        t.modes().iter().all(|&m| m == TripMode::Car),
        "{:?}",
        t.modes()
    );
}

#[test]
fn a_full_bus_leaves_the_others_waiting_for_the_next_one() {
    let dir = content_with_bus_capacity(1);
    let mut t = town(&dir, 800.0);
    t.line();
    t.household(3, false);
    t.run(120);
    t.send_all();
    let mut max_load = 0;
    let mut waited_while_bus_left = false;
    for _ in 0..3000 {
        t.sim.tick_once();
        let net = t.sim.world().resource::<TransitNet>();
        let fleet = t.sim.world().resource::<TransitFleet>();
        for (bus, _) in fleet.buses.iter() {
            let load = net
                .riders
                .values()
                .filter(|j| j.stage == Stage::Riding { bus })
                .count();
            max_load = max_load.max(load);
        }
        let riding = net
            .riders
            .values()
            .any(|j| matches!(j.stage, Stage::Riding { .. }));
        let waiting = net
            .riders
            .values()
            .any(|j| matches!(j.stage, Stage::Waiting { .. }));
        waited_while_bus_left |= riding && waiting;
        if net.riders.is_empty() && t.arrived() == 3 {
            break;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(max_load, 1, "no bus carries more than its capacity");
    assert!(
        waited_while_bus_left,
        "someone waited while a bus carried another"
    );
    assert_eq!(t.arrived(), 3, "everyone got there on a later bus");
}

#[test]
fn deleting_the_line_mid_journey_ends_the_trips_without_a_panic() {
    let mut t = town(&plain(), 800.0);
    t.line();
    t.household(3, false);
    t.run(120);
    t.send_all();
    // Until somebody is inside a bus.
    for _ in 0..600 {
        t.sim.tick_once();
        let net = t.sim.world().resource::<TransitNet>();
        if net
            .riders
            .values()
            .any(|j| matches!(j.stage, Stage::Riding { .. }))
        {
            break;
        }
    }
    let line = t
        .sim
        .world()
        .resource::<TransitNet>()
        .lines
        .iter()
        .next()
        .unwrap()
        .0;
    apply(t.sim.world_mut(), &TransitCommand::DeleteLine { line }).unwrap();
    t.run(1500);
    assert!(t.sim.world().resource::<TransitNet>().riders.is_empty());
    assert!(t.sim.world().resource::<TripBook>().is_empty());
}

#[test]
fn the_weekly_settlement_books_fares_and_bus_upkeep_and_the_treasury_follows() {
    use cp_core::time::TICKS_PER_WEEK;
    use cp_core::{Cost, Treasury};
    use cp_economy::EconomyState;

    let mut t = town(&plain(), 800.0);
    t.line();
    t.household(3, false);
    t.run(120);
    t.send_all();
    t.run(1500);
    let boardings: u32 = t
        .sim
        .world()
        .resource::<TransitNet>()
        .stats
        .values()
        .map(|s| s.riders_week)
        .sum();
    let buses = t.sim.world().resource::<TransitFleet>().buses.len();
    assert!(
        boardings >= 3 && buses >= 1,
        "{boardings} boardings, {buses} buses"
    );
    let b = t
        .sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .balance()
        .clone();
    let before = t.sim.world().resource::<Treasury>().balance;
    t.sim.world_mut().insert_resource(Tick(TICKS_PER_WEEK - 1));
    t.run(3);
    let w = t.sim.world().resource::<EconomyState>().last_week;
    assert_eq!(w.week, 1);
    assert_eq!(
        w.transit_fares,
        Cost::scaled(b.transit_fare, f64::from(boardings))
    );
    assert_eq!(
        w.transit_upkeep,
        Cost::scaled(b.bus_upkeep_week, buses as f64)
    );
    assert_eq!(
        t.sim.world().resource::<Treasury>().balance,
        before + t.sim.world().resource::<cp_economy::Payday>().net() + w.transit_fares
            - w.transit_upkeep
            - w.loan_payments,
        "the treasury changes by the seventh day's share and the week's fares, bus upkeep and loans"
    );
    assert!(
        t.sim.world().resource::<TransitLedger>().is_empty(),
        "the ledger starts over"
    );
}
