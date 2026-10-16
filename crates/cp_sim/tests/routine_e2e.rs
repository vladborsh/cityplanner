//! T-032 acceptance: a town of a thousand citizens commutes (morning and evening peaks),
//! every trip finishes, nobody is in two places, trips to vanished buildings are
//! cancelled, and the whole thing is deterministic.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_ecs::change_detection::Mut;
use bevy_math::Vec3;
use cp_building::{BuildingCommand, BuildingGrid, BuildingState, demolish_building};
use cp_citizen::{
    CitizenGrid, Location, TripBook, TripMode, TripState, choose_mode, hour_of_day, shift_for,
    travel_ticks,
};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_sim::{SimCommand, SimRunner};
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

/// The game content with `walk_limit_m` of the balance replaced.
fn content_walking_below(walk_limit_m: f32) -> Arc<ContentDb> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>, limit: f32) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            if path.is_dir() {
                walk(&path, out, limit);
            } else if path.extension().is_some_and(|x| x == "ron") {
                let mut text = std::fs::read_to_string(&path).unwrap();
                if path.file_name().is_some_and(|n| n == "balance.ron") {
                    text = format!("Balance((walk_limit_m: {limit}))");
                }
                out.push((path.display().to_string(), text));
            }
        }
    }
    let mut sources = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content"),
        &mut sources,
        walk_limit_m,
    );
    Arc::new(ContentDb::from_sources(sources).unwrap())
}

/// An `n`x`n`-line lattice of 90 m streets; blocks are zoned in a repeating pattern.
fn town(content: &Arc<ContentDb>, seed: u64, lines: usize) -> SimRunner {
    town_with(content, SimRunner::new(seed), lines)
}

fn town_with(content: &Arc<ContentDb>, mut sim: SimRunner, lines: usize) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    sim.set_content(Arc::clone(content));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let p = |x: f32, z: f32| Vec3::new(2000.0 + x, 0.0, 2000.0 + z);
        for line in 0..lines {
            let o = line as f32 * 90.0;
            for piece in 0..lines - 1 {
                let a = piece as f32 * 90.0;
                for (from, to) in [(p(a, o), p(a + 90.0, o)), (p(o, a), p(o, a + 90.0))] {
                    g.add_segment(&def, road, CubicBezier3::line(from, to), Elevation::Ground)
                        .unwrap();
                }
            }
        }
    }
    sim.tick_once();
    {
        let kinds = [
            ZoneType::ResHigh,
            ZoneType::ResHigh,
            ZoneType::Industrial,
            ZoneType::ComHigh,
        ];
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = grid.blocks().map(|(id, b)| (id, b.cell_count())).collect();
        for (n, (id, cells)) in ids.into_iter().enumerate() {
            for cell in 0..cells {
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    kinds[n % kinds.len()],
                );
            }
        }
    }
    sim
}

/// Runs until the town has `n` citizens and everyone had time to find work, then to the
/// start of a game day.
fn grown(content: &Arc<ContentDb>, seed: u64, lines: usize, n: usize) -> SimRunner {
    grown_from(town(content, seed, lines), n)
}

fn grown_from(mut sim: SimRunner, n: usize) -> SimRunner {
    while sim.world().resource::<CitizenGrid>().citizen_count() < n {
        sim.tick_once();
        assert!(
            sim.tick().0 < 20 * TICKS_PER_DAY,
            "town never reached {n} citizens"
        );
    }
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    while !sim.tick().0.is_multiple_of(TICKS_PER_DAY) {
        sim.tick_once();
    }
    sim
}

#[test]
fn a_thousand_citizens_commute_with_morning_and_evening_peaks() {
    let content = content();
    let mut sim = grown(&content, 5, 7, 1000);
    let c = sim.world().resource::<CitizenGrid>();
    assert!(c.citizen_count() >= 1000 && c.employed() > 100);

    // Record one full game day of departures.
    let before = sim.world().resource::<TripBook>().departures_by_hour;
    for _ in 0..TICKS_PER_DAY {
        sim.tick_once();
    }
    let after = sim.world().resource::<TripBook>().departures_by_hour;
    let hist: Vec<u32> = (0..24).map(|h| after[h] - before[h]).collect();
    eprintln!("hourly departures: {hist:?}");
    let total: u32 = hist.iter().sum();
    assert!(total > 300, "{total} departures");
    let mean = total as f32 / 24.0;
    let morning = *hist[4..10].iter().max().unwrap() as f32;
    let evening = *hist[15..20].iter().max().unwrap() as f32;
    let night = hist[0..4].iter().sum::<u32>() as f32 / 4.0;
    assert!(
        morning >= 1.5 * mean,
        "morning peak {morning} vs mean {mean}"
    );
    assert!(
        evening >= 1.5 * mean,
        "evening peak {evening} vs mean {mean}"
    );
    assert!(night * 4.0 < mean, "nights are quiet: {night} vs {mean}");
    let peak_hour = |r: std::ops::Range<usize>| r.max_by_key(|&h| hist[h]).unwrap();
    assert!((4..10).contains(&peak_hour(4..10)) && (15..20).contains(&peak_hour(15..20)));
}

/// Walks every tick for two days checking the invariants.
#[test]
fn every_trip_finishes_and_nobody_is_in_two_places() {
    let content = content();
    let mut sim = grown(&content, 5, 7, 1000);
    let mut first_seen: BTreeMap<cp_citizen::TripId, u64> = BTreeMap::new();
    let mut seen_total = 0u64;
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
        let now = sim.tick().0;
        let c = sim.world().resource::<CitizenGrid>();
        let book = sim.world().resource::<TripBook>();

        // At most one trip per citizen, and trips agree with the location state machine.
        let mut per_citizen: BTreeMap<_, u32> = BTreeMap::new();
        for (tid, t) in book.iter() {
            *per_citizen.entry(t.citizen).or_default() += 1;
            let first = *first_seen.entry(tid).or_insert_with(|| {
                seen_total += 1;
                now
            });
            assert!(now - first < TICKS_PER_DAY, "trip {tid:?} is stuck");
            let loc = c
                .citizen(t.citizen)
                .expect("trip of a live citizen")
                .location;
            match t.state {
                TripState::InTransit { arrive_at } => {
                    assert_eq!(loc, Location::Moving, "in transit but {loc:?}");
                    assert!(arrive_at.0 >= now.saturating_sub(1), "overdue arrival");
                }
                TripState::Scheduled { .. } => assert_eq!(loc, Location::Shop),
                // Routed or being carried by an agent: the citizen is on the move.
                TripState::Planned
                | TripState::AwaitingPath { .. }
                | TripState::Agent
                | TripState::Transit => {
                    assert_eq!(loc, Location::Moving, "{:?} but {loc:?}", t.state)
                }
            }
        }
        assert!(
            per_citizen.values().all(|&n| n == 1),
            "two trips for one citizen"
        );
        first_seen.retain(|id, _| book.get(*id).is_some());
        for (cid, cz) in c.citizens() {
            if cz.location == Location::Moving && !per_citizen.contains_key(&cid) {
                // Only a household that has not arrived yet is `Moving` without a trip.
                let h = c.household(cz.household).unwrap();
                assert!(now < h.arrive_at.0 + 256, "stuck moving without a trip");
            }
        }
    }
    let book = sim.world().resource::<TripBook>();
    assert!(seen_total > 300, "{seen_total} trips observed");
    assert!(book.completed > 300, "{} trips completed", book.completed);
}

#[test]
fn the_mode_rule_and_travel_times_follow_the_spec() {
    let limit = content().balance().walk_limit_m;
    assert_eq!(limit, 1000.0, "balance.ron");
    assert_eq!(choose_mode(999.0, true, limit), TripMode::Walk);
    assert_eq!(choose_mode(limit, true, limit), TripMode::Car);
    assert_eq!(choose_mode(5000.0, true, limit), TripMode::Car);
    assert_eq!(choose_mode(5000.0, false, limit), TripMode::Walk);
    // The knob is data: a lower limit sends the same trip by car.
    assert_eq!(choose_mode(500.0, true, 300.0), TripMode::Car);
    assert_eq!(travel_ticks(0.0, TripMode::Walk), 1);
    // Movement clock (§2): 1 km on foot = 1000 / 1.4 / 2 s = 357 ticks (~2 game hours).
    assert_eq!(travel_ticks(1000.0, TripMode::Walk), 358);
    // 4 km by car at 13.9 m/s = 144 ticks (~50 game minutes, §2's yardstick).
    assert_eq!(travel_ticks(4000.0, TripMode::Car), 144);
    assert!(travel_ticks(5000.0, TripMode::Car) < travel_ticks(5000.0, TripMode::Walk));
    assert!((hour_of_day(TICKS_PER_DAY / 2) - 12.0).abs() < 1e-4);
    // Shifts: industry has two, commerce two, offices one; all inside the day.
    for z in [
        ZoneType::Industrial,
        ZoneType::ComLow,
        ZoneType::ComHigh,
        ZoneType::Office,
    ] {
        for salt in 0..6 {
            let s = shift_for(z, salt);
            assert!(0.0 <= s.start && s.start < s.end && s.end <= 24.0);
        }
    }
    assert_ne!(
        shift_for(ZoneType::Industrial, 0),
        shift_for(ZoneType::Industrial, 1)
    );
}

#[test]
fn trips_to_a_building_that_disappears_are_cancelled_cleanly() {
    let content = content();
    let mut sim = grown(&content, 5, 5, 400);
    // Wait for someone to be on their way to work, then demolish the destination.
    let mut victim = None;
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
        let book = sim.world().resource::<TripBook>();
        victim = book.iter().find_map(|(_, t)| {
            // On its way, as a timer or as an agent.
            matches!(
                t.state,
                TripState::InTransit { .. } | TripState::AwaitingPath { .. } | TripState::Agent
            )
            .then_some((t.citizen, t.to, t.from))
        });
        if victim.is_some() {
            break;
        }
    }
    let (citizen, to, _) = victim.expect("somebody travelled");
    let cancelled_before = sim.world().resource::<TripBook>().cancelled;
    sim.world_mut()
        .resource_scope(|w, mut b: Mut<BuildingGrid>| {
            let mut z = w.resource_mut::<ZoneGrid>();
            assert!(demolish_building(&mut b, &mut z, to));
        });
    // Long enough for the trip to have resolved one way or another.
    for _ in 0..(TICKS_PER_DAY / 2) {
        sim.tick_once();
    }
    let c = sim.world().resource::<CitizenGrid>();
    let book = sim.world().resource::<TripBook>();
    assert!(
        book.cancelled > cancelled_before,
        "the trip was cancelled, not lost"
    );
    if let Some(cz) = c.citizen(citizen) {
        // Wherever they are now, it is a single consistent place.
        let has_trip = book.iter().any(|(_, t)| t.citizen == citizen);
        assert_eq!(cz.location == Location::Moving, has_trip);
    }
    let b = sim.world().resource::<BuildingGrid>();
    assert!(b.get(to).is_none());
    assert!(b.iter().any(|(_, x)| x.state == BuildingState::Active));
}

#[test]
fn same_seed_same_trips_and_trips_are_checksummed() {
    let content = content();
    let (mut a, mut b, mut c) = (
        grown(&content, 5, 5, 300),
        grown(&content, 5, 5, 300),
        grown(&content, 6, 5, 300),
    );
    let before = a.checksum();
    for s in [&mut a, &mut b, &mut c] {
        for _ in 0..TICKS_PER_DAY {
            s.tick_once();
        }
    }
    assert_eq!(a.checksum(), b.checksum());
    assert_eq!(
        a.world().resource::<TripBook>(),
        b.world().resource::<TripBook>()
    );
    assert_ne!(a.checksum(), c.checksum());
    assert_ne!(a.checksum(), before);
    let g = a.view().stats;
    assert!(g.arenas.iter().any(|x| x.name == "trips"));
}

/// Runs a town and, as the chance arises, demolishes the workplace a commuter is heading to
/// and the home a walker (carried by an agent) is heading to, with `Demolish` commands. Runs
/// on for two game days, checking after every tick that nothing points at a missing
/// building and the agent pools match the trips.
fn demolish_occupied_buildings(content: &Arc<ContentDb>, seed: u64) -> SimRunner {
    let mut sim = grown(content, seed, 5, 400);
    let (mut home, mut work) = (None, None);
    let mut done_at = None;
    for _ in 0..(6 * TICKS_PER_DAY) {
        let book = sim.world().resource::<TripBook>();
        let c = sim.world().resource::<CitizenGrid>();
        let at = sim.tick();
        let mut pick = None;
        if work.is_none() {
            pick = book
                .iter()
                .find(|(_, t)| t.purpose == cp_citizen::TripPurpose::Work)
                .map(|(_, t)| (t.to, true));
        } else if home.is_none() {
            pick = book
                .iter()
                .find(|(_, t)| {
                    t.purpose == cp_citizen::TripPurpose::Home && t.state == TripState::Agent
                })
                .map(|(_, t)| (t.to, false));
        }
        if let Some((building, is_work)) = pick {
            let occupied = if is_work {
                (0..4).map(|l| c.jobs_filled(building, l)).sum::<u16>() > 0
            } else {
                c.households_in(building) > 0
            };
            if occupied {
                sim.push_command(
                    SimCommand::Building(BuildingCommand::Demolish { building }),
                    at,
                );
                if is_work {
                    work = Some(building);
                } else {
                    home = Some(building);
                    done_at = Some(at.0);
                }
            }
        }
        sim.tick_once();
        let b = sim.world().resource::<BuildingGrid>();
        let c = sim.world().resource::<CitizenGrid>();
        let book = sim.world().resource::<TripBook>();
        let agents = sim.world().resource::<cp_traffic::Agents>();
        let gone: Vec<_> = [home, work].into_iter().flatten().collect();
        for &g in &gone {
            assert!(b.get(g).is_none());
            assert_eq!(c.households_in(g), 0, "residents of a demolished home");
            assert!(
                (0..4).all(|l| c.jobs_filled(g, l) == 0),
                "workers of a demolished workplace"
            );
        }
        for (_, cz) in c.citizens() {
            assert!(cz.home.is_none_or(|h| !gone.contains(&h)));
            assert!(cz.work.is_none_or(|w| !gone.contains(&w)));
        }
        for (_, t) in book.iter() {
            assert!(c.citizen(t.citizen).is_some(), "trip of a missing citizen");
            assert!(!gone.contains(&t.to) && !gone.contains(&t.from));
        }
        let agent_trips = book
            .iter()
            .filter(|(_, t)| t.state == TripState::Agent)
            .count();
        assert_eq!(
            agent_trips,
            agents.pedestrian_count()
                + agents
                    .vehicles()
                    .filter(|(_, v)| v.citizen.index != u32::MAX)
                    .count(),
            "pools match the trips"
        );
        for (_, p) in agents.pedestrians() {
            assert!(book.get(p.trip).is_some(), "pedestrian of a missing trip");
        }
        for (_, v) in agents
            .vehicles()
            .filter(|(_, v)| v.citizen.index != u32::MAX)
        {
            assert!(book.get(v.trip).is_some(), "vehicle of a missing trip");
        }
        assert!(agents.occupancy_is_live(), "no dead vehicle left in a lane");
        if done_at.is_some_and(|d| sim.tick().0 >= d + 2 * TICKS_PER_DAY) {
            break;
        }
    }
    assert!(home.is_some() && work.is_some(), "both were demolished");
    sim
}

#[test]
fn demolishing_an_occupied_home_and_workplace_leaves_nothing_dangling() {
    let content = content();
    let mut a = demolish_occupied_buildings(&content, 5);
    let mut b = demolish_occupied_buildings(&content, 5);
    assert_eq!(a.checksum(), b.checksum(), "same command log, same state");
    assert!(a.world().resource::<TripBook>().cancelled > 0);
}

/// Cars and pedestrians on the road right now.
fn on_the_road(sim: &SimRunner) -> (usize, usize) {
    let a = sim.world().resource::<cp_traffic::Agents>();
    (a.vehicle_count(), a.pedestrian_count())
}

/// BUG-022: commuters are cars and pedestrians, not abstract timers. The road is empty at
/// night, fills in the morning peak, and nothing runs abstractly while the pools have room.
#[test]
fn commuters_drive_and_walk_in_the_morning_peak() {
    // Trips under the walking limit (1 km) are walked; a small town needs a short limit to
    // have cars too.
    let content = content_walking_below(150.0);
    let mut sim = grown(&content, 5, 7, 1000);
    let abstract_before = sim
        .world()
        .resource::<cp_traffic::Agents>()
        .stats
        .abstract_trips;
    let (mut night_max, mut peak_cars, mut peak_peds) = (0, 0, 0);
    let mut growing = false;
    for t in 0..TICKS_PER_DAY {
        sim.tick_once();
        let (cars, peds) = on_the_road(&sim);
        let hour = hour_of_day(sim.tick().0);
        if hour < 3.0 {
            night_max = night_max.max(cars + peds);
        }
        if (5.0..10.0).contains(&hour) {
            peak_cars = peak_cars.max(cars);
            peak_peds = peak_peds.max(peds);
            // More trips depart than finish early in the peak, so the count grows.
            growing |= cars + peds > night_max + 5;
        }
        let _ = t;
    }
    eprintln!("peak: {peak_cars} cars, {peak_peds} pedestrians; night max {night_max}");
    assert!(peak_cars > 0, "no car on the road in the morning peak");
    assert!(peak_cars + peak_peds > 10, "{peak_cars} + {peak_peds}");
    assert!(growing, "the road never filled up");
    let a = sim.world().resource::<cp_traffic::Agents>();
    assert_eq!(
        a.stats.abstract_trips, abstract_before,
        "trips ran abstractly with free pools"
    );
    assert!(a.stats.driven > 0);
}

/// BUG-022: with ten agent slots the overflow runs abstractly and every trip still finishes.
#[test]
fn a_full_pool_sends_the_overflow_abstract_and_every_trip_finishes() {
    let content = content();
    let mut limits = cp_core::SimLimits::desktop();
    limits.vehicles = 10;
    limits.pedestrians = 10;
    let sim = town_with(&content, SimRunner::with_limits(5, limits), 7);
    let mut sim = grown_from(sim, 1000);
    let mut first_seen: BTreeMap<cp_citizen::TripId, u64> = BTreeMap::new();
    let (mut max_cars, mut max_peds) = (0, 0);
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
        let now = sim.tick().0;
        let (cars, peds) = on_the_road(&sim);
        (max_cars, max_peds) = (max_cars.max(cars), max_peds.max(peds));
        let book = sim.world().resource::<TripBook>();
        for (tid, _) in book.iter() {
            let first = *first_seen.entry(tid).or_insert(now);
            assert!(now - first < TICKS_PER_DAY, "trip {tid:?} is stuck");
        }
        first_seen.retain(|id, _| book.get(*id).is_some());
    }
    assert!(
        max_cars <= 10 && max_peds <= 10,
        "{max_cars} cars, {max_peds} peds"
    );
    assert!(max_cars + max_peds > 0, "the pool was never used");
    let a = sim.world().resource::<cp_traffic::Agents>();
    assert!(a.stats.abstract_trips > 0, "no overflow with ten slots");
    let book = sim.world().resource::<TripBook>();
    assert!(book.completed > 300, "{} trips completed", book.completed);
}

#[test]
fn the_sick_stay_home_from_work() {
    let content = content();
    let mut sim = grown(&content, 5, 7, 300);
    let ids: Vec<_> = {
        let g = sim.world().resource::<CitizenGrid>();
        g.citizens().map(|(id, _)| id).collect()
    };
    let now = sim.tick();
    {
        let mut g = sim.world_mut().resource_mut::<CitizenGrid>();
        g.ensure_derived();
        for id in ids {
            g.set_sick(id, Some(now));
        }
    }
    for t in 0..TICKS_PER_DAY {
        sim.tick_once();
        if t % 64 == 0 {
            let at_work = sim
                .world()
                .resource::<CitizenGrid>()
                .citizens()
                .filter(|(_, c)| c.location == Location::Work)
                .count();
            assert_eq!(at_work, 0, "tick {t}");
        }
    }
}
