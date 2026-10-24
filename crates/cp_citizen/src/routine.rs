//! Daily routine and trip generation (`architecture/system-design.md` §9.2).
//!
//! Every citizen is visited once per 256 ticks (bucket `index % 256`, §2), about 16 times a
//! game day, and decides what to do next from where they are:
//!
//! * **At home**, adult with a job whose workplace is `Active`: leave for work in time to
//!   arrive at the shift start, i.e. from `shift_start − travel_ticks − 256` (the last term
//!   is one visit interval, because the citizen only looks every 256 ticks) until shortly
//!   before the shift ends. Otherwise, in the evening ([`SHOP_FROM`]..[`SHOP_TO`]), shop with
//!   a probability that grows with the household's goods need.
//! * **At work** outside the shift (or without a valid workplace): go home.
//! * **At a shop**: go home (normally the return trip was scheduled on arrival).
//!
//! **Goods need** (balance placeholder): `25 %` per day since the household last shopped,
//! capped at `100 %`; the chance that a visit in the shopping window turns into a trip.
//! Shops are `ComLow`/`ComHigh` buildings; the nearest by straight line wins. After arriving
//! the citizen stays [`SHOP_DWELL_MIN`]..+[`SHOP_DWELL_SPAN`] ticks, then returns.
//!
//! **Shifts** (balance placeholders, one table, hours of the day): see [`shift_for`].
//!
//! **Mode (M1):** walk if the straight line is shorter than `balance.ron`'s `walk_limit_m`
//! (1 km), else car if the citizen has one, else walk. **Travel time** is abstract and lives
//! on the **movement clock** (§2): `ceil(distance / speed / MOVE_SECONDS_PER_TICK)` ticks,
//! at least one, with content-style real speeds: walking [`WALK_SPEED_MPS`] = 1.4 m/s and
//! a documented placeholder [`CAR_SPEED_MPS`] = 13.9 m/s (50 km/h, the medium road limit)
//! until paths exist. Schedules (shifts, "evening", visits) live on the day clock. The path
//! service is not used yet (see [`crate::trips`]).
//!
//! **Invariants.** A citizen's `location` is `Moving` exactly while they have a trip in
//! transit (or before their household has arrived); it changes only when a trip departs
//! (`→ Moving`) or arrives (`→ Home|Work|Shop`). A citizen has at most one trip. A trip
//! whose citizen or destination disappeared is cancelled: the citizen (if still there) is
//! put back at home.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_content::{ContentDb, SimContent, ZoneType};
use cp_core::time::{MOVE_SECONDS_PER_TICK, TICKS_PER_DAY};
use cp_core::{SimLimits, SimRng, Tick};

use crate::grid::CitizenGrid;
use crate::leisure::{LEISURE_STREAM, LeisureTargets, pick_leisure};
use crate::model::{CitizenId, Location};
use crate::system::CITIZEN_BUCKETS;
use crate::trips::{LeisureArrival, Trip, TripBook, TripMode, TripPurpose, TripState};

pub const ROUTINE_STREAM: u64 = 0x2007;

/// Knobs of the routine (not saved).
#[derive(Resource, Clone, Copy, Debug)]
pub struct RoutineSettings {
    /// When `false`, citizens decide nothing new (trips in flight still finish). For tests,
    /// scenarios and benches that drive trips themselves.
    pub decide: bool,
}

impl Default for RoutineSettings {
    fn default() -> Self {
        Self { decide: true }
    }
}
/// Walking speed, m/s (real units; applied on the movement clock).
pub const WALK_SPEED_MPS: f32 = 1.4;
/// Driving speed, m/s: 50 km/h placeholder until agents and paths exist.
pub const CAR_SPEED_MPS: f32 = 13.9;
/// Shopping window, hours of the day.
pub const SHOP_FROM: f32 = 16.0;
pub const SHOP_TO: f32 = 21.0;
/// Goods need grows by this many percent per day since the last shopping trip.
pub const NEED_PCT_PER_DAY: u32 = 25;
/// Time spent at a shop: `SHOP_DWELL_MIN + rng(SHOP_DWELL_SPAN)` ticks.
pub const SHOP_DWELL_MIN: u64 = 170;
pub const SHOP_DWELL_SPAN: u32 = 340;
/// A leisure visit lasts this many ticks until the routine has read the balance (3 game hours).
pub const LEISURE_DWELL_FALLBACK: u64 = TICKS_PER_DAY / 8;

/// A working shift: start and end, hours of the day (`0..24`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shift {
    pub start: f32,
    pub end: f32,
}

/// The shift of a worker at a workplace of `zone`; `salt` (the citizen's slot index)
/// splits a workforce between shifts. PLACEHOLDERS for balance:
///
/// | Zone | Shifts |
/// |---|---|
/// | Industrial | 6–14 (even) / 14–22 (odd) |
/// | ComLow, ComHigh | 9–17 (two thirds) / 12–20 (one third) |
/// | Office, other | 8–16 |
pub fn shift_for(zone: ZoneType, salt: u32) -> Shift {
    let (start, end) = match zone {
        ZoneType::Industrial => {
            if salt.is_multiple_of(2) {
                (6.0, 14.0)
            } else {
                (14.0, 22.0)
            }
        }
        ZoneType::ComLow | ZoneType::ComHigh => {
            if salt % 3 == 2 {
                (12.0, 20.0)
            } else {
                (9.0, 17.0)
            }
        }
        _ => (8.0, 16.0),
    };
    Shift { start, end }
}

/// Hour of the day (`0.0..24.0`) at `tick`.
pub fn hour_of_day(tick: u64) -> f32 {
    (tick % TICKS_PER_DAY) as f32 / TICKS_PER_DAY as f32 * 24.0
}

/// M1 mode rule.
pub fn choose_mode(distance_m: f32, has_car: bool, walk_limit_m: f32) -> TripMode {
    if distance_m < walk_limit_m || !has_car {
        TripMode::Walk
    } else {
        TripMode::Car
    }
}

/// Abstract travel time, ticks (at least 1).
pub fn travel_ticks(distance_m: f32, mode: TripMode) -> u64 {
    let speed = match mode {
        TripMode::Walk | TripMode::Transit => WALK_SPEED_MPS,
        TripMode::Car => CAR_SPEED_MPS,
    };
    ((distance_m / speed / MOVE_SECONDS_PER_TICK).ceil() as u64).max(1)
}

fn pos_of(b: &BuildingGrid, id: BuildingId) -> Option<Vec2> {
    b.get(id).map(|b| b.pos)
}

fn is_active(b: &BuildingGrid, id: BuildingId) -> bool {
    b.get(id).is_some_and(|b| b.state == BuildingState::Active)
}

/// The active shops, in building order: `(id, position)`. Built once per tick when a citizen
/// wants one (scanning every building for every shopper was the cost).
type Shops = Option<Vec<(BuildingId, Vec2)>>;

/// How many nearest shops a shopper chooses from, and how fast their chance falls with the
/// distance (BUG-066).
const SHOP_CHOICES: usize = 64;
const SHOP_DECAY_M: f32 = 800.0;

/// One of the [`SHOP_CHOICES`] nearest active shops to `from`, a shop's chance falling as
/// `exp(-distance / SHOP_DECAY_M)`; `pick` is the uniform draw (by citizen and day). The same
/// household does not always walk to the shop next door.
fn nearest_shop(
    b: &BuildingGrid,
    db: &ContentDb,
    shops: &mut Shops,
    from: Vec2,
    pick: f32,
) -> Option<BuildingId> {
    let shops = shops.get_or_insert_with(|| {
        b.iter()
            .filter(|(_, bd)| {
                bd.state == BuildingState::Active
                    && matches!(
                        db.building(bd.def).zone,
                        ZoneType::ComLow | ZoneType::ComHigh
                    )
            })
            .map(|(id, bd)| (id, bd.pos))
            .collect()
    });
    let mut all: Vec<(f32, BuildingId)> = shops
        .iter()
        .map(|&(id, pos)| (pos.distance_squared(from), id))
        .collect();
    let order =
        |a: &(f32, BuildingId), b: &(f32, BuildingId)| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1));
    if all.len() > SHOP_CHOICES {
        all.select_nth_unstable_by(SHOP_CHOICES - 1, order);
        all.truncate(SHOP_CHOICES);
    }
    all.sort_by(order);
    let weights: Vec<f32> = all
        .iter()
        .map(|(d2, _)| (-d2.sqrt() / SHOP_DECAY_M).exp())
        .collect();
    let mut at = pick * weights.iter().sum::<f32>();
    for ((_, id), w) in all.iter().zip(&weights) {
        if at < *w {
            return Some(*id);
        }
        at -= w;
    }
    all.last().map(|(_, id)| *id)
}

fn set_location(grid: &mut CitizenGrid, id: CitizenId, loc: Location) {
    if let Some(c) = grid.citizen_mut(id) {
        c.location = loc;
    }
}

/// Puts the citizen back at home (cancelled trip) if they still exist.
fn send_home(grid: &mut CitizenGrid, id: CitizenId) {
    set_location(grid, id, Location::Home);
}

/// Cancels a trip (citizen or destination gone, or no route): the trip is dropped and
/// counted, and the citizen — if they still exist — is put back at home.
pub fn cancel_trip(grid: &mut CitizenGrid, book: &mut TripBook, tid: crate::trips::TripId) {
    let Some(trip) = book.remove(tid) else { return };
    book.cancelled += 1;
    if grid.citizen(trip.citizen).is_some() {
        send_home(grid, trip.citizen);
    }
}

/// Finishes a trip that has reached its destination: the citizen's location follows the
/// purpose, and after a shopping trip the way home is scheduled. A trip whose citizen or
/// destination is gone is cancelled instead. Used by the routine's timer for abstract trips
/// and by `cp_traffic` when an agent arrives.
pub fn complete_trip(
    grid: &mut CitizenGrid,
    book: &mut TripBook,
    buildings: &BuildingGrid,
    rng: &SimRng,
    now: u64,
    tid: crate::trips::TripId,
) {
    let Some(trip) = book.get(tid).cloned() else {
        return;
    };
    if grid.citizen(trip.citizen).is_none() || !is_active(buildings, trip.to) {
        cancel_trip(grid, book, tid);
        return;
    }
    book.remove(tid);
    book.completed += 1;
    match trip.purpose {
        TripPurpose::Work => {
            set_location(grid, trip.citizen, Location::Work);
            // T-318b: the minutes on the way, for the home's commute average.
            if let Some(home) = grid.citizen(trip.citizen).and_then(|c| c.home) {
                let secs = now.saturating_sub(trip.started.0) as f32 * MOVE_SECONDS_PER_TICK;
                book.commutes
                    .push((home, (secs / 60.0).round().min(f32::from(u16::MAX)) as u16));
            }
        }
        TripPurpose::Home => set_location(grid, trip.citizen, Location::Home),
        TripPurpose::MoveIn => grid.household_arrived(trip.citizen, now),
        TripPurpose::Shop => {
            set_location(grid, trip.citizen, Location::Shop);
            // Dwell, then head home.
            let mut r = rng.fork(
                ROUTINE_STREAM ^ 0x5,
                Tick(now ^ u64::from(trip.citizen.index) << 32),
            );
            let dwell = SHOP_DWELL_MIN + u64::from(r.below(SHOP_DWELL_SPAN));
            if let Some(home) = grid.citizen(trip.citizen).and_then(|c| c.home) {
                book.add(Trip {
                    citizen: trip.citizen,
                    from: trip.to,
                    to: home,
                    purpose: TripPurpose::Home,
                    mode: trip.mode,
                    state: TripState::Scheduled {
                        depart_at: Tick(now + dwell),
                    },
                    started: Tick(now + dwell),
                    park: Default::default(),
                });
            }
        }
        TripPurpose::Leisure => {
            set_location(grid, trip.citizen, Location::Leisure);
            let dwell = if book.leisure_dwell > 0 {
                book.leisure_dwell
            } else {
                LEISURE_DWELL_FALLBACK
            };
            // `cp_park` counts the visit and the time the visitor is inside.
            book.leisure_arrivals.push(LeisureArrival {
                citizen: trip.citizen,
                building: trip.to,
                leave_at: Tick(now + dwell),
            });
            if let Some(home) = grid.citizen(trip.citizen).and_then(|c| c.home) {
                book.add(Trip {
                    citizen: trip.citizen,
                    from: trip.to,
                    to: home,
                    purpose: TripPurpose::Home,
                    mode: trip.mode,
                    state: TripState::Scheduled {
                        depart_at: Tick(now + dwell),
                    },
                    started: Tick(now + dwell),
                    park: Default::default(),
                });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn update_routine(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    rng: Res<SimRng>,
    limits: Res<SimLimits>,
    mut grid: ResMut<CitizenGrid>,
    mut book: ResMut<TripBook>,
    settings: Option<Res<RoutineSettings>>,
    leisure: Option<Res<LeisureTargets>>,
) {
    let Some(content) = content else { return };
    let db = &content.0;
    let now = tick.0;
    book.leisure_dwell =
        (db.balance().leisure_visit_h * cp_core::time::TICKS_PER_HOUR).ceil() as u64;
    grid.ensure_derived();
    book.ensure_derived();
    let day = (now / TICKS_PER_DAY) as u32;
    let hour = hour_of_day(now);

    // --- due events: departures and arrivals ---
    for tid in book.due(now) {
        let Some(trip) = book.get(tid).cloned() else {
            continue;
        };
        let alive = grid.citizen(trip.citizen).is_some();
        match trip.state {
            TripState::Scheduled { .. } => {
                // Departure: needs the citizen and a destination.
                if !alive || !is_active(&buildings, trip.to) {
                    book.remove(tid);
                    book.cancelled += 1;
                    if alive {
                        send_home(&mut grid, trip.citizen);
                    }
                    continue;
                }
                let from = pos_of(&buildings, trip.from);
                let Some(to) = pos_of(&buildings, trip.to) else {
                    book.remove(tid);
                    book.cancelled += 1;
                    send_home(&mut grid, trip.citizen);
                    continue;
                };
                let d = from.map_or(0.0, |f| f.distance(to));
                let has_car = grid.citizen(trip.citizen).is_some_and(|c| c.has_car);
                let mode = choose_mode(d, has_car, db.balance().walk_limit_m);
                set_location(&mut grid, trip.citizen, Location::Moving);
                book.departures_by_hour[(hour as usize).min(23)] += 1;
                book.remove(tid);
                // `cp_traffic` routes it from here.
                book.add(Trip {
                    mode,
                    state: TripState::Planned,
                    started: Tick(now),
                    park: Default::default(),
                    ..trip
                });
            }
            TripState::InTransit { .. } => {
                complete_trip(&mut grid, &mut book, &buildings, &rng, now, tid);
            }
            // In `cp_traffic`'s hands; it completes them itself.
            TripState::Planned
            | TripState::AwaitingPath { .. }
            | TripState::Agent
            | TripState::Transit => {}
        }
    }

    // --- decisions for this tick's bucket ---
    if settings.is_none_or(|s| s.decide) && book.len() < limits.citizens as usize {
        let bucket = (now % u64::from(CITIZEN_BUCKETS)) as u32;
        let citizens: Vec<CitizenId> = grid
            .citizens_bucket(CITIZEN_BUCKETS, bucket)
            .map(|(id, _)| id)
            .collect();
        let mut r = rng.fork(ROUTINE_STREAM, *tick);
        let mut shops: Shops = None;
        let no_targets = LeisureTargets::default();
        let targets = leisure.as_deref().unwrap_or(&no_targets);
        for cid in citizens {
            decide(
                cid, now, day, hour, &buildings, db, &mut grid, &mut book, &mut r, &mut shops,
                targets,
            );
        }
    }

    // Households that left take their shopping history with them.
    if now.is_multiple_of(TICKS_PER_DAY) {
        let g = &*grid;
        book.prune_households(|h| g.household(h).is_some());
    }
}

#[allow(clippy::too_many_arguments)]
fn decide(
    cid: CitizenId,
    now: u64,
    day: u32,
    hour: f32,
    buildings: &BuildingGrid,
    db: &ContentDb,
    grid: &mut CitizenGrid,
    book: &mut TripBook,
    rng: &mut SimRng,
    shops: &mut Shops,
    targets: &LeisureTargets,
) {
    let Some(c) = grid.citizen(cid).cloned() else {
        return;
    };
    if !c.is_adult() || book.active_of(cid).is_some() {
        return;
    }
    // The sick stay home (T-139); one who fell ill at work or a shop still goes home.
    if c.sick_since.is_some() && c.location == Location::Home {
        return;
    }
    let Some(home) = c.home.filter(|h| is_active(buildings, *h)) else {
        return;
    };
    let workplace = c.work.filter(|w| is_active(buildings, *w));
    let shift = workplace
        .and_then(|w| buildings.get(w))
        .map(|b| shift_for(db.building(b.def).zone, cid.index));
    // Leave in time to arrive at the shift start: travel time (movement clock) plus one
    // visit interval, since the citizen only looks again after CITIZEN_BUCKETS ticks.
    let lead_ticks = workplace
        .and_then(|w| Some(pos_of(buildings, home)?.distance(pos_of(buildings, w)?)))
        .map_or(0, |d| {
            travel_ticks(d, choose_mode(d, c.has_car, db.balance().walk_limit_m))
        })
        + u64::from(CITIZEN_BUCKETS);
    let to_ticks = |h: f32| (h / 24.0 * TICKS_PER_DAY as f32) as i64;
    let tod = (now % TICKS_PER_DAY) as i64;
    let on_shift_window = shift.is_some_and(|s| {
        tod >= to_ticks(s.start) - lead_ticks as i64 && tod < to_ticks(s.end - 0.5)
    });

    let go =
        |purpose, from: BuildingId, to: BuildingId, grid: &mut CitizenGrid, book: &mut TripBook| {
            let (Some(a), Some(b)) = (pos_of(buildings, from), pos_of(buildings, to)) else {
                return false;
            };
            let d = a.distance(b);
            let mode = choose_mode(d, c.has_car, db.balance().walk_limit_m);
            let ok = book
                .add(Trip {
                    citizen: cid,
                    from,
                    to,
                    purpose,
                    mode,
                    // `cp_traffic` routes it and takes an agent from the pool; only a full
                    // pool or a missing agent makes it run abstractly (§9.4).
                    state: TripState::Planned,
                    started: Tick(now),
                    park: Default::default(),
                })
                .is_some();
            if ok {
                set_location(grid, cid, Location::Moving);
                book.departures_by_hour[(hour as usize).min(23)] += 1;
            }
            ok
        };

    match c.location {
        Location::Home => {
            if let (Some(w), true) = (workplace, on_shift_window) {
                go(TripPurpose::Work, home, w, grid, book);
            } else if (SHOP_FROM..SHOP_TO).contains(&hour) {
                let need = (NEED_PCT_PER_DAY * book.days_since_shop(c.household, day)).min(100);
                if rng.below(100) < need
                    && let Some(shop) = pos_of(buildings, home).and_then(|p| {
                        nearest_shop(
                            buildings,
                            db,
                            shops,
                            p,
                            crate::system::unit(u64::from(cid.index) << 32 | u64::from(day)),
                        )
                    })
                    && go(TripPurpose::Shop, home, shop, grid, book)
                {
                    book.note_shopped(c.household, day);
                }
            }
            // Free hours in the leisure window (T-224): a chance per visit, spread over the
            // visits of the window; the choice is a pure function of citizen and tick.
            let bal = db.balance();
            if book.active_of(cid).is_none()
                && (bal.leisure_from..bal.leisure_to).contains(&hour)
                && !targets.is_empty()
            {
                let visits = ((bal.leisure_to - bal.leisure_from) / 24.0
                    * (TICKS_PER_DAY / u64::from(CITIZEN_BUCKETS)) as f32)
                    .max(1.0);
                let seed = LEISURE_STREAM ^ (u64::from(cid.index) << 32) ^ now;
                if crate::system::unit(seed) < bal.leisure_chance_per_day / visits
                    && let Some(target) = pos_of(buildings, home).and_then(|p| {
                        pick_leisure(
                            targets,
                            p,
                            bal.leisure_dist_m,
                            crate::system::unit(seed ^ 0xA5A5),
                        )
                    })
                {
                    go(TripPurpose::Leisure, home, target, grid, book);
                }
            }
        }
        Location::Work => {
            if !on_shift_window
                && let Some(from) = c.work.filter(|w| is_active(buildings, *w)).or(Some(home))
            {
                go(TripPurpose::Home, from, home, grid, book);
            }
        }
        Location::Shop | Location::Leisure => {
            // Stranded at a shop or park (its return trip was cancelled): go home from home's spot.
            go(TripPurpose::Home, home, home, grid, book);
        }
        Location::Moving | Location::Outside => {}
    }
}

/// Drops everything that pointed at buildings that no longer exist (`removed`), at once:
///
/// * households living there are gone (they leave the city, the same path as an eviction),
///   with their members, jobs and trips;
/// * workers lose the job (the job search resumes), and a citizen inside a removed workplace
///   or shop is sent home;
/// * trips from or to a removed building, or of a citizen that no longer exists, are
///   cancelled (the citizen, if still there, is put back at home).
///
/// Returns the cancelled trips so `cp_traffic` can free their agents.
pub fn purge_buildings(
    grid: &mut CitizenGrid,
    book: &mut TripBook,
    removed: &[BuildingId],
) -> Vec<crate::trips::TripId> {
    if removed.is_empty() {
        return Vec::new();
    }
    grid.ensure_derived();
    book.ensure_derived();
    let gone = |b: BuildingId| removed.contains(&b);
    // Bodies waiting at a demolished deathcare building vanish with it; schoolchildren of a
    // demolished school have none (T-135).
    grid.dead.retain(|b, _| !gone(*b));
    grid.clear_schools(gone);
    let homes: Vec<_> = grid
        .households()
        .filter(|(_, h)| gone(h.home))
        .map(|(id, _)| id)
        .collect();
    for hid in homes {
        grid.remove_household(hid);
    }
    let affected: Vec<(CitizenId, bool, bool)> = grid
        .citizens()
        .filter_map(|(id, c)| {
            let job = c.work.is_some_and(gone);
            let inside = match c.location {
                Location::Work => c.work.is_some_and(gone),
                _ => false,
            };
            (job || inside).then_some((id, job, inside))
        })
        .collect();
    for (cid, job, inside) in affected {
        if job {
            grid.clear_job(cid);
        }
        if inside {
            send_home(grid, cid);
        }
    }
    let doomed: Vec<crate::trips::TripId> = book
        .iter()
        .filter(|(_, t)| gone(t.from) || gone(t.to) || grid.citizen(t.citizen).is_none())
        .map(|(id, _)| id)
        .collect();
    for &tid in &doomed {
        cancel_trip(grid, book, tid);
    }
    doomed
}
