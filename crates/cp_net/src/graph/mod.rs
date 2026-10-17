//! [`NetGraph`]: the arenas and the operations on them.
//!
//! Operations are all-or-nothing: on `Err` the graph is unchanged. Every node whose
//! segment set or incident geometry changes is put in the **dirty set**, which a later
//! system (junction recompute) drains with [`NetGraph::take_dirty`].

use std::collections::BTreeSet;
use std::fmt;

use bevy_ecs::resource::Resource;
use bevy_math::Vec3;
use cp_content::{LaneDir, RoadDef, RoadDefId};
use cp_core::Arena;
use cp_core::curve::CubicBezier3;
use serde::{Deserialize, Serialize};

use crate::model::*;

/// An existing node within this distance (metres, 3D) of a new segment's end point is
/// reused instead of creating a new node.
pub const NODE_REUSE_EPS: f32 = 0.1;

/// Segments shorter than this (metres) are rejected.
pub const MIN_SEGMENT_LENGTH: f32 = 0.5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetOpError {
    /// Both ends resolve to the same node, or the curve is shorter than [`MIN_SEGMENT_LENGTH`].
    DegenerateSegment,
    /// The node already has [`MAX_SEGMENTS_PER_NODE`] segments.
    NodeFull(NodeId),
    UnknownSegment(SegmentId),
    UnknownNode(NodeId),
    /// Fewer than 3 segments meet at the node: a traffic light makes no sense there.
    NotAJunction(NodeId),
    /// A split parameter is not strictly inside `(0, 1)` / not strictly increasing, or a
    /// resulting piece would be shorter than [`MIN_SEGMENT_LENGTH`].
    BadSplit,
}

impl fmt::Display for NetOpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DegenerateSegment => write!(f, "segment is degenerate"),
            Self::NodeFull(n) => {
                write!(f, "node {n:?} already has {MAX_SEGMENTS_PER_NODE} segments")
            }
            Self::UnknownSegment(s) => write!(f, "unknown segment {s:?}"),
            Self::UnknownNode(n) => write!(f, "unknown node {n:?}"),
            Self::NotAJunction(n) => write!(f, "node {n:?} has fewer than 3 segments"),
            Self::BadSplit => write!(f, "invalid split position"),
        }
    }
}

impl std::error::Error for NetOpError {}

/// Result of [`NetGraph::split_segments`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitResult {
    /// The new nodes in order along the segment (one per split position).
    pub nodes: Vec<NodeId>,
    /// The replacement segments, start to end (`nodes.len() + 1` of them).
    pub segments: Vec<SegmentId>,
}

#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetGraph {
    nodes: Arena<Node>,
    segments: Arena<Segment>,
    lanes: Arena<Lane>,
    dirty: BTreeSet<NodeId>,
}

impl NetGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }
    /// Replaces every segment's road type id by `f(old)` (a save loaded against content whose
    /// definitions sit at other indices). Lanes are kept as saved.
    pub fn remap_roads(&mut self, f: impl Fn(RoadDefId) -> RoadDefId) {
        for (_, s) in self.segments.iter_mut() {
            s.road = f(s.road);
        }
    }

    pub fn segment(&self, id: SegmentId) -> Option<&Segment> {
        self.segments.get(id)
    }
    pub fn lane(&self, id: LaneId) -> Option<&Lane> {
        self.lanes.get(id)
    }
    /// Nodes in arena (index) order.
    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes.iter()
    }
    pub fn segments(&self) -> impl Iterator<Item = (SegmentId, &Segment)> {
        self.segments.iter()
    }
    pub fn lanes(&self) -> impl Iterator<Item = (LaneId, &Lane)> {
        self.lanes.iter()
    }
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }
    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    /// Nodes needing a junction recompute, in id order.
    pub fn dirty_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.dirty.iter().copied()
    }
    pub fn is_dirty(&self, id: NodeId) -> bool {
        self.dirty.contains(&id)
    }
    /// Drains the dirty set (id order). Called by the consumer of the set.
    pub fn take_dirty(&mut self) -> Vec<NodeId> {
        std::mem::take(&mut self.dirty).into_iter().collect()
    }

    /// Adds a node not attached to anything (e.g. an `Outside` connection).
    pub fn add_node(&mut self, pos: Vec3, kind: NodeKind, elevation: Elevation) -> NodeId {
        let id = self.nodes.insert(Node {
            pos,
            segments: Vec::new(),
            kind,
            control: JunctionControl::default(),
            elevation,
        });
        self.dirty.insert(id);
        id
    }

    /// Finds an existing node within [`NODE_REUSE_EPS`] of `p` (nearest, ties by id).
    pub fn find_node(&self, p: Vec3) -> Option<NodeId> {
        let mut best: Option<(f32, NodeId)> = None;
        for (id, n) in self.nodes.iter() {
            let d = n.pos.distance(p);
            if d <= NODE_REUSE_EPS && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Adds a segment along `curve`, reusing a node at either end when one is within
    /// [`NODE_REUSE_EPS`] and creating it otherwise. The stored curve is snapped to the
    /// reused nodes' positions. Lanes are built from `def`.
    pub fn add_segment(
        &mut self,
        def: &RoadDef,
        road: RoadDefId,
        curve: CubicBezier3,
        elevation: Elevation,
    ) -> Result<SegmentId, NetOpError> {
        let a = self.find_node(curve.p0);
        let b = self.find_node(curve.p3);
        if a.is_some() && a == b {
            return Err(NetOpError::DegenerateSegment);
        }
        for n in [a, b].into_iter().flatten() {
            if self
                .nodes
                .get(n)
                .is_some_and(|n| n.segments.len() >= MAX_SEGMENTS_PER_NODE)
            {
                return Err(NetOpError::NodeFull(n));
            }
        }
        let mut curve = curve;
        if let Some(n) = a {
            curve.p0 = self.nodes.get(n).expect("found node").pos;
        }
        if let Some(n) = b {
            curve.p3 = self.nodes.get(n).expect("found node").pos;
        }
        let length = curve.arc_table().length();
        if length < MIN_SEGMENT_LENGTH {
            return Err(NetOpError::DegenerateSegment);
        }
        let start = a.unwrap_or_else(|| self.add_node(curve.p0, NodeKind::End, elevation));
        let end = b.unwrap_or_else(|| self.add_node(curve.p3, NodeKind::End, elevation));
        Ok(self.insert_segment(def, road, curve, elevation, start, end, length))
    }

    /// Dry run of [`Self::split_segments`] for every `(segment, ts)` of `splits` followed by
    /// [`Self::add_segment`] for every curve of `segments`: `Ok` exactly when all of them
    /// would succeed, decided without changing anything (so a batch can be applied without a
    /// backup of the graph). It tracks the nodes and node degrees the earlier steps of the
    /// batch would create, and applies the same rules in the same order as the real
    /// operations (a test compares the two on crafted batches).
    pub fn check_build(
        &self,
        splits: &[(SegmentId, Vec<f32>)],
        segments: &[CubicBezier3],
    ) -> Result<(), NetOpError> {
        // Nodes the batch creates: position and number of segments on them.
        let mut fresh: Vec<(Vec3, usize)> = Vec::new();
        // Segments added to existing nodes by the batch.
        let mut extra: Vec<(NodeId, usize)> = Vec::new();
        let mut taken: Vec<SegmentId> = Vec::new();
        for (id, ts) in splits {
            if taken.contains(id) {
                return Err(NetOpError::UnknownSegment(*id));
            }
            let seg = self
                .segments
                .get(*id)
                .ok_or(NetOpError::UnknownSegment(*id))?;
            let mut prev = 0.0;
            for &t in ts {
                if !(t > prev && t < 1.0) {
                    return Err(NetOpError::BadSplit);
                }
                prev = t;
            }
            let mut bounds = vec![0.0];
            bounds.extend_from_slice(ts);
            bounds.push(1.0);
            let pieces: Vec<CubicBezier3> = bounds
                .windows(2)
                .map(|w| seg.curve.sub_curve(w[0], w[1]))
                .collect();
            if pieces
                .iter()
                .any(|c| c.arc_table().length() < MIN_SEGMENT_LENGTH)
            {
                return Err(NetOpError::BadSplit);
            }
            taken.push(*id);
            // The pieces replace the segment on its end nodes; the joints are new nodes.
            for piece in &pieces[..pieces.len() - 1] {
                fresh.push((piece.p3, 2));
            }
        }
        // The node a position resolves to: the nearest within the reuse distance, an
        // existing one before one made by the batch on a tie.
        enum Found {
            Old(NodeId),
            New(usize),
        }
        let find = |fresh: &[(Vec3, usize)], p: Vec3| -> Option<Found> {
            let mut best: Option<(f32, Found)> = None;
            for (id, n) in self.nodes.iter() {
                let d = n.pos.distance(p);
                if d <= NODE_REUSE_EPS && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                    best = Some((d, Found::Old(id)));
                }
            }
            for (i, (pos, _)) in fresh.iter().enumerate() {
                let d = pos.distance(p);
                if d <= NODE_REUSE_EPS && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                    best = Some((d, Found::New(i)));
                }
            }
            best.map(|(_, f)| f)
        };
        for curve in segments {
            let (a, b) = (find(&fresh, curve.p0), find(&fresh, curve.p3));
            let same = match (&a, &b) {
                (Some(Found::Old(x)), Some(Found::Old(y))) => x == y,
                (Some(Found::New(x)), Some(Found::New(y))) => x == y,
                _ => false,
            };
            if same {
                return Err(NetOpError::DegenerateSegment);
            }
            for f in [&a, &b].into_iter().flatten() {
                match f {
                    Found::Old(n) => {
                        let node = self.nodes.get(*n);
                        let added = extra.iter().find(|(m, _)| m == n).map_or(0, |e| e.1);
                        if node.is_some_and(|n| n.segments.len() + added >= MAX_SEGMENTS_PER_NODE) {
                            return Err(NetOpError::NodeFull(*n));
                        }
                    }
                    Found::New(i) => {
                        if fresh[*i].1 >= MAX_SEGMENTS_PER_NODE {
                            // A node made by this batch: it has no id yet. Cannot happen with
                            // fewer than MAX_SEGMENTS_PER_NODE segments in a batch.
                            return Err(NetOpError::DegenerateSegment);
                        }
                    }
                }
            }
            let mut snapped = *curve;
            if let Some(f) = &a {
                snapped.p0 = match f {
                    Found::Old(n) => self.nodes.get(*n).expect("found node").pos,
                    Found::New(i) => fresh[*i].0,
                };
            }
            if let Some(f) = &b {
                snapped.p3 = match f {
                    Found::Old(n) => self.nodes.get(*n).expect("found node").pos,
                    Found::New(i) => fresh[*i].0,
                };
            }
            if snapped.arc_table().length() < MIN_SEGMENT_LENGTH {
                return Err(NetOpError::DegenerateSegment);
            }
            // Both ends now carry one more segment; an end that finds no node makes one.
            for (f, p) in [(a, snapped.p0), (b, snapped.p3)] {
                match f {
                    Some(Found::Old(n)) => match extra.iter_mut().find(|(m, _)| *m == n) {
                        Some(e) => e.1 += 1,
                        None => extra.push((n, 1)),
                    },
                    Some(Found::New(i)) => fresh[i].1 += 1,
                    None => fresh.push((p, 1)),
                }
            }
        }
        Ok(())
    }

    /// Removes a segment and its lanes. Nodes left with no segments are removed too
    /// (except `Outside` nodes); the remaining end nodes become dirty.
    pub fn remove_segment(&mut self, id: SegmentId) -> Result<(), NetOpError> {
        let seg = self.take_segment(id)?;
        for n in [seg.start, seg.end] {
            self.dirty.insert(n);
            self.prune_node(n);
        }
        Ok(())
    }

    /// Splits a segment at curve parameter `t` (see [`Self::split_segments`]).
    pub fn split_segment(
        &mut self,
        def: &RoadDef,
        id: SegmentId,
        t: f32,
    ) -> Result<SplitResult, NetOpError> {
        self.split_segments(def, id, &[t])
    }

    /// Splits a segment at the strictly increasing parameters `ts` (each in `(0, 1)`).
    /// The old segment and its lanes are replaced by `ts.len() + 1` segments joined by
    /// new `Middle` nodes. `def` must be the segment's road definition.
    pub fn split_segments(
        &mut self,
        def: &RoadDef,
        id: SegmentId,
        ts: &[f32],
    ) -> Result<SplitResult, NetOpError> {
        let seg = self
            .segments
            .get(id)
            .ok_or(NetOpError::UnknownSegment(id))?
            .clone();
        let mut prev = 0.0;
        for &t in ts {
            if !(t > prev && t < 1.0) {
                return Err(NetOpError::BadSplit);
            }
            prev = t;
        }
        let mut bounds = vec![0.0];
        bounds.extend_from_slice(ts);
        bounds.push(1.0);
        let pieces: Vec<CubicBezier3> = bounds
            .windows(2)
            .map(|w| seg.curve.sub_curve(w[0], w[1]))
            .collect();
        let lengths: Vec<f32> = pieces.iter().map(|c| c.arc_table().length()).collect();
        if lengths.iter().any(|&l| l < MIN_SEGMENT_LENGTH) {
            return Err(NetOpError::BadSplit);
        }

        self.take_segment(id).expect("checked above");
        let mut nodes = Vec::with_capacity(ts.len());
        let mut chain = vec![seg.start];
        for piece in &pieces[..pieces.len() - 1] {
            let n = self.add_node(piece.p3, NodeKind::Middle, seg.elevation);
            nodes.push(n);
            chain.push(n);
        }
        chain.push(seg.end);
        let mut segments = Vec::with_capacity(pieces.len());
        for (i, (piece, length)) in pieces.into_iter().zip(lengths).enumerate() {
            segments.push(self.insert_segment(
                def,
                seg.road,
                piece,
                seg.elevation,
                chain[i],
                chain[i + 1],
                length,
            ));
        }
        self.dirty.insert(seg.start);
        self.dirty.insert(seg.end);
        Ok(SplitResult { nodes, segments })
    }

    /// Changes a segment's road type: same nodes and curve, lanes rebuilt from `def`.
    pub fn upgrade_segment(
        &mut self,
        id: SegmentId,
        def: &RoadDef,
        road: RoadDefId,
    ) -> Result<(), NetOpError> {
        let seg = self
            .segments
            .get(id)
            .ok_or(NetOpError::UnknownSegment(id))?;
        let old_lanes = seg.lanes.clone();
        let (curve, start, end) = (seg.curve, seg.start, seg.end);
        for l in old_lanes {
            self.lanes.remove(l);
        }
        let lanes = self.build_lanes(id, def, &curve);
        let seg = self.segments.get_mut(id).expect("checked above");
        seg.road = road;
        seg.lanes = lanes;
        self.dirty.insert(start);
        self.dirty.insert(end);
        Ok(())
    }

    /// The player's `ToggleTrafficLight`: a node with a light (automatic or not) becomes
    /// `ManualPriority`, any other `ManualTrafficLight`. Needs a junction (>= 3 segments).
    /// Marks the node dirty so the Network phase rebuilds its plan. Returns the new control.
    pub fn toggle_traffic_light(&mut self, id: NodeId) -> Result<JunctionControl, NetOpError> {
        let node = self.nodes.get_mut(id).ok_or(NetOpError::UnknownNode(id))?;
        if node.segments.len() < 3 {
            return Err(NetOpError::NotAJunction(id));
        }
        node.control = if node.control.has_light() {
            JunctionControl::ManualPriority
        } else {
            JunctionControl::ManualTrafficLight
        };
        let c = node.control;
        self.dirty.insert(id);
        Ok(c)
    }

    /// Sets an *automatic* control state (the §6.5 rule) without marking the node dirty.
    /// Manual states are never overwritten.
    pub fn set_auto_control(&mut self, id: NodeId, control: JunctionControl) {
        if let Some(n) = self.nodes.get_mut(id)
            && !n.control.is_manual()
            && !control.is_manual()
        {
            n.control = control;
        }
    }

    // --- internals ---

    #[allow(clippy::too_many_arguments)]
    fn insert_segment(
        &mut self,
        def: &RoadDef,
        road: RoadDefId,
        curve: CubicBezier3,
        elevation: Elevation,
        start: NodeId,
        end: NodeId,
        length: f32,
    ) -> SegmentId {
        let id = self.segments.insert(Segment {
            start,
            end,
            curve,
            road,
            elevation,
            lanes: Vec::new(),
            length,
        });
        let lanes = self.build_lanes(id, def, &curve);
        self.segments.get_mut(id).expect("just inserted").lanes = lanes;
        for n in [start, end] {
            self.nodes
                .get_mut(n)
                .expect("segment ends exist")
                .segments
                .push(id);
            self.refresh_kind(n);
            self.dirty.insert(n);
        }
        id
    }

    fn build_lanes(&mut self, seg: SegmentId, def: &RoadDef, curve: &CubicBezier3) -> Vec<LaneId> {
        let speed = def.speed_kmh / 3.6;
        def.lanes
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let chain = curve.offset_xz_pieces(l.offset, LANE_CURVE_PIECES);
                let mut travel: [CubicBezier3; LANE_CURVE_PIECES] =
                    chain.try_into().expect("piece count is fixed");
                if l.dir == LaneDir::Backward {
                    travel.reverse();
                    for c in &mut travel {
                        *c = CubicBezier3::new(c.p3, c.p2, c.p1, c.p0);
                    }
                }
                self.lanes.insert(Lane {
                    segment: seg,
                    index: i as u8,
                    kind: l.kind,
                    dir: l.dir,
                    width: l.width,
                    offset: l.offset,
                    speed_limit: speed,
                    curve: travel,
                })
            })
            .collect()
    }

    /// Removes the segment, its lanes and its links from the end nodes; does not touch
    /// dirty flags or prune nodes.
    fn take_segment(&mut self, id: SegmentId) -> Result<Segment, NetOpError> {
        let seg = self
            .segments
            .remove(id)
            .ok_or(NetOpError::UnknownSegment(id))?;
        for &l in &seg.lanes {
            self.lanes.remove(l);
        }
        for n in [seg.start, seg.end] {
            if let Some(node) = self.nodes.get_mut(n) {
                node.segments.retain(|&s| s != id);
            }
        }
        Ok(seg)
    }

    fn prune_node(&mut self, id: NodeId) {
        let Some(node) = self.nodes.get(id) else {
            return;
        };
        if node.segments.is_empty() && node.kind != NodeKind::Outside {
            self.nodes.remove(id);
            self.dirty.remove(&id);
        } else {
            self.refresh_kind(id);
        }
    }

    fn refresh_kind(&mut self, id: NodeId) {
        if let Some(n) = self.nodes.get_mut(id)
            && n.kind != NodeKind::Outside
        {
            n.kind = match n.segments.len() {
                0 | 1 => NodeKind::End,
                2 => NodeKind::Middle,
                _ => NodeKind::Junction,
            };
            // Level of a node: ground if any ground segment meets there, else elevated if
            // any elevated one does, else bridge. Independent of attachment order.
            let mut level: Option<Elevation> = None;
            for sid in &n.segments {
                if let Some(seg) = self.segments.get(*sid) {
                    let rank = |e: Elevation| match e {
                        Elevation::Ground => 0,
                        Elevation::Elevated => 1,
                        Elevation::Bridge => 2,
                        Elevation::Tunnel => 3,
                    };
                    if level.is_none_or(|l| rank(seg.elevation) < rank(l)) {
                        level = Some(seg.elevation);
                    }
                }
            }
            // The end of a road tunnel is its portal, on the surface (T-173): a node where only
            // one tunnel segment of a road (not a track) ends counts as ground.
            if level == Some(Elevation::Tunnel)
                && n.segments.len() == 1
                && self.segments.get(n.segments[0]).is_some_and(|seg| {
                    seg.lanes
                        .first()
                        .and_then(|&l| self.lanes.get(l))
                        .is_some_and(|l| l.kind != cp_content::LaneKind::Rail)
                })
            {
                level = Some(Elevation::Ground);
            }
            if let Some(l) = level {
                n.elevation = l;
            }
        }
    }
}

#[cfg(test)]
mod tests;
