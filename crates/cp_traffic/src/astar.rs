//! A* over [`PathGraph`] (`architecture/system-design.md` §9.3).
//!
//! Cost = `length / speed × (1 + k × congestion)` per lane, plus connection travel time and
//! turn penalties (left +4 s, U-turn +15 s). Heuristic: straight-line distance to the goal
//! divided by the fastest speed in the graph (admissible: no lane is faster, congestion
//! and penalties only add). Ties are broken by state index, so a search is a pure,
//! deterministic function of `(graph, congestion, request)`.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use bevy_math::Vec2;
use cp_net::LaneId;
use serde::{Deserialize, Serialize};

use crate::graph::{Access, CONGESTION_K, CongestionMap, PED_SPEED, PathGraph, SegEnd};

/// No route exists (or an endpoint is not on a usable lane).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoPath;

/// A vehicle route: start lane (at `start_s` lane-metres), the junction connections to
/// cross as stable `(from, to)` lane pairs, and the goal lane (at `goal_s`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CarPath {
    pub start: LaneId,
    pub start_s: f32,
    pub steps: Vec<(LaneId, LaneId)>,
    pub goal: LaneId,
    pub goal_s: f32,
    /// Estimated travel time, seconds.
    pub cost: f32,
}

impl CarPath {
    /// Lanes in travel order.
    pub fn lanes(&self) -> impl Iterator<Item = LaneId> + '_ {
        std::iter::once(self.start).chain(self.steps.iter().map(|(_, to)| *to))
    }
}

/// A walking route: start lane position, the lane ends visited in order, goal position.
/// Consecutive states on the same lane mean walking along it; a change of lane means
/// crossing a junction link at that node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PedPath {
    pub start: LaneId,
    pub start_s: f32,
    pub states: Vec<(LaneId, SegEnd)>,
    pub goal: LaneId,
    pub goal_s: f32,
    /// Estimated walking time, seconds.
    pub cost: f32,
}

const NONE: u32 = u32::MAX;

fn key(f: f32, node: u32) -> Reverse<(u32, u32)> {
    // Non-negative floats order like their bit patterns.
    Reverse((f.max(0.0).to_bits(), node))
}

fn lane_time(len: f32, speed: f32, congestion: f32) -> f32 {
    len / speed * (1.0 + CONGESTION_K * congestion)
}

/// Fastest route for a vehicle between two access points.
pub fn find_car_path(
    graph: &PathGraph,
    cong: &CongestionMap,
    from: &Access,
    to: &Access,
) -> Result<CarPath, NoPath> {
    let (start, s0) = graph.resolve_car(from).ok_or(NoPath)?;
    let (goal, s1) = graph.resolve_car(to).ok_or(NoPath)?;
    let lane = |i: u32| graph.car[i as usize].as_ref().expect("car lane");
    let c = |i: u32| cong.get(lane(i).segment);
    let vmax = graph.max_car_speed.max(0.1);
    let goal_pos = lane(goal).point_at(s1);
    let h = |i: u32| lane(i).last_point().distance(goal_pos) / vmax;

    let n = graph.car.len();
    let mut g = vec![f32::INFINITY; n];
    let mut parent = vec![NONE; n];
    let mut closed = vec![false; n];
    let mut heap = BinaryHeap::new();

    // (cost, last lane before the goal or NONE for a same-lane run)
    let mut best: Option<(f32, u32)> = None;
    if start == goal && s1 >= s0 {
        let l = lane(start);
        best = Some((lane_time(s1 - s0, l.speed, c(start)), NONE));
    }
    let l0 = lane(start);
    g[start as usize] = lane_time((l0.len - s0).max(0.0), l0.speed, c(start));
    heap.push(key(g[start as usize] + h(start), start));

    while let Some(Reverse((fbits, u))) = heap.pop() {
        if best.is_some_and(|(b, _)| f32::from_bits(fbits) >= b) {
            break;
        }
        if std::mem::replace(&mut closed[u as usize], true) {
            continue;
        }
        for e in &graph.car_edges[u as usize] {
            let v = e.to;
            if v == goal {
                let l = lane(v);
                let total = g[u as usize] + e.cost + lane_time(s1, l.speed, c(v));
                if best.is_none_or(|(b, _)| total < b) {
                    best = Some((total, u));
                }
                continue;
            }
            if closed[v as usize] {
                continue;
            }
            let l = lane(v);
            let ng = g[u as usize] + e.cost + lane_time(l.len, l.speed, c(v));
            if ng < g[v as usize] {
                g[v as usize] = ng;
                parent[v as usize] = u;
                heap.push(key(ng + h(v), v));
            }
        }
    }

    let (cost, last) = best.ok_or(NoPath)?;
    let mut lanes = vec![goal];
    if last != NONE {
        let mut cur = last;
        while cur != NONE {
            lanes.push(cur);
            cur = parent[cur as usize];
        }
    } else {
        lanes.push(start); // same-lane run
    }
    lanes.reverse();
    // `lanes` is start..goal; for a same-lane run it is [start, goal] with start == goal.
    let ids: Vec<LaneId> = lanes.iter().map(|&i| lane(i).id).collect();
    let steps = if last == NONE {
        Vec::new()
    } else {
        ids.windows(2).map(|w| (w[0], w[1])).collect()
    };
    Ok(CarPath {
        start: lane(start).id,
        start_s: s0,
        steps,
        goal: lane(goal).id,
        goal_s: s1,
        cost,
    })
}

/// Fastest walking route between two access points over sidewalks and junction links.
pub fn find_ped_path(graph: &PathGraph, from: &Access, to: &Access) -> Result<PedPath, NoPath> {
    let (start, s0) = graph.resolve_ped(from).ok_or(NoPath)?;
    let (goal, s1) = graph.resolve_ped(to).ok_or(NoPath)?;
    let lane = |i: u32| graph.ped[i as usize].as_ref().expect("ped lane");
    let state_pos = |st: u32| {
        let end = if st.is_multiple_of(2) {
            SegEnd::Start
        } else {
            SegEnd::End
        };
        lane(st / 2).end_point(end)
    };
    let goal_pos: Vec2 = lane(goal).point_at(s1);
    let h = |st: u32| state_pos(st).distance(goal_pos) / PED_SPEED;
    // Distance along a lane from the start/end to a position (lane metres).
    let to_end = |l: u32, s: f32, end: SegEnd| {
        let ln = lane(l);
        // Lane metres of the lane end in travel direction.
        let at_travel_end = (end == SegEnd::End) == ln.forward;
        if at_travel_end { ln.len - s } else { s }
    };

    let n = graph.ped.len() * 2;
    let mut g = vec![f32::INFINITY; n];
    let mut parent = vec![NONE; n];
    let mut closed = vec![false; n];
    let mut heap = BinaryHeap::new();

    let mut best: Option<(f32, u32)> = None; // (cost, final state or NONE = same lane)
    if start == goal {
        best = Some(((s1 - s0).abs() / PED_SPEED, NONE));
    }
    for end in [SegEnd::Start, SegEnd::End] {
        let st = start * 2 + if end == SegEnd::Start { 0 } else { 1 };
        g[st as usize] = to_end(start, s0, end).max(0.0) / PED_SPEED;
        heap.push(key(g[st as usize] + h(st), st));
    }
    // Candidate completions are evaluated when a state is closed.
    while let Some(Reverse((fbits, u))) = heap.pop() {
        if best.is_some_and(|(b, _)| f32::from_bits(fbits) >= b) {
            break;
        }
        if std::mem::replace(&mut closed[u as usize], true) {
            continue;
        }
        if u / 2 == goal {
            let end = if u.is_multiple_of(2) {
                SegEnd::Start
            } else {
                SegEnd::End
            };
            let total = g[u as usize] + to_end(goal, s1, end).max(0.0) / PED_SPEED;
            if best.is_none_or(|(b, _)| total < b) {
                best = Some((total, u));
            }
        }
        for e in &graph.ped_edges[u as usize] {
            let v = e.to;
            if closed[v as usize] {
                continue;
            }
            let ng = g[u as usize] + e.cost;
            if ng < g[v as usize] {
                g[v as usize] = ng;
                parent[v as usize] = u;
                heap.push(key(ng + h(v), v));
            }
        }
    }

    let (cost, last) = best.ok_or(NoPath)?;
    let mut states = Vec::new();
    if last != NONE {
        let mut cur = last;
        while cur != NONE {
            states.push(cur);
            cur = parent[cur as usize];
        }
        states.reverse();
    }
    let states = states
        .into_iter()
        .map(|st| {
            let end = if st.is_multiple_of(2) {
                SegEnd::Start
            } else {
                SegEnd::End
            };
            (lane(st / 2).id, end)
        })
        .collect();
    Ok(PedPath {
        start: lane(start).id,
        start_s: s0,
        states,
        goal: lane(goal).id,
        goal_s: s1,
        cost,
    })
}

impl PathGraph {
    /// Does `path` still exist in this graph: all its lanes are alive and every junction
    /// connection is present? Call after network changes to decide whether to re-request.
    pub fn car_path_valid(&self, path: &CarPath) -> bool {
        self.car_slot(path.start).is_some()
            && self.car_slot(path.goal).is_some()
            && path.steps.iter().all(|(a, b)| self.has_connection(*a, *b))
    }

    /// Like [`Self::car_path_valid`] for a pedestrian route: all lanes alive and every
    /// lane-to-lane hop is a known link.
    pub fn ped_path_valid(&self, path: &PedPath) -> bool {
        if self.ped_slot(path.start).is_none() || self.ped_slot(path.goal).is_none() {
            return false;
        }
        path.states.windows(2).all(|w| {
            let (a, b) = (w[0], w[1]);
            let (Some(sa), Some(sb)) = (self.ped_slot(a.0), self.ped_slot(b.0)) else {
                return false;
            };
            let (ia, ib) = (
                sa * 2 + (a.1 == SegEnd::End) as u32,
                sb * 2 + (b.1 == SegEnd::End) as u32,
            );
            self.ped_edges[ia as usize].iter().any(|e| e.to == ib)
        })
    }
}
