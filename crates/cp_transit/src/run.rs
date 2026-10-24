//! Buses on their lines (§16 M3 A "Депо и автобусы"): the `Agents`-phase system that sends
//! buses out of the depots, keeps them going round the ring and takes the surplus back.

use bevy_ecs::system::{Res, ResMut};
use cp_building::{AccessPoint, BuildingFlags, BuildingGrid, BuildingId, BuildingState};
use cp_content::budget::{BUDGET_DEFAULT, cost_factor};
use cp_content::{Balance, Budgets, ServiceKind, SimContent, VehicleKind};
use cp_core::time::{MOVE_SECONDS_PER_TICK, TICKS_PER_WEEK};
use cp_core::{Cost, Tick};
use cp_net::NetGraph;
use cp_service::RoadGraph;
use cp_traffic::{Access, Agents, RunPoint, RunState, ServiceRuns, Side};

use crate::fleet::{Bus, BusPhase, TransitFleet, TransitLedger};
use crate::model::{Line, LineId, LineState, StopId, TransitMode, TransitNet};
use crate::plan::{LinePlans, is_terminus, visits};

fn ticks_of(seconds: f32) -> u64 {
    ((seconds / MOVE_SECONDS_PER_TICK).ceil() as u64).max(1)
}

/// The depot of `mode` nearest by road to `stop` that has a place left for a vehicle.
/// The Transit budget as the lines run it (§20.1).
#[derive(Clone, Copy)]
struct Out<'a> {
    pct: u8,
    bal: &'a Balance,
}

impl Out<'_> {
    /// The buses `line` puts on the road.
    fn of(&self, line: &Line) -> usize {
        line.running(self.pct, self.bal)
    }

    /// One running bus's part of the line's weekly upkeep, in buses: `vehicles × pct / 100`
    /// over the buses that run (exactly 1 at 100 %).
    fn share(&self, line: &Line) -> f64 {
        let running = self.of(line);
        if running == 0 {
            return cost_factor(self.pct);
        }
        f64::from(line.vehicles) * cost_factor(self.pct) / running as f64
    }
}

fn find_depot(
    net: &NetGraph,
    buildings: &BuildingGrid,
    db: &cp_content::ContentDb,
    fleet: &TransitFleet,
    stop: StopId,
    transit: &TransitNet,
    mode: TransitMode,
) -> Option<BuildingId> {
    let st = transit.stops.get(stop)?;
    let seg = net.segment(st.kind.segment())?;
    let origin = AccessPoint {
        segment: st.kind.segment(),
        t: (st.kind.s() / seg.length.max(1e-3)).clamp(0.0, 1.0),
    };
    let g = RoadGraph::new(net);
    let dist = g.node_distances(origin, f32::INFINITY);
    let mut best: Option<(f32, BuildingId)> = None;
    for (id, b) in buildings.iter() {
        if b.state != BuildingState::Active || b.flags.has(BuildingFlags::NO_POWER) {
            continue;
        }
        let Some(to) = b.access() else { continue };
        let def = db.building(b.def);
        let Some(svc) = def
            .service
            .as_deref()
            .and_then(|s| db.service_id(s))
            .map(|s| db.service(s))
        else {
            continue;
        };
        // A station (with a track) is not a depot.
        if svc.kind != ServiceKind::Transit
            || svc.transit_mode != mode.kind()
            || def.track.is_some()
        {
            continue;
        }
        let assigned = fleet
            .buses
            .iter()
            .filter(|(_, bus)| bus.depot == id)
            .count();
        if assigned >= svc.capacity as usize {
            continue;
        }
        let d = g.distance_to(&dist, origin, to);
        if d.is_finite() && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, id));
        }
    }
    best.map(|(_, id)| id)
}

/// `Agents` phase, after `move_agents` and before `route_trips`.
///
/// * every bus whose run stands at a stop waits `dwell_s`, then drives to the next stop of the
///   ring; a bus beyond the line's `vehicles`, or on a `Broken` or deleted line, drives to its
///   depot instead and is taken off the road there;
/// * a line with fewer buses than `vehicles` sends one out of the nearest depot with room every
///   `headway = cycle_s / vehicles`; without such a depot the line is `NoDepot`;
/// * a bus whose run failed (the road under it went) is gone;
/// * every week each bus out on a line costs `bus_upkeep_week` in the [`TransitLedger`].
///
/// The Transit budget (§20.1) sets how many of a line's `vehicles` run: [`Line::running`]
/// (4 at 50 % → 2, at 150 % → 5); the line's setting stays. The week's cost of a line stays
/// linear in the budget, `vehicles × upkeep × pct / 100`, spread over the buses that run.
#[allow(clippy::too_many_arguments)]
pub fn run_lines(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    buildings: Res<BuildingGrid>,
    plans: Res<LinePlans>,
    mut transit: ResMut<TransitNet>,
    mut fleet: ResMut<TransitFleet>,
    mut ledger: ResMut<TransitLedger>,
    mut runs: ResMut<ServiceRuns>,
    mut agents: ResMut<Agents>,
    budgets: Option<Res<Budgets>>,
) {
    let Some(content) = content else { return };
    let db = &*content.0;
    let balance = db.balance();
    let now = *tick;
    let out = Out {
        pct: budgets.map_or(BUDGET_DEFAULT, |b| b.get(ServiceKind::Transit)),
        bal: balance,
    };
    if fleet.is_empty() && transit.lines.is_empty() {
        return;
    }

    if now.0 > 0 && now.0.is_multiple_of(TICKS_PER_WEEK) {
        for (_, bus) in fleet.buses.iter() {
            let line = transit.lines.get(bus.line);
            let per_week = match line.map(|l| l.mode) {
                Some(TransitMode::Metro) => balance.metro_upkeep_week,
                _ => balance.bus_upkeep_week,
            };
            ledger.upkeep += Cost::scaled(per_week, line.map_or(1.0, |l| out.share(l)));
        }
    }

    // The buses, in id order.
    let ids: Vec<_> = fleet.buses.iter().map(|(id, _)| id).collect();
    for id in ids {
        let Some(bus) = fleet.buses.get(id).copied() else {
            continue;
        };
        let Some(run) = runs.get(bus.run).copied() else {
            fleet.buses.remove(id);
            continue;
        };
        let standing = matches!(
            run.state,
            RunState::Parked { .. } | RunState::Arrived { .. }
        );
        match (run.state, bus.phase) {
            (RunState::Failed, _) => {
                runs.release(bus.run, &mut agents);
                fleet.buses.remove(id);
            }
            (_, BusPhase::ToDepot) if standing => {
                runs.release(bus.run, &mut agents);
                fleet.buses.remove(id);
            }
            (_, BusPhase::Leaving) if standing => {
                set_phase(&mut fleet, id, BusPhase::Dwell { idx: 0, until: now });
                let turn = turn_s(&transit, bus, 0, balance.metro_turn_s);
                dwell(&mut fleet, id, now, balance.dwell_s + turn);
            }
            (_, BusPhase::Driving { idx }) if standing => {
                set_phase(&mut fleet, id, BusPhase::Dwell { idx, until: now });
                let turn = turn_s(&transit, bus, idx, balance.metro_turn_s);
                dwell(&mut fleet, id, now, balance.dwell_s + turn);
            }
            (_, BusPhase::Dwell { idx, until })
                if standing
                    && now >= until
                    && transit
                        .lines
                        .get(bus.line)
                        .is_some_and(|l| l.mode == TransitMode::Metro) =>
            {
                match metro_next(&transit, &fleet, &plans, out, id, bus, idx) {
                    Some((next, goal, reverse)) => {
                        let to = RunPoint::Access(goal);
                        let _ = if reverse {
                            runs.continue_run_reversed(bus.run, to)
                        } else {
                            runs.continue_run(bus.run, to)
                        };
                        set_phase(&mut fleet, id, BusPhase::Driving { idx: next });
                    }
                    None => {
                        // No depot to drive to: a train that is not wanted is taken off here.
                        runs.release(bus.run, &mut agents);
                        fleet.buses.remove(id);
                    }
                }
            }
            (_, BusPhase::Dwell { idx, until }) if standing && now >= until => {
                send_on(&transit, &fleet, &mut runs, out, id, bus, idx);
                // `send_on` changes the phase through the fleet below.
                let next = next_phase(&transit, &fleet, out, id, bus, idx);
                set_phase(&mut fleet, id, next);
            }
            _ => {}
        }
    }

    // Lines: send buses out, or note that nothing can be sent.
    let line_ids: Vec<LineId> = transit.lines.iter().map(|(id, _)| id).collect();
    fleet.last_out.retain(|l, _| transit.lines.contains(*l));
    for line_id in line_ids {
        let Some(line) = transit.lines.get(line_id) else {
            continue;
        };
        if line.mode == TransitMode::Train || line.stops.len() < 2 {
            continue;
        }
        if line.state == LineState::Broken {
            continue;
        }
        if line.mode == TransitMode::Metro {
            send_metro(
                &mut transit,
                &plans,
                &mut fleet,
                &mut runs,
                out,
                line_id,
                now,
            );
            continue;
        }
        let active = fleet
            .of_line(line_id)
            .filter(|(_, b)| b.phase != BusPhase::ToDepot)
            .count();
        let wanted = out.of(line);
        if active >= wanted {
            if line.state == LineState::NoDepot {
                set_line_state(&mut transit, line_id, LineState::Ok);
            }
            continue;
        }
        let first = line.stops[0];
        let mode = line.mode;
        let Some(depot) = find_depot(&net, &buildings, db, &fleet, first, &transit, mode) else {
            set_line_state(&mut transit, line_id, LineState::NoDepot);
            continue;
        };
        if line.state == LineState::NoDepot {
            set_line_state(&mut transit, line_id, LineState::Ok);
        }
        let cycle = plans.get(line_id).map_or(0.0, |p| p.cycle_s);
        let headway = (ticks_of(cycle) / wanted as u64).max(1);
        let due = fleet
            .last_out
            .get(&line_id)
            .is_none_or(|t| now.0 >= t.0 + headway);
        if !due {
            continue;
        }
        let Some(access) = transit.access(first) else {
            continue;
        };
        let run = runs.start_run(
            mode.vehicle_kind(),
            RunPoint::Building(depot),
            RunPoint::Access(access),
            true,
        );
        fleet.buses.insert(Bus {
            line: line_id,
            depot,
            run,
            phase: BusPhase::Leaving,
        });
        fleet.last_out.insert(line_id, now);
    }
}

fn set_phase(fleet: &mut TransitFleet, id: crate::BusId, phase: BusPhase) {
    if let Some(b) = fleet.buses.get_mut(id) {
        b.phase = phase;
    }
}

fn set_line_state(transit: &mut TransitNet, line: LineId, state: LineState) {
    if let Some(l) = transit.lines.get_mut(line)
        && l.state != state
    {
        l.state = state;
    }
}

/// Starts the dwell of a bus that has just arrived: it leaves `dwell_s` from now.
fn dwell(fleet: &mut TransitFleet, id: crate::BusId, now: Tick, dwell_s: f32) {
    if let Some(b) = fleet.buses.get_mut(id)
        && let BusPhase::Dwell { idx, .. } = b.phase
    {
        b.phase = BusPhase::Dwell {
            idx,
            until: Tick(now.0 + ticks_of(dwell_s)),
        };
    }
}

/// Where a bus standing at stop `idx` goes next: the next stop of its ring, or the depot when it
/// is surplus or its line is broken or gone.
fn target(
    transit: &TransitNet,
    fleet: &TransitFleet,
    out: Out,
    id: crate::BusId,
    bus: Bus,
    idx: u16,
) -> Option<(u16, StopId)> {
    let line = transit.lines.get(bus.line)?;
    if line.state == LineState::Broken || line.stops.len() < 2 {
        return None;
    }
    // Buses already on their way home do not count: a new one is not surplus because of them.
    let rank = fleet
        .of_line(bus.line)
        .filter(|(_, b)| b.phase != BusPhase::ToDepot)
        .position(|(b, _)| b == id)?;
    if rank >= out.of(line) {
        return None;
    }
    let next = (usize::from(idx) + 1) % line.stops.len();
    Some((next as u16, line.stops[next]))
}

fn send_on(
    transit: &TransitNet,
    fleet: &TransitFleet,
    runs: &mut ServiceRuns,
    out: Out,
    id: crate::BusId,
    bus: Bus,
    idx: u16,
) {
    let to = match target(transit, fleet, out, id, bus, idx) {
        Some((_, stop)) => transit.access(stop).map(RunPoint::Access),
        None => None,
    }
    .unwrap_or(RunPoint::Building(bus.depot));
    let _ = runs.continue_run(bus.run, to);
}

fn next_phase(
    transit: &TransitNet,
    fleet: &TransitFleet,
    out: Out,
    id: crate::BusId,
    bus: Bus,
    idx: u16,
) -> BusPhase {
    match target(transit, fleet, out, id, bus, idx) {
        Some((next, stop)) if transit.access(stop).is_some() => BusPhase::Driving { idx: next },
        _ => BusPhase::ToDepot,
    }
}

/// Seconds a train stands longer to turn round when it is at the end of its line.
fn turn_s(transit: &TransitNet, bus: Bus, idx: u16, turn_s: f32) -> f32 {
    match transit.lines.get(bus.line) {
        Some(l) if l.mode == TransitMode::Metro && is_terminus(l.stops.len(), usize::from(idx)) => {
            turn_s
        }
        _ => 0.0,
    }
}

/// Where a train standing at position `idx` of its ride goes next: the next position, the
/// place on the track it goes to and whether it turns round first; `None` when it is surplus
/// or its line cannot be run.
fn metro_next(
    transit: &TransitNet,
    fleet: &TransitFleet,
    plans: &LinePlans,
    out: Out,
    id: crate::BusId,
    bus: Bus,
    idx: u16,
) -> Option<(u16, Access, bool)> {
    let line = transit.lines.get(bus.line)?;
    let n = line.stops.len();
    if line.state == LineState::Broken || n < 2 {
        return None;
    }
    let plan = plans.get(bus.line).filter(|p| p.is_ok())?;
    if plan.legs.len() != visits(n) {
        return None;
    }
    let rank = fleet
        .of_line(bus.line)
        .filter(|(_, b)| b.phase != BusPhase::ToDepot)
        .position(|(b, _)| b == id)?;
    if rank >= out.of(line) {
        return None;
    }
    let pos = usize::from(idx) % visits(n);
    let leg = plan.legs.get(pos)?;
    Some((
        ((pos + 1) % visits(n)) as u16,
        leg.goal,
        is_terminus(n, pos),
    ))
}

/// Sends a train of the metro line `line_id` out of its first station when the line has fewer
/// trains than it asks for and the headway is up. The train appears on the first platform (there
/// is no depot) and runs the out-and-back ride from there.
fn send_metro(
    transit: &mut TransitNet,
    plans: &LinePlans,
    fleet: &mut TransitFleet,
    runs: &mut ServiceRuns,
    out: Out,
    line_id: LineId,
    now: Tick,
) {
    let Some(line) = transit.lines.get(line_id) else {
        return;
    };
    let n = line.stops.len();
    let active = fleet
        .of_line(line_id)
        .filter(|(_, b)| b.phase != BusPhase::ToDepot)
        .count();
    let wanted = out.of(line);
    let Some(plan) = plans
        .get(line_id)
        .filter(|p| p.is_ok() && p.legs.len() == visits(n))
    else {
        return;
    };
    if active >= wanted {
        return;
    }
    let cycle = plan.cycle_s;
    let headway = (ticks_of(cycle) / wanted as u64).max(1);
    let due = fleet
        .last_out
        .get(&line_id)
        .is_none_or(|t| now.0 >= t.0 + headway);
    if !due {
        return;
    }
    let Some(crate::model::StopKind::Station { building, .. }) =
        transit.stops.get(line.stops[0]).map(|s| s.kind)
    else {
        return;
    };
    // The train stands at the first platform on the lane it arrives on: the first leg leaves
    // from the opposite one, after the turn.
    let start = plan.legs[0].start;
    let arrived = Access {
        side: match start.side {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        },
        ..start
    };
    let run = runs.start_run(
        VehicleKind::Metro,
        RunPoint::Access(arrived),
        RunPoint::Access(arrived),
        true,
    );
    fleet.buses.insert(Bus {
        line: line_id,
        depot: building,
        run,
        phase: BusPhase::Leaving,
    });
    fleet.last_out.insert(line_id, now);
}
