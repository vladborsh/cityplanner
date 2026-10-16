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

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use cp_building::{BuildingGrid, BuildingId};
use cp_citizen::{CitizenId, TripId};
use cp_content::VehicleKind;
use cp_core::{SimLimits, Tick};
use cp_net::{LaneId, NetGraph, NodeId, NodeKind};
use serde::{Deserialize, Serialize};

use crate::agents::{Agents, KIND_KEEP, Vehicle, VehicleId, access_of, travel_ticks};
use crate::graph::{Access, PathGraph, Side};
use crate::service::{Mode, PathNetwork, PathRequest, PathResult, PathResults, PathService};

/// Generation of the `trip` id of a service vehicle (never reached by a real trip arena).
pub const SERVICE_GEN: u32 = u32::MAX;

pub type RunId = u32;

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

#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceRuns {
    next: RunId,
    runs: BTreeMap<RunId, ServiceRun>,
}

impl ServiceRuns {
    /// Rebuilds the runs from saved parts (the migrations of `cp_save`).
    pub fn from_parts(next: RunId, runs: BTreeMap<RunId, ServiceRun>) -> Self {
        Self { next, runs }
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
        id
    }

    /// Sends a run that stands at its goal on to `to`, from where it stands. A parked vehicle
    /// asks for a route from its lane; a keep run that arrived abstractly (the pool was full)
    /// starts over from its old goal.
    pub fn continue_run(&mut self, id: RunId, to: RunPoint) -> Result<(), RunError> {
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

    /// Ends a run and takes its vehicle, if any, off the road, freeing its slot in the pool.
    /// Works in every state; returns whether the run existed.
    pub fn release(&mut self, id: RunId, agents: &mut Agents) -> bool {
        agents.remove_run_vehicle(id);
        self.runs.remove(&id).is_some()
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
        self.iter()
            .find(|(_, r)| r.kind == kind as u8 && r.to == RunPoint::Building(to))
    }

    /// Forgets a run (consumed or cancelled); its vehicle, if still driving, arrives unnoticed.
    pub fn remove(&mut self, id: RunId) -> Option<ServiceRun> {
        self.runs.remove(&id)
    }

    pub(crate) fn set_state(&mut self, id: RunId, state: RunState) {
        if let Some(r) = self.runs.get_mut(&id) {
            r.state = state;
        }
    }
}

/// Whether `v` is a service vehicle; its run is `v.trip.index`.
pub fn run_of(v: &Vehicle) -> Option<RunId> {
    (v.trip.generation == SERVICE_GEN).then_some(v.trip.index)
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
    let plain = |p: RunPoint| match p {
        RunPoint::Building(b) => access_of(net, buildings.get(b)?),
        RunPoint::Access(a) => Some(a),
        _ => None,
    };
    if from_lane.is_none()
        && let (Some(a), Some(b)) = (plain(run.from), plain(run.to))
    {
        return Some(PathRequest {
            mode: Mode::Car,
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
        mode: Mode::CarLanes { from, to },
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
) {
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
        match &outcome.result {
            PathResult::Stale => runs.set_state(id, RunState::Planned),
            PathResult::Car(Ok(path)) => {
                if agents.vehicle_count() < limits.vehicles as usize {
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
