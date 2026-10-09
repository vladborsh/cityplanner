//! Road build validation and snapping (`system-design.md` §6.3) — a **pure** function.
//!
//! [`validate_road_path`] takes only shared references: it never mutates the graph. It
//! snaps the raw cursor path, cuts it into segments, checks every §6.3 rule and returns
//! either a [`RoadBuildPlan`] (what `BuildRoad` will apply, and what the tool preview
//! draws) or **all** the errors found.
//!
//! **Snap priority** per path point: existing node within [`NODE_SNAP_RADIUS`] → point on an
//! existing ground segment within [`SEGMENT_SNAP_RADIUS`] (the segment is split there) →
//! direction snapped to a multiple of [`ANGLE_STEP_DEG`] relative to the neighbouring
//! segments (or the previous leg, or the world X axis), when within [`ANGLE_SNAP_TOL_DEG`]
//! → zone grid (TODO(T-026): needs `cp_zone`; the option exists and does nothing yet).
//!
//! **Conventions.** Paths are in the XZ plane (`Vec2` = x, z); heights come from the
//! terrain. Candidates are scanned in arena order and ties broken by id, so the result is
//! independent of anything but the inputs.

use bevy_math::{Vec2, Vec3};
use cp_content::{ContentDb, RoadDefId};
use cp_core::Cost;
use cp_core::curve::{CubicBezier3, to_xz};
use cp_terrain::{Terrain, TileOwnership};
use serde::{Deserialize, Serialize};

use crate::graph::NetGraph;
use crate::model::{Elevation, MAX_SEGMENTS_PER_NODE, NodeId, SegmentId};

/// Longest allowed segment, metres. Longer legs are cut into equal pieces.
pub const MAX_SEGMENT_LENGTH: f32 = 96.0;
/// Shortest allowed segment, metres.
pub const MIN_SEGMENT_LENGTH_BUILD: f32 = 8.0;
/// Smallest angle between two segments leaving a node, degrees.
pub const MIN_JUNCTION_ANGLE_DEG: f32 = 30.0;
pub const NODE_SNAP_RADIUS: f32 = 12.0;
pub const SEGMENT_SNAP_RADIUS: f32 = 6.0;
pub const ANGLE_STEP_DEG: f32 = 15.0;
pub const ANGLE_SNAP_TOL_DEG: f32 = 4.0;
/// Spacing of terrain/ownership samples along a piece, metres.
const SAMPLE_STEP: f32 = 4.0;
/// Crossings closer than this (metres) to an auto-cut point replace it.
const CUT_MERGE: f32 = 0.5;

/// Height mode of a build request (the command-facing type of §4; segments store the
/// data type [`Elevation`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ElevationMode {
    Ground,
    /// Raised by `h` metres above the ground at each node.
    Elevated(f32),
}

/// The drawn path, in world XZ.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RoadPath {
    Straight {
        from: Vec2,
        to: Vec2,
    },
    /// Three clicks: a quadratic curve through control point `control`.
    Curve {
        from: Vec2,
        control: Vec2,
        to: Vec2,
    },
    /// Free mode: a polyline of straight legs.
    Free(Vec<Vec2>),
}

/// Which snaps are active (the tool turns them on and off).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapOptions {
    pub node: bool,
    pub segment: bool,
    pub angle: bool,
    /// Not implemented yet — TODO(T-026), needs the zone grid from `cp_zone`.
    pub zone_grid: bool,
}

impl Default for SnapOptions {
    fn default() -> Self {
        Self {
            node: true,
            segment: true,
            angle: true,
            zone_grid: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoadPathRequest {
    pub road: RoadDefId,
    pub path: RoadPath,
    pub elevation: ElevationMode,
    pub snap: SnapOptions,
}

/// Where a plan node comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeRef {
    Existing(NodeId),
    /// Index into [`RoadBuildPlan::new_nodes`].
    New(usize),
    /// Index into [`RoadBuildPlan::splits`]: a node created by splitting an existing segment.
    Split(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedSegment {
    pub start: NodeRef,
    pub end: NodeRef,
    pub curve: CubicBezier3,
    pub elevation: Elevation,
    pub length: f32,
    pub cost: Cost,
}

/// An existing segment to split at curve parameter `t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlannedSplit {
    pub segment: SegmentId,
    pub t: f32,
    pub pos: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    Node(NodeId),
    Segment(SegmentId),
    /// Direction snapped to this many degrees from the reference direction.
    Angle(i32),
    /// TODO(T-026): never produced yet.
    ZoneGrid,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppliedSnap {
    /// Index of the path vertex that was moved.
    pub vertex: usize,
    pub kind: SnapKind,
    pub pos: Vec2,
}

/// Placeholder for a building the road would run through. TODO(T-028): `cp_building`
/// provides footprints; this becomes its `BuildingId`. Always empty until then.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DemolishRef(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub struct RoadBuildPlan {
    pub road: RoadDefId,
    pub segments: Vec<PlannedSegment>,
    pub new_nodes: Vec<Vec3>,
    pub splits: Vec<PlannedSplit>,
    pub snaps: Vec<AppliedSnap>,
    /// Buildings to demolish (with player confirmation). TODO(T-028): always empty now.
    pub demolish: Vec<DemolishRef>,
    pub cost: Cost,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RoadBuildError {
    UnknownRoad(RoadDefId),
    /// `Elevated` requested but the road type has no elevated variant.
    NoElevatedVariant(RoadDefId),
    /// Fewer than two distinct points.
    PathTooShort,
    /// A leg's two ends resolve to the same node.
    DegenerateLeg {
        pos: Vec2,
    },
    SegmentTooShort {
        pos: Vec3,
        length: f32,
    },
    SharpAngle {
        pos: Vec3,
        degrees: f32,
    },
    TooSteep {
        pos: Vec3,
        slope: f32,
        max: f32,
    },
    WaterWithoutBridge {
        pos: Vec3,
    },
    TooManySegments {
        pos: Vec3,
    },
    TileNotOwned {
        pos: Vec3,
    },
}

/// Money for one segment: `(metres * cost_per_m * 100).round()` cents. One helper so the
/// conversion is easy to change when the Architect rules on money.
pub fn segment_cost(length_m: f32, cost_per_m: f32) -> Cost {
    Cost((f64::from(length_m) * f64::from(cost_per_m) * 100.0).round() as i64)
}

fn dir_xz(v: Vec3) -> Vec2 {
    to_xz(v).normalize_or_zero()
}

/// A path vertex after snapping.
struct Vertex {
    pos: Vec2,
    node: NodeRef,
    /// Reference directions for the angle snap of the next leg.
    refs: Vec<Vec2>,
}

/// Mutable plan state while validating (local only; inputs stay immutable).
struct Builder<'a> {
    graph: &'a NetGraph,
    terrain: &'a Terrain,
    elev: ElevationMode,
    new_nodes: Vec<Vec3>,
    splits: Vec<PlannedSplit>,
}

impl Builder<'_> {
    fn ground_y(&self, p: Vec2) -> f32 {
        let y = self.terrain.height(p);
        match self.elev {
            ElevationMode::Ground => y,
            ElevationMode::Elevated(h) => y + h,
        }
    }

    fn node_pos(&self, n: NodeRef) -> Vec3 {
        match n {
            NodeRef::Existing(id) => self.graph.node(id).expect("snapped node exists").pos,
            NodeRef::New(i) => self.new_nodes[i],
            NodeRef::Split(i) => self.splits[i].pos,
        }
    }

    fn new_node(&mut self, p: Vec2) -> NodeRef {
        let pos = Vec3::new(p.x, self.ground_y(p), p.y);
        self.new_nodes.push(pos);
        NodeRef::New(self.new_nodes.len() - 1)
    }

    /// Splits of the same segment closer than `CUT_MERGE` share one split node.
    fn split_at(&mut self, segment: SegmentId, t: f32) -> NodeRef {
        let curve = self.graph.segment(segment).expect("segment exists").curve;
        let pos = curve.point(t);
        if let Some(i) = self
            .splits
            .iter()
            .position(|s| s.segment == segment && s.pos.distance(pos) < CUT_MERGE)
        {
            return NodeRef::Split(i);
        }
        self.splits.push(PlannedSplit { segment, t, pos });
        NodeRef::Split(self.splits.len() - 1)
    }

    /// Directions leaving the node, for the angle reference.
    fn node_dirs(&self, n: NodeRef) -> Vec<Vec2> {
        match n {
            NodeRef::Existing(id) => {
                let mut v = Vec::new();
                for &sid in &self.graph.node(id).expect("exists").segments {
                    let s = self.graph.segment(sid).expect("exists");
                    v.push(if s.start == id {
                        dir_xz(s.curve.tangent(0.0))
                    } else {
                        -dir_xz(s.curve.tangent(1.0))
                    });
                }
                v
            }
            NodeRef::Split(i) => {
                let s = self.splits[i];
                let t = dir_xz(
                    self.graph
                        .segment(s.segment)
                        .expect("exists")
                        .curve
                        .tangent(s.t),
                );
                vec![t, -t]
            }
            NodeRef::New(_) => Vec::new(),
        }
    }

    fn nearest_node(&self, p: Vec2) -> Option<NodeId> {
        let mut best: Option<(f32, NodeId)> = None;
        for (id, n) in self.graph.nodes() {
            let d = to_xz(n.pos).distance(p);
            if d <= NODE_SNAP_RADIUS && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, id));
            }
        }
        best.map(|(_, id)| id)
    }

    fn nearest_segment(&self, p: Vec2) -> Option<(SegmentId, f32)> {
        let mut best: Option<(f32, SegmentId, f32)> = None;
        for (id, s) in self.graph.segments() {
            if s.elevation != Elevation::Ground {
                continue;
            }
            let (t, d) = s.curve.nearest_xz(p);
            if d <= SEGMENT_SNAP_RADIUS && best.is_none_or(|(bd, ..)| d < bd) {
                best = Some((d, id, t));
            }
        }
        best.map(|(_, id, t)| (id, t))
    }
}

fn rotate(v: Vec2, deg: f32) -> Vec2 {
    let r = deg.to_radians();
    let (s, c) = r.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Snaps the direction of `d` to a multiple of [`ANGLE_STEP_DEG`] from the best reference.
/// Returns the new direction (same length) and the step angle in degrees.
fn snap_angle(d: Vec2, refs: &[Vec2]) -> Option<(Vec2, i32)> {
    let len = d.length();
    if len < 1e-3 {
        return None;
    }
    let mut best: Option<(f32, Vec2, i32)> = None;
    for &r in refs {
        let a = r.y.atan2(r.x);
        let ang = (d.y.atan2(d.x) - a).to_degrees();
        let ang = (ang + 180.0).rem_euclid(360.0) - 180.0;
        let step = (ang / ANGLE_STEP_DEG).round();
        let dev = (ang - step * ANGLE_STEP_DEG).abs();
        if dev <= ANGLE_SNAP_TOL_DEG && best.is_none_or(|(bd, ..)| dev < bd) {
            best = Some((
                dev,
                rotate(r, step * ANGLE_STEP_DEG) * len,
                (step * ANGLE_STEP_DEG) as i32,
            ));
        }
    }
    best.map(|(_, v, s)| (v, s))
}

/// Control points of a leg as a planar (Y = 0) cubic.
fn flat_curve(a: Vec2, b: Vec2, control: Option<Vec2>) -> CubicBezier3 {
    let v3 = |p: Vec2| Vec3::new(p.x, 0.0, p.y);
    match control {
        None => CubicBezier3::line(v3(a), v3(b)),
        Some(c) => CubicBezier3::new(
            v3(a),
            v3(a + (c - a) * (2.0 / 3.0)),
            v3(b + (c - b) * (2.0 / 3.0)),
            v3(b),
        ),
    }
}

/// Gives a planar piece heights: ends from the nodes, controls interpolated.
fn lift(piece: CubicBezier3, a: Vec3, b: Vec3) -> CubicBezier3 {
    let at = |p: Vec3, y: f32| Vec3::new(p.x, y, p.z);
    CubicBezier3::new(
        a,
        at(piece.p1, a.y + (b.y - a.y) / 3.0),
        at(piece.p2, a.y + (b.y - a.y) * 2.0 / 3.0),
        b,
    )
}

/// Validates `req` against the world and returns the build plan, or every error found.
///
/// Pure: reads `graph`, `terrain`, `tiles` and `content`, mutates nothing.
pub fn validate_road_path(
    graph: &NetGraph,
    terrain: &Terrain,
    tiles: &TileOwnership,
    content: &ContentDb,
    req: &RoadPathRequest,
) -> Result<RoadBuildPlan, Vec<RoadBuildError>> {
    if req.road.index() >= content.roads().count() {
        return Err(vec![RoadBuildError::UnknownRoad(req.road)]);
    }
    let def = content.road(req.road);
    let mut errors = Vec::new();
    let elevated = matches!(req.elevation, ElevationMode::Elevated(_));
    let variant = if elevated {
        match &def.elevated {
            Some(v) => Some(v),
            None => return Err(vec![RoadBuildError::NoElevatedVariant(req.road)]),
        }
    } else {
        None
    };
    let cost_per_m = variant.map_or(def.cost_per_m, |v| v.cost_per_m);
    let seg_elevation = if elevated {
        Elevation::Elevated
    } else {
        Elevation::Ground
    };

    // Raw vertices and the control point of a curve leg.
    let (raw, control): (Vec<Vec2>, Option<Vec2>) = match &req.path {
        RoadPath::Straight { from, to } => (vec![*from, *to], None),
        RoadPath::Curve { from, control, to } => (vec![*from, *to], Some(*control)),
        RoadPath::Free(pts) => (pts.clone(), None),
    };
    let mut distinct = raw.clone();
    distinct.dedup_by(|a, b| a.distance(*b) < 1e-3);
    if distinct.len() < 2 {
        return Err(vec![RoadBuildError::PathTooShort]);
    }

    let mut b = Builder {
        graph,
        terrain,
        elev: req.elevation,
        new_nodes: Vec::new(),
        splits: Vec::new(),
    };
    let angle_ok = req.snap.angle && control.is_none();

    // --- snap the vertices ---
    let mut snaps = Vec::new();
    let mut verts: Vec<Vertex> = Vec::new();
    for (i, &p) in distinct.iter().enumerate() {
        let mut pos = p;
        let mut node = None;
        if req.snap.node
            && let Some(n) = b.nearest_node(p)
        {
            pos = to_xz(graph.node(n).expect("exists").pos);
            node = Some(NodeRef::Existing(n));
            snaps.push(AppliedSnap {
                vertex: i,
                kind: SnapKind::Node(n),
                pos,
            });
        }
        if node.is_none()
            && req.snap.segment
            && !elevated
            && let Some((s, t)) = b.nearest_segment(p)
        {
            let r = b.split_at(s, t);
            pos = to_xz(b.node_pos(r));
            node = Some(r);
            snaps.push(AppliedSnap {
                vertex: i,
                kind: SnapKind::Segment(s),
                pos,
            });
        }
        if node.is_none() && angle_ok && i > 0 {
            let prev = &verts[i - 1];
            if let Some((v, step)) = snap_angle(p - prev.pos, &prev.refs) {
                pos = prev.pos + v;
                snaps.push(AppliedSnap {
                    vertex: i,
                    kind: SnapKind::Angle(step),
                    pos,
                });
            }
        }
        // TODO(T-026): `req.snap.zone_grid` — snap to the zone grid once `cp_zone` exists.
        let node = node.unwrap_or_else(|| b.new_node(pos));
        let mut refs = b.node_dirs(node);
        if i > 0 {
            // The leg just drawn is a neighbour too: continue in its direction.
            let back = verts[i - 1].pos - pos;
            if back.length() > 1e-3 {
                refs.push(-back.normalize());
            }
        }
        if refs.is_empty() {
            refs.push(Vec2::X);
        }
        verts.push(Vertex { pos, node, refs });
    }

    // --- cut legs into pieces ---
    let mut segments: Vec<PlannedSegment> = Vec::new();
    for (li, w) in verts.windows(2).enumerate() {
        let (v0, v1) = (&w[0], &w[1]);
        if v0.node == v1.node {
            errors.push(RoadBuildError::DegenerateLeg { pos: v0.pos });
            continue;
        }
        let leg = flat_curve(v0.pos, v1.pos, control.filter(|_| li == 0));
        let tab = leg.arc_table();
        let len = tab.length();
        if len < MIN_SEGMENT_LENGTH_BUILD {
            errors.push(RoadBuildError::SegmentTooShort {
                pos: b.node_pos(v0.node),
                length: len,
            });
            continue;
        }
        // Cut points: (t, node). Auto-cuts first, then crossings replace close ones.
        let n = (len / MAX_SEGMENT_LENGTH).ceil().max(1.0) as usize;
        let mut cuts: Vec<(f32, Option<NodeRef>)> = (1..n)
            .map(|k| (tab.s_to_t(len * k as f32 / n as f32), None))
            .collect();
        if !elevated {
            for (sid, seg) in graph.segments() {
                if seg.elevation != Elevation::Ground {
                    continue;
                }
                for (tc, ts) in leg.intersect_xz(&seg.curve) {
                    if !(0.002..=0.998).contains(&tc) || !(0.002..=0.998).contains(&ts) {
                        continue;
                    }
                    let r = b.split_at(sid, ts);
                    let sc = tab.t_to_s(tc);
                    cuts.retain(|(t, node)| {
                        node.is_some() || (tab.t_to_s(*t) - sc).abs() > CUT_MERGE
                    });
                    if !cuts.iter().any(|(_, nd)| *nd == Some(r)) {
                        cuts.push((tc, Some(r)));
                    }
                }
            }
        }
        cuts.sort_by(|a, c| a.0.total_cmp(&c.0));
        let mut bounds = vec![(0.0, Some(v0.node))];
        bounds.extend(cuts);
        bounds.push((1.0, Some(v1.node)));
        let mut nodes: Vec<NodeRef> = Vec::with_capacity(bounds.len());
        for (t, nd) in &bounds {
            nodes.push(match nd {
                Some(nd) => *nd,
                None => b.new_node(to_xz(leg.point(*t))),
            });
        }
        for (k, pair) in bounds.windows(2).enumerate() {
            let (na, nb) = (nodes[k], nodes[k + 1]);
            let (pa, pb) = (b.node_pos(na), b.node_pos(nb));
            let planar = leg.sub_curve(pair[0].0, pair[1].0);
            let curve = lift(planar, pa, pb);
            let length = curve.arc_table().length();
            segments.push(PlannedSegment {
                start: na,
                end: nb,
                curve,
                elevation: seg_elevation,
                length,
                cost: segment_cost(length, cost_per_m),
            });
        }
    }

    // --- per-piece checks ---
    for s in &segments {
        let at = b.node_pos(s.start);
        if s.length < MIN_SEGMENT_LENGTH_BUILD {
            errors.push(RoadBuildError::SegmentTooShort {
                pos: at,
                length: s.length,
            });
        }
        let steps = ((s.length / SAMPLE_STEP).ceil() as usize).max(1);
        let mut prev: Option<(Vec2, f32)> = None;
        let (mut water, mut foreign) = (None, None);
        let mut worst: Option<(f32, Vec3)> = None;
        for k in 0..=steps {
            let p3 = s.curve.point(k as f32 / steps as f32);
            let p = to_xz(p3);
            if water.is_none() && !elevated && terrain.is_water(p) {
                water = Some(p3);
            }
            if foreign.is_none() && !tiles.contains_point(p) {
                foreign = Some(p3);
            }
            if !elevated {
                let h = terrain.height(p);
                if let Some((pp, ph)) = prev {
                    let run = pp.distance(p).max(1e-3);
                    let slope = (h - ph).abs() / run;
                    if worst.is_none_or(|(w, _)| slope > w) {
                        worst = Some((slope, p3));
                    }
                }
                prev = Some((p, h));
            }
        }
        if elevated {
            let run = to_xz(s.curve.p3).distance(to_xz(s.curve.p0)).max(1e-3);
            worst = Some(((s.curve.p3.y - s.curve.p0.y).abs() / run, at));
        }
        if let Some(pos) = water {
            errors.push(RoadBuildError::WaterWithoutBridge { pos });
        }
        if let Some(pos) = foreign {
            errors.push(RoadBuildError::TileNotOwned { pos });
        }
        if let Some((slope, pos)) = worst
            && slope > def.max_slope
        {
            errors.push(RoadBuildError::TooSteep {
                pos,
                slope,
                max: def.max_slope,
            });
        }
    }

    // --- junction checks: segment count and angles ---
    let mut seen: Vec<NodeRef> = Vec::new();
    for s in &segments {
        for n in [s.start, s.end] {
            if !seen.contains(&n) {
                seen.push(n);
            }
        }
    }
    for n in seen {
        let mut planned: Vec<Vec2> = Vec::new();
        for s in &segments {
            if s.start == n {
                planned.push(dir_xz(s.curve.tangent(0.0)));
            }
            if s.end == n {
                planned.push(-dir_xz(s.curve.tangent(1.0)));
            }
        }
        let existing = b.node_dirs(n);
        let pos = b.node_pos(n);
        if existing.len() + planned.len() > MAX_SEGMENTS_PER_NODE {
            errors.push(RoadBuildError::TooManySegments { pos });
        }
        let mut min_angle: Option<f32> = None;
        for (i, &a) in planned.iter().enumerate() {
            for &o in planned[i + 1..].iter().chain(existing.iter()) {
                let ang = a.dot(o).clamp(-1.0, 1.0).acos().to_degrees();
                min_angle = Some(min_angle.map_or(ang, |m: f32| m.min(ang)));
            }
        }
        if let Some(deg) = min_angle
            && deg < MIN_JUNCTION_ANGLE_DEG - 1e-3
        {
            errors.push(RoadBuildError::SharpAngle { pos, degrees: deg });
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    let cost = Cost(segments.iter().map(|s| s.cost.0).sum());
    Ok(RoadBuildPlan {
        road: req.road,
        segments,
        new_nodes: b.new_nodes,
        splits: b.splits,
        snaps,
        demolish: Vec::new(),
        cost,
    })
}

#[cfg(test)]
mod tests;
