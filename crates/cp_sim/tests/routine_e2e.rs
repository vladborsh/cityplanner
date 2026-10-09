//! T-032 acceptance: a town of a thousand citizens commutes (morning and evening peaks),
//! every trip finishes, nobody is in two places, trips to vanished buildings are
//! cancelled, and the whole thing is deterministic.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_ecs::change_detection::Mut;
use bevy_math::Vec3;
use cp_building::{BuildingGrid, BuildingState, demolish_building};
use cp_citizen::{
    CitizenGrid, Location, TripBook, TripMode, TripState, choose_mode, hour_of_day, shift_for,
    travel_ticks,
};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

/// An `n`x`n`-line lattice of 90 m streets; blocks are zoned in a repeating pattern.
fn town(content: &Arc<ContentDb>, seed: u64, lines: usize) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(seed);
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
    let mut sim = town(content, seed, lines);
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
                TripState::Planned | TripState::AwaitingPath { .. } | TripState::Agent => {
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
            matches!(t.state, TripState::InTransit { .. }).then_some((t.citizen, t.to, t.from))
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
    // Past the longest abstract trip: the trip has resolved one way or another.
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
