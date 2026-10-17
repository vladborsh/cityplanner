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
//! **Vehicles** drive their path lane by lane and through the junction connections
//! ([`crate::drive`]): leader following, junction gates, parallel lane updates, removal of
//! stuck cars. Arrival, an invalid route and the stuck timeout all complete the trip.
//!
//! **Clocks (§2).** The traffic-light plan counts 50 ms game ticks; vehicles and pedestrians
//! read it on the *movement* clock ([`crate::drive::signal_tick`]), so a 20 s green is 20
//! seconds of movement, not 13 minutes of it.

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_tasks::{ComputeTaskPool, TaskPool};
use cp_building::{Building, BuildingFlags, BuildingGrid};
use cp_citizen::{
    CitizenGrid, CitizenId, TripBook, TripId, TripMode, TripPurpose, TripState, cancel_trip,
    complete_trip,
};
use cp_core::curve::{CubicBezier3, to_xz};
use cp_core::time::MOVE_SECONDS_PER_TICK;
use cp_core::{Arena, Id, SimLimits, SimRng, Tick};
use cp_net::{JunctionGeometry, LaneId, NetGraph, NodeId, NodeKind};
use serde::{Deserialize, Serialize};

use crate::astar::{CarPath, NoPath, PedPath};
use crate::drive::{
    DriveState, STUCK_TICKS, VehicleSpecs, drive_tick, lane_node, path_ok, signal_tick,
};
use crate::graph::{Access, CongestionMap, PED_SPEED, PathGraph, SegEnd, Side};
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
    /// The resolved current leg, so it is not looked up again every tick. Derived, not saved
    /// and not compared: it refills on the first tick after a load.
    #[serde(skip)]
    pub(crate) cache: LegCache,
}

/// A leg of a pedestrian's route resolved against the network of one path-graph generation.
#[derive(Clone, Debug, Default)]
pub(crate) struct LegCache(Option<(u64, u32, Leg)>);

impl PartialEq for LegCache {
    /// A cache never makes two pedestrians different.
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vehicle {
    pub trip: TripId,
    pub citizen: CitizenId,
    pub path: CarPath,
    /// `cp_content::VehicleKind` as a number.
    pub kind: u8,
    /// Index of the current lane in the path (`0` = start lane); while `in_conn`, the
    /// connection `path.steps[hop]` is being crossed.
    pub hop: u16,
    pub in_conn: bool,
    /// On the road (a new car waits until its start has room).
    pub spawned: bool,
    /// Front bumper, metres along the lane (or the connection curve).
    pub s: f32,
    /// Metres per second.
    pub speed: f32,
    /// Standstill counters packed in one `u32` (save format unchanged): the high 16 bits count
    /// ticks standing for any reason (hard cap 10 × the threshold), the low 16 bits ticks
    /// standing for a suspicious reason; the car disappears past [`AgentSettings::stuck_ticks`]
    /// of the latter (§9.4 v1.1).
    pub stuck_timer: u32,
    /// Id of the re-route request in flight (T-066): the network changed under the rest of
    /// the route, the car keeps to its lane (and stops at the end of the valid part) until the
    /// new route arrives.
    pub reroute: Option<u64>,
}

/// Flag bits in [`Vehicle::kind`] (T-154; the byte keeps its place in saves). The low bits are
/// the `cp_content::VehicleKind` number.
pub const KIND_MASK: u8 = 0x0F;
/// The vehicle of a keep run: it stops at its goal and stays instead of leaving the road.
pub const KIND_KEEP: u8 = 0x80;
/// A keep vehicle that has reached its goal and stands there.
pub const KIND_PARKED: u8 = 0x40;

impl Vehicle {
    /// The `cp_content::VehicleKind` number, without the flag bits.
    pub fn kind_id(&self) -> u8 {
        self.kind & KIND_MASK
    }

    /// Stops at its goal and stays (a keep run).
    pub fn is_keep(&self) -> bool {
        self.kind & KIND_KEEP != 0
    }

    /// Stands at the goal of a keep run.
    pub fn is_parked(&self) -> bool {
        self.kind & KIND_PARKED != 0
    }

    /// The lane it is on, or — inside a junction — the lane it came from.
    pub fn lane(&self) -> LaneId {
        if self.hop == 0 {
            self.path.start
        } else if self.in_conn {
            self.path.steps[usize::from(self.hop)].0
        } else {
            self.path.steps[usize::from(self.hop) - 1].1
        }
    }
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
    /// Cars removed after standing still for too long (trip completed abstractly).
    pub despawned_stuck: u64,
    /// Cars removed because the lane or junction connection they were on is gone, or the
    /// trip's destination lane no longer exists.
    pub invalid_vehicles: u64,
    /// Cars whose route changed under them and got a new one (T-066).
    pub rerouted: u64,
    /// Cars that asked for a new route and were told there is none (trip completed abstractly).
    pub reroute_failed: u64,
}

/// Load of every road segment, indexed by segment arena slot: the length of cars over the
/// length of vehicle lanes, `0..=1`. `inst` is the last tick's value (overlay), `ema` the
/// average over ~2 game hours (path costs, `CongestionMap`). Saved with the agents.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SegmentLoad {
    pub inst: Vec<f32>,
    pub ema: Vec<f32>,
    /// The `ema` last handed to the pathfinder (refreshed every [`LOAD_PUSH_TICKS`] ticks);
    /// kept so a load restores the exact congestion map the continuous run is using.
    pub published: Vec<f32>,
}

impl SegmentLoad {
    pub fn inst(&self, segment: cp_net::SegmentId) -> f32 {
        self.inst
            .get(segment.index as usize)
            .copied()
            .unwrap_or(0.0)
    }
}

/// Knobs (not saved).
#[derive(Resource, Clone, Copy, Debug)]
pub struct AgentSettings {
    /// Step pedestrians on the compute pool (the result is the same either way).
    pub parallel: bool,
    /// Ticks a car may stand still before it disappears (§9.4: 60 ticks = 120 s of movement).
    pub stuck_ticks: u32,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            parallel: true,
            stuck_ticks: STUCK_TICKS,
        }
    }
}

/// The agent pools.
#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct Agents {
    pedestrians: Arena<Pedestrian>,
    vehicles: Arena<Vehicle>,
    pub stats: AgentStats,
    pub load: SegmentLoad,
    /// Lane/connection occupancy; derived from the vehicles, rebuilt lazily.
    #[serde(skip)]
    drive: DriveState,
    /// Path request -> trip; derived from the trips, rebuilt lazily after a load.
    #[serde(skip)]
    requests: BTreeMap<u64, TripId>,
    /// Re-route request -> car; derived from `Vehicle::reroute`, rebuilt lazily after a load.
    #[serde(skip)]
    reroutes: BTreeMap<u64, VehicleId>,
    #[serde(skip)]
    ready: bool,
}

impl PartialEq for Agents {
    fn eq(&self, other: &Self) -> bool {
        self.pedestrians == other.pedestrians
            && self.vehicles == other.vehicles
            && self.stats == other.stats
            && self.load == other.load
    }
}

/// Rebuilds [`Agents`] from saved parts (loads and migrations).
impl Agents {
    /// Vehicle dimensions of the last driven tick (defaults before the first one).
    pub(crate) fn specs(&self) -> &VehicleSpecs {
        &self.drive.specs
    }

    pub fn from_parts(
        pedestrians: Arena<Pedestrian>,
        vehicles: Arena<Vehicle>,
        stats: AgentStats,
        load: SegmentLoad,
    ) -> Self {
        Self {
            pedestrians,
            vehicles,
            stats,
            load,
            ..Self::default()
        }
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// Puts a service vehicle (T-151) on the road: it waits for room at its start like a car.
    /// Puts a walking run's pedestrian (T-155) on the street.
    pub(crate) fn spawn_service_pedestrian(&mut self, p: Pedestrian) {
        self.pedestrians.insert(p);
    }

    pub(crate) fn vehicle_mut(&mut self, id: VehicleId) -> Option<&mut Vehicle> {
        self.vehicles.get_mut(id)
    }

    pub(crate) fn spawn_service_vehicle(&mut self, v: Vehicle) {
        let id = self.vehicles.insert(v);
        self.drive.waiting.push(id);
    }

    /// Takes the vehicle of service run `run` off the road, wherever it is (T-154 `release`).
    /// Returns whether there was one.
    pub(crate) fn remove_run_vehicle(&mut self, run: crate::runs::RunId) -> bool {
        let walker = self
            .pedestrians
            .iter()
            .find(|(_, p)| crate::runs::walk_run_of(p) == Some(run))
            .map(|(id, _)| id);
        if let Some(id) = walker {
            self.pedestrians.remove(id);
            return true;
        }
        let Some(id) = self
            .vehicles
            .iter()
            .find(|(_, v)| crate::runs::run_of(v) == Some(run))
            .map(|(id, _)| id)
        else {
            return false;
        };
        if let Some(v) = self.vehicles.remove(id) {
            self.drive.remove(&v, id);
            if let Some(r) = v.reroute {
                self.reroutes.remove(&r);
            }
        }
        true
    }

    pub fn vehicle(&self, id: VehicleId) -> Option<&Vehicle> {
        self.vehicles.get(id)
    }

    pub fn pedestrian(&self, id: PedestrianId) -> Option<&Pedestrian> {
        self.pedestrians.get(id)
    }

    pub fn pedestrians(&self) -> impl Iterator<Item = (PedestrianId, &Pedestrian)> {
        self.pedestrians.iter()
    }

    pub fn vehicles(&self) -> impl Iterator<Item = (VehicleId, &Vehicle)> {
        self.vehicles.iter()
    }

    /// Adds a vehicle that is not tied to a trip ([`crate::demo`]).
    pub(crate) fn insert_demo_vehicle(&mut self, v: Vehicle) {
        let id = self.vehicles.insert(v);
        self.drive.waiting.push(id);
    }

    /// Adds a pedestrian that is not tied to a trip ([`crate::demo`]).
    pub(crate) fn insert_demo_pedestrian(&mut self, p: Pedestrian) {
        self.pedestrians.insert(p);
    }

    /// The vehicle arena (frozen save writers read it).
    pub fn vehicle_arena(&self) -> &Arena<Vehicle> {
        &self.vehicles
    }

    /// The pedestrian arena (frozen save writers read it).
    pub fn pedestrian_arena(&self) -> &Arena<Pedestrian> {
        &self.pedestrians
    }

    pub fn pedestrian_count(&self) -> usize {
        self.pedestrians.len()
    }

    pub fn vehicle_count(&self) -> usize {
        self.vehicles.len()
    }

    /// Frees the agents and pending path requests of cancelled trips.
    /// Every id in the lane/connection lists and the spawn queue is a live vehicle (a debug
    /// check for tests that remove vehicles from outside the driving code).
    pub fn occupancy_is_live(&self) -> bool {
        let live = |id: &VehicleId| self.vehicles.contains(*id);
        self.drive.lanes.iter().flatten().all(live)
            && self.drive.conns.values().flatten().all(live)
            && self.drive.waiting.iter().all(live)
    }

    pub(crate) fn drop_trips(&mut self, trips: &[TripId]) {
        let ped: Vec<PedestrianId> = self
            .pedestrians
            .iter()
            .filter(|(_, p)| trips.contains(&p.trip))
            .map(|(id, _)| id)
            .collect();
        for id in ped {
            self.pedestrians.remove(id);
        }
        let veh: Vec<VehicleId> = self
            .vehicles
            .iter()
            .filter(|(_, v)| trips.contains(&v.trip))
            .map(|(id, _)| id)
            .collect();
        for id in veh {
            if let Some(v) = self.vehicles.remove(id) {
                // Also leave the lane/connection lists, or the dead id would block them.
                self.drive.remove(&v, id);
            }
        }
        self.requests.retain(|_, t| !trips.contains(t));
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
        self.reroutes = self
            .vehicles
            .iter()
            .filter_map(|(id, v)| v.reroute.map(|r| (r, id)))
            .collect();
        self.ready = true;
    }
}

/// `Citizens` phase, before `update_citizens`: reacts to buildings removed this tick (the
/// `Demolish` command, later abandonment) by dropping what pointed at them — households,
/// jobs, trips — and freeing the agents of the cancelled trips, so pools stay consistent.
pub fn purge_removed_buildings(
    mut buildings: ResMut<BuildingGrid>,
    mut grid: ResMut<CitizenGrid>,
    mut book: ResMut<TripBook>,
    mut agents: ResMut<Agents>,
    mut runs: Option<ResMut<crate::runs::ServiceRuns>>,
) {
    if !buildings.has_removed() {
        return;
    }
    let removed = buildings.take_removed();
    if let Some(runs) = runs.as_deref_mut() {
        crate::runs::fail_walks_to(runs, &mut agents, &removed);
    }
    book.ensure_derived();
    agents.ensure_derived(&book);
    let cancelled = cp_citizen::purge_buildings(&mut grid, &mut book, &removed);
    agents.drop_trips(&cancelled);
}

/// The access point of a building as the path service wants it.
pub(crate) fn access_of(net: &NetGraph, b: &Building) -> Option<Access> {
    let access = b.access()?;
    let seg = net.segment(access.segment)?;
    let s = seg.curve.arc_table().t_to_s(access.t);
    let p = seg.curve.point(access.t);
    let tg = seg.curve.tangent(access.t);
    // Left of the travel direction in XZ is (t.z, -t.x), as in `cp_zone`.
    let left = Vec2::new(tg.z, -tg.x);
    let side = if (b.pos - to_xz(p)).dot(left) >= 0.0 {
        Side::Left
    } else {
        Side::Right
    };
    Some(Access {
        segment: access.segment,
        s,
        side,
    })
}

/// The path request of a move-in car (T-118): from the lane leaving the `Outside` node nearest
/// to `home` to the lane of the home's access point.
fn move_in_request(net: &NetGraph, pnet: &PathNetwork, home: &Building) -> Option<PathRequest> {
    let g = pnet.graph();
    let (node, segment) = net
        .nodes()
        .filter(|(_, n)| n.kind == NodeKind::Outside && !cp_net::node_is_rail(net, n))
        .filter_map(|(id, n)| Some((id, *n.segments.first()?, n.pos)))
        .min_by(|a, b| {
            let d = |p: bevy_math::Vec3| to_xz(p).distance_squared(home.pos);
            d(a.2).total_cmp(&d(b.2)).then(a.0.cmp(&b.0))
        })
        .map(|(id, seg, _)| (id, seg))?;
    let from_start = net.segment(segment)?.start == node;
    let (from, from_s) = g.entry_lane(segment, from_start)?;
    let (to, to_s) = g.resolve_car_lane(&access_of(net, home)?)?;
    let lane_access = |segment, s| Access {
        segment,
        s,
        side: Side::Right,
    };
    Some(PathRequest {
        mode: Mode::CarLanes { from, to },
        from: lane_access(segment, from_s),
        to: lane_access(segment, to_s),
    })
}

pub(crate) fn travel_ticks(cost_s: f32) -> u64 {
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
        if trip.purpose == TripPurpose::MoveIn && trip.from == trip.to {
            // Waiting for the train to the station (T-167): `cp_transit` holds it.
            if trip.mode == TripMode::Transit {
                continue;
            }
            let request = buildings
                .get(trip.to)
                .and_then(|home| move_in_request(&net, &pnet, home));
            match request {
                Some(request) => {
                    let RequestId(id) = service.submit(&pnet, *tick, request);
                    agents.requests.insert(id, tid);
                    book.set_state(tid, TripState::AwaitingPath { request: id });
                }
                // No way in from the highway: the household arrives abstractly.
                None => cancel_trip(&mut grid, &mut book, tid),
            }
            continue;
        }
        // Riders belong to `cp_transit` (T-159).
        if trip.mode == TripMode::Transit {
            continue;
        }
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
            TripMode::Transit => continue,
        };
        let RequestId(request) = service.submit(&pnet, *tick, PathRequest { mode, from, to });
        agents.requests.insert(request, tid);
        book.set_state(tid, TripState::AwaitingPath { request });
    }
}

/// Submits the re-route request of `v`: from the lane it is on (or, inside a junction, the lane
/// it is entering) to its destination lane. `None` when either lane is gone from `g`.
fn submit_reroute(
    service: &mut PathService,
    pnet: &PathNetwork,
    now: Tick,
    v: &Vehicle,
) -> Option<u64> {
    let g = pnet.graph();
    let (from, from_s) = if v.in_conn {
        let to = v.path.steps.get(usize::from(v.hop))?.1;
        (to, lane_node(g, to)?.s_in)
    } else {
        (v.lane(), v.s)
    };
    g.car_slot(from)?;
    g.car_slot(v.path.goal)?;
    let access = |s: f32| Access {
        segment: Id::new(u32::MAX, 0),
        s,
        side: Side::Right,
    };
    let request = PathRequest {
        mode: Mode::CarLanes {
            from,
            to: v.path.goal,
        },
        from: access(from_s),
        to: access(v.path.goal_s),
    };
    Some(service.submit(pnet, now, request).0)
}

/// Swaps the vehicle onto `p` when `p` passes through the place it is at now. A route computed
/// from the lane it was on 8 ticks ago contains the lane (or junction connection) it has driven
/// on to since; a route that starts at the lane the car is entering gets that connection put in
/// front of it.
fn adopt_route(v: &mut Vehicle, p: CarPath) -> bool {
    if v.in_conn {
        let cur = v.path.steps[usize::from(v.hop)];
        if let Some(i) = p.steps.iter().position(|s| *s == cur) {
            v.hop = i as u16;
            v.path = p;
        } else if p.start == cur.1 {
            let mut steps = vec![cur];
            steps.extend(p.steps.iter().copied());
            v.hop = 0;
            v.path = CarPath {
                start: cur.0,
                start_s: v.path.start_s,
                steps,
                goal: p.goal,
                goal_s: p.goal_s,
                cost: p.cost,
            };
        } else {
            return false;
        }
    } else {
        let lane = v.lane();
        let Some(i) = p.lanes().position(|l| l == lane) else {
            return false;
        };
        v.hop = i as u16;
        v.path = p;
    }
    v.reroute = None;
    true
}

/// Removes a car that cannot go on and ends its trip abstractly.
#[allow(clippy::too_many_arguments)]
fn drop_vehicle(
    agents: &mut Agents,
    id: VehicleId,
    grid: &mut CitizenGrid,
    book: &mut TripBook,
    buildings: &BuildingGrid,
    rng: &SimRng,
    now: u64,
    runs: &mut Option<ResMut<crate::runs::ServiceRuns>>,
) {
    if let Some(v) = agents.vehicles.remove(id) {
        agents.drive.remove(&v, id);
        if let Some(run) = crate::runs::run_of(&v) {
            if let Some(runs) = runs.as_deref_mut() {
                runs.set_state(run, crate::runs::RunState::Failed);
            }
            return;
        }
        complete_trip(grid, book, buildings, rng, now, v.trip);
    }
}

/// `Paths` phase (after `deliver_paths`): turns results into agents or abstract trips.
#[allow(clippy::too_many_arguments)]
pub fn receive_paths(
    tick: Res<Tick>,
    results: Res<PathResults>,
    limits: Res<SimLimits>,
    pnet: Res<PathNetwork>,
    rng: Res<SimRng>,
    mut service: ResMut<PathService>,
    mut buildings: ResMut<BuildingGrid>,
    mut book: ResMut<TripBook>,
    mut grid: ResMut<CitizenGrid>,
    mut agents: ResMut<Agents>,
    mut runs: Option<ResMut<crate::runs::ServiceRuns>>,
) {
    book.ensure_derived();
    agents.ensure_derived(&book);
    let now = tick.0;
    for outcome in &results.0 {
        if let Some(vid) = agents.reroutes.remove(&outcome.id.0) {
            let live = agents
                .vehicles
                .get(vid)
                .is_some_and(|v| v.reroute == Some(outcome.id.0));
            if !live {
                continue;
            }
            // A route: adopt it if it passes through where the car is. No route: the trip
            // ends. Stale, or a route that misses the car: ask again from where it is now.
            let adopted = match &outcome.result {
                PathResult::Car(Ok(p)) => {
                    let v = agents.vehicles.get_mut(vid).expect("live vehicle");
                    adopt_route(v, p.clone())
                }
                _ => false,
            };
            if adopted {
                agents.stats.rerouted += 1;
            } else if matches!(outcome.result, PathResult::Car(Err(NoPath))) {
                agents.stats.reroute_failed += 1;
                drop_vehicle(
                    &mut agents,
                    vid,
                    &mut grid,
                    &mut book,
                    &buildings,
                    &rng,
                    now,
                    &mut runs,
                );
            } else {
                let again = agents
                    .vehicles
                    .get(vid)
                    .and_then(|v| submit_reroute(&mut service, &pnet, *tick, v));
                match again {
                    Some(r) => {
                        agents.vehicles.get_mut(vid).expect("vehicle").reroute = Some(r);
                        agents.reroutes.insert(r, vid);
                    }
                    None => {
                        agents.stats.reroute_failed += 1;
                        drop_vehicle(
                            &mut agents,
                            vid,
                            &mut grid,
                            &mut book,
                            &buildings,
                            &rng,
                            now,
                            &mut runs,
                        );
                    }
                }
            }
            continue;
        }
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
        if matches!(
            outcome.result,
            PathResult::Pedestrian(Ok(_)) | PathResult::Car(Ok(_))
        ) && trip.purpose != TripPurpose::MoveIn
        {
            for b in [trip.from, trip.to] {
                buildings.set_flag(b, BuildingFlags::NO_PATH, false);
            }
        }
        match &outcome.result {
            PathResult::Stale => book.set_state(tid, TripState::Planned),
            PathResult::Pedestrian(Err(NoPath)) | PathResult::Car(Err(NoPath)) => {
                agents.stats.no_path += 1;
                // Either end may be the cut-off one; a later successful trip clears the other.
                if trip.purpose != TripPurpose::MoveIn {
                    for b in [trip.from, trip.to] {
                        buildings.set_flag(b, BuildingFlags::NO_PATH, true);
                    }
                }
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
                        cache: LegCache::default(),
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
                if agents.vehicles.len() < limits.vehicles as usize {
                    let id = agents.vehicles.insert(Vehicle {
                        trip: tid,
                        citizen: trip.citizen,
                        path: path.clone(),
                        kind: 0,
                        hop: 0,
                        in_conn: false,
                        spawned: false,
                        s: path.start_s,
                        speed: 0.0,
                        stuck_timer: 0,
                        reroute: None,
                    });
                    agents.drive.waiting.push(id);
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
            // A result of the wrong mode cannot happen for a request we submitted.
            _ => book.set_state(tid, TripState::Planned),
        }
    }
}

/// What a leg of a route is.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Leg {
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
    /// Generation of the path graph: a cached leg is valid for one generation.
    generation: u64,
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
                // By arc length: a corner curve walked by its parameter runs faster in the
                // middle than at the ends (BUG-064).
                let t = if len > 1e-3 {
                    curve.arc_table().s_to_t(s)
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
        let leg = match p.cache.0 {
            Some((g, i, leg)) if g == ctx.generation && i == p.leg => leg,
            _ => {
                let Some(leg) = ctx.leg(&p.route, p.leg as usize) else {
                    return Step::Invalid;
                };
                p.cache.0 = Some((ctx.generation, p.leg, leg));
                leg
            }
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
        generation: 0,
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
    mut service: ResMut<PathService>,
    buildings: Res<BuildingGrid>,
    rng: Res<SimRng>,
    settings: Option<Res<AgentSettings>>,
    content: Option<Res<cp_content::SimContent>>,
    mut agents: ResMut<Agents>,
    mut grid: ResMut<CitizenGrid>,
    mut book: ResMut<TripBook>,
    mut runs: Option<ResMut<crate::runs::ServiceRuns>>,
) {
    let now = tick.0;
    book.ensure_derived();
    agents.ensure_derived(&book);
    let ctx = Ctx {
        net: &net,
        graph: pnet.graph(),
        geo: &geo,
        // Plan time on the movement clock (§2, three clocks).
        tick: signal_tick(now, 0),
        generation: pnet.generation(),
    };
    let (parallel, stuck_ticks) =
        settings.map_or((true, STUCK_TICKS), |s| (s.parallel, s.stuck_ticks));

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
                    if let Some(run) = crate::runs::walk_run_of(&p) {
                        // A walking run reports to its owner; the trip is not the walker's.
                        if let Some(runs) = runs.as_deref_mut() {
                            runs.set_state(run, crate::runs::RunState::Arrived { at: Tick(now) });
                        }
                    } else {
                        complete_trip(&mut grid, &mut book, &buildings, &rng, now, p.trip);
                    }
                }
            }
            Step::Invalid => {
                if let Some(p) = agents.pedestrians.remove(id) {
                    agents.stats.invalid_routes += 1;
                    if let Some(run) = crate::runs::walk_run_of(&p) {
                        if let Some(runs) = runs.as_deref_mut() {
                            runs.set_state(run, crate::runs::RunState::Failed);
                        }
                    } else {
                        cancel_trip(&mut grid, &mut book, p.trip);
                    }
                }
            }
        }
    }
    agents.stats.red_waits += red;

    // Vehicles.
    let graph = pnet.graph();
    let Agents {
        vehicles,
        drive,
        stats,
        reroutes,
        ..
    } = &mut *agents;
    let mut gone: Vec<Vehicle> = Vec::new();
    if !drive.ready || drive.generation != pnet.generation() {
        // Cars whose own place is gone vanish; cars whose route ahead is gone ask for a new
        // one from where they are and carry on meanwhile (T-066).
        for id in drive.rebuild(vehicles, graph, pnet.generation()) {
            if let Some(v) = vehicles.remove(id) {
                stats.invalid_vehicles += 1;
                gone.push(v);
            }
        }
        let broken: Vec<VehicleId> = vehicles
            .iter()
            .filter(|(_, v)| v.reroute.is_none() && !path_ok(graph, v))
            .map(|(id, _)| id)
            .collect();
        for id in broken {
            let request = vehicles
                .get(id)
                .and_then(|v| submit_reroute(&mut service, &pnet, *tick, v));
            match request {
                Some(r) => {
                    vehicles.get_mut(id).expect("vehicle").reroute = Some(r);
                    reroutes.insert(r, id);
                }
                None => {
                    // The destination lane itself is gone.
                    if let Some(v) = vehicles.remove(id) {
                        drive.remove(&v, id);
                        stats.invalid_vehicles += 1;
                        gone.push(v);
                    }
                }
            }
        }
    }
    let pool = parallel.then(|| ComputeTaskPool::get_or_init(TaskPool::default));
    let specs = content.map_or_else(VehicleSpecs::default, |c| VehicleSpecs::from_content(&c.0));
    let fin = drive_tick(
        vehicles,
        drive,
        graph,
        now,
        pool.map(|p| &**p),
        stuck_ticks,
        &specs,
    );
    for id in &fin.parked {
        if let Some(run) = vehicles.get(*id).and_then(crate::runs::run_of)
            && let Some(runs) = runs.as_deref_mut()
        {
            runs.set_state(
                run,
                crate::runs::RunState::Parked {
                    vehicle: *id,
                    since: Tick(now),
                },
            );
        }
    }
    stats.driven += fin.arrived.len() as u64;
    stats.invalid_vehicles += fin.invalid.len() as u64;
    stats.despawned_stuck += fin.stuck.len() as u64;
    let arrived = fin.arrived.into_iter().map(|v| (v, true));
    let lost = gone
        .into_iter()
        .chain(fin.invalid)
        .chain(fin.stuck)
        .map(|v| (v, false));
    for (v, ok) in arrived.chain(lost) {
        if let Some(run) = crate::runs::run_of(&v) {
            // A service vehicle: its run reports to `cp_service` (a lost one makes it estimate).
            if let Some(runs) = runs.as_deref_mut() {
                let state = if ok {
                    crate::runs::RunState::Arrived { at: Tick(now) }
                } else {
                    crate::runs::RunState::Failed
                };
                runs.set_state(run, state);
            }
            continue;
        }
        complete_trip(&mut grid, &mut book, &buildings, &rng, now, v.trip);
    }
}

/// `Stats` phase: segment load (instantaneous and EMA over ~2 game hours) from the lane
/// occupancy; every [`LOAD_PUSH_TICKS`] ticks the EMA becomes the pathfinder's congestion.
pub fn update_traffic_load(
    tick: Res<Tick>,
    mut pnet: ResMut<PathNetwork>,
    mut agents: ResMut<Agents>,
) {
    let graph = std::sync::Arc::clone(pnet.graph());
    let Agents {
        drive,
        load,
        vehicles,
        ..
    } = &mut *agents;
    let n = graph.seg_capacity.len();
    load.inst.clear();
    load.inst.resize(n, 0.0);
    load.ema.resize(n, 0.0);
    for slot in drive.occ.iter() {
        let l = &drive.lanes[slot];
        if let (false, Some(node)) = (l.is_empty(), graph.car.get(slot).and_then(Option::as_ref)) {
            // Road length the queue covers: the sum of the cars' lengths.
            let covered: f32 = l
                .iter()
                .filter_map(|&id| vehicles.get(id))
                .map(|v| drive.specs.of(v).length)
                .sum();
            load.inst[node.segment.index as usize] += covered;
        }
    }
    let alpha = 1.0 / EMA_TICKS;
    for i in 0..n {
        let cap = graph.seg_capacity[i];
        let x = if cap > 1.0 {
            (load.inst[i] / cap).min(1.0)
        } else {
            0.0
        };
        load.inst[i] = x;
        load.ema[i] += (x - load.ema[i]) * alpha;
    }
    if tick.0.is_multiple_of(LOAD_PUSH_TICKS) {
        load.published = load.ema.clone();
        pnet.set_congestion(CongestionMap(load.published.clone()));
    }
}

/// Averaging window of the segment load: 2 game hours of 170.67 ticks.
const EMA_TICKS: f32 = 341.0;
/// How often the pathfinder's congestion map is refreshed.
const LOAD_PUSH_TICKS: u64 = 32;
