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
//! Save stability: serialised with postcard; enums are append-only.

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use cp_building::{BuildingGrid, BuildingId};
use cp_citizen::{CitizenId, TripId};
use cp_content::VehicleKind;
use cp_core::{SimLimits, Tick};
use cp_net::NetGraph;
use serde::{Deserialize, Serialize};

use crate::agents::{Agents, Vehicle, access_of, travel_ticks};
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceRun {
    /// `cp_content::VehicleKind` as a number.
    pub kind: u8,
    pub from: BuildingId,
    pub to: BuildingId,
    pub state: RunState,
}

#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceRuns {
    next: RunId,
    runs: BTreeMap<RunId, ServiceRun>,
}

impl ServiceRuns {
    /// Starts a run of `kind` from `from` to `to`.
    pub fn start(&mut self, kind: VehicleKind, from: BuildingId, to: BuildingId) -> RunId {
        let id = self.next;
        self.next += 1;
        self.runs.insert(
            id,
            ServiceRun {
                kind: kind as u8,
                from,
                to,
                state: RunState::Planned,
            },
        );
        id
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
            .find(|(_, r)| r.kind == kind as u8 && r.to == to)
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

/// `Agents` phase, after `route_trips`: asks for the route of every planned run.
pub fn route_service_runs(
    tick: Res<Tick>,
    net: Res<NetGraph>,
    pnet: Res<PathNetwork>,
    buildings: Res<BuildingGrid>,
    mut service: ResMut<PathService>,
    mut runs: ResMut<ServiceRuns>,
) {
    let planned: Vec<RunId> = runs
        .iter()
        .filter(|(_, r)| r.state == RunState::Planned)
        .map(|(id, _)| id)
        .collect();
    for id in planned {
        let Some(run) = runs.get(id).copied() else {
            continue;
        };
        let access = buildings
            .get(run.from)
            .zip(buildings.get(run.to))
            .and_then(|(a, b)| Some((access_of(&net, a)?, access_of(&net, b)?)));
        match access {
            Some((from, to)) => {
                let request = PathRequest {
                    mode: Mode::Car,
                    from,
                    to,
                };
                let r = service.submit(&pnet, *tick, request).0;
                runs.set_state(id, RunState::AwaitingPath { request: r });
            }
            None => runs.set_state(id, RunState::Failed),
        }
    }
}

/// `Paths` phase, after `receive_paths`: turns the routes of runs into vehicles or timers.
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
            .find(|(_, r)| {
                r.state
                    == RunState::AwaitingPath {
                        request: outcome.id.0,
                    }
            })
            .map(|(id, r)| (id, *r))
        else {
            continue;
        };
        match &outcome.result {
            PathResult::Stale => runs.set_state(id, RunState::Planned),
            PathResult::Car(Ok(path)) => {
                if agents.vehicle_count() < limits.vehicles as usize {
                    agents.spawn_service_vehicle(Vehicle {
                        trip: TripId::new(id, SERVICE_GEN),
                        citizen: CitizenId::new(u32::MAX, u32::MAX),
                        path: path.clone(),
                        kind: run.kind,
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
