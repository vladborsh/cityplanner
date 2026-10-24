//! A* over [`PathGraph`] (`architecture/system-design.md` §9.3).
//!
//! Cost = `length / speed + delay × length / lane length` per lane (T-315: `delay` is the
//! lane's measured delay, [`CongestionMap`]), plus connection travel time and turn penalties
//! (left +4 s, U-turn +15 s). Heuristic: straight-line distance to the goal divided by the
//! fastest speed in the graph (admissible: no lane is faster, delays and penalties only add). Ties are broken by state index, so a search is a pure,
//! deterministic function of `(graph, congestion, request)`.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use bevy_math::Vec2;
use cp_content::{VehicleClass, lane_allows};
use cp_net::LaneId;
use serde::{Deserialize, Serialize};

use crate::graph::{Access, CarEdge, CongestionMap, LaneNode, PED_SPEED, PathGraph, SegEnd};
use crate::heavy::HeavyBan;

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

/// Seconds to drive `len` metres of a lane of `lane_len` metres at `speed`, with `delay` seconds
/// lost on the whole lane (spread along it).
fn lane_time(len: f32, speed: f32, lane_len: f32, delay: f32) -> f32 {
    len / speed + delay * (len / lane_len.max(1.0)).min(1.0)
}

/// Fastest route for a vehicle between two access points.
pub fn find_car_path(
    graph: &PathGraph,
    cong: &CongestionMap,
    from: &Access,
    to: &Access,
) -> Result<CarPath, NoPath> {
    find_class_path(graph, cong, VehicleClass::Car, from, to)
}

/// [`find_car_path`] for a vehicle of `class` (T-302): the access points resolve to lanes the
/// class may use and the route keeps to them (`cp_content::lane_allows`). A bus rides car and
/// bus lanes, a tram its rails only, a trolleybus its wired lanes only.
pub fn find_class_path(
    graph: &PathGraph,
    cong: &CongestionMap,
    class: VehicleClass,
    from: &Access,
    to: &Access,
) -> Result<CarPath, NoPath> {
    let (start, s0) = graph.resolve_class(from, class).ok_or(NoPath)?;
    let (goal, s1) = graph.resolve_class(to, class).ok_or(NoPath)?;
    car_route(graph, cong, None, class, (start, s0), (goal, s1))
}

/// [`find_car_path_lanes`] for a vehicle of `class` (T-302).
pub fn find_class_path_lanes(
    graph: &PathGraph,
    cong: &CongestionMap,
    class: VehicleClass,
    (from, from_s): (LaneId, f32),
    (to, to_s): (LaneId, f32),
) -> Result<CarPath, NoPath> {
    let start = graph.car_slot(from).ok_or(NoPath)?;
    let goal = graph.car_slot(to).ok_or(NoPath)?;
    car_route(graph, cong, None, class, (start, from_s), (goal, to_s))
}

/// [`find_car_path`] for a truck (T-195): lanes in districts with the heavy traffic ban cost
/// `heavy_ban_cost_mul` times as much, unless the trip starts or ends in that district.
pub fn find_truck_path(
    graph: &PathGraph,
    cong: &CongestionMap,
    heavy: &HeavyBan,
    from: &Access,
    to: &Access,
) -> Result<CarPath, NoPath> {
    let (start, s0) = graph.resolve_car(from).ok_or(NoPath)?;
    let (goal, s1) = graph.resolve_car(to).ok_or(NoPath)?;
    car_route(
        graph,
        cong,
        Some(heavy),
        VehicleClass::Car,
        (start, s0),
        (goal, s1),
    )
}

/// [`find_car_path_lanes`] for a truck, see [`find_truck_path`].
pub fn find_truck_path_lanes(
    graph: &PathGraph,
    cong: &CongestionMap,
    heavy: &HeavyBan,
    (from, from_s): (LaneId, f32),
    (to, to_s): (LaneId, f32),
) -> Result<CarPath, NoPath> {
    let start = graph.car_slot(from).ok_or(NoPath)?;
    let goal = graph.car_slot(to).ok_or(NoPath)?;
    car_route(
        graph,
        cong,
        Some(heavy),
        VehicleClass::Car,
        (start, from_s),
        (goal, to_s),
    )
}

/// Fastest route from lane `from` (at `from_s` lane-metres) to lane `to` (at `to_s`): the
/// re-route of a car that is already on the road (`Mode::CarLanes`).
pub fn find_car_path_lanes(
    graph: &PathGraph,
    cong: &CongestionMap,
    (from, from_s): (LaneId, f32),
    (to, to_s): (LaneId, f32),
) -> Result<CarPath, NoPath> {
    let start = graph.car_slot(from).ok_or(NoPath)?;
    let goal = graph.car_slot(to).ok_or(NoPath)?;
    car_route(
        graph,
        cong,
        None,
        VehicleClass::Car,
        (start, from_s),
        (goal, to_s),
    )
}

/// Fastest route along the track between two access points (T-161). Trains do not slow each
/// other down here (no congestion): a route is the shortest travel time over the rail graph.
pub fn find_rail_path(graph: &PathGraph, from: &Access, to: &Access) -> Result<CarPath, NoPath> {
    let (start, s0) = graph.resolve_rail(from).ok_or(NoPath)?;
    let (goal, s1) = graph.resolve_rail(to).ok_or(NoPath)?;
    route(
        &graph.rail,
        &graph.rail_edges,
        graph.max_rail_speed,
        &CongestionMap::default(),
        None,
        VehicleClass::Rail,
        (start, s0),
        (goal, s1),
    )
}

fn car_route(
    graph: &PathGraph,
    cong: &CongestionMap,
    heavy: Option<&HeavyBan>,
    class: VehicleClass,
    start: (u32, f32),
    goal: (u32, f32),
) -> Result<CarPath, NoPath> {
    route(
        &graph.car,
        &graph.car_edges,
        graph.max_car_speed,
        cong,
        heavy,
        class,
        start,
        goal,
    )
}

/// A* over one lane graph (the road graph or the track graph), on the lanes `class` may use
/// (T-302): a lane the class is not allowed on is never entered, and a start or goal lane it
/// is not allowed on is no path.
#[allow(clippy::too_many_arguments)]
fn route(
    lanes: &[Option<LaneNode>],
    edges: &[Vec<CarEdge>],
    max_speed: f32,
    cong: &CongestionMap,
    heavy: Option<&HeavyBan>,
    class: VehicleClass,
    (start, s0): (u32, f32),
    (goal, s1): (u32, f32),
) -> Result<CarPath, NoPath> {
    let lane = |i: u32| lanes[i as usize].as_ref().expect("lane");
    let allowed = |i: u32| lane_allows(lane(i).kind, class);
    if !allowed(start) || !allowed(goal) {
        return Err(NoPath);
    }
    let c = |i: u32| cong.delay(i);
    // A truck's lanes in a banned district cost more, unless it starts or ends there (T-195).
    let ends = heavy.map(|h| {
        (
            h.district_of(lane(start).segment),
            h.district_of(lane(goal).segment),
        )
    });
    let time = |i: u32, len: f32| {
        let l = lane(i);
        let t = lane_time(len, l.speed, l.len, c(i));
        match (heavy, ends) {
            (Some(h), Some(e)) => t * h.factor(l.segment, e),
            _ => t,
        }
    };
    let vmax = max_speed.max(0.1);
    let goal_pos = lane(goal).point_at(s1);
    let h = |i: u32| lane(i).last_point().distance(goal_pos) / vmax;

    let n = lanes.len();
    let mut g = vec![f32::INFINITY; n];
    let mut parent = vec![NONE; n];
    let mut closed = vec![false; n];
    let mut heap = BinaryHeap::new();

    // (cost, last lane before the goal or NONE for a same-lane run)
    let mut best: Option<(f32, u32)> = None;
    if start == goal && s1 >= s0 {
        best = Some((time(start, s1 - s0), NONE));
    }
    let l0 = lane(start);
    g[start as usize] = time(start, (l0.len - s0).max(0.0));
    heap.push(key(g[start as usize] + h(start), start));

    while let Some(Reverse((fbits, u))) = heap.pop() {
        if best.is_some_and(|(b, _)| f32::from_bits(fbits) >= b) {
            break;
        }
        if std::mem::replace(&mut closed[u as usize], true) {
            continue;
        }
        for e in &edges[u as usize] {
            let v = e.to;
            if !allowed(v) {
                continue;
            }
            if v == goal {
                let tail = if e.lane_change { 0.0 } else { time(v, s1) };
                let total = g[u as usize] + e.cost + tail;
                if best.is_none_or(|(b, _)| total < b) {
                    best = Some((total, u));
                }
                continue;
            }
            if closed[v as usize] {
                continue;
            }
            let l = lane(v);
            // A lateral hop stays on the same stretch of road: no second lane travel time.
            let body = if e.lane_change { 0.0 } else { time(v, l.len) };
            let ng = g[u as usize] + e.cost + body;
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

/// What the rest of `v`'s route costs from where it is, on `cong` (T-315): the same lane times,
/// connection costs and turn penalties as the search, so two routes of one car compare fairly.
/// `None` when a lane or connection of the route is not in `g`.
pub(crate) fn cost_ahead(g: &PathGraph, cong: &CongestionMap, v: &crate::Vehicle) -> Option<f32> {
    let lanes: Vec<LaneId> = v.path.lanes().collect();
    let last = lanes.len() - 1;
    let here = usize::from(v.hop);
    let node = |s: u32| g.car.get(s as usize).and_then(Option::as_ref);
    let time =
        |s: u32, len: f32| node(s).map(|n| lane_time(len.max(0.0), n.speed, n.len, cong.delay(s)));
    let mut t = 0.0;
    if !v.in_conn {
        let s0 = g.car_slot(*lanes.get(here)?)?;
        let end = if here == last {
            v.path.goal_s
        } else {
            node(s0)?.len
        };
        t += time(s0, end - v.s)?;
    }
    for k in here..v.path.steps.len() {
        let (a, b) = v.path.steps[k];
        let (sa, sb) = (g.car_slot(a)?, g.car_slot(b)?);
        let e = g.car_edges.get(sa as usize)?.iter().find(|e| e.to == sb)?;
        let body = if e.lane_change {
            0.0
        } else if k + 1 == last {
            time(sb, v.path.goal_s)?
        } else {
            time(sb, node(sb)?.len)?
        };
        t += e.cost + body;
    }
    Some(t)
}

/// Fastest walking route between two access points over sidewalks and junction links.
pub fn find_ped_path(graph: &PathGraph, from: &Access, to: &Access) -> Result<PedPath, NoPath> {
    find_walk_path(graph, from, to, WalkClass::Walk)
}

/// The class of a route over the walking graph (T-322, §23.1): a walker keeps to the sidewalks
/// and footpaths at [`PED_SPEED`]; a cyclist also rides the bike lanes, along each lane at the
/// speed of its kind, from bike lane to bike lane through a junction at bike-lane speed, and
/// round corners and over crossings on foot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WalkClass {
    Walk,
    Bike(BikeSpeeds),
}

/// A cyclist's speeds (`f32` bits, so a request stays `Eq`), m/s: on a bike lane, on a
/// sidewalk, on a footpath or bike path (`Balance::bike_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BikeSpeeds {
    lane: u32,
    sidewalk: u32,
    path: u32,
}

impl BikeSpeeds {
    pub fn new(lane: f32, sidewalk: f32, path: f32) -> Self {
        Self {
            lane: lane.to_bits(),
            sidewalk: sidewalk.to_bits(),
            path: path.to_bits(),
        }
    }

    pub fn of_balance(b: &cp_content::Balance) -> Self {
        Self::new(b.bike_speed, b.bike_sidewalk_speed, b.bike_path_speed)
    }

    /// The speed along a lane of `kind` (a bike path's lanes are bike lanes; a park path's
    /// are a footpath's when `footpath`).
    pub fn along(&self, kind: cp_content::LaneKind, footpath: bool) -> f32 {
        f32::from_bits(if kind == cp_content::LaneKind::Bike {
            self.lane
        } else if footpath {
            self.path
        } else {
            self.sidewalk
        })
    }

    fn max(&self) -> f32 {
        [self.lane, self.sidewalk, self.path]
            .into_iter()
            .map(f32::from_bits)
            .fold(PED_SPEED, f32::max)
    }
}

impl WalkClass {
    /// The speed along `l` (walkers: [`PED_SPEED`] everywhere).
    pub(crate) fn speed(&self, l: &LaneNode) -> f32 {
        match self {
            WalkClass::Walk => PED_SPEED,
            WalkClass::Bike(s) => s.along(l.kind, l.footpath),
        }
    }
}

/// Fastest route of `class` between two access points over the walking graph: the walker's
/// route of T-104 for [`WalkClass::Walk`] (bike lanes are no state of it).
pub fn find_walk_path(
    graph: &PathGraph,
    from: &Access,
    to: &Access,
    class: WalkClass,
) -> Result<PedPath, NoPath> {
    let (start, s0) = graph.resolve_ped(from).ok_or(NoPath)?;
    let (goal, s1) = graph.resolve_ped(to).ok_or(NoPath)?;
    let lane = |i: u32| graph.ped[i as usize].as_ref().expect("ped lane");
    let walk = class == WalkClass::Walk;
    let top = match class {
        WalkClass::Walk => PED_SPEED,
        WalkClass::Bike(s) => s.max(),
    };
    let state_pos = |st: u32| {
        let end = if st.is_multiple_of(2) {
            SegEnd::Start
        } else {
            SegEnd::End
        };
        lane(st / 2).end_point(end)
    };
    let goal_pos: Vec2 = lane(goal).point_at(s1);
    let h = |st: u32| state_pos(st).distance(goal_pos) / top;
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
        best = Some(((s1 - s0).abs() / class.speed(lane(start)), NONE));
    }
    for end in [SegEnd::Start, SegEnd::End] {
        let st = start * 2 + if end == SegEnd::Start { 0 } else { 1 };
        g[st as usize] = to_end(start, s0, end).max(0.0) / class.speed(lane(start));
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
            let total = g[u as usize] + to_end(goal, s1, end).max(0.0) / class.speed(lane(goal));
            if best.is_none_or(|(b, _)| total < b) {
                best = Some((total, u));
            }
        }
        for e in &graph.ped_edges[u as usize] {
            let v = e.to;
            if closed[v as usize] {
                continue;
            }
            // A walker never sets foot on a bike lane; a cyclist rides along a lane at its
            // kind's speed (the edge's cost is the walker's) and through junctions on foot.
            let cost = if walk {
                if lane(v / 2).kind == cp_content::LaneKind::Bike {
                    continue;
                }
                e.cost
            } else if v / 2 == u / 2 {
                lane(u / 2).len / class.speed(lane(u / 2))
            } else if let (true, WalkClass::Bike(s)) = (e.ride, class) {
                e.cost * PED_SPEED / s.along(cp_content::LaneKind::Bike, false)
            } else {
                e.cost
            };
            let ng = g[u as usize] + cost;
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
