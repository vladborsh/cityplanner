//! Agent pools, pedestrians and trip routing (`architecture/system-design.md` §3, §9.4).
//!
//! A trip created by the routine (`TripState::Planned`) goes through three steps:
//!
//! 1. [`route_trips`] (`Agents` phase) submits a path request between the two buildings'
//!    access points and parks the trip in `AwaitingPath { request }`;
//! 2. [`receive_paths`] (`Paths` phase, after the service delivered the result 8 ticks later)
//!    takes an agent from the pool: a pedestrian for a walking trip, a vehicle for a car trip.
//!    If the pool is full — or the car task has not replaced the placeholder yet — the trip
//!    runs **abstractly**: `InTransit` for `ceil(cost / MOVE_SECONDS_PER_TICK)` ticks, counted
//!    in [`AgentStats::abstract_trips`]. A trip without a route is cancelled
//!    ([`AgentStats::no_path`]); a stale result is asked for again;
//! 3. [`move_agents`] (`Agents` phase) moves the agents; arrival completes the trip and frees
//!    the slot, so pools drain when trips end.
//!
//! **Pedestrians** (`PedRoute` = the pathfinder's `PedPath`) walk the sidewalks and junction
//! links at `PED_SPEED × MOVE_SECONDS_PER_TICK` = 2.8 m per tick, one step per tick (§2).
//! A route is a list of *legs*: walk along the start lane to its first end, then for every
//! hop along the lane to its other end (same lane) or across the junction link (a corner,
//! or a crossing), and finally along the goal lane to the goal. **A pedestrian only starts a
//! crossing while the junction's light shows it walkable** ([`LightPlan::walk`] at that tick);
//! otherwise they wait where they stand. A route whose lanes or links no longer exist cancels
//! the trip ([`AgentStats::invalid_routes`]). Pedestrians never interact with each other, so
//! the step runs in parallel over fixed-size chunks and is applied in id order (identical for
//! any thread count; [`AgentSettings::parallel`] switches to a sequential run for tests).
//!
//! **Vehicles** are a pool slot plus the car path and an arrival tick from its cost —
//! PLACEHOLDER motion until the car task (T-035) adds lanes, leaders and junctions; the slot
//! is held until the trip ends.

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_tasks::{ComputeTaskPool, TaskPool};
use cp_building::{Building, BuildingGrid};
use cp_citizen::{
    CitizenGrid, CitizenId, TripBook, TripId, TripMode, TripState, cancel_trip, complete_trip,
};
use cp_core::curve::{CubicBezier3, to_xz};
use cp_core::time::MOVE_SECONDS_PER_TICK;
use cp_core::{Arena, Id, SimLimits, SimRng, Tick};
use cp_net::{JunctionGeometry, LaneId, NetGraph, NodeId};
use serde::{Deserialize, Serialize};

use crate::astar::{CarPath, NoPath, PedPath};
use crate::graph::{Access, PED_SPEED, PathGraph, SegEnd, Side};
use crate::service::{
    Mode, PathNetwork, PathRequest, PathResult, PathResults, PathService, RequestId,
};

pub type PedestrianId = Id<Pedestrian>;
pub type VehicleId = Id<Vehicle>;

/// Metres a pedestrian walks per tick (1.4 m/s on the movement clock).
pub const PED_STEP: f32 = PED_SPEED * MOVE_SECONDS_PER_TICK;
/// Pedestrians per parallel chunk (fixed, so results never depend on the thread count).
pub const PED_CHUNK: usize = 256;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pedestrian {
    pub trip: TripId,
    pub citizen: CitizenId,
    pub route: PedPath,
    /// Index of the current leg (`0..=route.states.len()`).
    pub leg: u32,
    /// Metres walked on the current leg.
    pub s: f32,
    /// Length of the current leg, metres (0 until the leg is entered).
    pub leg_len: f32,
    /// Whether the leg has been entered (its length measured, a crossing's light checked).
    pub started: bool,
    /// Ticks spent waiting for a walk signal, in total.
    pub waited: u32,
    /// World position (XZ), kept for rendering.
    pub pos: Vec2,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vehicle {
    pub trip: TripId,
    pub citizen: CitizenId,
    pub path: CarPath,
    /// Placeholder motion: the tick the car reaches its destination.
    pub arrive_at: Tick,
    /// Ticks without moving (used by the car task; the despawn threshold is a setting).
    pub stuck_timer: u32,
}

/// Counters for the debug window and for balance.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStats {
    /// Trips that ran abstractly because the pool was full.
    pub abstract_trips: u64,
    pub walked: u64,
    pub driven: u64,
    /// Trips cancelled because no route exists.
    pub no_path: u64,
    /// Trips cancelled because the network changed under a walking route.
    pub invalid_routes: u64,
    /// Pedestrian-ticks spent waiting at a red crossing.
    pub red_waits: u64,
}

/// Knobs (not saved).
#[derive(Resource, Clone, Copy, Debug)]
pub struct AgentSettings {
    /// Step pedestrians on the compute pool (the result is the same either way).
    pub parallel: bool,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self { parallel: true }
    }
}

/// The agent pools.
#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct Agents {
    pedestrians: Arena<Pedestrian>,
    vehicles: Arena<Vehicle>,
    pub stats: AgentStats,
    /// Path request -> trip; derived from the trips, rebuilt lazily after a load.
    #[serde(skip)]
    requests: BTreeMap<u64, TripId>,
    #[serde(skip)]
    ready: bool,
}

impl PartialEq for Agents {
    fn eq(&self, other: &Self) -> bool {
        self.pedestrians == other.pedestrians
            && self.vehicles == other.vehicles
            && self.stats == other.stats
    }
}

impl Agents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pedestrians(&self) -> impl Iterator<Item = (PedestrianId, &Pedestrian)> {
        self.pedestrians.iter()
    }

    pub fn vehicles(&self) -> impl Iterator<Item = (VehicleId, &Vehicle)> {
        self.vehicles.iter()
    }

    pub fn pedestrian_count(&self) -> usize {
        self.pedestrians.len()
    }

    pub fn vehicle_count(&self) -> usize {
        self.vehicles.len()
    }

    fn ensure_derived(&mut self, book: &TripBook) {
        if self.ready {
            return;
        }
        self.requests = book
            .iter()
            .filter_map(|(id, t)| match t.state {
                TripState::AwaitingPath { request } => Some((request, id)),
                _ => None,
            })
            .collect();
        self.ready = true;
    }
}

/// The access point of a building as the path service wants it.
fn access_of(net: &NetGraph, b: &Building) -> Option<Access> {
    let seg = net.segment(b.access.segment)?;
    let s = seg.curve.arc_table().t_to_s(b.access.t);
    let p = seg.curve.point(b.access.t);
    let tg = seg.curve.tangent(b.access.t);
    // Left of the travel direction in XZ is (t.z, -t.x), as in `cp_zone`.
    let left = Vec2::new(tg.z, -tg.x);
    let side = if (b.pos - to_xz(p)).dot(left) >= 0.0 {
        Side::Left
    } else {
        Side::Right
    };
    Some(Access {
        segment: b.access.segment,
        s,
        side,
    })
}

fn travel_ticks(cost_s: f32) -> u64 {
    ((cost_s / MOVE_SECONDS_PER_TICK).ceil() as u64).max(1)
}

/// `Agents` phase: submits path requests for freshly planned trips.
#[allow(clippy::too_many_arguments)]
pub fn route_trips(
    tick: Res<Tick>,
    net: Res<NetGraph>,
    pnet: Res<PathNetwork>,
    buildings: Res<BuildingGrid>,
    mut service: ResMut<PathService>,
    mut book: ResMut<TripBook>,
    mut grid: ResMut<CitizenGrid>,
    mut agents: ResMut<Agents>,
) {
    book.ensure_derived();
    agents.ensure_derived(&book);
    for tid in book.planned() {
        let Some(trip) = book.get(tid).cloned() else {
            continue;
        };
        let access = buildings
            .get(trip.from)
            .zip(buildings.get(trip.to))
            .and_then(|(a, b)| Some((access_of(&net, a)?, access_of(&net, b)?)));
        let Some((from, to)) = access else {
            agents.stats.no_path += 1;
            cancel_trip(&mut grid, &mut book, tid);
            continue;
        };
        let mode = match trip.mode {
            TripMode::Walk => Mode::Pedestrian,
            TripMode::Car => Mode::Car,
        };
        let RequestId(request) = service.submit(&pnet, *tick, PathRequest { mode, from, to });
        agents.requests.insert(request, tid);
        book.set_state(tid, TripState::AwaitingPath { request });
    }
}

/// `Paths` phase (after `deliver_paths`): turns results into agents or abstract trips.
pub fn receive_paths(
    tick: Res<Tick>,
    results: Res<PathResults>,
    limits: Res<SimLimits>,
    mut book: ResMut<TripBook>,
    mut grid: ResMut<CitizenGrid>,
    mut agents: ResMut<Agents>,
) {
    book.ensure_derived();
    agents.ensure_derived(&book);
    let now = tick.0;
    for outcome in &results.0 {
        let Some(tid) = agents.requests.remove(&outcome.id.0) else {
            continue;
        };
        let Some(trip) = book.get(tid).cloned() else {
            continue;
        };
        if grid.citizen(trip.citizen).is_none() {
            book.remove(tid);
            book.cancelled += 1;
            continue;
        }
        match &outcome.result {
            PathResult::Stale => book.set_state(tid, TripState::Planned),
            PathResult::Pedestrian(Err(NoPath)) | PathResult::Car(Err(NoPath)) => {
                agents.stats.no_path += 1;
                cancel_trip(&mut grid, &mut book, tid);
            }
            PathResult::Pedestrian(Ok(path)) if trip.mode == TripMode::Walk => {
                if agents.pedestrians.len() < limits.pedestrians as usize {
                    agents.pedestrians.insert(Pedestrian {
                        trip: tid,
                        citizen: trip.citizen,
                        route: path.clone(),
                        leg: 0,
                        s: 0.0,
                        leg_len: 0.0,
                        started: false,
                        waited: 0,
                        pos: Vec2::ZERO,
                    });
                    book.set_state(tid, TripState::Agent);
                } else {
                    agents.stats.abstract_trips += 1;
                    book.set_state(
                        tid,
                        TripState::InTransit {
                            arrive_at: Tick(now + travel_ticks(path.cost)),
                        },
                    );
                }
            }
            PathResult::Car(Ok(path)) if trip.mode == TripMode::Car => {
                let arrive_at = Tick(now + travel_ticks(path.cost));
                if agents.vehicles.len() < limits.vehicles as usize {
                    agents.vehicles.insert(Vehicle {
                        trip: tid,
                        citizen: trip.citizen,
                        path: path.clone(),
                        arrive_at,
                        stuck_timer: 0,
                    });
                    book.set_state(tid, TripState::Agent);
                } else {
                    agents.stats.abstract_trips += 1;
                    book.set_state(tid, TripState::InTransit { arrive_at });
                }
            }
            // A result of the wrong mode cannot happen for a request we submitted.
            _ => book.set_state(tid, TripState::Planned),
        }
    }
}

/// What a leg of a route is.
enum Leg {
    Walk {
        lane: LaneId,
        from_s: f32,
        to_s: f32,
    },
    Cross {
        curve: CubicBezier3,
        node: NodeId,
        link: u16,
    },
}

/// Everything a pedestrian step reads (all shared and immutable).
struct Ctx<'a> {
    net: &'a NetGraph,
    graph: &'a PathGraph,
    geo: &'a JunctionGeometry,
    tick: u64,
}

impl Ctx<'_> {
    /// Leg `i` of `route`, resolved against the current network.
    fn leg(&self, route: &PedPath, i: usize) -> Option<Leg> {
        let n = route.states.len();
        let end_s =
            |state: (LaneId, SegEnd)| self.graph.ped_lane(state.0).map(|l| l.end_s(state.1));
        if n == 0 {
            self.graph.ped_lane(route.start)?;
            return Some(Leg::Walk {
                lane: route.start,
                from_s: route.start_s,
                to_s: route.goal_s,
            });
        }
        if i == 0 {
            return Some(Leg::Walk {
                lane: route.start,
                from_s: route.start_s,
                to_s: end_s(route.states[0])?,
            });
        }
        if i == n {
            return Some(Leg::Walk {
                lane: route.goal,
                from_s: end_s(route.states[n - 1])?,
                to_s: route.goal_s,
            });
        }
        let (a, b) = (route.states[i - 1], route.states[i]);
        if a.0 == b.0 {
            return Some(Leg::Walk {
                lane: a.0,
                from_s: end_s(a)?,
                to_s: end_s(b)?,
            });
        }
        // Crossing a junction link at the node where `a` ends.
        let seg = self.net.segment(self.net.lane(a.0)?.segment)?;
        let node = if a.1 == SegEnd::Start {
            seg.start
        } else {
            seg.end
        };
        let junction = self.geo.get(node)?;
        let (idx, link) = junction
            .links
            .pedestrian
            .iter()
            .enumerate()
            .find(|(_, l)| (l.a == a.0 && l.b == b.0) || (l.a == b.0 && l.b == a.0))?;
        // Orient the curve from `a` towards `b`.
        let curve = if link.a == a.0 {
            link.curve
        } else {
            CubicBezier3::new(link.curve.p3, link.curve.p2, link.curve.p1, link.curve.p0)
        };
        Some(Leg::Cross {
            curve,
            node,
            link: idx as u16,
        })
    }

    fn leg_len(&self, leg: &Leg) -> f32 {
        match leg {
            Leg::Walk { from_s, to_s, .. } => (to_s - from_s).abs(),
            Leg::Cross { curve, .. } => curve.arc_table().length(),
        }
    }

    fn walk_green(&self, node: NodeId, link: u16) -> bool {
        let Some(j) = self.geo.get(node) else {
            return true;
        };
        let crossing = j.links.pedestrian.get(usize::from(link));
        match (crossing, j.control.light.as_ref()) {
            (Some(l), Some(plan)) if l.kind == cp_net::PedLinkKind::Crossing => {
                plan.walk(link, self.tick)
            }
            _ => true,
        }
    }

    fn position(&self, leg: &Leg, s: f32, len: f32) -> Vec2 {
        match leg {
            Leg::Walk { lane, from_s, to_s } => {
                let ls = if to_s >= from_s {
                    from_s + s
                } else {
                    from_s - s
                };
                self.graph
                    .ped_lane(*lane)
                    .map_or(Vec2::ZERO, |l| l.point_at(ls))
            }
            Leg::Cross { curve, .. } => {
                let t = if len > 1e-3 {
                    (s / len).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                to_xz(curve.point(t))
            }
        }
    }
}

enum Step {
    Moving,
    Arrived,
    Invalid,
}

/// One tick of walking for one pedestrian.
fn step_pedestrian(p: &mut Pedestrian, ctx: &Ctx) -> Step {
    let mut budget = PED_STEP;
    loop {
        let Some(leg) = ctx.leg(&p.route, p.leg as usize) else {
            return Step::Invalid;
        };
        if !p.started {
            // Entering a leg: measure it; a crossing may only be entered on a walk signal.
            if let Leg::Cross { node, link, .. } = &leg
                && !ctx.walk_green(*node, *link)
            {
                p.waited += 1;
                return Step::Moving;
            }
            p.leg_len = ctx.leg_len(&leg);
            p.s = 0.0;
            p.started = true;
        }
        let room = p.leg_len - p.s;
        if budget < room {
            p.s += budget;
            p.pos = ctx.position(&leg, p.s, p.leg_len);
            return Step::Moving;
        }
        budget -= room.max(0.0);
        p.pos = ctx.position(&leg, p.leg_len, p.leg_len);
        if p.leg as usize >= p.route.states.len() {
            return Step::Arrived;
        }
        p.leg += 1;
        p.started = false;
        p.s = 0.0;
        if budget <= 0.0 {
            return Step::Moving;
        }
    }
}

/// Which crossing `p` is currently walking across: `(junction node, index into its
/// pedestrian links)`, when its current leg is a junction crossing that it has entered. For
/// debug overlays and the "no walking on red" test.
pub fn crossing_of(
    p: &Pedestrian,
    net: &NetGraph,
    graph: &PathGraph,
    geo: &JunctionGeometry,
) -> Option<(NodeId, u16)> {
    if !p.started {
        return None;
    }
    let ctx = Ctx {
        net,
        graph,
        geo,
        tick: 0,
    };
    match ctx.leg(&p.route, p.leg as usize)? {
        Leg::Cross { node, link, .. } => Some((node, link)),
        Leg::Walk { .. } => None,
    }
}

/// `Agents` phase: moves pedestrians, finishes arrived agents' trips and frees their slots.
#[allow(clippy::too_many_arguments)]
pub fn move_agents(
    tick: Res<Tick>,
    net: Res<NetGraph>,
    geo: Res<JunctionGeometry>,
    pnet: Res<PathNetwork>,
    buildings: Res<BuildingGrid>,
    rng: Res<SimRng>,
    settings: Option<Res<AgentSettings>>,
    mut agents: ResMut<Agents>,
    mut grid: ResMut<CitizenGrid>,
    mut book: ResMut<TripBook>,
) {
    let now = tick.0;
    book.ensure_derived();
    agents.ensure_derived(&book);
    let ctx = Ctx {
        net: &net,
        graph: pnet.graph(),
        geo: &geo,
        tick: now,
    };
    let parallel = settings.is_none_or(|s| s.parallel);

    // Pedestrians: read-only shared state, each writes only itself.
    let mut refs: Vec<(PedestrianId, &mut Pedestrian)> = agents.pedestrians.iter_mut().collect();
    let results: Vec<(PedestrianId, Step)> = if parallel && refs.len() > PED_CHUNK {
        let pool = ComputeTaskPool::get_or_init(TaskPool::default);
        let chunks: Vec<Vec<(PedestrianId, Step)>> = pool.scope(|scope| {
            for chunk in refs.chunks_mut(PED_CHUNK) {
                let ctx = &ctx;
                scope.spawn(async move {
                    chunk
                        .iter_mut()
                        .map(|(id, p)| (*id, step_pedestrian(p, ctx)))
                        .collect()
                });
            }
        });
        chunks.into_iter().flatten().collect()
    } else {
        refs.iter_mut()
            .map(|(id, p)| (*id, step_pedestrian(p, &ctx)))
            .collect()
    };
    drop(refs);

    // Apply in id order.
    let mut red = 0u64;
    for (id, step) in results {
        match step {
            Step::Moving => {}
            Step::Arrived => {
                if let Some(p) = agents.pedestrians.remove(id) {
                    red += u64::from(p.waited);
                    agents.stats.walked += 1;
                    complete_trip(&mut grid, &mut book, &buildings, &rng, now, p.trip);
                }
            }
            Step::Invalid => {
                if let Some(p) = agents.pedestrians.remove(id) {
                    agents.stats.invalid_routes += 1;
                    cancel_trip(&mut grid, &mut book, p.trip);
                }
            }
        }
    }
    agents.stats.red_waits += red;

    // Vehicles (placeholder motion): arrive when the path cost says so.
    let due: Vec<VehicleId> = agents
        .vehicles
        .iter()
        .filter(|(_, v)| v.arrive_at.0 <= now)
        .map(|(id, _)| id)
        .collect();
    for id in due {
        if let Some(v) = agents.vehicles.remove(id) {
            agents.stats.driven += 1;
            complete_trip(&mut grid, &mut book, &buildings, &rng, now, v.trip);
        }
    }
}
