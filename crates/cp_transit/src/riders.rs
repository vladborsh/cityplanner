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

/// Seconds a ride on `line` takes from stop index `ib` to stop index `ia` on an empty road:
/// the legs and the dwell at each stop passed, and for a train the turn at the ends (the quicker
/// of the ways round). `None` if the plan does not fit the line.
fn ride_secs(
    line: &crate::model::Line,
    plan: &crate::plan::LinePlan,
    balance: &cp_content::Balance,
    ib: usize,
    ia: usize,
) -> Option<f32> {
    let n = line.stops.len();
    if line.mode == TransitMode::Metro {
        // Out and back: the first position of the ride that is the boarding stop, either
        // way round, to the next one that is the alighting stop; the quicker counts.
        let mut ride = f32::INFINITY;
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
        ride.is_finite().then_some(ride)
    } else {
        let mut ride = 0.0;
        let mut i = ib;
        while i != ia {
            ride += plan.legs.get(i)?.time_s + balance.dwell_s;
            i = (i + 1) % n;
        }
        Some(ride)
    }
}

/// A line a rider can take now, with its stop nearest to one end of the trip.
struct Near {
    line: LineId,
    /// Index in the line's stops, and the distance to it.
    stop: usize,
    walk: f32,
    /// Seconds a rider waits on average: half the headway.
    wait: f32,
}

/// `Citizens` phase, after `update_routine`: a freshly planned trip whose ends have a stop of
/// one line within `transit_walk_m` takes the bus when the estimate beats walking (no car) or
/// `car_bias ×` the car's estimate (car owner). With two lines it may change from one to the
/// other (T-165): at a stop they share, or on foot between two stops within `transfer_walk_m`.
/// No RNG: the order of line ids decides ties (the smaller wins).
///
/// `cost_transit = walk_a/1.4 + headway/2 + ride + walk_b/1.4 + transit_penalty_s`; with a
/// change, `+ walk_x/1.4 + headway₂/2 + ride₂ + transfer_penalty_s`.
#[allow(clippy::too_many_arguments)]
pub fn choose_transit(
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    plans: Res<LinePlans>,
    fleet: Res<TransitFleet>,
    mut book: ResMut<TripBook>,
    mut transit: ResMut<TransitNet>,
    mut runs: ResMut<ServiceRuns>,
    policies: cp_district::PolicyParams,
) {
    let Some(content) = content else { return };
    if transit.lines.is_empty() {
        return;
    }
    let balance = content.0.balance();
    let policies = policies.get();
    book.ensure_derived();
    // The lines that run now and fit their plan, in line id order.
    let usable: Vec<LineId> = transit
        .lines
        .iter()
        .filter(|(id, line)| {
            line.mode != TransitMode::Train
                && line.state == LineState::Ok
                && line.stops.len() >= 2
                && fleet
                    .of_line(*id)
                    .any(|(_, bus)| bus.phase != BusPhase::ToDepot)
                && plans.get(*id).is_some_and(|p| {
                    p.is_ok()
                        && p.legs.len()
                            == if line.mode == TransitMode::Metro {
                                visits(line.stops.len())
                            } else {
                                line.stops.len()
                            }
                })
        })
        .map(|(id, _)| id)
        .collect();
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
        // FreeTransit (T-194) makes the bus look better for trips that start in its area: the
        // waiting and the transit penalty count `1 - value` as much.
        let free = policies.map_or(1.0, |p| {
            p.factor_at(&content.0, pa, cp_content::PolicyKind::FreeTransit)
        });
        let dist = pa.distance(pb);
        let baseline = match trip.mode {
            TripMode::Walk => dist / WALK_SPEED_MPS,
            TripMode::Car => balance.car_bias * dist * 1.3 / balance.car_speed_est.max(0.1),
            TripMode::Transit => continue,
        };
        // The lines with a stop within reach of each end, nearest first, at most
        // `transit_lines_cap` of them.
        let near = |p: Vec2| -> Vec<Near> {
            let mut v: Vec<Near> = usable
                .iter()
                .filter_map(|&line_id| {
                    let line = transit.lines.get(line_id)?;
                    let plan = plans.get(line_id)?;
                    let (stop, walk) =
                        nearest_stop(&transit, &line.stops, p, balance.transit_walk_m)?;
                    Some(Near {
                        line: line_id,
                        stop,
                        walk,
                        wait: plan.cycle_s / f32::from(line.vehicles.max(1)) / 2.0,
                    })
                })
                .collect();
            v.sort_by(|x, y| x.walk.total_cmp(&y.walk).then(x.line.cmp(&y.line)));
            v.truncate(usize::from(balance.transit_lines_cap));
            v.sort_by_key(|n| n.line);
            v
        };
        let (from, to) = (near(pa), near(pb));
        let mut best: Option<(f32, Vec<Ride>)> = None;
        let mut offer = |cost: f32, rides: Vec<Ride>| {
            if cost < baseline && best.as_ref().is_none_or(|(c, _)| cost < *c) {
                best = Some((cost, rides));
            }
        };
        // One line.
        for f in &from {
            let Some(t) = to.iter().find(|t| t.line == f.line) else {
                continue;
            };
            if f.stop == t.stop {
                continue;
            }
            let (line, plan) = (
                transit.lines.get(f.line).expect("usable"),
                plans.get(f.line).expect("usable"),
            );
            let Some(ride) = ride_secs(line, plan, balance, f.stop, t.stop) else {
                continue;
            };
            let cost = f.walk / WALK_SPEED_MPS
                + f.wait * free
                + ride
                + t.walk / WALK_SPEED_MPS
                + balance.transit_penalty_s * free;
            offer(
                cost,
                vec![Ride {
                    line: f.line,
                    board: line.stops[f.stop],
                    alight: line.stops[t.stop],
                }],
            );
        }
        // Two lines, changing at a shared stop or on foot between two stops.
        for f in &from {
            for t in to.iter().filter(|t| t.line != f.line) {
                let (l1, l2) = (
                    transit.lines.get(f.line).expect("usable"),
                    transit.lines.get(t.line).expect("usable"),
                );
                let (p1, p2) = (
                    plans.get(f.line).expect("usable"),
                    plans.get(t.line).expect("usable"),
                );
                for (x, &sx) in l1.stops.iter().enumerate().filter(|(x, _)| *x != f.stop) {
                    let Some(px) = transit.stops.get(sx).map(|s| s.pos) else {
                        continue;
                    };
                    for (y, &sy) in l2.stops.iter().enumerate().filter(|(y, _)| *y != t.stop) {
                        let Some(py) = transit.stops.get(sy).map(|s| s.pos) else {
                            continue;
                        };
                        let walk = if sx == sy { 0.0 } else { px.distance(py) };
                        if walk > balance.transfer_walk_m {
                            continue;
                        }
                        let (Some(r1), Some(r2)) = (
                            ride_secs(l1, p1, balance, f.stop, x),
                            ride_secs(l2, p2, balance, y, t.stop),
                        ) else {
                            continue;
                        };
                        let cost = f.walk / WALK_SPEED_MPS
                            + f.wait * free
                            + r1
                            + walk / WALK_SPEED_MPS
                            + t.wait * free
                            + r2
                            + t.walk / WALK_SPEED_MPS
                            + balance.transit_penalty_s * free
                            + balance.transfer_penalty_s;
                        offer(
                            cost,
                            vec![
                                Ride {
                                    line: f.line,
                                    board: l1.stops[f.stop],
                                    alight: sx,
                                },
                                Ride {
                                    line: t.line,
                                    board: sy,
                                    alight: l2.stops[t.stop],
                                },
                            ],
                        );
                    }
                }
            }
        }
        let Some((_, legs)) = best else { continue };
        let Some(at) = transit.walk_point(legs[0].board) else {
            continue;
        };
        let run = runs.start_walk(RunPoint::Building(trip.from), at);
        book.set_mode(tid, TripMode::Transit);
        book.set_state(tid, TripState::Transit);
        transit.riders.insert(
            tid,
            Journey {
                legs,
                leg: 0,
                stage: Stage::WalkTo { run },
            },
        );
    }
}

/// The stage of a journey that goes on abstractly from `transit_pos` (the clock decides when
/// it ends).
fn go_abstract_stage(now: Tick, transit_pos: Option<Vec2>, goal: Option<Vec2>) -> Stage {
    let ticks = match (transit_pos, goal) {
        (Some(p), Some(g)) => travel_ticks(p.distance(g), TripMode::Walk),
        _ => 1,
    };
    Stage::Abstract {
        arrive_at: Tick(now.0 + ticks),
    }
}

/// The journey goes on abstractly from `from`.
fn go_abstract(j: &mut Journey, now: Tick, transit_pos: Option<Vec2>, goal: Option<Vec2>) {
    j.stage = go_abstract_stage(now, transit_pos, goal);
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
    policies: cp_district::PolicyParams,
) {
    let Some(content) = content else { return };
    let db = &*content.0;
    let balance = db.balance();
    let policies = policies.get();
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
        // FreeTransit (T-194): no ticket for boarding in its area.
        let free_here = policies.is_some_and(|p| {
            transit
                .stops
                .get(stop)
                .is_some_and(|s| p.at(s.pos, cp_content::PolicyKind::FreeTransit))
        }) && db.policy(cp_content::PolicyKind::FreeTransit).is_some();
        let fare = if free_here {
            0.0
        } else if metro {
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
            let Some(mut j) = transit.riders.get(&tid).cloned() else {
                continue;
            };
            if usize::from(j.leg) + 1 < j.legs.len() {
                // A change (T-165): on to the next line, at once when it is the same stop,
                // else on foot.
                j.leg += 1;
                let board = j.legs[usize::from(j.leg)].board;
                j.stage = if board == stop {
                    Stage::Waiting { since: now }
                } else if let Some(next) = transit.walk_point(board) {
                    Stage::WalkTo {
                        run: runs.start_walk(at, next),
                    }
                } else {
                    go_abstract_stage(now, pos_of_stop(&transit, stop), pos_of_building(to))
                };
            } else {
                let run = runs.start_walk(at, RunPoint::Building(to));
                j.stage = Stage::WalkFrom { run };
            }
            transit.riders.insert(tid, j);
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
                    // On a change the walk began at the stop the rider got out at.
                    let from = match j
                        .leg
                        .checked_sub(1)
                        .and_then(|l| j.legs.get(usize::from(l)))
                    {
                        Some(prev) => pos_of_stop(&transit, prev.alight),
                        None => pos_of_building(trip.from),
                    };
                    go_abstract(&mut j, now, from, goal);
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
