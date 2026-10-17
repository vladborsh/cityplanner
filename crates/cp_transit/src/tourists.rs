//! Tourists (§16 M3 C, T-168): a visit without a citizen. A car appears at the `Outside` node of
//! an entry highway, drives to a commercial building (weighted by its distance from the entry),
//! stands there for the visit, brings `tourism_income` when it arrives, and drives back out.
//!
//! The flow a day is `tourism_base + tourism_per_com × commercial buildings`; the number of cars
//! at once is a share of the vehicle pool. With no `Outside` node (no entry highway) or no shop
//! there are no visits. A visit is a keep run of `cp_traffic` plus a record here; the run does
//! the driving, this system the decisions. Those who come by train walk from the station
//! ([`bring_by_train`], T-167).

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_math::Vec2;
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_content::{PolicyKind, SimContent, VehicleKind, ZoneType};
use cp_core::time::{TICKS_PER_DAY, TICKS_PER_HOUR, TICKS_PER_WEEK};
use cp_core::{Cost, SimLimits, SimRng, Tick};
use cp_net::{NetGraph, NodeId, NodeKind, RailReach, node_is_rail};
use cp_traffic::{Agents, RunId, RunPoint, RunState, ServiceRuns, WALK_KIND};
use serde::{Deserialize, Serialize};

use crate::fleet::TransitLedger;
use crate::model::TransitNet;
use crate::station::connected_station;

/// RNG stream of the tourists' arrivals and goals.
pub const TOURISM_STREAM: u64 = 0x7019_5757;

/// Where a visit is. Append-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TouristPhase {
    /// Driving in from the entry.
    Arriving,
    /// At the building until `until`.
    Visiting { until: Tick },
    /// Driving back out; the record goes when the car is there.
    Leaving,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tourist {
    pub goal: BuildingId,
    pub entry: NodeId,
    pub phase: TouristPhase,
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
}

impl Tourists {
    pub fn is_empty(&self) -> bool {
        self.visits.is_empty() && self.customers.is_empty() && self.visits_week == 0
    }
}

/// Tourists a day per unit of weight: a unique building pulls `tourists_per_day / this` times
/// what a shop does when the tourist picks a goal (T-200). // tune later
const UNIQUE_WEIGHT_UNIT: f32 = 10.0;

fn is_shop(z: ZoneType) -> bool {
    matches!(z, ZoneType::ComLow | ZoneType::ComHigh)
}

/// `Agents` phase, after `run_riders`: moves the visits on, then lets new tourists in.
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
    mut tourists: ResMut<Tourists>,
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
    }

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
            continue;
        }
        if !at_goal {
            continue;
        }
        match t.phase {
            TouristPhase::Arriving => {
                ledger.tourism += Cost::scaled(balance.tourism_income, 1.0);
                *tourists.customers.entry(t.goal).or_default() += 1;
                tourists.visits_week += 1;
                let stay = (balance.tourism_visit_h * TICKS_PER_HOUR).ceil() as u64;
                if let Some(v) = tourists.visits.get_mut(&id) {
                    v.phase = TouristPhase::Visiting {
                        until: Tick(now.0 + stay.max(1)),
                    };
                }
            }
            TouristPhase::Visiting { until } if now >= until => {
                // A tourist who came by train walks back to the station (T-167).
                if runs.get(id).is_some_and(|r| r.kind == WALK_KIND) {
                    let home = connected_station(&transit, &rail, &buildings);
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
                } else if runs.continue_run(id, RunPoint::Outside(t.entry)).is_ok() {
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
        }
    }

    // 2. New tourists.
    let entries: Vec<(NodeId, Vec2)> = net
        .nodes()
        .filter(|(_, n)| {
            n.kind == NodeKind::Outside && !n.segments.is_empty() && !node_is_rail(&net, n)
        })
        .map(|(id, n)| (id, Vec2::new(n.pos.x, n.pos.z)))
        .collect();
    if entries.is_empty() {
        return;
    }
    // Goals: the shops (weight 1) and the unique buildings (weight by their tourists a day).
    let (mut unique_per_day, mut shop_count) = (0.0, 0usize);
    let shops: Vec<(BuildingId, Vec2, f32)> = buildings
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active && b.access().is_some())
        .filter_map(|(id, b)| {
            let def = db.building(b.def);
            if let Some(u) = def.unique.as_ref().filter(|u| u.tourists_per_day > 0.0) {
                unique_per_day += u.tourists_per_day;
                Some((id, b.pos, u.tourists_per_day / UNIQUE_WEIGHT_UNIT))
            } else {
                shop_count += usize::from(is_shop(def.zone));
                is_shop(def.zone).then_some((id, b.pos, 1.0))
            }
        })
        .collect();
    if shops.is_empty() {
        return;
    }
    let cap = (limits.vehicles as f32 * balance.tourism_pool_share) as usize;
    if tourists.visits.len() >= cap {
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
    };
    let per_tick = (balance.tourism_base
        + balance.tourism_per_com * shop_count as f32
        + unique_per_day)
        * promotion
        / TICKS_PER_DAY as f32;
    let mut r = rng.fork(TOURISM_STREAM, now);
    let mut n = per_tick.floor() as u32;
    if r.next_f32() < per_tick.fract() {
        n += 1;
    }
    for _ in 0..n {
        if tourists.visits.len() >= cap {
            break;
        }
        let (entry, from) = entries[r.below(entries.len() as u32) as usize];
        let weight =
            |p: Vec2, w: f32| w / (1.0 + p.distance(from) / balance.tourism_reach_m.max(1.0));
        let total: f32 = shops.iter().map(|&(_, p, w)| weight(p, w)).sum();
        let mut pick = r.next_f32() * total;
        let mut goal = shops[shops.len() - 1].0;
        for &(id, p, w) in &shops {
            pick -= weight(p, w);
            if pick <= 0.0 {
                goal = id;
                break;
            }
        }
        let run = runs.start_run(
            VehicleKind::Car,
            RunPoint::Outside(entry),
            RunPoint::Building(goal),
            true,
        );
        tourists.visits.insert(
            run,
            Tourist {
                goal,
                entry,
                phase: TouristPhase::Arriving,
            },
        );
    }
}

/// `n` tourists step off the train at `station` and walk to shops, the nearer ones more likely
/// (T-167). `entry` is the railway's `Outside` node; they leave on foot back to the station.
/// Counts against the same cap as the cars.
#[allow(clippy::too_many_arguments)]
pub fn bring_by_train(
    tourists: &mut Tourists,
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
    let shops: Vec<(BuildingId, Vec2)> = buildings
        .iter()
        .filter(|(_, b)| {
            b.state == BuildingState::Active
                && b.access().is_some()
                && is_shop(db.building(b.def).zone)
        })
        .map(|(id, b)| (id, b.pos))
        .collect();
    if shops.is_empty() {
        return;
    }
    let cap = (limits.vehicles as f32 * balance.tourism_pool_share) as usize;
    let weight = |p: Vec2| 1.0 / (1.0 + p.distance(from) / balance.tourism_reach_m.max(1.0));
    let total: f32 = shops.iter().map(|&(_, p)| weight(p)).sum();
    for _ in 0..n {
        if tourists.visits.len() >= cap {
            break;
        }
        let mut pick = r.next_f32() * total;
        let mut goal = shops[shops.len() - 1].0;
        for &(id, p) in &shops {
            pick -= weight(p);
            if pick <= 0.0 {
                goal = id;
                break;
            }
        }
        let run = runs.start_walk(RunPoint::Building(station), RunPoint::Building(goal));
        tourists.visits.insert(
            run,
            Tourist {
                goal,
                entry,
                phase: TouristPhase::Arriving,
            },
        );
    }
}
