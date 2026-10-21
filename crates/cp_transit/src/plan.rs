//! The route of a line: derived from the stops and the road network, never saved (§16 M3 A
//! "Маршрут линии — производное").

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use cp_content::VehicleClass;
use cp_traffic::{
    Access, CarPath, CongestionMap, PathGraph, Side, find_class_path, find_rail_path,
};

use crate::model::{LineId, StopId};

/// One leg: the way from a stop to the next one of the ring.
#[derive(Clone, Debug, PartialEq)]
pub struct Leg {
    pub from: StopId,
    pub to: StopId,
    pub path: CarPath,
    pub length_m: f32,
    /// Estimated travel time on an empty road, seconds.
    pub time_s: f32,
    /// Where the leg starts and ends (for a train: the lane side at each end, T-163).
    pub start: Access,
    pub goal: Access,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// Fewer than two stops.
    TooFewStops,
    /// No way from stop `from` to stop `to` (leg `leg` of the ring).
    NoPath {
        leg: usize,
        from: StopId,
        to: StopId,
    },
}

/// The route of a line. `legs` holds one leg per stop (the last one closes the ring) when
/// every leg has a path; `errors` names the legs that have none.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinePlan {
    pub legs: Vec<Leg>,
    pub length_m: f32,
    pub cycle_s: f32,
    pub errors: Vec<PlanError>,
}

impl LinePlan {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Routes the ring `stops[0] → stops[1] → … → stops[0]` on `graph`, one A* per leg, on an
/// empty-road cost (no congestion). Synchronous: a line has a handful of legs and the plan
/// is redone only when the network changes. The same function gives the tool's ghost.
pub fn plan_line(graph: &PathGraph, class: VehicleClass, stops: &[(StopId, Access)]) -> LinePlan {
    let mut plan = LinePlan::default();
    if stops.len() < 2 {
        plan.errors.push(PlanError::TooFewStops);
        return plan;
    }
    let cong = CongestionMap::default();
    for (leg, (&(from, ref a), &(to, ref b))) in stops
        .iter()
        .zip(stops.iter().cycle().skip(1))
        .take(stops.len())
        .enumerate()
    {
        match find_class_path(graph, &cong, class, a, b) {
            Ok(path) => {
                let length_m = graph.car_path_length(&path);
                plan.length_m += length_m;
                plan.cycle_s += path.cost;
                plan.legs.push(Leg {
                    from,
                    to,
                    time_s: path.cost,
                    length_m,
                    path,
                    start: *a,
                    goal: *b,
                });
            }
            Err(_) => plan.errors.push(PlanError::NoPath { leg, from, to }),
        }
    }
    plan
}

/// The stop a train is at on position `pos` of the out-and-back ride of a line of `n` stops:
/// `0, 1, .., n-1, n-2, .., 1`, then round again (T-163).
pub fn visit_index(n: usize, pos: usize) -> usize {
    let m = 2 * (n - 1);
    let p = pos % m;
    if p < n { p } else { m - p }
}

/// Positions of the ride of a line of `n` stops (`2 (n - 1)`; `n >= 2`).
pub fn visits(n: usize) -> usize {
    2 * n.saturating_sub(1)
}

/// Whether the train turns round at position `pos` (the two ends of the line).
pub fn is_terminus(n: usize, pos: usize) -> bool {
    let p = pos % visits(n).max(1);
    p == 0 || p == n - 1
}

/// The cheapest ride so far: its cost and the `(departure side, arrival side)` of each leg.
type Chain = Option<(f32, Vec<(usize, usize)>)>;

/// Routes the out-and-back ride of a metro line over the track graph (T-163): one leg per
/// position of the ride (`visits(n)` legs). A train keeps its lane from stop to stop and takes
/// the opposite lane of the same platform at a terminus, so the side it arrives on decides
/// the side it leaves on. The sides are chosen for the cheapest closed ride; the plan is empty
/// of legs (and has errors) when no choice closes it.
pub fn plan_metro(graph: &PathGraph, stops: &[(StopId, Access)]) -> LinePlan {
    let mut plan = LinePlan::default();
    let n = stops.len();
    if n < 2 {
        plan.errors.push(PlanError::TooFewStops);
        return plan;
    }
    let m = visits(n);
    let at = |pos: usize, side: Side| Access {
        side,
        ..stops[visit_index(n, pos)].1
    };
    // cost[pos][departure side][arrival side] of the leg from `pos` to `pos + 1`.
    let sides = [Side::Right, Side::Left];
    let mut table: Vec<[[Option<CarPath>; 2]; 2]> = Vec::with_capacity(m);
    for pos in 0..m {
        let mut cell: [[Option<CarPath>; 2]; 2] = Default::default();
        for (d, &ds) in sides.iter().enumerate() {
            for (a, &as_) in sides.iter().enumerate() {
                cell[d][a] = find_rail_path(graph, &at(pos, ds), &at(pos + 1, as_)).ok();
            }
        }
        table.push(cell);
    }
    let cost = |p: &Option<CarPath>| p.as_ref().map(|p| p.cost);
    // Walk the ride for each side the first leg may start on; the best closed ride wins.
    let mut best: Chain = None;
    for d0 in 0..2 {
        // dp[arrival side at the next position] = (cost, sides chosen so far)
        let mut states: [Chain; 2] = [None, None];
        for a in 0..2 {
            if let Some(c) = cost(&table[0][d0][a]) {
                states[a] = Some((c, vec![(d0, a)]));
            }
        }
        for (pos, legs) in table.iter().enumerate().skip(1) {
            let mut next: [Chain; 2] = [None, None];
            for (arrived, st) in states.iter().enumerate() {
                let Some((c0, chain)) = st else { continue };
                // Departure side: the same lane, or the opposite one at a terminus.
                let d = if is_terminus(n, pos) {
                    1 - arrived
                } else {
                    arrived
                };
                for a in 0..2 {
                    let Some(c) = cost(&legs[d][a]) else {
                        continue;
                    };
                    if next[a].as_ref().is_none_or(|(b, _)| c0 + c < *b) {
                        let mut ch = chain.clone();
                        ch.push((d, a));
                        next[a] = Some((c0 + c, ch));
                    }
                }
            }
            states = next;
        }
        // Closing: arriving at position 0 again on the lane the first leg left from, mirrored.
        for (arrived, st) in states.iter().enumerate() {
            if let Some((c, chain)) = st
                && 1 - arrived == d0
                && best.as_ref().is_none_or(|(b, _)| *c < *b)
            {
                best = Some((*c, chain.clone()));
            }
        }
    }
    let Some((_, chain)) = best else {
        plan.errors.push(PlanError::NoPath {
            leg: 0,
            from: stops[0].0,
            to: stops[n - 1].0,
        });
        return plan;
    };
    for (pos, &(d, a)) in chain.iter().enumerate() {
        let path = table[pos][d][a].clone().expect("chosen");
        let length_m = graph.rail_path_length(&path);
        plan.length_m += length_m;
        plan.cycle_s += path.cost;
        plan.legs.push(Leg {
            from: stops[visit_index(n, pos)].0,
            to: stops[visit_index(n, pos + 1)].0,
            time_s: path.cost,
            length_m,
            start: at(pos, sides[d]),
            goal: at(pos + 1, sides[a]),
            path,
        });
    }
    plan
}

/// The plans of all lines, by line. Derived state: rebuilt by [`crate::update_transit`] after
/// every rebuild of the `PathGraph` and after a load; not saved, not in the checksum.
#[derive(Resource, Clone, Debug, Default)]
pub struct LinePlans {
    pub(crate) by_line: BTreeMap<LineId, LinePlan>,
    /// `PathNetwork::generation` the plans were made on; `0` = none yet.
    pub(crate) generation: u64,
    /// The stops have been attached to the network since the load (false until the first
    /// run of [`crate::update_transit`]).
    pub(crate) bound: bool,
    /// Stops or lines changed since the plans were made.
    pub(crate) dirty: bool,
}

impl LinePlans {
    pub fn get(&self, line: LineId) -> Option<&LinePlan> {
        self.by_line.get(&line)
    }

    pub fn iter(&self) -> impl Iterator<Item = (LineId, &LinePlan)> {
        self.by_line.iter().map(|(id, p)| (*id, p))
    }

    /// A number that changes whenever a plan does (generation and the shape of every plan).
    pub fn fingerprint(&self) -> u64 {
        let mut h = self.generation;
        for (id, p) in &self.by_line {
            for v in [
                u64::from(id.index),
                u64::from(id.generation),
                p.legs.len() as u64,
                p.errors.len() as u64,
                u64::from(p.length_m.to_bits()),
                u64::from(p.cycle_s.to_bits()),
            ] {
                h = h.wrapping_mul(0x100000001b3).wrapping_add(v);
            }
        }
        h
    }

    /// Marks the plans out of date (the lines changed).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}
