//! Tourists (§16 M3 C, T-168): a visit without a citizen. A car appears at the `Outside` node of
//! an entry highway, drives to a commercial building (weighted by its distance from the entry),
//! stands there for the visit, brings `tourism_income` when it arrives, and drives back out.
//!
//! The flow a day is `tourism_base + tourism_per_com × commercial buildings`; the number of cars
//! at once is a share of the vehicle pool. With no `Outside` node (no entry highway) or no shop
//! there are no visits. A visit is a keep run of `cp_traffic` plus a record here; the run does
//! the driving, this system the decisions. Trains to a station are not done yet.

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_math::Vec2;
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_content::{SimContent, VehicleKind, ZoneType};
use cp_core::time::{TICKS_PER_DAY, TICKS_PER_HOUR, TICKS_PER_WEEK};
use cp_core::{Cost, SimLimits, SimRng, Tick};
use cp_net::{NetGraph, NodeId, NodeKind};
use cp_traffic::{Agents, RunId, RunPoint, RunState, ServiceRuns};
use serde::{Deserialize, Serialize};

use crate::fleet::TransitLedger;

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
    buildings: Res<BuildingGrid>,
    mut tourists: ResMut<Tourists>,
    mut ledger: ResMut<TransitLedger>,
    mut runs: ResMut<ServiceRuns>,
    mut agents: ResMut<Agents>,
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
                if runs.continue_run(id, RunPoint::Outside(t.entry)).is_ok() {
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
        .filter(|(_, n)| n.kind == NodeKind::Outside && !n.segments.is_empty())
        .map(|(id, n)| (id, Vec2::new(n.pos.x, n.pos.z)))
        .collect();
    if entries.is_empty() {
        return;
    }
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
    if tourists.visits.len() >= cap {
        return;
    }
    let per_tick = (balance.tourism_base + balance.tourism_per_com * shops.len() as f32)
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
        let weight = |p: Vec2| 1.0 / (1.0 + p.distance(from) / balance.tourism_reach_m.max(1.0));
        let total: f32 = shops.iter().map(|&(_, p)| weight(p)).sum();
        let mut pick = r.next_f32() * total;
        let mut goal = shops[shops.len() - 1].0;
        for &(id, p) in &shops {
            pick -= weight(p);
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
