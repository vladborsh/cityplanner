//! [`NetGraph`]: the arenas and the operations on them.
//!
//! Operations are all-or-nothing: on `Err` the graph is unchanged. Every node whose
//! segment set or incident geometry changes is put in the **dirty set**, which a later
//! system (junction recompute) drains with [`NetGraph::take_dirty`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use bevy_ecs::resource::Resource;
use bevy_math::{Vec2, Vec3};
use cp_content::{LaneDir, LaneKind, RoadClass, RoadDef, RoadDefId};
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
    /// The junction cannot be controlled by the player (§6.5a): fewer than 3 arms with vehicle
    /// lanes, or none of them with 4 or more (two-lane roads only).
    NotControllable(NodeId),
    /// The segment does not meet the node.
    NotAnArm(NodeId, SegmentId),
    /// The sign would make a third main arm.
    TooManyMain(NodeId),
    /// A lane rule names a lane the arm does not have (or one that does not run that way, or
    /// that the inbound lane may not enter) (T-313, §18.2).
    NoSuchLane(NodeId, SegmentId, u8),
    /// A lane rule without an exit, or one that leaves a lane with no connection.
    DeadLane(NodeId, SegmentId, u8),
    /// The rules would leave an outgoing arm unreachable from an arm it can be reached from by
    /// default.
    ArmUnreachable(NodeId, SegmentId),
    /// A segment end over or under the node at its place, off the node's height by more than
    /// [`crate::LEVEL_TOL`] and less than [`crate::ELEVATED_CLEARANCE`] (T-332): a node takes
    /// roads only at its own height.
    LevelMismatch(NodeId),
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
            Self::NotControllable(n) => {
                write!(
                    f,
                    "node {n:?} joins two-lane roads only: control is automatic"
                )
            }
            Self::NotAnArm(n, s) => write!(f, "segment {s:?} does not meet node {n:?}"),
            Self::TooManyMain(n) => write!(f, "node {n:?} would have more than two main arms"),
            Self::NoSuchLane(n, s, l) => {
                write!(f, "node {n:?}: segment {s:?} has no lane {l} there")
            }
            Self::DeadLane(n, s, l) => {
                write!(
                    f,
                    "node {n:?}: lane {l} of segment {s:?} would lead nowhere"
                )
            }
            Self::ArmUnreachable(n, s) => {
                write!(
                    f,
                    "node {n:?}: no lane would lead to segment {s:?} any more"
                )
            }
            Self::LevelMismatch(n) => write!(f, "node {n:?} is on another level"),
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
    /// The `order` the next new segment gets (T-262, save v55).
    next_order: u32,
}

impl NetGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// A graph from its saved parts (the save migration to v55 builds one this way).
    pub fn from_parts(
        nodes: Arena<Node>,
        segments: Arena<Segment>,
        lanes: Arena<Lane>,
        dirty: BTreeSet<NodeId>,
        next_order: u32,
    ) -> Self {
        Self {
            nodes,
            segments,
            lanes,
            dirty,
            next_order,
        }
    }

    /// The saved parts of the graph (copies), the inverse of [`Self::from_parts`]: nodes,
    /// segments, lanes, the dirty set and the next order.
    #[allow(clippy::type_complexity)]
    pub fn to_parts(
        &self,
    ) -> (
        Arena<Node>,
        Arena<Segment>,
        Arena<Lane>,
        BTreeSet<NodeId>,
        u32,
    ) {
        (
            self.nodes.clone(),
            self.segments.clone(),
            self.lanes.clone(),
            self.dirty.clone(),
            self.next_order,
        )
    }

    /// The `order` the next new segment gets.
    pub fn next_order(&self) -> u32 {
        self.next_order
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
            signs: Vec::new(),
            lane_rules: Vec::new(),
        });
        self.dirty.insert(id);
        id
    }

    /// Sets the class of a node by its height (T-332: a plan decides it for the nodes it makes).
    pub fn set_node_elevation(&mut self, id: NodeId, elevation: Elevation) {
        if let Some(n) = self.nodes.get_mut(id)
            && n.elevation != elevation
        {
            n.elevation = elevation;
            self.dirty.insert(id);
        }
    }

    /// A node a segment end at `p` that reuses no node would stand over or under, off its height
    /// by more than [`crate::LEVEL_TOL`] and less than [`crate::ELEVATED_CLEARANCE`] (T-332):
    /// [`NetOpError::LevelMismatch`]. Farther apart, two levels pass each other.
    pub(crate) fn level_mismatch(&self, p: Vec3) -> Option<NodeId> {
        self.nodes
            .iter()
            .find(|(_, n)| {
                let dy = (n.pos.y - p.y).abs();
                Vec2::new(n.pos.x - p.x, n.pos.z - p.z).length() <= NODE_REUSE_EPS
                    && dy > crate::LEVEL_TOL
                    && dy < crate::ELEVATED_CLEARANCE
            })
            .map(|(id, _)| id)
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
        for (found, p) in [(a, curve.p0), (b, curve.p3)] {
            if found.is_none()
                && let Some(n) = self.level_mismatch(p)
            {
                return Err(NetOpError::LevelMismatch(n));
            }
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
        let order = self.next_order;
        self.next_order += 1;
        Ok(self.insert_segment(def, road, curve, elevation, start, end, length, order))
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
            for (found, p) in [(&a, curve.p0), (&b, curve.p3)] {
                if found.is_none()
                    && let Some(n) = self.level_mismatch(p)
                {
                    return Err(NetOpError::LevelMismatch(n));
                }
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
            self.drop_control_if_uncontrollable(n);
            self.tidy_lane_rules(n);
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

        let signs = [self.sign_at(seg.start, id), self.sign_at(seg.end, id)];
        let rules = [self.rules_at(seg.start), self.rules_at(seg.end)];
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
            let id = self.insert_segment(
                def,
                seg.road,
                piece,
                seg.elevation,
                chain[i],
                chain[i + 1],
                length,
                seg.order,
            );
            self.segments.get_mut(id).expect("just inserted").trees = seg.trees;
            segments.push(id);
        }
        // The end pieces take the old segment's signs at the old ends (§6.5a).
        let ends = [
            (seg.start, segments[0]),
            (seg.end, segments[segments.len() - 1]),
        ];
        for (((node, piece), sign), rules) in ends.into_iter().zip(signs).zip(rules) {
            if let Some(sign) = sign {
                self.put_sign(node, piece, sign);
            }
            // The lane rules of the end nodes follow the end pieces, and so do the exits to them
            // (§18.2).
            self.retarget_lane_rules(node, rules, id, piece);
        }
        self.dirty.insert(seg.start);
        self.dirty.insert(seg.end);
        Ok(SplitResult { nodes, segments })
    }

    /// Removes the joint `node` between two straight segments of one road and level that run
    /// on through it in one direction (BUG-069): both are replaced by one straight segment, which
    /// is returned. `None` (nothing changed) for any other node. `def` must be the road's
    /// definition.
    pub fn merge_joint(&mut self, def: &RoadDef, node: NodeId) -> Option<SegmentId> {
        self.merge_joint_in(def, node, false)
    }

    /// [`Self::merge_joint`] with the straightness judged in the plane (T-264): a cut of a
    /// street on a slope is not on the 3D line between the ends, and is a cut of a straight
    /// street all the same. The merged street runs straight between the heights of its ends.
    pub fn merge_joint_planar(&mut self, def: &RoadDef, node: NodeId) -> Option<SegmentId> {
        self.merge_joint_in(def, node, true)
    }

    fn merge_joint_in(&mut self, def: &RoadDef, node: NodeId, planar: bool) -> Option<SegmentId> {
        let flat = |v: Vec3| if planar { Vec3::new(v.x, 0.0, v.z) } else { v };
        let n = self.nodes.get(node)?;
        if n.kind != NodeKind::Middle || n.segments.len() != 2 {
            return None;
        }
        let (a, b) = (n.segments[0], n.segments[1]);
        let (sa, sb) = (self.segments.get(a)?, self.segments.get(b)?);
        // `first` ends in the joint and `second` starts there.
        let (first, second, first_id, second_id) = if sa.end == node && sb.start == node {
            (sa, sb, a, b)
        } else if sb.end == node && sa.start == node {
            (sb, sa, b, a)
        } else {
            return None;
        };
        let straight = |c: &CubicBezier3| {
            let d = flat(c.p3 - c.p0).normalize_or_zero();
            [c.p1, c.p2]
                .iter()
                .all(|p| flat(*p - c.p0).cross(d).length() < 0.01)
        };
        let (from, to) = (first.curve.p0, second.curve.p3);
        let dir = flat(to - from).normalize_or_zero();
        let on_line = |p: Vec3| flat(p - from).cross(dir).length() < 0.05;
        if first.road != second.road
            || first.elevation != second.elevation
            || first.start == second.end
        {
            return None;
        }
        let curve = if straight(&first.curve) && straight(&second.curve) && on_line(first.curve.p3)
        {
            CubicBezier3::line(from, to)
        } else if planar {
            // T-268: two pieces of one curve (a cut of an arc) are that curve again.
            unsplit(&first.curve, &second.curve)?
        } else {
            return None;
        };
        let (start, end, road, elevation) = (first.start, second.end, first.road, first.elevation);
        let (order_a, order_b) = (first.order, second.order);
        let trees = first.trees && second.trees;
        let signs = [self.sign_at(start, first_id), self.sign_at(end, second_id)];
        let rules = [self.rules_at(start), self.rules_at(end)];
        self.take_segment(a).ok()?;
        self.take_segment(b).ok()?;
        self.prune_node(node);
        let length = curve.arc_table().length();
        let order = order_a.min(order_b);
        let merged = self.insert_segment(def, road, curve, elevation, start, end, length, order);
        self.segments.get_mut(merged).expect("just inserted").trees = trees;
        // The far ends keep their signs on the merged segment (§6.5a).
        for ((n, sign), rules) in [start, end].into_iter().zip(signs).zip(rules) {
            if let Some(sign) = sign {
                self.put_sign(n, merged, sign);
            }
            let old = if n == start { first_id } else { second_id };
            self.retarget_lane_rules(n, rules, old, merged);
        }
        Some(merged)
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
        // A road that takes no trees (a path, a railway) loses the row (§24.1).
        seg.trees &= trees_allowed(def, seg.elevation);
        // A footpath carries no sign; a node left with two-lane roads only loses its control.
        let footpath = crate::control::vehicle_lanes(self, id) == 0;
        for n in [start, end] {
            if footpath && let Some(node) = self.nodes.get_mut(n) {
                node.signs.retain(|a| a.segment != id);
            }
            self.drop_control_if_uncontrollable(n);
            // The lane indices stay; the ones the new road lacks go (§18.2).
            self.tidy_lane_rules(n);
            self.dirty.insert(n);
        }
        Ok(())
    }

    /// Sets the street-tree flag of a segment (`SetTrees`, §24). The caller has checked
    /// [`trees_allowed`].
    pub fn set_trees(&mut self, id: SegmentId, trees: bool) -> Result<(), NetOpError> {
        self.segments
            .get_mut(id)
            .ok_or(NetOpError::UnknownSegment(id))?
            .trees = trees;
        Ok(())
    }

    /// Turns a segment round (`FlipSegment`, T-091): start and end change places, the curve
    /// runs the other way and the lanes are made again from `def` along it, so a one-way
    /// road's lanes now point the opposite way. Both end nodes are marked dirty, so the
    /// junction plans and the lane connections there are recomputed in the Network phase.
    pub fn flip_segment(&mut self, id: SegmentId, def: &RoadDef) -> Result<(), NetOpError> {
        let seg = self
            .segments
            .get(id)
            .ok_or(NetOpError::UnknownSegment(id))?;
        let old_lanes = seg.lanes.clone();
        let c = seg.curve;
        let (start, end) = (seg.start, seg.end);
        let curve = CubicBezier3 {
            p0: c.p3,
            p1: c.p2,
            p2: c.p1,
            p3: c.p0,
        };
        for l in old_lanes {
            self.lanes.remove(l);
        }
        let lanes = self.build_lanes(id, def, &curve);
        let seg = self.segments.get_mut(id).expect("checked above");
        seg.curve = curve;
        seg.start = end;
        seg.end = start;
        seg.lanes = lanes;
        // Its lanes run the other way now: the rules that name it, and the exits to it, go.
        for n in [start, end] {
            if let Some(node) = self.nodes.get_mut(n) {
                node.lane_rules.retain(|r| r.segment != id);
                for r in &mut node.lane_rules {
                    r.exits.retain(|e| e.arm != id);
                }
                node.lane_rules.retain(|r| !r.exits.is_empty());
            }
        }
        self.dirty.insert(start);
        self.dirty.insert(end);
        Ok(())
    }

    /// The player's `ToggleTrafficLight`: a node with a light (automatic or not) becomes
    /// `ManualPriority`, any other `ManualTrafficLight`. Needs a junction (>= 3 segments).
    /// Marks the node dirty so the Network phase rebuilds its plan. Returns the new control.
    pub fn toggle_traffic_light(&mut self, id: NodeId) -> Result<JunctionControl, NetOpError> {
        let node = self.nodes.get(id).ok_or(NetOpError::UnknownNode(id))?;
        if node.segments.len() < 3 {
            return Err(NetOpError::NotAJunction(id));
        }
        if !self.is_controllable(id) {
            return Err(NetOpError::NotControllable(id));
        }
        let node = self.nodes.get_mut(id).expect("checked above");
        node.control = if node.control.has_light() {
            JunctionControl::ManualPriority
        } else {
            JunctionControl::ManualTrafficLight
        };
        let c = node.control;
        self.dirty.insert(id);
        Ok(c)
    }

    /// Whether the player may control the junction (§6.5a): at least 3 arms with vehicle
    /// (`Car`/`Bus`) lanes, at least one of them with [`BIG_ROAD_LANES`] or more. A junction of
    /// two-lane roads only keeps the automatic priority by rank.
    ///
    /// [`BIG_ROAD_LANES`]: crate::control::BIG_ROAD_LANES
    pub fn is_controllable(&self, id: NodeId) -> bool {
        let Some(node) = self.nodes.get(id) else {
            return false;
        };
        let lanes: Vec<usize> = node
            .segments
            .iter()
            .map(|&s| crate::control::vehicle_lanes(self, s))
            .collect();
        lanes.iter().filter(|&&l| l > 0).count() >= 3
            && lanes.iter().any(|&l| l >= crate::control::BIG_ROAD_LANES)
    }

    /// The player's `SetSign` (§6.5a): puts `sign` on the arm `segment` of `id`, or takes the
    /// arm's sign away (`None`). Refused, changing nothing, for a segment that does not meet
    /// the node, an uncontrollable node and a third `Main`. Marks the node dirty.
    pub fn set_sign(
        &mut self,
        id: NodeId,
        segment: SegmentId,
        sign: Option<Sign>,
    ) -> Result<(), NetOpError> {
        let node = self.nodes.get(id).ok_or(NetOpError::UnknownNode(id))?;
        if !node.segments.contains(&segment) {
            return Err(NetOpError::NotAnArm(id, segment));
        }
        if !self.is_controllable(id) {
            return Err(NetOpError::NotControllable(id));
        }
        let mains = node
            .signs
            .iter()
            .filter(|a| a.segment != segment && a.sign == Sign::Main)
            .count();
        if sign == Some(Sign::Main) && mains >= 2 {
            return Err(NetOpError::TooManyMain(id));
        }
        let node = self.nodes.get_mut(id).expect("checked above");
        node.signs.retain(|a| a.segment != segment);
        if let Some(sign) = sign {
            self.put_sign(id, segment, sign);
        }
        self.dirty.insert(id);
        Ok(())
    }

    /// The player's `SetLaneRule` (T-313, §18.2): sets the exits of the inbound lane `lane` (its
    /// index in the segment) of the arm `segment` of `id`, or takes the lane's rule away (`None`).
    /// Refused, changing nothing, for a node of fewer than 3 arms, a segment that does not meet
    /// it, a lane or exit that does not exist, a lane left without a connection, and rules that
    /// would leave an arm unreachable from an arm it could be reached from by default. Marks the
    /// node dirty.
    pub fn set_lane_rule(
        &mut self,
        id: NodeId,
        segment: SegmentId,
        lane: u8,
        exits: Option<Vec<Exit>>,
    ) -> Result<(), NetOpError> {
        let node = self.nodes.get(id).ok_or(NetOpError::UnknownNode(id))?;
        if node.segments.len() < 3 {
            return Err(NetOpError::NotAJunction(id));
        }
        if !node.segments.contains(&segment) {
            return Err(NetOpError::NotAnArm(id, segment));
        }
        let old = node.lane_rules.clone();
        let mut rules = old.clone();
        rules.retain(|r| !(r.segment == segment && r.lane == lane));
        if let Some(mut exits) = exits {
            let Some(inbound) = self.inbound_lane(id, segment, lane) else {
                return Err(NetOpError::NoSuchLane(id, segment, lane));
            };
            if exits.is_empty() {
                return Err(NetOpError::DeadLane(id, segment, lane));
            }
            let from_kind = inbound.kind;
            for e in &exits {
                let arm_ok = self
                    .nodes
                    .get(id)
                    .is_some_and(|n| n.segments.contains(&e.arm));
                if !arm_ok {
                    return Err(NetOpError::NotAnArm(id, e.arm));
                }
                if let Some(k) = e.lane {
                    let ok = self.outbound_lane(id, e.arm, k).is_some_and(|l| {
                        crate::connections::exit_kinds(from_kind).contains(&l.kind)
                    });
                    if !ok {
                        return Err(NetOpError::NoSuchLane(id, e.arm, k));
                    }
                }
            }
            exits.sort();
            exits.dedup();
            let at = rules.partition_point(|r| (r.segment, r.lane) < (segment, lane));
            rules.insert(
                at,
                LaneRule {
                    segment,
                    lane,
                    exits,
                },
            );
        }
        if rules == old {
            return Ok(());
        }
        self.nodes.get_mut(id).expect("checked above").lane_rules = Vec::new();
        let before = self.arm_reach(id);
        self.nodes.get_mut(id).expect("checked above").lane_rules = rules;
        let verdict = self.check_lane_rules(id, &before);
        if let Err(e) = verdict {
            self.nodes.get_mut(id).expect("checked above").lane_rules = old;
            return Err(e);
        }
        self.dirty.insert(id);
        Ok(())
    }

    /// Which arms each arm's inbound lanes reach through the node (U-turns left out), with the
    /// node's lane rules as they stand.
    fn arm_reach(&self, id: NodeId) -> BTreeMap<SegmentId, BTreeSet<SegmentId>> {
        let mut reach: BTreeMap<SegmentId, BTreeSet<SegmentId>> = BTreeMap::new();
        let Some(j) = crate::junction::compute_junction(self, id, 0) else {
            return reach;
        };
        for c in &j.links.connections {
            let (Some(a), Some(b)) = (self.lane(c.from), self.lane(c.to)) else {
                continue;
            };
            if a.segment != b.segment {
                reach.entry(a.segment).or_default().insert(b.segment);
            }
        }
        reach
    }

    /// The invariants of the node's rules (§18.2): every ruled lane leads somewhere, and every
    /// arm in `before` (what the default reached) is still reached from the same arms.
    fn check_lane_rules(
        &self,
        id: NodeId,
        before: &BTreeMap<SegmentId, BTreeSet<SegmentId>>,
    ) -> Result<(), NetOpError> {
        let Some(j) = crate::junction::compute_junction(self, id, 0) else {
            return Ok(());
        };
        let rules = self.nodes.get(id).map_or(&[][..], |n| &n.lane_rules[..]);
        for r in rules {
            let lane = self
                .segment(r.segment)
                .and_then(|s| s.lanes.get(usize::from(r.lane)).copied());
            if !lane.is_some_and(|l| j.links.connections.iter().any(|c| c.from == l)) {
                return Err(NetOpError::DeadLane(id, r.segment, r.lane));
            }
        }
        let after = self.arm_reach(id);
        for (from, arms) in before {
            for &to in arms {
                if !after.get(from).is_some_and(|s| s.contains(&to)) {
                    return Err(NetOpError::ArmUnreachable(id, to));
                }
            }
        }
        Ok(())
    }

    /// The inbound vehicle lane `index` of `segment` at `node`.
    fn inbound_lane(&self, node: NodeId, segment: SegmentId, index: u8) -> Option<&Lane> {
        let s = self.segments.get(segment)?;
        let l = self.lanes.get(*s.lanes.get(usize::from(index))?)?;
        let towards = if s.end == node {
            l.dir == LaneDir::Forward
        } else {
            s.start == node && l.dir == LaneDir::Backward
        };
        (towards && l.kind.is_road_vehicle()).then_some(l)
    }

    /// The outbound vehicle lane `index` of `segment` at `node`.
    fn outbound_lane(&self, node: NodeId, segment: SegmentId, index: u8) -> Option<&Lane> {
        let s = self.segments.get(segment)?;
        let l = self.lanes.get(*s.lanes.get(usize::from(index))?)?;
        let away = if s.start == node {
            l.dir == LaneDir::Forward
        } else {
            s.end == node && l.dir == LaneDir::Backward
        };
        (away && l.kind.is_road_vehicle()).then_some(l)
    }

    fn rules_at(&self, node: NodeId) -> Vec<LaneRule> {
        self.nodes
            .get(node)
            .map_or_else(Vec::new, |n| n.lane_rules.clone())
    }

    /// Puts back the lane `rules` of `node` with the arm `old` renamed `new`, then tidies them.
    fn retarget_lane_rules(
        &mut self,
        node: NodeId,
        mut rules: Vec<LaneRule>,
        old: SegmentId,
        new: SegmentId,
    ) {
        for r in &mut rules {
            if r.segment == old {
                r.segment = new;
            }
            for e in &mut r.exits {
                if e.arm == old {
                    e.arm = new;
                }
            }
        }
        if let Some(n) = self.nodes.get_mut(node) {
            n.lane_rules = rules;
        }
        self.tidy_lane_rules(node);
    }

    /// Drops what the node's lane rules name that is no longer there (§18.2): all of them when
    /// fewer than 3 arms are left, a rule of a gone arm or lane, an exit to a gone arm, and a
    /// fixed exit lane that is gone becomes a free one.
    fn tidy_lane_rules(&mut self, node: NodeId) {
        let Some(n) = self.nodes.get(node) else {
            return;
        };
        if n.lane_rules.is_empty() {
            return;
        }
        let mut rules = n.lane_rules.clone();
        if n.segments.len() < 3 {
            rules.clear();
        } else {
            rules.retain(|r| n.segments.contains(&r.segment));
            rules.retain(|r| self.inbound_lane(node, r.segment, r.lane).is_some());
            for r in &mut rules {
                r.exits.retain(|e| n.segments.contains(&e.arm));
                for e in &mut r.exits {
                    if let Some(k) = e.lane
                        && self.outbound_lane(node, e.arm, k).is_none()
                    {
                        e.lane = None;
                    }
                }
                r.exits.sort();
                r.exits.dedup();
            }
            rules.retain(|r| !r.exits.is_empty());
            rules.sort_by_key(|r| (r.segment, r.lane));
        }
        if let Some(n) = self.nodes.get_mut(node) {
            n.lane_rules = rules;
        }
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
        order: u32,
    ) -> SegmentId {
        let id = self.segments.insert(Segment {
            start,
            end,
            curve,
            road,
            elevation,
            lanes: Vec::new(),
            length,
            order,
            trees: false,
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
                node.signs.retain(|a| a.segment != id);
                node.lane_rules.retain(|r| r.segment != id);
                for r in &mut node.lane_rules {
                    r.exits.retain(|e| e.arm != id);
                }
                node.lane_rules.retain(|r| !r.exits.is_empty());
            }
        }
        Ok(seg)
    }

    /// The sign on the arm `segment` of `node`.
    fn sign_at(&self, node: NodeId, segment: SegmentId) -> Option<Sign> {
        self.nodes
            .get(node)?
            .signs
            .iter()
            .find(|a| a.segment == segment)
            .map(|a| a.sign)
    }

    /// Puts `sign` on the arm `segment` of `node`, keeping the signs sorted by segment.
    fn put_sign(&mut self, node: NodeId, segment: SegmentId, sign: Sign) {
        if let Some(n) = self.nodes.get_mut(node) {
            match n.signs.binary_search_by_key(&segment, |a| a.segment) {
                Ok(i) => n.signs[i].sign = sign,
                Err(i) => n.signs.insert(i, ArmSign { segment, sign }),
            }
        }
    }

    /// A node the player can no longer control (§6.5a: an arm went or became a footpath)
    /// loses its signs, and a manual control goes back to the automatic `Priority`.
    fn drop_control_if_uncontrollable(&mut self, id: NodeId) {
        if self.is_controllable(id) {
            return;
        }
        if let Some(n) = self.nodes.get_mut(id) {
            n.signs.clear();
            if n.control.is_manual() {
                n.control = JunctionControl::Priority;
            }
        }
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

mod invariants;
pub use invariants::{MAX_LEN, MIN_LEN, Violation};

#[cfg(test)]
mod tests;

/// The cubic that `f` and `g` were split from (`f` its part up to the split, `g` the rest), in
/// the plane: the inverse of de Casteljau's split, checked by splitting the result again (within
/// 5 cm). The heights of the control points run evenly between the ends, like a merged straight
/// street. `None` when the two are not one curve.
pub fn unsplit(f: &CubicBezier3, g: &CubicBezier3) -> Option<CubicBezier3> {
    let (fx, gx) = (f.xz(), g.xz());
    let (a, b) = (fx[3].distance(fx[2]), gx[1].distance(gx[0]));
    if a + b < 1e-4 || fx[3].distance(gx[0]) > 0.05 {
        return None;
    }
    let tau = a / (a + b);
    let p1 = fx[0] + (fx[1] - fx[0]) / tau;
    let p2 = gx[3] + (gx[2] - gx[3]) / (1.0 - tau);
    let (y0, y3) = (f.p0.y, g.p3.y);
    let v = |p: Vec2, k: f32| Vec3::new(p.x, y0 + (y3 - y0) * k, p.y);
    let c = CubicBezier3 {
        p0: f.p0,
        p1: v(p1, 1.0 / 3.0),
        p2: v(p2, 2.0 / 3.0),
        p3: g.p3,
    };
    let (sf, sg) = c.split(tau);
    let close = |x: [Vec2; 4], y: [Vec2; 4]| x.iter().zip(y).all(|(p, q)| p.distance(q) < 0.05);
    (c.is_finite() && close(sf.xz(), fx) && close(sg.xz(), gx)).then_some(c)
}

/// Whether a segment of road `def` at `elevation` may carry street trees (§24.1): on the ground,
/// not a path (parks have their own props) and not a railway.
pub fn trees_allowed(def: &RoadDef, elevation: Elevation) -> bool {
    elevation == Elevation::Ground
        && !matches!(def.class, RoadClass::Path | RoadClass::Rail)
        && def.lanes.iter().all(|l| l.kind != LaneKind::Rail)
}
