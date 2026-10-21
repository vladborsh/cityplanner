//! Service runs (T-151, `architecture/system-design.md` §16 M2 D, v2.0): a fire engine or a
//! garbage truck that drives from a service building to a building asking for it. `cp_service`
//! starts a run and reads its outcome; `cp_traffic` finds the route, puts the vehicle on the
//! road from the shared pool (§9.4) and reports the tick it arrives. With the pool full the run
//! is abstract: it ends after the route's travel time (§3).
//!
//! A service vehicle is an ordinary [`Vehicle`] whose `kind` is not `Car` and whose `trip` is
//! `Id::new(run, SERVICE_GEN)`: no real trip ever has that generation, so the trip book never
//! finds it. A run that cannot be carried out ends as [`RunState::Failed`] and the requester
//! falls back to its abstract estimate.
//!
//! **Points and keep runs (T-154, §16 M3).** A run goes between [`RunPoint`]s: a building, an
//! access point beside a road, the `Outside` node of an entry road or a spot on a lane. A run
//! started with `keep` does not end at its goal: the vehicle stops there, holds its place on the
//! lane and the run becomes [`RunState::Parked`]. [`ServiceRuns::continue_run`] sends it on to
//! the next point from where it stands (a re-route from its lane, T-066); [`ServiceRuns::release`]
//! takes it off the road. This is how buses and trains of `cp_transit` drive a line.
//!
//! Save stability: serialised with postcard; enums are append-only.

use std::collections::{BTreeMap, BTreeSet};

use bevy_ecs::prelude::*;
use cp_building::{BuildingGrid, BuildingId};
use cp_citizen::{CitizenId, TripId};
use cp_content::{VehicleClass, VehicleKind};
use cp_core::{SimLimits, Tick};
use cp_net::{LaneId, NetGraph, NodeId, NodeKind};
use serde::{Deserialize, Serialize};

use crate::agents::{Agents, KIND_KEEP, Pedestrian, Vehicle, VehicleId, access_of, travel_ticks};
use crate::graph::{Access, PathGraph, Side};
use crate::service::{Mode, PathNetwork, PathRequest, PathResult, PathResults, PathService};

/// Generation of the `trip` id of a service vehicle (never reached by a real trip arena).
pub const SERVICE_GEN: u32 = u32::MAX;

pub type RunId = u32;

/// `ServiceRun::kind` of a walking run (T-155): a pedestrian, not a vehicle. No
/// `cp_content::VehicleKind` has this number, so the saved layout of a run is unchanged.
pub const WALK_KIND: u8 = u8::MAX;

/// Whether runs of this `ServiceRun::kind` are goods trucks (T-280): the cargo kinds, not the
/// garbage `Truck` nor any other service vehicle.
/// Whether `run` is a tourist's car (BUG-101): a car run that comes in from or goes out to an
/// `Outside` node (`cp_transit::tourists`; citizens travel on trips, not runs).
pub fn is_tourist_run(run: &ServiceRun) -> bool {
    run.kind == VehicleKind::Car as u8
        && (matches!(run.from, RunPoint::Outside(_)) || matches!(run.to, RunPoint::Outside(_)))
}

pub fn is_goods_kind(kind: u8) -> bool {
    [
        VehicleKind::Cargo,
        VehicleKind::CargoFarm,
        VehicleKind::CargoLogs,
        VehicleKind::CargoOre,
        VehicleKind::CargoTanker,
    ]
    .iter()
    .any(|k| *k as u8 == kind)
}

/// Whether runs of this `ServiceRun::kind` ride the track (metro and intercity trains).
pub fn is_rail_kind(kind: u8) -> bool {
    kind == VehicleKind::Metro as u8
        || kind == VehicleKind::Train as u8
        || kind == VehicleKind::CargoTrain as u8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    /// Waiting for `cp_traffic` to submit a path request.
    Planned,
    AwaitingPath {
        request: u64,
    },
    /// The vehicle is on the road.
    Driving,
    /// Abstract: no free agent, arrives at `arrive_at`.
    Timer {
        arrive_at: Tick,
    },
    Arrived {
        at: Tick,
    },
    /// No route, no access point or the vehicle was lost: the requester estimates itself.
    Failed,
    /// A keep run's vehicle stands at the goal since `since` (T-154).
    Parked {
        vehicle: VehicleId,
        since: Tick,
    },
    /// [`ServiceRuns::continue_run`] was called: waiting for `cp_traffic` to ask for the route
    /// from where the parked `vehicle` stands.
    ContinuePlanned {
        vehicle: VehicleId,
    },
    /// The route from the parked `vehicle` is being computed.
    Continuing {
        request: u64,
        vehicle: VehicleId,
    },
}

/// An end of a run (T-154).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum RunPoint {
    Building(BuildingId),
    /// A place beside a road (a bus stop).
    Access(Access),
    /// The `Outside` node of an entry road: runs start on the lane leaving it and end on the
    /// lane that reaches it.
    Outside(NodeId),
    /// A spot on a vehicle lane, `s` lane metres from the lane start.
    Lane {
        lane: LaneId,
        s: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceRun {
    /// `cp_content::VehicleKind` as a number.
    pub kind: u8,
    pub from: RunPoint,
    pub to: RunPoint,
    /// The vehicle stays at the goal ([`RunState::Parked`]) until continued or released.
    pub keep: bool,
    pub state: RunState,
}

/// Why a run cannot be continued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunError {
    NoSuchRun,
    /// The run is not standing at a goal (not parked, nor arrived abstractly).
    NotAtGoal,
}

#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct ServiceRuns {
    next: RunId,
    runs: BTreeMap<RunId, ServiceRun>,
    /// The runs going to each building, by kind (T-256: [`Self::find`] walked every run for
    /// each building a service looked at). Derived: not saved; built by the first change after
    /// a load, and until then `find` walks the runs as before.
    #[serde(skip)]
    by_to: Option<BTreeMap<(u8, BuildingId), BTreeSet<RunId>>>,
}

impl PartialEq for ServiceRuns {
    fn eq(&self, other: &Self) -> bool {
        self.next == other.next && self.runs == other.runs
    }
}

impl ServiceRuns {
    /// Rebuilds the runs from saved parts (the migrations of `cp_save`).
    pub fn from_parts(next: RunId, runs: BTreeMap<RunId, ServiceRun>) -> Self {
        Self {
            next,
            runs,
            by_to: None,
        }
    }

    /// The index of runs by goal building, built when missing.
    fn index(&mut self) -> &mut BTreeMap<(u8, BuildingId), BTreeSet<RunId>> {
        let runs = &self.runs;
        self.by_to.get_or_insert_with(|| {
            let mut m: BTreeMap<(u8, BuildingId), BTreeSet<RunId>> = BTreeMap::new();
            for (&id, r) in runs {
                if let RunPoint::Building(b) = r.to {
                    m.entry((r.kind, b)).or_default().insert(id);
                }
            }
            m
        })
    }

    fn index_add(&mut self, id: RunId) {
        let Some(r) = self.runs.get(&id).copied() else {
            return;
        };
        if let RunPoint::Building(b) = r.to {
            self.index().entry((r.kind, b)).or_default().insert(id);
        }
    }

    fn index_drop(&mut self, id: RunId) {
        let Some(r) = self.runs.get(&id).copied() else {
            return;
        };
        if let RunPoint::Building(b) = r.to {
            let m = self.index();
            if let Some(set) = m.get_mut(&(r.kind, b)) {
                set.remove(&id);
                if set.is_empty() {
                    m.remove(&(r.kind, b));
                }
            }
        }
    }

    /// The next run id and the runs (frozen save writers read them).
    pub fn parts(&self) -> (RunId, &BTreeMap<RunId, ServiceRun>) {
        (self.next, &self.runs)
    }

    /// Starts a run of `kind` from building `from` to building `to`.
    pub fn start(&mut self, kind: VehicleKind, from: BuildingId, to: BuildingId) -> RunId {
        self.start_run(
            kind,
            RunPoint::Building(from),
            RunPoint::Building(to),
            false,
        )
    }

    /// Starts a run between any two points; with `keep` the vehicle stays at `to`.
    pub fn start_run(
        &mut self,
        kind: VehicleKind,
        from: RunPoint,
        to: RunPoint,
        keep: bool,
    ) -> RunId {
        let id = self.next;
        self.next += 1;
        self.runs.insert(
            id,
            ServiceRun {
                kind: kind as u8,
                from,
                to,
                keep,
                state: RunState::Planned,
            },
        );
        self.index_add(id);
        id
    }

    /// Starts a walking run (T-155) between two points that have pavement: buildings or access
    /// points. A pedestrian walks it from the pool of §9.4; on arrival the run is
    /// [`RunState::Arrived`] and no trip is completed (the owner, `cp_transit`, runs the trip).
    /// With the pool full it arrives after the walking time; if a building at either end is
    /// demolished on the way it is [`RunState::Failed`].
    pub fn start_walk(&mut self, from: RunPoint, to: RunPoint) -> RunId {
        let id = self.next;
        self.next += 1;
        self.runs.insert(
            id,
            ServiceRun {
                kind: WALK_KIND,
                from,
                to,
                keep: false,
                state: RunState::Planned,
            },
        );
        self.index_add(id);
        id
    }

    /// Sends a run that stands at its goal on to `to`, from where it stands. A parked vehicle
    /// asks for a route from its lane; a keep run that arrived abstractly (the pool was full)
    /// starts over from its old goal.
    pub fn continue_run(&mut self, id: RunId, to: RunPoint) -> Result<(), RunError> {
        let at_goal = matches!(
            self.runs.get(&id).ok_or(RunError::NoSuchRun)?,
            ServiceRun {
                state: RunState::Parked { .. },
                ..
            } | ServiceRun {
                state: RunState::Arrived { .. },
                keep: true,
                ..
            }
        );
        if at_goal {
            self.index_drop(id);
        }
        let result = self.continue_run_inner(id, to);
        if at_goal {
            self.index_add(id);
        }
        result
    }

    fn continue_run_inner(&mut self, id: RunId, to: RunPoint) -> Result<(), RunError> {
        let run = self.runs.get_mut(&id).ok_or(RunError::NoSuchRun)?;
        match run.state {
            RunState::Parked { vehicle, .. } => {
                run.from = run.to;
                run.to = to;
                run.state = RunState::ContinuePlanned { vehicle };
            }
            RunState::Arrived { .. } if run.keep => {
                run.from = run.to;
                run.to = to;
                run.state = RunState::Planned;
            }
            _ => return Err(RunError::NotAtGoal),
        }
        Ok(())
    }

    /// [`Self::continue_run`] for a train that turns round at the end of its line (T-163): the
    /// next leg starts from the opposite lane of the same segment at the same place (the other
    /// `side` of the access point it stands at). The turn-round time is the caller's to wait.
    pub fn continue_run_reversed(&mut self, id: RunId, to: RunPoint) -> Result<(), RunError> {
        let run = self.runs.get(&id).ok_or(RunError::NoSuchRun)?;
        let RunPoint::Access(at) = run.to else {
            return Err(RunError::NotAtGoal);
        };
        self.continue_run(id, to)?;
        if let Some(run) = self.runs.get_mut(&id) {
            run.from = RunPoint::Access(Access {
                side: match at.side {
                    Side::Left => Side::Right,
                    Side::Right => Side::Left,
                },
                ..at
            });
        }
        Ok(())
    }

    /// Ends a run and takes its vehicle, if any, off the road, freeing its slot in the pool.
    /// Works in every state; returns whether the run existed.
    pub fn release(&mut self, id: RunId, agents: &mut Agents) -> bool {
        agents.remove_run_vehicle(id);
        self.index_drop(id);
        self.runs.remove(&id).is_some()
    }

    /// Takes the vehicle of a parked keep run off the road (BUG-100): it waits off the street,
    /// not in its lane, where every car behind it would stand until it leaves. The run stands at
    /// its goal abstractly ([`RunState::Arrived`]); [`Self::continue_run`] starts it over from
    /// there. Returns whether the run was parked.
    pub fn park_off_road(&mut self, id: RunId, agents: &mut Agents) -> bool {
        let Some(run) = self.runs.get_mut(&id) else {
            return false;
        };
        let RunState::Parked { since, .. } = run.state else {
            return false;
        };
        agents.remove_run_vehicle(id);
        run.state = RunState::Arrived { at: since };
        true
    }

    pub fn get(&self, id: RunId) -> Option<&ServiceRun> {
        self.runs.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (RunId, &ServiceRun)> {
        self.runs.iter().map(|(&id, r)| (id, r))
    }

    pub fn len(&self) -> usize {
        self.runs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// The run of `kind` going to `to`, if any (lowest id).
    pub fn find(&self, kind: VehicleKind, to: BuildingId) -> Option<(RunId, &ServiceRun)> {
        match &self.by_to {
            Some(m) => {
                let id = *m.get(&(kind as u8, to))?.first()?;
                self.runs.get(&id).map(|r| (id, r))
            }
            None => self
                .iter()
                .find(|(_, r)| r.kind == kind as u8 && r.to == RunPoint::Building(to)),
        }
    }

    /// Forgets a run (consumed or cancelled); its vehicle, if still driving, arrives unnoticed.
    pub fn remove(&mut self, id: RunId) -> Option<ServiceRun> {
        self.index_drop(id);
        self.runs.remove(&id)
    }

    /// Sets the state of a run (the drivers; owners' tests use it to let a vehicle arrive).
    pub fn set_state(&mut self, id: RunId, state: RunState) {
        if let Some(r) = self.runs.get_mut(&id) {
            r.state = state;
        }
    }
}

/// Whether `v` is a service vehicle; its run is `v.trip.index`.
pub fn run_of(v: &Vehicle) -> Option<RunId> {
    (v.trip.generation == SERVICE_GEN).then_some(v.trip.index)
}

/// Whether `p` walks a walking run (T-155); its run is `p.trip.index`.
pub fn walk_run_of(p: &crate::agents::Pedestrian) -> Option<RunId> {
    (p.trip.generation == SERVICE_GEN).then_some(p.trip.index)
}

/// The vehicle lane and lane metre a run point stands for: `origin` for the start of a run (an
/// `Outside` node is where the entry road's cars appear), otherwise for its goal (the lane
/// that reaches the node).
fn point_lane(
    net: &NetGraph,
    g: &PathGraph,
    buildings: &BuildingGrid,
    p: RunPoint,
    origin: bool,
) -> Option<(LaneId, f32)> {
    match p {
        RunPoint::Building(b) => {
            let access = access_of(net, buildings.get(b)?)?;
            g.resolve_car_lane(&access)
        }
        RunPoint::Access(a) => g.resolve_car_lane(&a),
        RunPoint::Lane { lane, s } => g.car_slot(lane).map(|_| (lane, s)),
        RunPoint::Outside(node) => {
            let n = net.node(node).filter(|n| n.kind == NodeKind::Outside)?;
            let segment = *n.segments.first()?;
            let from_start = net.segment(segment)?.start == node;
            // Leaving the node: forwards from the start node; reaching it: the other way.
            let (lane, s_in) = g.entry_lane(segment, from_start == origin)?;
            let s = if origin {
                s_in
            } else {
                crate::drive::lane_node(g, lane)?.s_out
            };
            Some((lane, s))
        }
    }
}

/// The path request of a run: between building or access points an ordinary car request, else
/// a lane-to-lane one (the entry road, a parked vehicle's lane). `from_lane` overrides the start
/// with where a parked vehicle stands.
fn run_request(
    net: &NetGraph,
    g: &PathGraph,
    buildings: &BuildingGrid,
    run: &ServiceRun,
    from_lane: Option<(LaneId, f32)>,
) -> Option<PathRequest> {
    if run.kind == WALK_KIND {
        let pave = |p: RunPoint| match p {
            RunPoint::Building(b) => access_of(net, buildings.get(b)?),
            RunPoint::Access(a) => Some(a),
            _ => None,
        };
        return Some(PathRequest {
            mode: Mode::Pedestrian,
            from: pave(run.from)?,
            to: pave(run.to)?,
        });
    }
    let plain = |p: RunPoint| match p {
        RunPoint::Building(b) => access_of(net, buildings.get(b)?),
        RunPoint::Access(a) => Some(a),
        _ => None,
    };
    if is_rail_kind(run.kind) {
        // A train goes between two places on the track, on the lanes the access points name.
        let (RunPoint::Access(from), RunPoint::Access(to)) = (run.from, run.to) else {
            return None;
        };
        return Some(PathRequest {
            mode: Mode::Rail,
            from,
            to,
        });
    }
    // Trucks avoid districts with the heavy traffic ban (T-195).
    let truck = run.kind == VehicleKind::Truck as u8;
    // T-302: a bus, tram or trolleybus keeps to the lanes its class may use.
    let class = VehicleKind::ALL
        .get(usize::from(run.kind))
        .map(|k| k.class())
        .filter(|c| !matches!(c, VehicleClass::Car | VehicleClass::Rail));
    if from_lane.is_none()
        && let (Some(a), Some(b)) = (plain(run.from), plain(run.to))
    {
        return Some(PathRequest {
            mode: match class {
                Some(class) => Mode::Class { class },
                None if truck => Mode::Truck,
                None => Mode::Car,
            },
            from: a,
            to: b,
        });
    }
    let (from, from_s) = match from_lane {
        Some(f) => f,
        None => point_lane(net, g, buildings, run.from, true)?,
    };
    let (to, to_s) = point_lane(net, g, buildings, run.to, false)?;
    let at = |s| Access {
        segment: cp_core::Id::new(u32::MAX, 0),
        s,
        side: Side::Right,
    };
    Some(PathRequest {
        mode: match class {
            Some(class) => Mode::ClassLanes { class, from, to },
            None if truck => Mode::TruckLanes { from, to },
            None => Mode::CarLanes { from, to },
        },
        from: at(from_s),
        to: at(to_s),
    })
}

/// `Agents` phase, after `route_trips`: asks for the route of every planned run, and of every
/// parked vehicle that is to continue.
pub fn route_service_runs(
    tick: Res<Tick>,
    net: Res<NetGraph>,
    pnet: Res<PathNetwork>,
    buildings: Res<BuildingGrid>,
    agents: Res<Agents>,
    mut service: ResMut<PathService>,
    mut runs: ResMut<ServiceRuns>,
) {
    let waiting: Vec<RunId> = runs
        .iter()
        .filter(|(_, r)| {
            matches!(
                r.state,
                RunState::Planned | RunState::ContinuePlanned { .. }
            )
        })
        .map(|(id, _)| id)
        .collect();
    for id in waiting {
        let Some(run) = runs.get(id).copied() else {
            continue;
        };
        let (from_lane, vehicle) = match run.state {
            RunState::ContinuePlanned { vehicle } => {
                match agents.vehicle(vehicle).filter(|v| v.is_parked()) {
                    Some(v) => (Some((v.lane(), v.s)), Some(vehicle)),
                    None => {
                        runs.set_state(id, RunState::Failed);
                        continue;
                    }
                }
            }
            _ => (None, None),
        };
        match run_request(&net, pnet.graph(), &buildings, &run, from_lane) {
            Some(request) => {
                let r = service.submit(&pnet, *tick, request).0;
                let state = match vehicle {
                    Some(vehicle) => RunState::Continuing {
                        request: r,
                        vehicle,
                    },
                    None => RunState::AwaitingPath { request: r },
                };
                runs.set_state(id, state);
            }
            None => runs.set_state(id, RunState::Failed),
        }
    }
}

/// `Paths` phase, after `receive_paths`: turns the routes of runs into vehicles or timers, and
/// puts parked vehicles on their next leg.
pub fn receive_service_paths(
    tick: Res<Tick>,
    results: Res<PathResults>,
    limits: Res<SimLimits>,
    mut runs: ResMut<ServiceRuns>,
    mut agents: ResMut<Agents>,
    rng: Res<cp_core::SimRng>,
    content: Option<Res<cp_content::SimContent>>,
) {
    // T-280: most goods trucks run on a timer; service vehicles always drive. Without content
    // (bare tests) every truck drives. BUG-101: so do most tourists, and no more than
    // `spawn_queue_cap` cars wait to enter at one lane of the entry.
    let (truck_share, tourist_share, queue_cap) = content.map_or((1.0, 1.0, u32::MAX), |c| {
        let b = c.0.balance();
        (
            b.truck_visible_share,
            b.tourist_visible_share,
            b.spawn_queue_cap,
        )
    });
    // Cars not yet on the road, by the lane they start on; counted when a run from outside first
    // needs it, and kept up as this pass adds cars.
    let mut waiting: Option<BTreeMap<LaneId, u32>> = None;
    for outcome in &results.0 {
        let Some((id, run)) = runs
            .iter()
            .find(|(_, r)| match r.state {
                RunState::AwaitingPath { request } | RunState::Continuing { request, .. } => {
                    request == outcome.id.0
                }
                _ => false,
            })
            .map(|(id, r)| (id, *r))
        else {
            continue;
        };
        if let RunState::Continuing { vehicle, .. } = run.state {
            match &outcome.result {
                PathResult::Stale => runs.set_state(id, RunState::ContinuePlanned { vehicle }),
                PathResult::Car(Ok(path)) => match agents.vehicle_mut(vehicle) {
                    Some(v) if v.is_parked() => {
                        v.path = path.clone();
                        v.hop = 0;
                        v.s = path.start_s;
                        v.speed = 0.0;
                        v.stuck_timer = 0;
                        v.kind &= !crate::agents::KIND_PARKED;
                        runs.set_state(id, RunState::Driving);
                    }
                    _ => runs.set_state(id, RunState::Failed),
                },
                // No route: the vehicle stays where it is; the owner decides (release).
                _ => runs.set_state(id, RunState::Failed),
            }
            continue;
        }
        if run.kind == WALK_KIND {
            match &outcome.result {
                PathResult::Stale => runs.set_state(id, RunState::Planned),
                PathResult::Pedestrian(Ok(path)) => {
                    if agents.pedestrian_count() < limits.pedestrians as usize {
                        agents.spawn_service_pedestrian(Pedestrian {
                            trip: TripId::new(id, SERVICE_GEN),
                            citizen: CitizenId::new(u32::MAX, u32::MAX),
                            route: path.clone(),
                            leg: 0,
                            s: 0.0,
                            leg_len: 0.0,
                            started: false,
                            waited: 0,
                            pos: bevy_math::Vec2::ZERO,
                            cache: Default::default(),
                        });
                        runs.set_state(id, RunState::Driving);
                    } else {
                        agents.stats.abstract_trips += 1;
                        runs.set_state(
                            id,
                            RunState::Timer {
                                arrive_at: Tick(tick.0 + travel_ticks(path.cost)),
                            },
                        );
                    }
                }
                _ => runs.set_state(id, RunState::Failed),
            }
            continue;
        }
        match &outcome.result {
            PathResult::Stale => runs.set_state(id, RunState::Planned),
            // A train is not an agent: it arrives when the route's time is up.
            PathResult::Rail(Ok(path)) if is_rail_kind(run.kind) => runs.set_state(
                id,
                RunState::Timer {
                    arrive_at: Tick(tick.0 + travel_ticks(path.cost)),
                },
            ),
            PathResult::Car(Ok(path)) => {
                let share = if is_goods_kind(run.kind) {
                    truck_share
                } else if is_tourist_run(&run) {
                    tourist_share
                } else {
                    1.0
                };
                let shown =
                    crate::agents::car_visible(TripId::new(id, SERVICE_GEN), rng.seed(), share);
                // Back-pressure at the entry: a full queue at the spawn lane takes no more.
                let queued = |waiting: &mut Option<BTreeMap<LaneId, u32>>, agents: &Agents| {
                    *waiting
                        .get_or_insert_with(|| {
                            let mut m = BTreeMap::new();
                            for (_, v) in agents.vehicles().filter(|(_, v)| !v.spawned) {
                                *m.entry(v.path.start).or_insert(0) += 1;
                            }
                            m
                        })
                        .get(&path.start)
                        .unwrap_or(&0)
                };
                let crowded = shown
                    && matches!(run.from, RunPoint::Outside(_))
                    && queued(&mut waiting, &agents) >= queue_cap;
                if !shown || crowded {
                    runs.set_state(
                        id,
                        RunState::Timer {
                            arrive_at: Tick(tick.0 + travel_ticks(path.cost)),
                        },
                    );
                } else if agents.vehicle_count() < limits.vehicles as usize {
                    if let Some(m) = waiting.as_mut() {
                        *m.entry(path.start).or_insert(0) += 1;
                    }
                    agents.spawn_service_vehicle(Vehicle {
                        trip: TripId::new(id, SERVICE_GEN),
                        citizen: CitizenId::new(u32::MAX, u32::MAX),
                        path: path.clone(),
                        kind: run.kind | if run.keep { KIND_KEEP } else { 0 },
                        hop: 0,
                        in_conn: false,
                        spawned: false,
                        s: path.start_s,
                        speed: 0.0,
                        stuck_timer: 0,
                        reroute: None,
                    });
                    runs.set_state(id, RunState::Driving);
                } else {
                    agents.stats.abstract_trips += 1;
                    runs.set_state(
                        id,
                        RunState::Timer {
                            arrive_at: Tick(tick.0 + travel_ticks(path.cost)),
                        },
                    );
                }
            }
            _ => runs.set_state(id, RunState::Failed),
        }
    }
}

/// `Agents` phase: abstract runs arrive when their time is up.
pub fn advance_service_runs(tick: Res<Tick>, mut runs: ResMut<ServiceRuns>) {
    let due: Vec<(RunId, Tick)> = runs
        .iter()
        .filter_map(|(id, r)| match r.state {
            RunState::Timer { arrive_at } if arrive_at <= *tick => Some((id, arrive_at)),
            _ => None,
        })
        .collect();
    for (id, at) in due {
        runs.set_state(id, RunState::Arrived { at });
    }
}

/// Fails the walking runs (T-155) that start or end at one of the `removed` buildings and takes
/// their pedestrians off the street.
pub(crate) fn fail_walks_to(runs: &mut ServiceRuns, agents: &mut Agents, removed: &[BuildingId]) {
    let hit: Vec<RunId> = runs
        .iter()
        .filter(|(_, r)| r.kind == WALK_KIND)
        .filter(|(_, r)| !matches!(r.state, RunState::Arrived { .. } | RunState::Failed))
        .filter(|(_, r)| {
            [r.from, r.to]
                .iter()
                .any(|p| matches!(p, RunPoint::Building(b) if removed.contains(b)))
        })
        .map(|(id, _)| id)
        .collect();
    for id in hit {
        agents.remove_run_vehicle(id);
        runs.set_state(id, RunState::Failed);
    }
}

#[cfg(test)]
mod index_tests {
    use super::*;

    /// T-256: `find` through the index answers like walking every run, as runs start, arrive,
    /// go on to another building and end.
    #[test]
    fn find_by_index_matches_the_walk_over_all_runs() {
        let b = |i: u32| BuildingId::new(i, 0);
        let mut runs = ServiceRuns::default();
        let mut ids = Vec::new();
        for i in 0..30u32 {
            let kind = [VehicleKind::Truck, VehicleKind::Mail][(i % 2) as usize];
            ids.push(runs.start(kind, b(100), b(i % 7)));
        }
        runs.set_state(ids[3], RunState::Failed);
        runs.remove(ids[4]);
        runs.remove(ids[11]);
        let walk = |runs: &ServiceRuns, kind: VehicleKind, to: BuildingId| {
            runs.iter()
                .find(|(_, r)| r.kind == kind as u8 && r.to == RunPoint::Building(to))
                .map(|(id, _)| id)
        };
        for kind in [
            VehicleKind::Truck,
            VehicleKind::Mail,
            VehicleKind::Emergency,
        ] {
            for i in 0..8 {
                assert_eq!(
                    runs.find(kind, b(i)).map(|(id, _)| id),
                    walk(&runs, kind, b(i))
                );
            }
        }
        // Rebuilt from saved parts: the walk until the first change, then the index again.
        let (next, parts) = runs.parts();
        let mut loaded = ServiceRuns::from_parts(next, parts.clone());
        assert_eq!(
            loaded.find(VehicleKind::Mail, b(1)).map(|(id, _)| id),
            walk(&runs, VehicleKind::Mail, b(1))
        );
        loaded.start(VehicleKind::Mail, b(100), b(1));
        assert_eq!(
            loaded.find(VehicleKind::Mail, b(1)).map(|(id, _)| id),
            walk(&loaded, VehicleKind::Mail, b(1))
        );
        assert_eq!(loaded.find(VehicleKind::Truck, b(9)), None);
    }
}

#[cfg(test)]
mod entry_tests {
    //! BUG-101: tourists flooded the entry (919 cars waiting at one spawn lane). A share of them
    //! drive; the rest and every run from outside while the spawn lane's queue is full go by
    //! their travel time.

    use std::sync::Arc;

    use bevy_ecs::system::RunSystemOnce;

    use super::*;
    use crate::astar::CarPath;
    use crate::service::{PathOutcome, RequestId};

    const LANE: LaneId = LaneId::new(7, 0);

    fn content(share: f32, cap: u32) -> cp_content::SimContent {
        let src = format!("Balance((tourist_visible_share: {share:?}, spawn_queue_cap: {cap}))");
        cp_content::SimContent(Arc::new(
            cp_content::ContentDb::from_sources(vec![("balance.ron".into(), src)])
                .unwrap_or_else(|e| panic!("{e}")),
        ))
    }

    fn path() -> CarPath {
        CarPath {
            start: LANE,
            start_s: 0.0,
            steps: Vec::new(),
            goal: LANE,
            goal_s: 50.0,
            cost: 60.0,
        }
    }

    /// `n` runs of `kind` from `from` get their route on [`LANE`], where `queued` cars already
    /// wait to enter. Returns the world after `receive_service_paths`.
    fn route(
        share: f32,
        cap: u32,
        queued: usize,
        n: u64,
        kind: VehicleKind,
        from: RunPoint,
    ) -> World {
        let mut world = World::new();
        let mut agents = Agents::default();
        for i in 0..queued {
            agents.spawn_service_vehicle(Vehicle {
                trip: TripId::new(10_000 + i as u32, SERVICE_GEN),
                citizen: CitizenId::new(u32::MAX, u32::MAX),
                path: path(),
                kind: VehicleKind::Car as u8,
                hop: 0,
                in_conn: false,
                spawned: false,
                s: 0.0,
                speed: 0.0,
                stuck_timer: 0,
                reroute: None,
            });
        }
        let mut runs = ServiceRuns::default();
        let mut outcomes = Vec::new();
        for i in 0..n {
            let id = runs.start_run(kind, from, RunPoint::Building(BuildingId::new(1, 0)), true);
            runs.set_state(id, RunState::AwaitingPath { request: i });
            outcomes.push(PathOutcome {
                id: RequestId(i),
                submitted: Tick(0),
                result: PathResult::Car(Ok(path())),
            });
        }
        world.insert_resource(Tick(100));
        world.insert_resource(PathResults(outcomes));
        world.insert_resource(SimLimits::desktop());
        world.insert_resource(runs);
        world.insert_resource(agents);
        world.insert_resource(cp_core::SimRng::from_seed([3; 32]));
        world.insert_resource(content(share, cap));
        world.run_system_once(receive_service_paths).unwrap();
        world
    }

    fn driving(world: &World) -> usize {
        world
            .resource::<ServiceRuns>()
            .iter()
            .filter(|(_, r)| r.state == RunState::Driving)
            .count()
    }

    fn outside() -> RunPoint {
        RunPoint::Outside(NodeId::new(1, 0))
    }

    #[test]
    fn no_more_than_the_cap_wait_at_a_blocked_entry_the_rest_go_by_time() {
        // 37 already wait at the entry lane, the cap is 40: 3 more cars, 97 timers.
        let w = route(1.0, 40, 37, 100, VehicleKind::Car, outside());
        assert_eq!(driving(&w), 3);
        let waiting = w
            .resource::<Agents>()
            .vehicles()
            .filter(|(_, v)| !v.spawned && v.path.start == LANE)
            .count();
        assert_eq!(waiting, 40);
        let timers = w
            .resource::<ServiceRuns>()
            .iter()
            .filter(
                |(_, r)| matches!(r.state, RunState::Timer { arrive_at } if arrive_at > Tick(100)),
            )
            .count();
        assert_eq!(timers, 97, "the others travel by their route's time");
    }

    #[test]
    fn a_share_of_the_tourists_drive_and_the_same_ones_each_time() {
        let a = route(0.15, u32::MAX, 0, 2000, VehicleKind::Car, outside());
        let n = driving(&a);
        assert!(
            (240..=360).contains(&n),
            "{n} of 2000 tourists drive (15 %)"
        );
        let b = route(0.15, u32::MAX, 0, 2000, VehicleKind::Car, outside());
        let ids = |w: &World| -> Vec<RunId> {
            w.resource::<ServiceRuns>()
                .iter()
                .filter(|(_, r)| r.state == RunState::Driving)
                .map(|(id, _)| id)
                .collect()
        };
        assert_eq!(
            ids(&a),
            ids(&b),
            "the hash of the run decides, nothing else"
        );
        // Every tourist drives at share 1.
        assert_eq!(
            driving(&route(1.0, u32::MAX, 0, 50, VehicleKind::Car, outside())),
            50
        );
    }

    #[test]
    fn the_cap_is_for_runs_from_outside_and_service_vehicles_always_drive() {
        // A fire engine from its station drives whatever waits at the lane.
        let w = route(
            0.0,
            5,
            30,
            10,
            VehicleKind::Emergency,
            RunPoint::Building(BuildingId::new(2, 0)),
        );
        assert_eq!(driving(&w), 10);
    }
}
