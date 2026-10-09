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
use cp_content::{LaneDir, LaneKind};
use cp_core::curve::{CubicBezier3, to_xz};
use cp_net::{JunctionGeometry, Lane, LaneId, NetGraph, NodeId, SegmentId, Turn};
use serde::{Deserialize, Serialize};

/// Walking speed used for costs, m/s (pedestrians move at 1.2–1.6 m/s, §9.4).
pub const PED_SPEED: f32 = 1.4;
/// Extra seconds for a left turn (§9.3).
pub const LEFT_TURN_PENALTY: f32 = 4.0;
/// Extra seconds for a U-turn (§9.3).
pub const U_TURN_PENALTY: f32 = 15.0;
/// Congestion weight `k` in `length / speed × (1 + k × congestion)`.
pub const CONGESTION_K: f32 = 3.0;

/// Which end of a segment. Pedestrian lane ends are named by the segment end they touch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    pub len: f32,
    pub speed: f32,
    pub forward: bool,
    pub seg_len: f32,
    pub curve: [CubicBezier3; cp_net::LANE_CURVE_PIECES],
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

#[derive(Clone, Copy, Debug)]
pub(crate) struct CarEdge {
    pub to: u32,
    /// Connection travel time plus turn penalty, seconds.
    pub cost: f32,
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
}

/// Immutable snapshot of the lane network for pathfinding. `Send + Sync`, shared by `Arc`.
#[derive(Debug, Default)]
pub struct PathGraph {
    pub(crate) car: Vec<Option<LaneNode>>,
    pub(crate) car_edges: Vec<Vec<CarEdge>>,
    pub(crate) ped: Vec<Option<LaneNode>>,
    pub(crate) ped_edges: Vec<Vec<PedEdge>>,
    pub(crate) max_car_speed: f32,
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

fn node_of(lane: &Lane, id: LaneId, net: &NetGraph) -> Option<LaneNode> {
    let seg = net.segment(lane.segment)?;
    Some(LaneNode {
        id,
        segment: lane.segment,
        len: path_len(&lane.curve),
        speed: lane.speed_limit.max(0.1),
        forward: lane.dir == LaneDir::Forward,
        seg_len: seg.length,
        curve: lane.curve,
    })
}

fn is_vehicle(k: LaneKind) -> bool {
    matches!(k, LaneKind::Car | LaneKind::Bus)
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
            ..Default::default()
        };
        for (id, lane) in net.lanes() {
            let Some(node) = node_of(lane, id, net) else {
                continue;
            };
            let sl = g.segments.entry(lane.segment).or_default();
            if is_vehicle(lane.kind) {
                g.max_car_speed = g.max_car_speed.max(node.speed);
                sl.car.push((id, lane.offset));
                g.car[id.index as usize] = Some(node);
            } else if lane.kind == LaneKind::Pedestrian {
                sl.ped.push((id, lane.offset));
                g.ped[id.index as usize] = Some(node);
            }
        }
        // Vehicle edges: junction connections (nodes in id order => deterministic).
        for (node, junction) in geo.iter() {
            if net.node(*node).is_none() {
                continue;
            }
            for c in &junction.links.connections {
                let (Some(from), Some(to)) = (g.car_slot(c.from), g.car_slot(c.to)) else {
                    continue;
                };
                let speed = g.car[to as usize].as_ref().map_or(10.0, |l| l.speed);
                let penalty = match c.turn {
                    Turn::Left => LEFT_TURN_PENALTY,
                    Turn::UTurn => U_TURN_PENALTY,
                    Turn::Right | Turn::Straight => 0.0,
                };
                g.car_edges[from as usize].push(CarEdge {
                    to,
                    cost: c.length / speed + penalty,
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

    pub(crate) fn car_slot(&self, id: LaneId) -> Option<u32> {
        let l = self.car.get(id.index as usize)?.as_ref()?;
        (l.id == id).then_some(id.index)
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
        let sl = self.segments.get(&a.segment)?;
        let pick = pick_lane(&sl.car, a.side)?;
        let slot = self.car_slot(pick)?;
        let node = self.car[slot as usize].as_ref()?;
        Some((slot, node.lane_s(a.s)))
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
