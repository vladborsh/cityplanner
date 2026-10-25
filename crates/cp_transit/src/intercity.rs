//! Intercity trains (§16 M3 C, T-167). While a station is joined by rail to the `Outside` node of
//! the map's railway ([`connected_station`]), a train rides in from the edge every
//! `intercity_interval_h`, stands at the station for `intercity_dwell_h`, turns round and rides
//! out again. It is a timed run of `cp_traffic` on the track (like the metro), not an agent.
//!
//! What the train brings: the households that move in by train (a share of the arriving ones,
//! by their id; they wait in the `TripBook` as `MoveIn` trips of mode `Transit` and step off
//! with the next train to walk home) and a few tourists who walk from the station to the shops.
//! Without a station everything comes by car as before.

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use cp_building::BuildingGrid;
use cp_citizen::{CitizenGrid, TripBook, TripMode, TripPurpose, TripState};
use cp_content::{SimContent, VehicleKind};
use cp_core::time::TICKS_PER_HOUR;
use cp_core::{SimLimits, SimRng, Tick};
use cp_net::{NetGraph, NodeKind, RailReach, node_is_rail};
use cp_traffic::{
    Access, PathNetwork, RunId, RunPoint, RunState, ServiceRuns, Side, find_rail_path,
};
use serde::{Deserialize, Serialize};

use crate::model::{StopKind, TransitNet};
use crate::station::connected_station;
use crate::tourists::{Tourists, bring_by_train};

/// RNG stream of the tourists a train brings.
pub const INTERCITY_STREAM: u64 = 0x1274_3C17;
/// Hours the household that waits for a train is given on top of the wait before the abstract
/// arrival would let it in anyway.
const WAIT_MARGIN_H: f32 = 3.0;
/// Seconds of waiting before the next train when the first one is being sent: none.
const ENTRY_S: f32 = 1.0;

/// Where the train is. Append-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrainPhase {
    /// Riding in from the edge.
    Inbound,
    /// Standing at the station until `until`.
    Dwell { until: Tick },
    /// Riding out to the edge; the record goes when it is there.
    Outbound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Train {
    pub run: RunId,
    pub phase: TrainPhase,
    pub station: cp_building::BuildingId,
}

/// The intercity service: the train on the line (at most one) and what it has done. Serialized
/// sim state.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intercity {
    /// The next train leaves the edge at this tick or later.
    pub next_at: Tick,
    pub train: Option<Train>,
    /// Trains that have been to the station and back out.
    pub trains_done: u32,
    /// Households that stepped off a train to walk home.
    pub moved_in: u32,
}

impl Intercity {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

fn ticks_of_hours(h: f32) -> u64 {
    (h * TICKS_PER_HOUR).ceil().max(1.0) as u64
}

/// The two access points of the railway's edge end: where a train enters (on the lane that runs
/// away from the `Outside` node) and where it leaves (the other lane). `None` without a railway.
pub fn edge_access(net: &NetGraph) -> Option<(Access, Access, cp_net::NodeId)> {
    let (node, n) = net
        .nodes()
        .find(|(_, n)| n.kind == NodeKind::Outside && node_is_rail(net, n))?;
    let seg_id = *n.segments.first()?;
    let seg = net.segment(seg_id)?;
    // The forward lane is on the right of the segment's own direction.
    let (s, inbound) = if seg.start == node {
        (ENTRY_S.min(seg.length * 0.5), Side::Right)
    } else {
        ((seg.length - ENTRY_S).max(seg.length * 0.5), Side::Left)
    };
    let other = match inbound {
        Side::Right => Side::Left,
        Side::Left => Side::Right,
    };
    let access = |side| Access {
        segment: seg_id,
        s,
        side,
    };
    Some((access(inbound), access(other), node))
}

/// The side of `station`'s platform that the track reaches from the edge access `inbound`.
pub fn platform_goal(
    transit: &TransitNet,
    pnet: &PathNetwork,
    station: cp_building::BuildingId,
    inbound: &Access,
) -> Option<Access> {
    let (segment, s) = transit.stops.iter().find_map(|(_, st)| match st.kind {
        StopKind::Station { building, seg, s } if building == station => Some((seg, s)),
        _ => None,
    })?;
    [Side::Right, Side::Left]
        .into_iter()
        .map(|side| Access { segment, s, side })
        .find(|to| find_rail_path(pnet.graph(), inbound, to).is_ok())
}

/// `Agents` phase, after `run_tourists`: sends the trains, lets the households step off and
/// holds the new ones that will come by train.
#[allow(clippy::too_many_arguments)]
pub fn run_intercity(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    rng: Res<SimRng>,
    limits: Res<SimLimits>,
    net: Res<NetGraph>,
    pnet: Res<PathNetwork>,
    rail: Res<RailReach>,
    transit: Res<TransitNet>,
    buildings: Res<BuildingGrid>,
    mut intercity: ResMut<Intercity>,
    mut tourists: ResMut<Tourists>,
    mut book: ResMut<TripBook>,
    mut grid: ResMut<CitizenGrid>,
    mut runs: ResMut<ServiceRuns>,
    mut agents: ResMut<cp_traffic::Agents>,
) {
    let Some(content) = content else { return };
    let db = &*content.0;
    let balance = db.balance();
    let now = *tick;
    let station = connected_station(db, &transit, &rail, &buildings);
    book.ensure_derived();

    // 1. The train on the line.
    if let Some(train) = intercity.train {
        let state = runs.get(train.run).map(|r| r.state);
        let at_goal = matches!(
            state,
            Some(RunState::Parked { .. } | RunState::Arrived { .. })
        );
        let lost = state.is_none() || state == Some(RunState::Failed) || station.is_none();
        if lost {
            runs.release(train.run, &mut agents);
            intercity.train = None;
        } else if at_goal {
            match train.phase {
                TrainPhase::Inbound => {
                    let capacity = db
                        .vehicle_id("intercity")
                        .map_or(300, |v| usize::from(db.vehicle(v).capacity));
                    let stepped = step_off(&mut book, train.station, capacity);
                    intercity.moved_in += stepped;
                    let mut r = rng.fork(INTERCITY_STREAM, now);
                    let rail_node = edge_access(&net).map(|e| e.2);
                    if let Some(node) = rail_node {
                        bring_by_train(
                            &mut tourists,
                            &mut runs,
                            db,
                            &buildings,
                            &limits,
                            &mut r,
                            train.station,
                            node,
                            // T-321: the season's share of them (§22.1).
                            (f32::from(balance.tourism_by_train) * balance.tourism.season_mul(now))
                                .round() as u32,
                        );
                    }
                    let until = Tick(now.0 + ticks_of_hours(balance.intercity_dwell_h));
                    intercity.train = Some(Train {
                        phase: TrainPhase::Dwell { until },
                        ..train
                    });
                }
                TrainPhase::Dwell { until } if now >= until => {
                    let out = edge_access(&net).map(|e| e.1);
                    let sent = out.is_some_and(|out| {
                        runs.continue_run_reversed(train.run, RunPoint::Access(out))
                            .is_ok()
                    });
                    if sent {
                        intercity.train = Some(Train {
                            phase: TrainPhase::Outbound,
                            ..train
                        });
                    } else {
                        runs.release(train.run, &mut agents);
                        intercity.train = None;
                    }
                }
                TrainPhase::Dwell { .. } => {}
                TrainPhase::Outbound => {
                    runs.release(train.run, &mut agents);
                    intercity.train = None;
                    intercity.trains_done += 1;
                }
            }
        }
    }

    // 2. A new train.
    if intercity.train.is_none()
        && now >= intercity.next_at
        && let Some(station) = station
        && let Some((inbound, _, _)) = edge_access(&net)
    {
        let goal = platform_goal(&transit, &pnet, station, &inbound);
        intercity.next_at = Tick(now.0 + ticks_of_hours(balance.intercity_interval_h));
        if let Some(goal) = goal {
            let run = runs.start_run(
                VehicleKind::Train,
                RunPoint::Access(inbound),
                RunPoint::Access(goal),
                true,
            );
            intercity.train = Some(Train {
                run,
                phase: TrainPhase::Inbound,
                station,
            });
        }
    }

    // 3. The households that will come by train, and the ones whose station is gone.
    let share = u32::from(balance.move_in_by_train);
    let wait = Tick(
        now.0
            + ticks_of_hours(balance.intercity_interval_h + WAIT_MARGIN_H)
            + ticks_of_hours(balance.intercity_dwell_h),
    );
    for tid in book.planned() {
        let Some(trip) = book.get(tid).cloned() else {
            continue;
        };
        if trip.purpose != TripPurpose::MoveIn || trip.from != trip.to {
            continue;
        }
        match (trip.mode, station) {
            (TripMode::Car, Some(_)) => {
                let by_train = grid
                    .citizen(trip.citizen)
                    .is_some_and(|c| c.household.index % 100 < share);
                if by_train {
                    book.set_mode(tid, TripMode::Transit);
                    grid.delay_household(trip.citizen, wait);
                }
            }
            (TripMode::Transit, None) => book.set_mode(tid, TripMode::Car),
            _ => {}
        }
    }
}

/// Lets the households that wait for the train step off at `station`: their trips now start
/// there, on foot. At most `capacity` people (a household is one trip).
fn step_off(book: &mut TripBook, station: cp_building::BuildingId, capacity: usize) -> u32 {
    let waiting: Vec<_> = book
        .iter()
        .filter(|(_, t)| {
            t.purpose == TripPurpose::MoveIn
                && t.mode == TripMode::Transit
                && t.state == TripState::Planned
                && t.from == t.to
        })
        .map(|(id, _)| id)
        .take(capacity)
        .collect();
    for &tid in &waiting {
        book.set_from(tid, station);
        book.set_mode(tid, TripMode::Walk);
    }
    waiting.len() as u32
}
