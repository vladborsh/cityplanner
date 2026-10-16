//! Riders (§16 M3 A "Пассажиры", T-159): the choice between bus, car and walking, and the
//! journey of a trip by bus — a walk to the stop, the wait in its queue, the ride inside a bus
//! (not an agent), a walk to the goal.

use bevy_ecs::system::{Res, ResMut};
use bevy_math::Vec2;
use cp_building::BuildingGrid;
use cp_citizen::{
    CitizenGrid, TripBook, TripId, TripMode, TripPurpose, TripState, WALK_SPEED_MPS, cancel_trip,
    complete_trip, travel_ticks,
};
use cp_content::{SimContent, VehicleKind};
use cp_core::time::{MOVE_SECONDS_PER_TICK, TICKS_PER_DAY, TICKS_PER_WEEK};
use cp_core::{Cost, SimRng, Tick};
use cp_traffic::{Agents, RunPoint, RunState, ServiceRuns};

use crate::fleet::{BusPhase, TransitFleet, TransitLedger};
use crate::model::{Journey, LineId, LineState, Ride, Stage, StopId, TransitMode, TransitNet};
use crate::plan::{LinePlans, is_terminus, visit_index, visits};

fn ticks_of(seconds: f32) -> u64 {
    ((seconds / MOVE_SECONDS_PER_TICK).ceil() as u64).max(1)
}

/// The stop of `stops` nearest to `p` within `reach` metres (lowest index on a tie).
fn nearest_stop(
    transit: &TransitNet,
    stops: &[StopId],
    p: Vec2,
    reach: f32,
) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32)> = None;
    for (i, &s) in stops.iter().enumerate() {
        let Some(st) = transit.stops.get(s) else {
            continue;
        };
        let d = st.pos.distance(p);
        if d <= reach && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    best
}

/// `Citizens` phase, after `update_routine`: a freshly planned trip whose ends have a stop of
/// one line within `transit_walk_m` takes the bus when the estimate beats walking (no car) or
/// `car_bias ×` the car's estimate (car owner). No RNG: the order of trip ids decides.
///
/// `cost_transit = walk_a/1.4 + headway/2 + ride + walk_b/1.4 + transit_penalty_s`.
#[allow(clippy::too_many_arguments)]
pub fn choose_transit(
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    plans: Res<LinePlans>,
    fleet: Res<TransitFleet>,
    mut book: ResMut<TripBook>,
    mut transit: ResMut<TransitNet>,
    mut runs: ResMut<ServiceRuns>,
) {
    let Some(content) = content else { return };
    if transit.lines.is_empty() {
        return;
    }
    let balance = content.0.balance();
    book.ensure_derived();
    for tid in book.planned() {
        let Some(trip) = book.get(tid).cloned() else {
            continue;
        };
        if trip.purpose == TripPurpose::MoveIn || trip.from == trip.to {
            continue;
        }
        let (Some(a), Some(b)) = (buildings.get(trip.from), buildings.get(trip.to)) else {
            continue;
        };
        let (pa, pb) = (a.pos, b.pos);
        let dist = pa.distance(pb);
        let baseline = match trip.mode {
            TripMode::Walk => dist / WALK_SPEED_MPS,
            TripMode::Car => balance.car_bias * dist * 1.3 / balance.car_speed_est.max(0.1),
            TripMode::Transit => continue,
        };
        let mut best: Option<(f32, Ride)> = None;
        for (line_id, line) in transit.lines.iter() {
            if line.mode == TransitMode::Train
                || line.state != LineState::Ok
                || line.stops.len() < 2
                || !fleet
                    .of_line(line_id)
                    .any(|(_, bus)| bus.phase != BusPhase::ToDepot)
            {
                continue;
            }
            let Some(plan) = plans.get(line_id).filter(|p| p.is_ok()) else {
                continue;
            };
            let metro = line.mode == TransitMode::Metro;
            let expected = if metro {
                visits(line.stops.len())
            } else {
                line.stops.len()
            };
            if plan.legs.len() != expected {
                continue;
            }
            let Some((ib, wa)) = nearest_stop(&transit, &line.stops, pa, balance.transit_walk_m)
            else {
                continue;
            };
            let Some((ia, wb)) = nearest_stop(&transit, &line.stops, pb, balance.transit_walk_m)
            else {
                continue;
            };
            if ib == ia {
                continue;
            }
            let n = line.stops.len();
            let mut ride = 0.0;
            if metro {
                // Out and back: the first position of the ride that is the boarding stop, either
                // way round, to the next one that is the alighting stop; the quicker counts.
                ride = f32::INFINITY;
                for start in 0..visits(n) {
                    if visit_index(n, start) != ib {
                        continue;
                    }
                    let (mut t, mut pos) = (0.0, start);
                    while visit_index(n, pos) != ia || pos == start {
                        t += plan.legs[pos % visits(n)].time_s + balance.dwell_s;
                        if is_terminus(n, pos + 1) {
                            t += balance.metro_turn_s;
                        }
                        pos += 1;
                    }
                    ride = ride.min(t);
                }
            } else {
                let mut i = ib;
                while i != ia {
                    ride += plan.legs[i].time_s + balance.dwell_s;
                    i = (i + 1) % n;
                }
            }
            let headway = plan.cycle_s / f32::from(line.vehicles.max(1));
            let cost = wa / WALK_SPEED_MPS
                + headway / 2.0
                + ride
                + wb / WALK_SPEED_MPS
                + balance.transit_penalty_s;
            if cost < baseline && best.is_none_or(|(c, _)| cost < c) {
                best = Some((
                    cost,
                    Ride {
                        line: line_id,
                        board: line.stops[ib],
                        alight: line.stops[ia],
                    },
                ));
            }
        }
        let Some((_, ride)) = best else { continue };
        let Some(at) = transit.walk_point(ride.board) else {
            continue;
        };
        let run = runs.start_walk(RunPoint::Building(trip.from), at);
        book.set_mode(tid, TripMode::Transit);
        book.set_state(tid, TripState::Transit);
        transit.riders.insert(
            tid,
            Journey {
                legs: vec![ride],
                leg: 0,
                stage: Stage::WalkTo { run },
            },
        );
    }
}

/// The journey goes on abstractly from `from` (the clock decides when it ends).
fn go_abstract(j: &mut Journey, now: Tick, transit_pos: Option<Vec2>, goal: Option<Vec2>) {
    let ticks = match (transit_pos, goal) {
        (Some(p), Some(g)) => travel_ticks(p.distance(g), TripMode::Walk),
        _ => 1,
    };
    j.stage = Stage::Abstract {
        arrive_at: Tick(now.0 + ticks),
    };
}

/// `Agents` phase, after `run_lines`: moves every rider one step along its journey and lets the
/// buses standing at a stop let passengers out (first) and in (up to the bus's capacity).
#[allow(clippy::too_many_arguments)]
pub fn run_riders(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    rng: Res<SimRng>,
    fleet: Res<TransitFleet>,
    mut transit: ResMut<TransitNet>,
    mut ledger: ResMut<TransitLedger>,
    mut book: ResMut<TripBook>,
    mut grid: ResMut<CitizenGrid>,
    mut runs: ResMut<ServiceRuns>,
    mut agents: ResMut<Agents>,
) {
    let Some(content) = content else { return };
    let db = &*content.0;
    let balance = db.balance();
    let now = *tick;
    if transit.riders.is_empty() && transit.stats.is_empty() && transit.lines.is_empty() {
        return;
    }
    book.ensure_derived();
    grid.ensure_derived();

    if now.0.is_multiple_of(TICKS_PER_DAY) {
        for s in transit.stats.values_mut() {
            s.riders_today = 0;
        }
    }
    if now.0.is_multiple_of(TICKS_PER_WEEK) {
        for s in transit.stats.values_mut() {
            s.riders_week = 0;
        }
    }
    let max_wait = ticks_of(balance.max_wait_s);
    let capacity_of = |kind: VehicleKind| {
        db.vehicles()
            .find(|(_, v)| v.kind == kind)
            .map_or(0, |(_, v)| usize::from(v.capacity))
    };
    let pos_of_stop = |t: &TransitNet, s: StopId| t.stops.get(s).map(|st| st.pos);
    let pos_of_building = |b: cp_building::BuildingId| buildings.get(b).map(|b| b.pos);

    // 1. The buses standing at a stop: passengers out, then in.
    for (bus_id, bus) in fleet.buses.iter() {
        let BusPhase::Dwell { idx, .. } = bus.phase else {
            continue;
        };
        let Some(line) = transit.lines.get(bus.line) else {
            continue;
        };
        let metro = line.mode == TransitMode::Metro;
        // A bus's phase names a stop of the ring, a train's a position of its out-and-back ride.
        let stop_idx = if metro && line.stops.len() >= 2 {
            visit_index(line.stops.len(), usize::from(idx))
        } else {
            usize::from(idx)
        };
        let Some(&stop) = line.stops.get(stop_idx) else {
            continue;
        };
        let capacity = capacity_of(if metro {
            VehicleKind::Metro
        } else {
            VehicleKind::Bus
        });
        let fare = if metro {
            balance.metro_fare
        } else {
            balance.transit_fare
        };
        // The stops a train serves before it turns round: only riders bound for one of them
        // get in.
        let ahead: Vec<StopId> = if metro {
            let n = line.stops.len();
            let mut v = Vec::new();
            let mut pos = usize::from(idx) + 1;
            loop {
                v.push(line.stops[visit_index(n, pos)]);
                if is_terminus(n, pos) || v.len() > n {
                    break;
                }
                pos += 1;
            }
            v
        } else {
            Vec::new()
        };
        // Out.
        let leaving: Vec<TripId> = transit
            .riders
            .iter()
            .filter(|(_, j)| {
                j.stage == Stage::Riding { bus: bus_id }
                    && j.ride().is_some_and(|r| r.alight == stop)
            })
            .map(|(&t, _)| t)
            .collect();
        for tid in leaving {
            let Some(trip) = book.get(tid) else { continue };
            let to = trip.to;
            let Some(at) = transit.walk_point(stop) else {
                continue;
            };
            let run = runs.start_walk(at, RunPoint::Building(to));
            if let Some(j) = transit.riders.get_mut(&tid) {
                j.stage = Stage::WalkFrom { run };
            }
        }
        // In: oldest first, then trip id.
        let load = transit
            .riders
            .values()
            .filter(|j| j.stage == Stage::Riding { bus: bus_id })
            .count();
        if load >= capacity {
            continue;
        }
        let mut queue: Vec<(Tick, TripId)> = transit
            .riders
            .iter()
            .filter_map(|(&t, j)| match (j.stage, j.ride()) {
                (Stage::Waiting { since }, Some(r))
                    if r.line == bus.line
                        && r.board == stop
                        && (!metro || ahead.contains(&r.alight)) =>
                {
                    Some((since, t))
                }
                _ => None,
            })
            .collect();
        queue.sort();
        for &(_, tid) in queue.iter().take(capacity - load) {
            if let Some(j) = transit.riders.get_mut(&tid) {
                j.stage = Stage::Riding { bus: bus_id };
            }
            ledger.fares += Cost::scaled(fare, 1.0);
            let s = transit.stats.entry(bus.line).or_default();
            s.riders_today += 1;
            s.riders_week += 1;
        }
    }

    // 2. Every rider, in trip order.
    let ids: Vec<TripId> = transit.riders.keys().copied().collect();
    for tid in ids {
        let Some(mut j) = transit.riders.get(&tid).cloned() else {
            continue;
        };
        let Some(trip) = book.get(tid).cloned() else {
            // The trip is gone (citizen or building removed): drop the journey and its walk.
            if let Stage::WalkTo { run } | Stage::WalkFrom { run } = j.stage {
                runs.release(run, &mut agents);
            }
            transit.riders.remove(&tid);
            continue;
        };
        let goal = pos_of_building(trip.to);
        let ride = j.ride();
        match j.stage {
            Stage::WalkTo { run } => match runs.get(run).map(|r| r.state) {
                Some(RunState::Arrived { .. }) => {
                    runs.release(run, &mut agents);
                    j.stage = Stage::Waiting { since: now };
                }
                Some(RunState::Failed) | None => {
                    runs.release(run, &mut agents);
                    go_abstract(&mut j, now, pos_of_building(trip.from), goal);
                }
                _ => {}
            },
            Stage::Waiting { since } => {
                let line_ok = ride.is_some_and(|r| {
                    transit.stops.contains(r.board)
                        && transit
                            .lines
                            .get(r.line)
                            .is_some_and(|l| l.state != LineState::Broken)
                });
                if !line_ok || now.0 >= since.0 + max_wait {
                    let at = ride.and_then(|r| pos_of_stop(&transit, r.board));
                    go_abstract(&mut j, now, at, goal);
                }
            }
            Stage::Riding { bus } => {
                let alive = fleet
                    .buses
                    .get(bus)
                    .is_some_and(|b| b.phase != BusPhase::ToDepot);
                if !alive {
                    let at = ride.and_then(|r| pos_of_stop(&transit, r.board));
                    go_abstract(&mut j, now, at, goal);
                }
            }
            Stage::WalkFrom { run } => match runs.get(run).map(|r| r.state) {
                Some(RunState::Arrived { .. }) => {
                    runs.release(run, &mut agents);
                    transit.riders.remove(&tid);
                    complete_trip(&mut grid, &mut book, &buildings, &rng, now.0, tid);
                    continue;
                }
                Some(RunState::Failed) | None => {
                    runs.release(run, &mut agents);
                    transit.riders.remove(&tid);
                    // The goal is gone or unreachable on foot: the citizen goes home.
                    cancel_trip(&mut grid, &mut book, tid);
                    continue;
                }
                _ => {}
            },
            Stage::Abstract { arrive_at } => {
                if now >= arrive_at {
                    transit.riders.remove(&tid);
                    complete_trip(&mut grid, &mut book, &buildings, &rng, now.0, tid);
                    continue;
                }
            }
        }
        transit.riders.insert(tid, j);
    }

    // 3. The line figures that are counted, not summed.
    let lines: Vec<LineId> = transit.lines.iter().map(|(id, _)| id).collect();
    transit.stats.retain(|l, _| lines.contains(l));
    for line in lines {
        let waiting = transit
            .riders
            .values()
            .filter(|j| {
                matches!(j.stage, Stage::Waiting { .. }) && j.ride().is_some_and(|r| r.line == line)
            })
            .count() as u32;
        let out = fleet.of_line(line).count() as u16;
        let s = transit.stats.entry(line).or_default();
        s.waiting = waiting;
        s.vehicles_out = out;
    }
}
