//! Tourists (§16 M3 C, T-168): a visit without a citizen. A car appears at the `Outside` node of
//! an entry highway, drives to a commercial building (weighted by its distance from the entry),
//! stands there for the visit, brings `tourism_income` when it arrives, and drives back out.
//!
//! The flow a day is `tourism_base + tourism_per_com × commercial buildings`, times the season's
//! factor (T-321, §22.1); the number of cars at once is a share of the vehicle pool. The car
//! leaves the road for the visit and a new one drives out from the building (BUG-100: one parked
//! in its lane held up the street). With no `Outside` node (no entry highway) or no shop there
//! are no visits. A visit is a keep run of `cp_traffic` plus a record here; the run does the
//! driving, this system the decisions. Those who come by train walk from the station
//! ([`bring_by_train`], T-167).
//!
//! Lodgers (T-321, §22.3): `stay_share` of the new tourists take a room in a hotel near their
//! goal and go there first; then each day they make one outing (a visit like any other, from the
//! hotel and back), pay a night at midnight, and leave at the check-out. In their room they are
//! not a visit but an entry of the `lodged` queue, keyed by the tick of their next event.

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_math::Vec2;
use cp_building::{BuildingFlags, BuildingGrid, BuildingId, BuildingState};
use cp_content::{ContentDb, PolicyKind, SimContent, VehicleKind, ZoneType};
use cp_core::time::{TICKS_PER_DAY, TICKS_PER_HOUR, TICKS_PER_WEEK};
use cp_core::{Cost, SimLimits, SimRng, Tick};
use cp_net::{NetGraph, NodeId, RailReach};
use cp_traffic::{Agents, RunId, RunPoint, RunState, ServiceRuns, WALK_KIND};
use serde::{Deserialize, Serialize};

use crate::fleet::TransitLedger;
use crate::hotels::HotelState;
use crate::model::TransitNet;
use crate::station::connected_station;

/// RNG stream of the tourists' arrivals and goals.
pub const TOURISM_STREAM: u64 = 0x7019_5757;

/// The hour of the check-out (§22.3).
const CHECK_OUT_HOUR: u64 = 11;
/// Ticks between two looks at closed hotels (about 45 minutes).
const EVICT_EVERY: u64 = 64;

/// The first tick of hour `hour` of `day`.
fn hour_tick(day: u64, hour: u64) -> u64 {
    (day * 24 + hour) * TICKS_PER_DAY / 24
}

/// Where a visit is. Append-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TouristPhase {
    /// Driving in from the entry (or, for a lodger, out from the hotel to the goal).
    Arriving,
    /// At the building until `until`.
    Visiting { until: Tick },
    /// Driving back out; the record goes when the car is there.
    Leaving,
    /// A lodger's way back from the outing to the hotel (T-321).
    Outing { back_to: BuildingId },
    /// A new lodger on the way to the hotel to check in (T-321).
    Lodging,
}

/// A lodger's room (T-321, §22.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stay {
    pub hotel: BuildingId,
    pub check_out: Tick,
    /// Came by train: walks everywhere and leaves by the station.
    pub by_train: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tourist {
    pub goal: BuildingId,
    pub entry: NodeId,
    pub phase: TouristPhase,
    /// A lodger's room (T-321); `None` for a day visit.
    #[serde(default)]
    pub stay: Option<Stay>,
}

/// A lodger in the room until the next outing or the check-out (T-321).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lodged {
    pub stay: Stay,
    pub entry: NodeId,
}

/// The visits under way (by the id of their run) and what they left behind. Serialized sim
/// state.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tourists {
    pub visits: BTreeMap<RunId, Tourist>,
    /// Visits each building has had (the counter `NO_CUSTOMERS` will read).
    pub customers: BTreeMap<BuildingId, u32>,
    /// Visits that arrived since the week began.
    pub visits_week: u32,
    /// Percent more tourists the running grey deals bring (a casino, `architecture/corruption.md`
    /// §4); set by `cp_economy::update_corruption` once a day.
    pub boost_pct: u16,
    /// Arrivals at a building and the tick the visit ends, since `cp_park` last looked (T-224);
    /// transient, drained every tick.
    #[serde(skip)]
    pub arrivals: Vec<(BuildingId, Tick)>,
    /// The lodgers in their rooms, by the tick of their next outing or check-out (T-321).
    #[serde(default)]
    pub lodged: BTreeMap<Tick, Vec<Lodged>>,
    /// Tourists who wanted a room this week and found none (T-321).
    #[serde(default)]
    pub turned_away_week: u32,
}

impl Tourists {
    pub fn is_empty(&self) -> bool {
        self.visits.is_empty()
            && self.customers.is_empty()
            && self.visits_week == 0
            && self.boost_pct == 0
            && self.lodged.is_empty()
            && self.turned_away_week == 0
    }

    /// Lodgers in their rooms now.
    pub fn in_rooms(&self) -> usize {
        self.lodged.values().map(Vec::len).sum()
    }

    fn lodge(&mut self, at: Tick, l: Lodged) {
        self.lodged.entry(at).or_default().push(l);
    }
}

/// Tourists a day per unit of weight: a unique building pulls `tourists_per_day / this` times
/// what a shop does when the tourist picks a goal (T-200). // tune later
const UNIQUE_WEIGHT_UNIT: f32 = 10.0;

fn is_shop(z: ZoneType) -> bool {
    matches!(z, ZoneType::ComLow | ZoneType::ComHigh)
}

/// The goals of the tourists with their weight: the shops (1, or their specialisation's
/// `tourist_weight`) and the unique buildings (by their tourists a day); and the shop count and
/// the unique buildings' tourists a day, for the flow.
fn goals(db: &ContentDb, buildings: &BuildingGrid) -> (Vec<(BuildingId, Vec2, f32)>, usize, f32) {
    let (mut unique_per_day, mut shop_count) = (0.0, 0usize);
    let shops = buildings
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active && b.access().is_some())
        .filter_map(|(id, b)| {
            let def = db.building(b.def);
            if let Some(u) = def.unique.as_ref().filter(|u| u.tourists_per_day > 0.0) {
                unique_per_day += u.tourists_per_day;
                Some((id, b.pos, u.tourists_per_day / UNIQUE_WEIGHT_UNIT))
            } else {
                shop_count += usize::from(is_shop(def.zone));
                // A specialised shop draws more (or fewer) tourists (T-196).
                let w = def
                    .spec
                    .and_then(|k| db.spec(k))
                    .map_or(1.0, |s| s.tourist_weight);
                is_shop(def.zone).then_some((id, b.pos, w))
            }
        })
        .collect();
    (shops, shop_count, unique_per_day)
}

/// One goal from `from` by the weights of [`goals`] (nearer ones likelier), or a park or a
/// landmark with `tourist_leisure_share` (T-224).
fn pick_goal(
    shops: &[(BuildingId, Vec2, f32)],
    leisure: Option<&cp_citizen::LeisureTargets>,
    balance: &cp_content::Balance,
    from: Vec2,
    r: &mut SimRng,
) -> BuildingId {
    let leisure_goal = leisure
        .filter(|l| !l.is_empty() && r.next_f32() < balance.tourist_leisure_share)
        .and_then(|l| cp_citizen::pick_leisure(l, from, balance.tourism_reach_m, r.next_f32()));
    let weight = |p: Vec2, w: f32| w / (1.0 + p.distance(from) / balance.tourism_reach_m.max(1.0));
    let total: f32 = shops.iter().map(|&(_, p, w)| weight(p, w)).sum();
    let mut pick = r.next_f32() * total;
    let mut goal = shops[shops.len() - 1].0;
    for &(id, p, w) in shops {
        pick -= weight(p, w);
        if pick <= 0.0 {
            goal = id;
            break;
        }
    }
    leisure_goal.unwrap_or(goal)
}

/// Whether `hotel` takes guests: it stands, works and has its water and power (§22.3).
fn hotel_open(hotels: &HotelState, buildings: &BuildingGrid, hotel: BuildingId) -> bool {
    hotels.slots.contains_key(&hotel)
        && buildings.get(hotel).is_some_and(|b| {
            b.state == BuildingState::Active
                && b.access().is_some()
                && !b.flags.has(BuildingFlags::NO_POWER)
                && !b.flags.has(BuildingFlags::NO_WATER)
        })
}

/// A hotel with a free room for a tourist whose goal is at `goal`: by `stars / (1 + d / reach)`
/// from the goal (§22.3); `None` when every room is taken.
fn choose_hotel(
    hotels: &HotelState,
    db: &ContentDb,
    buildings: &BuildingGrid,
    goal: Vec2,
    r: &mut SimRng,
) -> Option<BuildingId> {
    let reach = db.balance().tourism_reach_m.max(1.0);
    let free: Vec<(BuildingId, f32)> = hotels
        .slots
        .iter()
        .filter(|&(&id, _)| hotel_open(hotels, buildings, id))
        .filter_map(|(&id, slot)| {
            let b = buildings.get(id)?;
            let h = db.building(b.def).hotel.as_ref()?;
            (slot.guests < h.rooms).then(|| {
                (
                    id,
                    f32::from(h.stars) / (1.0 + b.pos.distance(goal) / reach),
                )
            })
        })
        .collect();
    let total: f32 = free.iter().map(|f| f.1).sum();
    if free.is_empty() || total <= 0.0 {
        return None;
    }
    let mut pick = r.next_f32() * total;
    for &(id, w) in &free {
        pick -= w;
        if pick <= 0.0 {
            return Some(id);
        }
    }
    free.last().map(|f| f.0)
}

/// Takes a room for a new tourist whose goal is `goal` (with `stay_share`): the hotel gets the
/// guest now and the stay `1..=max_nights` nights from today; `None` (and one more turned away)
/// when the city has hotels but no free room, or the tourist does not want one.
#[allow(clippy::too_many_arguments)]
fn take_room(
    tourists: &mut Tourists,
    hotels: &mut HotelState,
    db: &ContentDb,
    buildings: &BuildingGrid,
    goal: BuildingId,
    by_train: bool,
    now: Tick,
    r: &mut SimRng,
) -> Option<Stay> {
    let k = &db.balance().tourism;
    if hotels.is_empty() || r.next_f32() >= k.stay_share {
        return None;
    }
    let at = buildings.get(goal)?.pos;
    let Some(hotel) = choose_hotel(hotels, db, buildings, at, r) else {
        tourists.turned_away_week += 1;
        return None;
    };
    let nights = u64::from(r.below(u32::from(k.max_nights.max(1)))) + 1;
    let day = now.0 / TICKS_PER_DAY;
    hotels.slots.get_mut(&hotel)?.guests += 1;
    Some(Stay {
        hotel,
        check_out: Tick(hour_tick(day + nights, CHECK_OUT_HOUR)),
        by_train,
    })
}

/// The tick of a lodger's next event after `now`: the outing of `day` at a random hour of
/// `outing_from..outing_to` (no earlier than the next tick), or the check-out when that day is
/// the check-out's.
fn next_event(db: &ContentDb, stay: &Stay, day: u64, now: Tick, r: &mut SimRng) -> Tick {
    let k = &db.balance().tourism;
    if day >= stay.check_out.0 / TICKS_PER_DAY {
        return stay.check_out.max(Tick(now.0 + 1));
    }
    let hours = u32::from(k.outing_to.saturating_sub(k.outing_from)).max(1);
    let hour = u64::from(k.outing_from) + u64::from(r.below(hours));
    let at = hour_tick(day, hour) + u64::from(r.below((TICKS_PER_DAY / 24) as u32));
    Tick(at.max(now.0 + 1))
}

/// A lodger leaves: the room is free, and the guest goes home by car to the exit nearest their
/// entry, or on foot to the station.
#[allow(clippy::too_many_arguments)]
fn check_out(
    tourists: &mut Tourists,
    hotels: &mut HotelState,
    runs: &mut ServiceRuns,
    db: &ContentDb,
    net: &NetGraph,
    transit: &TransitNet,
    rail: &RailReach,
    buildings: &BuildingGrid,
    l: Lodged,
) {
    if let Some(slot) = hotels.slots.get_mut(&l.stay.hotel) {
        slot.guests = slot.guests.saturating_sub(1);
    }
    if buildings.get(l.stay.hotel).is_none() {
        return;
    }
    let from = RunPoint::Building(l.stay.hotel);
    let run = if l.stay.by_train {
        connected_station(db, transit, rail, buildings)
            .map(|s| runs.start_walk(from, RunPoint::Building(s)))
    } else {
        Some(runs.start_run(
            VehicleKind::Car,
            from,
            RunPoint::Outside(exit_near(net, db, l.entry)),
            true,
        ))
    };
    if let Some(run) = run {
        tourists.visits.insert(
            run,
            Tourist {
                goal: l.stay.hotel,
                entry: l.entry,
                phase: TouristPhase::Leaving,
                stay: None,
            },
        );
    }
}

/// Midnight: every guest pays the hotel's night (§22.3); a new week starts the hotels' week.
fn pay_nights(
    hotels: &mut HotelState,
    ledger: &mut TransitLedger,
    db: &ContentDb,
    buildings: &BuildingGrid,
    now: Tick,
) {
    if !now.0.is_multiple_of(TICKS_PER_DAY) {
        return;
    }
    for (&id, slot) in &mut hotels.slots {
        if now.0.is_multiple_of(TICKS_PER_WEEK) {
            slot.nights_week = 0;
            slot.revenue_week = Cost::ZERO;
        }
        let Some(h) = buildings
            .get(id)
            .and_then(|b| db.building(b.def).hotel.as_ref())
        else {
            continue;
        };
        if slot.guests == 0 {
            continue;
        }
        let pay = Cost::scaled(h.price_night, f64::from(slot.guests));
        ledger.hotels += pay;
        slot.revenue_week += pay;
        slot.nights_week += u32::from(slot.guests);
    }
}

/// Every [`EVICT_EVERY`] ticks: the lodgers of a hotel that is gone or closed (no power or water) leave at once
/// (§22.3); a guest on an outing or on the way in becomes a day visitor.
fn evict_closed(
    tourists: &mut Tourists,
    hotels: &mut HotelState,
    runs: &mut ServiceRuns,
    agents: &mut Agents,
    buildings: &BuildingGrid,
) {
    let open = |hotels: &HotelState, h: BuildingId| hotel_open(hotels, buildings, h);
    let mut leaving = Vec::new();
    for list in tourists.lodged.values_mut() {
        list.retain(|l| {
            let ok = open(hotels, l.stay.hotel);
            if !ok {
                leaving.push(l.stay.hotel);
            }
            ok
        });
    }
    tourists.lodged.retain(|_, list| !list.is_empty());
    let ids: Vec<RunId> = tourists.visits.keys().copied().collect();
    for id in ids {
        let t = tourists.visits[&id];
        let Some(stay) = t.stay.filter(|s| !open(hotels, s.hotel)) else {
            continue;
        };
        leaving.push(stay.hotel);
        match t.phase {
            TouristPhase::Lodging | TouristPhase::Outing { .. } => {
                runs.release(id, agents);
                tourists.visits.remove(&id);
            }
            _ => {
                if let Some(v) = tourists.visits.get_mut(&id) {
                    v.stay = None;
                }
            }
        }
    }
    for h in leaving {
        if let Some(slot) = hotels.slots.get_mut(&h) {
            slot.guests = slot.guests.saturating_sub(1);
        }
    }
}

/// `Agents` phase, after `run_riders`: moves the visits on, then the lodgers, then lets new
/// tourists in.
#[allow(clippy::too_many_arguments)]
pub fn run_tourists(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    rng: Res<SimRng>,
    limits: Res<SimLimits>,
    net: Res<NetGraph>,
    rail: Res<RailReach>,
    transit: Res<TransitNet>,
    buildings: Res<BuildingGrid>,
    leisure: Option<Res<cp_citizen::LeisureTargets>>,
    mut tourists: ResMut<Tourists>,
    mut hotels: ResMut<HotelState>,
    mut ledger: ResMut<TransitLedger>,
    mut runs: ResMut<ServiceRuns>,
    mut agents: ResMut<Agents>,
    policies: cp_district::PolicyParams,
) {
    let Some(content) = content else { return };
    let db = &*content.0;
    let balance = db.balance();
    let now = *tick;
    if now.0.is_multiple_of(TICKS_PER_WEEK) {
        tourists.visits_week = 0;
        tourists.turned_away_week = 0;
    }
    // 0. The hotels' night and the hotels that closed (T-321).
    pay_nights(&mut hotels, &mut ledger, db, &buildings, now);
    if now.0.is_multiple_of(EVICT_EVERY) {
        evict_closed(
            &mut tourists,
            &mut hotels,
            &mut runs,
            &mut agents,
            &buildings,
        );
    }
    let mut r = rng.fork(TOURISM_STREAM, now);
    let day = now.0 / TICKS_PER_DAY;

    // 1. The visits under way, in run order.
    let ids: Vec<RunId> = tourists.visits.keys().copied().collect();
    for id in ids {
        let t = tourists.visits[&id];
        let at_goal = matches!(
            runs.get(id).map(|r| r.state),
            Some(RunState::Parked { .. } | RunState::Arrived { .. })
        );
        let state = runs.get(id).map(|r| r.state);
        if state.is_none() || state == Some(RunState::Failed) {
            runs.release(id, &mut agents);
            tourists.visits.remove(&id);
            // A lodger who cannot reach the hotel gives the room up; one whose outing failed
            // is back in the room for tomorrow's.
            match (t.stay, t.phase) {
                (Some(stay), TouristPhase::Lodging) => {
                    if let Some(slot) = hotels.slots.get_mut(&stay.hotel) {
                        slot.guests = slot.guests.saturating_sub(1);
                    }
                }
                (Some(stay), _) => {
                    let at = next_event(db, &stay, day + 1, now, &mut r);
                    tourists.lodge(
                        at,
                        Lodged {
                            stay,
                            entry: t.entry,
                        },
                    );
                }
                (None, _) => {}
            }
            continue;
        }
        if !at_goal {
            continue;
        }
        match t.phase {
            TouristPhase::Arriving => {
                // The car waits off the street for the visit (BUG-100).
                runs.park_off_road(id, &mut agents);
                ledger.tourism += Cost::scaled(balance.tourism_income, 1.0);
                *tourists.customers.entry(t.goal).or_default() += 1;
                tourists.visits_week += 1;
                let stay = (balance.tourism_visit_h * TICKS_PER_HOUR).ceil() as u64;
                tourists.arrivals.push((t.goal, Tick(now.0 + stay.max(1))));
                if let Some(v) = tourists.visits.get_mut(&id) {
                    v.phase = TouristPhase::Visiting {
                        until: Tick(now.0 + stay.max(1)),
                    };
                }
            }
            TouristPhase::Visiting { until } if now >= until => {
                let walker = runs.get(id).is_some_and(|r| r.kind == WALK_KIND);
                if let Some(stay) = t.stay {
                    // A lodger goes back to the hotel (T-321).
                    let back = TouristPhase::Outing {
                        back_to: stay.hotel,
                    };
                    if walker {
                        runs.release(id, &mut agents);
                        tourists.visits.remove(&id);
                        let run = runs
                            .start_walk(RunPoint::Building(t.goal), RunPoint::Building(stay.hotel));
                        tourists.visits.insert(run, Tourist { phase: back, ..t });
                    } else if runs
                        .continue_run(id, RunPoint::Building(stay.hotel))
                        .is_ok()
                    {
                        if let Some(v) = tourists.visits.get_mut(&id) {
                            v.phase = back;
                        }
                    } else {
                        // No way back by road: back in the room all the same.
                        runs.release(id, &mut agents);
                        tourists.visits.remove(&id);
                        let at = next_event(db, &stay, day + 1, now, &mut r);
                        tourists.lodge(
                            at,
                            Lodged {
                                stay,
                                entry: t.entry,
                            },
                        );
                    }
                } else if walker {
                    // A tourist who came by train walks back to the station (T-167).
                    let home = connected_station(db, &transit, &rail, &buildings);
                    runs.release(id, &mut agents);
                    tourists.visits.remove(&id);
                    if let Some(station) = home {
                        let back = runs
                            .start_walk(RunPoint::Building(t.goal), RunPoint::Building(station));
                        tourists.visits.insert(
                            back,
                            Tourist {
                                phase: TouristPhase::Leaving,
                                ..t
                            },
                        );
                    }
                } else if runs
                    .continue_run(id, RunPoint::Outside(exit_near(&net, db, t.entry)))
                    .is_ok()
                {
                    if let Some(v) = tourists.visits.get_mut(&id) {
                        v.phase = TouristPhase::Leaving;
                    }
                } else {
                    runs.release(id, &mut agents);
                    tourists.visits.remove(&id);
                }
            }
            TouristPhase::Visiting { .. } => {}
            TouristPhase::Leaving => {
                runs.release(id, &mut agents);
                tourists.visits.remove(&id);
            }
            // At the hotel: the car stays with it and the guest is in the room until the next
            // outing (T-321).
            TouristPhase::Lodging | TouristPhase::Outing { .. } => {
                runs.release(id, &mut agents);
                tourists.visits.remove(&id);
                let Some(stay) = t.stay else { continue };
                let l = Lodged {
                    stay,
                    entry: t.entry,
                };
                // An outing of today is done; a new guest still has today's.
                let next_day = if t.phase == TouristPhase::Lodging {
                    day
                } else {
                    day + 1
                };
                if now >= stay.check_out {
                    check_out(
                        &mut tourists,
                        &mut hotels,
                        &mut runs,
                        db,
                        &net,
                        &transit,
                        &rail,
                        &buildings,
                        l,
                    );
                } else {
                    let at = next_event(db, &stay, next_day, now, &mut r);
                    tourists.lodge(at, l);
                }
            }
        }
    }

    // 2. The lodgers whose time has come: check-out or the day's outing (T-321).
    let (shops, shop_count, unique_per_day) = goals(db, &buildings);
    let cap = (limits.vehicles as f32 * balance.tourism_pool_share) as usize;
    let due: Vec<Tick> = tourists.lodged.range(..=now).map(|(&t, _)| t).collect();
    for at in due {
        let Some(list) = tourists.lodged.remove(&at) else {
            continue;
        };
        for l in list {
            if now >= l.stay.check_out {
                check_out(
                    &mut tourists,
                    &mut hotels,
                    &mut runs,
                    db,
                    &net,
                    &transit,
                    &rail,
                    &buildings,
                    l,
                );
                continue;
            }
            let from = buildings.get(l.stay.hotel).map(|b| b.pos);
            // A full pool (or nothing to see): the guest rests in the hotel today.
            let goal = from
                .filter(|_| !shops.is_empty() && tourists.visits.len() < cap)
                .map(|p| (p, pick_goal(&shops, leisure.as_deref(), balance, p, &mut r)))
                .filter(|&(_, g)| g != l.stay.hotel);
            let Some((p, goal)) = goal else {
                let next = next_event(db, &l.stay, day + 1, now, &mut r);
                tourists.lodge(next, l);
                continue;
            };
            let near = buildings
                .get(goal)
                .is_some_and(|g| g.pos.distance(p) < balance.tourism.walk_m);
            let (start, end) = (RunPoint::Building(l.stay.hotel), RunPoint::Building(goal));
            let run = if near || l.stay.by_train {
                runs.start_walk(start, end)
            } else {
                runs.start_run(VehicleKind::Car, start, end, true)
            };
            tourists.visits.insert(
                run,
                Tourist {
                    goal,
                    entry: l.entry,
                    phase: TouristPhase::Arriving,
                    stay: Some(l.stay),
                },
            );
        }
    }

    // 3. New tourists.
    // They drive in where a car can enter the city (BUG-095).
    let (entries, _) = cp_net::outside_ways(&net, db);
    if entries.is_empty() || shops.is_empty() || tourists.visits.len() >= cap {
        return;
    }
    // TourismPromotion (T-194, city only) raises the flow by its `value`.
    let promotion = match db.policy(PolicyKind::TourismPromotion) {
        Some(p)
            if policies
                .get()
                .is_some_and(|x| x.city.has(PolicyKind::TourismPromotion)) =>
        {
            1.0 + p.value
        }
        _ => 1.0,
    } + f32::from(tourists.boost_pct) / 100.0;
    // T-321: and the season (§22.1: summer more, winter fewer).
    let per_tick =
        (balance.tourism_base + balance.tourism_per_com * shop_count as f32 + unique_per_day)
            * promotion
            * balance.tourism.season_mul(now)
            / TICKS_PER_DAY as f32;
    let mut n = per_tick.floor() as u32;
    if r.next_f32() < per_tick.fract() {
        n += 1;
    }
    for _ in 0..n {
        if tourists.visits.len() >= cap {
            break;
        }
        let (entry, from) = entries[r.below(entries.len() as u32) as usize];
        let goal = pick_goal(&shops, leisure.as_deref(), balance, from, &mut r);
        // A share of them takes a room near the goal and goes there first (T-321).
        let stay = take_room(
            &mut tourists,
            &mut hotels,
            db,
            &buildings,
            goal,
            false,
            now,
            &mut r,
        );
        let (to, phase) = match stay {
            Some(s) => (s.hotel, TouristPhase::Lodging),
            None => (goal, TouristPhase::Arriving),
        };
        let run = runs.start_run(
            VehicleKind::Car,
            RunPoint::Outside(entry),
            RunPoint::Building(to),
            true,
        );
        tourists.visits.insert(
            run,
            Tourist {
                goal: to,
                entry,
                phase,
                stay,
            },
        );
    }
}

/// `n` tourists step off the train at `station` and walk to shops, the nearer ones more likely
/// (T-167). `entry` is the railway's `Outside` node; they leave on foot back to the station.
/// Counts against the same cap as the cars. A share of them walks to a hotel first (T-321).
#[allow(clippy::too_many_arguments)]
pub fn bring_by_train(
    tourists: &mut Tourists,
    hotels: &mut HotelState,
    now: Tick,
    runs: &mut ServiceRuns,
    db: &cp_content::ContentDb,
    buildings: &BuildingGrid,
    limits: &SimLimits,
    r: &mut SimRng,
    station: BuildingId,
    entry: NodeId,
    n: u32,
) {
    let Some(from) = buildings.get(station).map(|b| b.pos) else {
        return;
    };
    let balance = db.balance();
    let shops: Vec<(BuildingId, Vec2, f32)> = buildings
        .iter()
        .filter(|(_, b)| {
            b.state == BuildingState::Active
                && b.access().is_some()
                && is_shop(db.building(b.def).zone)
        })
        .map(|(id, b)| {
            // A specialised shop draws more (or fewer) tourists (T-196).
            let w = db
                .building(b.def)
                .spec
                .and_then(|k| db.spec(k))
                .map_or(1.0, |s| s.tourist_weight);
            (id, b.pos, w)
        })
        .collect();
    if shops.is_empty() {
        return;
    }
    let cap = (limits.vehicles as f32 * balance.tourism_pool_share) as usize;
    let weight = |p: Vec2| 1.0 / (1.0 + p.distance(from) / balance.tourism_reach_m.max(1.0));
    let total: f32 = shops.iter().map(|&(_, p, w)| weight(p) * w).sum();
    for _ in 0..n {
        if tourists.visits.len() >= cap {
            break;
        }
        let mut pick = r.next_f32() * total;
        let mut goal = shops[shops.len() - 1].0;
        for &(id, p, w) in &shops {
            pick -= weight(p) * w;
            if pick <= 0.0 {
                goal = id;
                break;
            }
        }
        let stay = take_room(tourists, hotels, db, buildings, goal, true, now, r);
        let (to, phase) = match stay {
            Some(s) => (s.hotel, TouristPhase::Lodging),
            None => (goal, TouristPhase::Arriving),
        };
        let run = runs.start_walk(RunPoint::Building(station), RunPoint::Building(to));
        tourists.visits.insert(
            run,
            Tourist {
                goal: to,
                entry,
                phase,
                stay,
            },
        );
    }
}

/// Where a tourist that came in at `entry` drives out of the map (BUG-095): the exit nearest to
/// it (the entry highway's other carriageway), else `entry` itself.
fn exit_near(net: &cp_net::NetGraph, db: &cp_content::ContentDb, entry: NodeId) -> NodeId {
    let Some(at) = net.node(entry).map(|n| Vec2::new(n.pos.x, n.pos.z)) else {
        return entry;
    };
    let (_, exits) = cp_net::outside_ways(net, db);
    exits
        .into_iter()
        .min_by(|a, b| {
            a.1.distance(at)
                .total_cmp(&b.1.distance(at))
                .then(a.0.cmp(&b.0))
        })
        .map_or(entry, |(id, _)| id)
}
