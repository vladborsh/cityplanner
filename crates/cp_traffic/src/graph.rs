//! The path graph: dense, immutable snapshot of the lane network for A*
//! (`architecture/system-design.md` §9.3).
//!
//! Built from [`NetGraph`] lanes plus the junction links of [`JunctionGeometry`]; separate
//! sub-graphs for vehicles (`Car`/`Bus` lanes) and pedestrians (`Pedestrian` lanes).
//!
//! **Vehicle graph.** A state is a lane (index = arena slot of its `LaneId`). Edges are the
//! junction `LaneConnection`s `from → to`; an edge costs the connection's travel time plus
//! the turn penalty. **Pedestrian graph.** A state is a lane end `(lane, SegEnd)`; edges walk
//! along the lane to its other end or cross a junction `PedLink` to another lane's end.
//!
//! Paths use only **stable ids**: `LaneId`s (arena ids). A junction recompute renumbers
//! `(node, index)` connection ids, so a path stores `(from, to)` lane pairs and the curve is
//! looked up at traversal time. [`PathGraph::car_path_valid`] re-checks a stored path after
//! network changes.

use std::collections::BTreeMap;

use bevy_math::Vec2;
use cp_content::{LaneDir, LaneKind, VehicleClass, lane_allows};
use cp_core::curve::{CubicBezier3, to_xz};
use cp_net::{
    JunctionGeometry, Lane, LaneId, LightPlan, NetGraph, NodeId, Priority, SegmentId, Turn,
};
use serde::{Deserialize, Serialize};

/// Walking speed used for costs, m/s (pedestrians move at 1.2–1.6 m/s, §9.4).
pub const PED_SPEED: f32 = 1.4;
/// Extra seconds for a left turn (§9.3).
pub const LEFT_TURN_PENALTY: f32 = 4.0;
/// Extra seconds for a U-turn (§9.3).
pub const U_TURN_PENALTY: f32 = 15.0;
/// Extra seconds for a lateral hop to the neighbouring lane of the same direction.
pub const LANE_CHANGE_COST: f32 = 2.0;
/// Congestion weight `k` in `length / speed × (1 + k × congestion)`.
pub const CONGESTION_K: f32 = 30.0;

/// Speed through a left or right turn, m/s (about 22 km/h).
pub const TURN_SPEED: f32 = 6.0;
/// Speed through a U-turn, m/s.
pub const U_TURN_SPEED: f32 = 3.0;

/// Which end of a segment. Pedestrian lane ends are named by the segment end they touch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SegEnd {
    Start,
    End,
}

impl SegEnd {
    pub fn other(self) -> SegEnd {
        match self {
            SegEnd::Start => SegEnd::End,
            SegEnd::End => SegEnd::Start,
        }
    }
    fn idx(self) -> usize {
        match self {
            SegEnd::Start => 0,
            SegEnd::End => 1,
        }
    }
}

/// Side of the road as seen along the segment's own direction (start → end).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
}

/// A place beside a road where a trip starts or ends (a building's access point).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Access {
    pub segment: SegmentId,
    /// Metres from the segment start.
    pub s: f32,
    pub side: Side,
}

/// Per-segment congestion (smoothed load, `0` = empty), indexed by segment arena slot.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CongestionMap(pub Vec<f32>);

impl CongestionMap {
    pub fn get(&self, segment: SegmentId) -> f32 {
        self.0.get(segment.index as usize).copied().unwrap_or(0.0)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LaneNode {
    pub id: LaneId,
    pub segment: SegmentId,
    /// The lane's kind: which vehicle classes may use it (T-302, `lane_allows`).
    pub kind: LaneKind,
    pub len: f32,
    pub speed: f32,
    pub forward: bool,
    pub seg_len: f32,
    pub curve: [CubicBezier3; cp_net::LANE_CURVE_PIECES],
    /// Lane metres where the lane leaves the junction it comes from (the inbound setback).
    pub s_in: f32,
    /// Lane metres where the lane reaches the stop line of the junction it runs into.
    pub s_out: f32,
    /// Where [`Self::point_at`] meets the junction setbacks at the travel start / end, the
    /// ends of the pedestrian links (corners, crossings) that start there.
    pub at_in: f32,
    pub at_out: f32,
    /// The lane leaves an `Outside` node (BUG-117): cars appear on it coming in from the
    /// highway, at speed.
    pub from_outside: bool,
}

impl LaneNode {
    /// Travel-direction end points.
    pub fn first_point(&self) -> Vec2 {
        to_xz(self.curve[0].p0)
    }
    pub fn last_point(&self) -> Vec2 {
        to_xz(self.curve[cp_net::LANE_CURVE_PIECES - 1].p3)
    }

    /// Point `s` metres along the lane in travel direction.
    pub fn point_at(&self, s: f32) -> Vec2 {
        let n = self.curve.len();
        let u = (s / self.len.max(1e-3)).clamp(0.0, 1.0) * n as f32;
        let k = (u as usize).min(n - 1);
        to_xz(self.curve[k].point(u - k as f32))
    }

    /// Like [`Self::point_at`] with the height.
    pub fn point3_at(&self, s: f32) -> bevy_math::Vec3 {
        let n = self.curve.len();
        let u = (s / self.len.max(1e-3)).clamp(0.0, 1.0) * n as f32;
        let k = (u as usize).min(n - 1);
        self.curve[k].point(u - k as f32)
    }

    /// Lane metres (in travel direction) of the lane end at `end`, at the junction setback
    /// where the junction's pedestrian links start (`0` / `len` without a junction), so a walk
    /// along the lane ends where the next link begins (BUG-064).
    pub fn end_s(&self, end: SegEnd) -> f32 {
        if (end == SegEnd::End) == self.forward {
            self.at_out
        } else {
            self.at_in
        }
    }

    /// Position of the lane end at `end`.
    pub fn end_point(&self, end: SegEnd) -> Vec2 {
        let at_travel_end = (end == SegEnd::End) == self.forward;
        if at_travel_end {
            self.last_point()
        } else {
            self.first_point()
        }
    }

    /// Lane metres (in travel direction) of a position `s` metres from the segment start.
    pub fn lane_s(&self, s_seg: f32) -> f32 {
        let f = (s_seg / self.seg_len.max(1e-3)).clamp(0.0, 1.0);
        let along_seg = f * self.len;
        if self.forward {
            along_seg
        } else {
            self.len - along_seg
        }
    }
}

/// A connection that conflicts with an [`ExitConn`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct Conflict {
    pub from_slot: u32,
    pub to: LaneId,
    /// The other connection has priority over this one (no light at the node).
    pub higher: bool,
}

/// A junction connection leaving a vehicle lane, resolved for movement. Looked up by
/// `(from, to)` lane pair, never by connection index (indices change when a node is
/// recomputed, T-020).
#[derive(Clone, Debug)]
pub(crate) struct ExitConn {
    pub to: LaneId,
    pub to_slot: u32,
    /// Index in `Links::connections` of its node (valid for this snapshot only).
    pub index: u16,
    pub length: f32,
    /// Speed on the connection, m/s.
    pub speed: f32,
    /// Index into `PathGraph::lights`, or `u32::MAX` when the node has no light.
    pub light: u32,
    pub conflicts: Vec<Conflict>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CarEdge {
    pub to: u32,
    /// Connection travel time plus turn penalty (or [`LANE_CHANGE_COST`]), seconds.
    pub cost: f32,
    /// A lateral hop to the neighbouring lane of the same segment, not a junction connection.
    pub lane_change: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PedEdge {
    pub to: u32,
    pub cost: f32,
}

/// Candidate lanes of a segment for resolving an [`Access`].
#[derive(Clone, Debug, Default)]
struct SegLanes {
    car: Vec<(LaneId, f32)>,
    ped: Vec<(LaneId, f32)>,
    rail: Vec<(LaneId, f32)>,
}

/// Immutable snapshot of the lane network for pathfinding. `Send + Sync`, shared by `Arc`.
#[derive(Debug, Default)]
pub struct PathGraph {
    pub(crate) car: Vec<Option<LaneNode>>,
    pub(crate) car_edges: Vec<Vec<CarEdge>>,
    pub(crate) ped: Vec<Option<LaneNode>>,
    pub(crate) ped_edges: Vec<Vec<PedEdge>>,
    pub(crate) max_car_speed: f32,
    /// The track sub-graph (T-161), a graph of its own: a state is a `Rail` lane, an edge
    /// joins a lane arriving at a node to a lane of another segment leaving it. Cars and
    /// pedestrians never see it, trains never see roads.
    pub(crate) rail: Vec<Option<LaneNode>>,
    pub(crate) rail_edges: Vec<Vec<CarEdge>>,
    pub(crate) max_rail_speed: f32,
    /// Per vehicle-lane slot: the connections leaving it.
    pub(crate) exits: Vec<Vec<ExitConn>>,
    /// Signal plans of the nodes with a light, as referenced by [`ExitConn::light`].
    pub(crate) lights: Vec<LightPlan>,
    /// Per segment slot: total vehicle-lane length, metres (the denominator of the load).
    pub(crate) seg_capacity: Vec<f32>,
    segments: BTreeMap<SegmentId, SegLanes>,
}

fn path_len(c: &[CubicBezier3]) -> f32 {
    let mut len = 0.0;
    let mut prev = c[0].p0;
    for piece in c {
        for k in 1..=8 {
            let p = piece.point(k as f32 / 8.0);
            len += prev.distance(p);
            prev = p;
        }
    }
    len
}

/// Lane metres from the lane start to parameter `u` (`0..=1`, uniform per piece).
fn s_at_param(c: &[CubicBezier3], u: f32) -> f32 {
    let n = c.len();
    let x = u.clamp(0.0, 1.0) * n as f32;
    let k = (x as usize).min(n - 1);
    let mut len = if k == 0 { 0.0 } else { path_len(&c[..k]) };
    let frac = x - k as f32;
    let steps = 8;
    let mut prev = c[k].p0;
    for i in 1..=steps {
        let p = c[k].point(frac * i as f32 / steps as f32);
        len += prev.distance(p);
        prev = p;
    }
    len
}

fn node_of(lane: &Lane, id: LaneId, net: &NetGraph, geo: &JunctionGeometry) -> Option<LaneNode> {
    let seg = net.segment(lane.segment)?;
    let len = path_len(&lane.curve);
    let forward = lane.dir == LaneDir::Forward;
    // Trim parameter of the arm of this segment at `node`, if the node has a junction.
    let trim = |node: NodeId, at_start: bool| {
        geo.get(node)?
            .arms
            .iter()
            .find(|a| a.segment == lane.segment && a.at_start == at_start)
            .map(|a| a.trim_t)
    };
    let (entry, exit) = if forward {
        (trim(seg.start, true), trim(seg.end, false))
    } else {
        (
            trim(seg.end, false).map(|t| 1.0 - t),
            trim(seg.start, true).map(|t| 1.0 - t),
        )
    };
    let mut s_in = entry.map_or(0.0, |u| s_at_param(&lane.curve, u));
    let mut s_out = exit.map_or(len, |u| s_at_param(&lane.curve, u));
    // `point_at` maps lane metres to the curve parameter linearly.
    let mut at_in = entry.map_or(0.0, |u| u.clamp(0.0, 1.0) * len);
    let mut at_out = exit.map_or(len, |u| u.clamp(0.0, 1.0) * len);
    if s_out - s_in < 1.0 {
        s_in = 0.0;
        s_out = len;
    }
    if at_out - at_in < 1.0 {
        at_in = 0.0;
        at_out = len;
    }
    Some(LaneNode {
        id,
        segment: lane.segment,
        kind: lane.kind,
        len,
        speed: lane.speed_limit.max(0.1),
        forward,
        seg_len: seg.length,
        curve: lane.curve,
        s_in,
        s_out,
        at_in,
        at_out,
        from_outside: net
            .node(if forward { seg.start } else { seg.end })
            .is_some_and(|n| n.kind == cp_net::NodeKind::Outside),
    })
}

fn is_vehicle(k: LaneKind) -> bool {
    k.is_road_vehicle()
}

impl PathGraph {
    /// Builds the snapshot.
    pub fn build(net: &NetGraph, geo: &JunctionGeometry) -> PathGraph {
        let slots = net
            .lanes()
            .map(|(id, _)| id.index as usize + 1)
            .max()
            .unwrap_or(0);
        let mut g = PathGraph {
            car: vec![None; slots],
            car_edges: vec![Vec::new(); slots],
            ped: vec![None; slots],
            ped_edges: vec![Vec::new(); slots * 2],
            rail: vec![None; slots],
            rail_edges: vec![Vec::new(); slots],
            exits: vec![Vec::new(); slots],
            ..Default::default()
        };
        for (id, lane) in net.lanes() {
            let Some(node) = node_of(lane, id, net, geo) else {
                continue;
            };
            let sl = g.segments.entry(lane.segment).or_default();
            if is_vehicle(lane.kind) {
                // T-302: a tram or trolleybus lane is no capacity for the general traffic.
                if lane.kind.is_general() {
                    let si = lane.segment.index as usize;
                    if g.seg_capacity.len() <= si {
                        g.seg_capacity.resize(si + 1, 0.0);
                    }
                    g.seg_capacity[si] += node.s_out - node.s_in;
                }
                g.max_car_speed = g.max_car_speed.max(node.speed);
                sl.car.push((id, lane.offset));
                g.car[id.index as usize] = Some(node);
            } else if lane.kind == LaneKind::Pedestrian {
                sl.ped.push((id, lane.offset));
                g.ped[id.index as usize] = Some(node);
            } else if lane.kind == LaneKind::Rail {
                g.max_rail_speed = g.max_rail_speed.max(node.speed);
                sl.rail.push((id, lane.offset));
                g.rail[id.index as usize] = Some(node);
            }
        }
        g.build_rail_edges(net);
        // Vehicle edges: junction connections (nodes in id order => deterministic).
        for (node, junction) in geo.iter() {
            if net.node(*node).is_none() {
                continue;
            }
            let light = junction.control.light.as_ref().map_or(u32::MAX, |plan| {
                g.lights.push(plan.clone());
                g.lights.len() as u32 - 1
            });
            let links = &junction.links;
            for (ci, c) in links.connections.iter().enumerate() {
                let (Some(from), Some(to)) = (g.car_slot(c.from), g.car_slot(c.to)) else {
                    continue;
                };
                let prio = |i: usize| {
                    junction
                        .control
                        .priority
                        .get(i)
                        .copied()
                        .unwrap_or(Priority::Main)
                };
                let conflicts = links
                    .conflicts
                    .iter()
                    .filter_map(|&(a, b)| {
                        let other = if usize::from(a) == ci {
                            usize::from(b)
                        } else if usize::from(b) == ci {
                            usize::from(a)
                        } else {
                            return None;
                        };
                        let o = links.connections.get(other)?;
                        let from_slot = g.car_slot(o.from)?;
                        g.car_slot(o.to)?;
                        Some(Conflict {
                            from_slot,
                            to: o.to,
                            higher: light == u32::MAX
                                && prio(ci) == Priority::Yield
                                && prio(other) == Priority::Main,
                        })
                    })
                    .collect();
                let turn_speed = match c.turn {
                    Turn::Straight => f32::MAX,
                    Turn::Right | Turn::Left => TURN_SPEED,
                    Turn::UTurn => U_TURN_SPEED,
                };
                let lane_speed =
                    |slot: u32| g.car[slot as usize].as_ref().map_or(10.0, |l| l.speed);
                let conn_speed = lane_speed(from).min(lane_speed(to)).min(turn_speed);
                g.exits[from as usize].push(ExitConn {
                    to: c.to,
                    to_slot: to,
                    index: ci as u16,
                    length: c.length,
                    speed: conn_speed,
                    light,
                    conflicts,
                });
                let speed = g.car[to as usize].as_ref().map_or(10.0, |l| l.speed);
                let penalty = match c.turn {
                    Turn::Left => LEFT_TURN_PENALTY,
                    Turn::UTurn => U_TURN_PENALTY,
                    Turn::Right | Turn::Straight => 0.0,
                };
                g.car_edges[from as usize].push(CarEdge {
                    to,
                    cost: c.length / speed + penalty,
                    lane_change: false,
                });
            }
            for link in &junction.links.pedestrian {
                let (Some(a), Some(b)) = (
                    g.ped_state(net, *node, link.a),
                    g.ped_state(net, *node, link.b),
                ) else {
                    continue;
                };
                let cost = path_len(std::slice::from_ref(&link.curve)) / PED_SPEED;
                g.ped_edges[a as usize].push(PedEdge { to: b, cost });
                g.ped_edges[b as usize].push(PedEdge { to: a, cost });
            }
        }
        // Lane changes: neighbouring vehicle lanes of one direction on a segment, both ways.
        for sl in g.segments.values() {
            for forward in [true, false] {
                let mut lanes: Vec<(f32, u32)> = sl
                    .car
                    .iter()
                    .filter_map(|&(id, off)| {
                        let slot = g.car_slot(id)?;
                        (g.car[slot as usize].as_ref()?.forward == forward).then_some((off, slot))
                    })
                    .collect();
                lanes.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                for w in lanes.windows(2) {
                    for (a, b) in [(w[0].1, w[1].1), (w[1].1, w[0].1)] {
                        g.car_edges[a as usize].push(CarEdge {
                            to: b,
                            cost: LANE_CHANGE_COST,
                            lane_change: true,
                        });
                    }
                }
            }
        }
        // Walking along a lane joins its two ends.
        for slot in 0..slots {
            if let Some(l) = &g.ped[slot] {
                let cost = l.len / PED_SPEED;
                g.ped_edges[slot * 2].push(PedEdge {
                    to: slot as u32 * 2 + 1,
                    cost,
                });
                g.ped_edges[slot * 2 + 1].push(PedEdge {
                    to: slot as u32 * 2,
                    cost,
                });
            }
        }
        g
    }

    /// Joins the track lanes at every node where they meet: arriving lane to leaving lane of
    /// another segment, straight on or round a bend, never back along the same segment (a train
    /// turns round at a terminus, T-163). Nodes in id order, so the edge lists are stable.
    fn build_rail_edges(&mut self, net: &NetGraph) {
        for (node, n) in net.nodes() {
            let mut arriving: Vec<u32> = Vec::new();
            let mut leaving: Vec<u32> = Vec::new();
            for &sid in &n.segments {
                let Some(seg) = net.segment(sid) else {
                    continue;
                };
                for &lid in &seg.lanes {
                    let Some(slot) = self.rail_slot(lid) else {
                        continue;
                    };
                    let forward = self.rail[slot as usize].as_ref().is_some_and(|l| l.forward);
                    // A forward lane leaves the segment's start node and arrives at its end.
                    let arrives = (seg.end == node) == forward;
                    if arrives {
                        arriving.push(slot);
                    } else {
                        leaving.push(slot);
                    }
                }
            }
            for &a in &arriving {
                for &b in &leaving {
                    let (la, lb) = (
                        self.rail[a as usize].as_ref().expect("rail lane"),
                        self.rail[b as usize].as_ref().expect("rail lane"),
                    );
                    if la.segment == lb.segment {
                        continue;
                    }
                    self.rail_edges[a as usize].push(CarEdge {
                        to: b,
                        cost: la.last_point().distance(lb.first_point()) / lb.speed,
                        lane_change: false,
                    });
                }
            }
        }
    }

    pub(crate) fn rail_slot(&self, id: LaneId) -> Option<u32> {
        let l = self.rail.get(id.index as usize)?.as_ref()?;
        (l.id == id).then_some(id.index)
    }

    /// Resolves an access point to a track lane, like [`Self::resolve_car`].
    pub(crate) fn resolve_rail(&self, a: &Access) -> Option<(u32, f32)> {
        let sl = self.segments.get(&a.segment)?;
        let pick = pick_lane(&sl.rail, a.side)?;
        let slot = self.rail_slot(pick)?;
        let node = self.rail[slot as usize].as_ref()?;
        Some((slot, node.lane_s(a.s)))
    }

    pub fn rail_lane_count(&self) -> usize {
        self.rail.iter().flatten().count()
    }

    pub(crate) fn car_slot(&self, id: LaneId) -> Option<u32> {
        let l = self.car.get(id.index as usize)?.as_ref()?;
        (l.id == id).then_some(id.index)
    }

    /// The pedestrian lane `id`, if it is in this snapshot.
    pub(crate) fn car_lane(&self, id: LaneId) -> Option<&LaneNode> {
        self.car
            .get(id.index as usize)?
            .as_ref()
            .filter(|l| l.id == id)
    }

    /// Whether the vehicle lane runs with its segment (start to end).
    pub(crate) fn lane_forward(&self, id: LaneId) -> Option<bool> {
        self.car_lane(id).map(|l| l.forward)
    }

    pub(crate) fn ped_lane(&self, id: LaneId) -> Option<&LaneNode> {
        self.ped
            .get(id.index as usize)?
            .as_ref()
            .filter(|l| l.id == id)
    }

    pub(crate) fn ped_slot(&self, id: LaneId) -> Option<u32> {
        let l = self.ped.get(id.index as usize)?.as_ref()?;
        (l.id == id).then_some(id.index)
    }

    /// State index of the end of pedestrian lane `lane` that touches `node`.
    fn ped_state(&self, net: &NetGraph, node: NodeId, lane: LaneId) -> Option<u32> {
        let slot = self.ped_slot(lane)?;
        let seg = net.segment(self.ped[slot as usize].as_ref()?.segment)?;
        let end = if seg.start == node {
            SegEnd::Start
        } else {
            SegEnd::End
        };
        Some(slot * 2 + end.idx() as u32)
    }

    pub fn car_lane_count(&self) -> usize {
        self.car.iter().flatten().count()
    }

    pub fn car_edge_count(&self) -> usize {
        self.car_edges.iter().map(Vec::len).sum()
    }

    /// Length in metres of a car route: the stretch of the first lane after `start_s`, the
    /// lanes in between, the stretch of the last lane up to `goal_s` and the junction
    /// connections (T-153, the length of a line's leg).
    pub fn car_path_length(&self, path: &crate::astar::CarPath) -> f32 {
        let len = |l: LaneId| {
            self.car_slot(l)
                .and_then(|i| self.car[i as usize].as_ref())
                .map_or(0.0, |n| n.len)
        };
        if path.steps.is_empty() {
            return (path.goal_s - path.start_s).max(0.0);
        }
        let mut total = (len(path.start) - path.start_s).max(0.0) + path.goal_s;
        for (i, &(from, to)) in path.steps.iter().enumerate() {
            if i + 1 < path.steps.len() {
                total += len(to);
            }
            if let Some(c) = self
                .car_slot(from)
                .and_then(|f| self.exits[f as usize].iter().find(|e| e.to == to))
            {
                total += c.length;
            }
        }
        total
    }

    /// The length of a route over the track graph, metres (T-163).
    pub fn rail_path_length(&self, path: &crate::astar::CarPath) -> f32 {
        let len = |l: LaneId| {
            self.rail_slot(l)
                .and_then(|i| self.rail[i as usize].as_ref())
                .map_or(0.0, |n| n.len)
        };
        if path.steps.is_empty() {
            return (path.goal_s - path.start_s).max(0.0);
        }
        let mut total = (len(path.start) - path.start_s).max(0.0) + path.goal_s;
        for (i, &(_, to)) in path.steps.iter().enumerate() {
            if i + 1 < path.steps.len() {
                total += len(to);
            }
        }
        total
    }

    /// The route of a car path as a ground-plane polyline, a point about every `step` metres
    /// along each lane it runs on (T-157: the ribbon of a line). Junction connections show as
    /// the straight hop between one lane's last point and the next lane's first.
    pub fn car_path_points(&self, path: &crate::astar::CarPath, step: f32) -> Vec<Vec2> {
        let step = step.max(0.5);
        let lanes: Vec<LaneId> = path.lanes().collect();
        let mut out = Vec::new();
        for (i, &id) in lanes.iter().enumerate() {
            let Some(node) = self.car_lane(id) else {
                continue;
            };
            let from = if i == 0 { path.start_s } else { node.s_in };
            let to = if i + 1 == lanes.len() {
                path.goal_s
            } else {
                node.s_out
            };
            let (from, to) = (from.clamp(0.0, node.len), to.clamp(0.0, node.len));
            let n = (((to - from).abs() / step).ceil() as usize).max(1);
            for k in 0..=n {
                out.push(node.point_at(from + (to - from) * k as f32 / n as f32));
            }
        }
        out
    }

    /// The route of a rail path as a polyline with heights, a point about every `step` metres
    /// along each track lane it runs on (T-164: the ribbon of a metro line and the ride of a
    /// train along its leg).
    pub fn rail_path_points3(
        &self,
        path: &crate::astar::CarPath,
        step: f32,
    ) -> Vec<bevy_math::Vec3> {
        let step = step.max(0.5);
        let lanes: Vec<LaneId> = path.lanes().collect();
        let mut out = Vec::new();
        for (i, &id) in lanes.iter().enumerate() {
            let Some(node) = self
                .rail_slot(id)
                .and_then(|slot| self.rail[slot as usize].as_ref())
            else {
                continue;
            };
            let from = if i == 0 { path.start_s } else { node.s_in };
            let to = if i + 1 == lanes.len() {
                path.goal_s
            } else {
                node.s_out
            };
            let (from, to) = (from.clamp(0.0, node.len), to.clamp(0.0, node.len));
            let n = (((to - from).abs() / step).ceil() as usize).max(1);
            for k in 0..=n {
                out.push(node.point3_at(from + (to - from) * k as f32 / n as f32));
            }
        }
        out
    }

    /// Where an access point stands on its track lane (with the height), if the segment has one.
    pub fn rail_access_point3(&self, a: &Access) -> Option<bevy_math::Vec3> {
        let (slot, s) = self.resolve_rail(a)?;
        Some(self.rail[slot as usize].as_ref()?.point3_at(s))
    }

    /// Is the connection `from → to` still in the network?
    pub fn has_connection(&self, from: LaneId, to: LaneId) -> bool {
        match (self.car_slot(from), self.car_slot(to)) {
            (Some(f), Some(t)) => self.car_edges[f as usize].iter().any(|e| e.to == t),
            _ => false,
        }
    }

    /// Resolves an access point to a vehicle lane: the outermost vehicle lane on the
    /// requested side (positive lane offset is left of the segment direction), or the
    /// outermost one when the road has none on that side. Returns the lane and the position
    /// on it in lane metres.
    pub(crate) fn resolve_car(&self, a: &Access) -> Option<(u32, f32)> {
        self.resolve_class(a, VehicleClass::Car)
    }

    /// Resolves an access point to the lane a vehicle of `class` may use on its segment
    /// (T-302: a bus stop on a road with bus lanes resolves to the bus lane, a car never to a
    /// transit lane) and the position on it (lane metres).
    pub(crate) fn resolve_class(&self, a: &Access, class: VehicleClass) -> Option<(u32, f32)> {
        let sl = self.segments.get(&a.segment)?;
        let allowed: Vec<(LaneId, f32)> = sl
            .car
            .iter()
            .filter(|&&(id, _)| {
                self.car_lane(id)
                    .is_some_and(|n| lane_allows(n.kind, class))
            })
            .copied()
            .collect();
        let pick = pick_lane(&allowed, a.side)?;
        let slot = self.car_slot(pick)?;
        let node = self.car[slot as usize].as_ref()?;
        Some((slot, node.lane_s(a.s)))
    }

    /// Like [`Self::resolve_car`], but gives the lane id.
    pub(crate) fn resolve_car_lane(&self, a: &Access) -> Option<(LaneId, f32)> {
        self.resolve_class_lane(a, VehicleClass::Car)
    }

    /// Like [`Self::resolve_class`], but gives the lane id.
    pub(crate) fn resolve_class_lane(
        &self,
        a: &Access,
        class: VehicleClass,
    ) -> Option<(LaneId, f32)> {
        let (slot, s) = self.resolve_class(a, class)?;
        Some((self.car[slot as usize].as_ref()?.id, s))
    }

    /// The lane (lowest id) of `segment` that runs away from its start (`from_start`) or its
    /// end, with the lane metre where cars enter it: where a move-in car appears at an
    /// `Outside` node (T-118).
    pub(crate) fn entry_lane(&self, segment: SegmentId, from_start: bool) -> Option<(LaneId, f32)> {
        self.entry_lanes(segment, from_start).into_iter().next()
    }

    /// Every car lane of `segment` leaving its start node (`from_start`) or its end node, with
    /// the lane metre where it leaves the junction, ascending by lane id (BUG-117: a run from an
    /// `Outside` node picks the least loaded of them, not always the first).
    pub(crate) fn entry_lanes(&self, segment: SegmentId, from_start: bool) -> Vec<(LaneId, f32)> {
        let Some(sl) = self.segments.get(&segment) else {
            return Vec::new();
        };
        let mut lanes: Vec<(LaneId, f32)> = sl
            .car
            .iter()
            .filter_map(|&(id, _)| self.car_lane(id))
            .filter(|l| l.forward == from_start)
            .map(|l| (l.id, l.s_in))
            .collect();
        lanes.sort_by_key(|(id, _)| *id);
        lanes
    }

    /// Resolves an access point to a sidewalk lane and the position on it (lane metres).
    pub(crate) fn resolve_ped(&self, a: &Access) -> Option<(u32, f32)> {
        let sl = self.segments.get(&a.segment)?;
        let pick = pick_lane(&sl.ped, a.side)?;
        let slot = self.ped_slot(pick)?;
        let node = self.ped[slot as usize].as_ref()?;
        Some((slot, node.lane_s(a.s)))
    }
}

/// Outermost lane on `side` (largest `|offset|` with the matching sign), else the
/// outermost lane overall. Ties go to the lower `LaneId`.
fn pick_lane(lanes: &[(LaneId, f32)], side: Side) -> Option<LaneId> {
    let want = |off: f32| match side {
        Side::Left => off > 0.0,
        Side::Right => off < 0.0,
    };
    let best = |it: &mut dyn Iterator<Item = &(LaneId, f32)>| {
        it.fold(None::<(LaneId, f32)>, |acc, &(id, off)| match acc {
            Some((bid, boff))
                if boff.abs() > off.abs() || (boff.abs() == off.abs() && bid < id) =>
            {
                Some((bid, boff))
            }
            _ => Some((id, off)),
        })
    };
    best(&mut lanes.iter().filter(|(_, o)| want(*o)))
        .or_else(|| best(&mut lanes.iter()))
        .map(|(id, _)| id)
}

#[cfg(test)]
impl PathGraph {
    pub(crate) fn resolve_car_for_test(&self, a: &Access) -> (u32, f32) {
        let (slot, _) = self.resolve_car(a).expect("lane");
        (slot, self.car[slot as usize].as_ref().unwrap().speed)
    }

    pub(crate) fn lane_segment_for_test(&self, lane: LaneId) -> SegmentId {
        self.car[self.car_slot(lane).expect("car lane") as usize]
            .as_ref()
            .unwrap()
            .segment
    }

    pub(crate) fn max_speed_for_test(&self) -> f32 {
        self.max_car_speed
    }

    pub(crate) fn point_for_test(&self, a: &Access) -> Vec2 {
        let (slot, s) = self
            .resolve_car(a)
            .or_else(|| self.resolve_ped(a))
            .expect("lane");
        self.car[slot as usize]
            .as_ref()
            .or(self.ped[slot as usize].as_ref())
            .unwrap()
            .point_at(s)
    }
}
