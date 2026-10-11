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
//! **Elevation.** `ElevationMode::Ground` follows the terrain and must not touch water
//! (`WaterWithoutBridge`). `Elevated(h)` puts every node `h` metres above the ground — or
//! above the water surface where there is water — so the road is raised end to end;
//! each piece must stay within the road's `max_slope`. A piece with water under it
//! automatically becomes [`Elevation::Bridge`] (the road's `bridge` variant price), the
//! rest stays [`Elevation::Elevated`]; a road type without the needed variant is an
//! error. Raised pieces are grade-separated: they never snap onto or split ground
//! segments and never create a node where they cross a ground road (same-level
//! crossings of ground roads still do). An elevated path's **end** vertices may snap to any
//! node (ground nodes included, that is how a bridge joins the street network); its
//! interior vertices snap to raised nodes only, and ground paths snap to ground nodes
//! only. Piers are derived data: see [`pier_positions`].
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
use crate::model::{Elevation, MAX_SEGMENTS_PER_NODE, NodeId, Segment, SegmentId};

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
/// Distance between bridge/elevated piers, metres.
pub const PIER_SPACING: f32 = 48.0;
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

impl std::fmt::Display for RoadBuildError {
    /// Short player-facing text (shown next to the ghost).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownRoad(_) => write!(f, "unknown road type"),
            Self::NoElevatedVariant(_) => write!(f, "this road type cannot be raised"),
            Self::RampTooShort { .. } => write!(f, "too short to ramp up and down"),
            Self::RampStartsOverWater { .. } => write!(f, "ramp would start over water"),
            Self::NoBridgeVariant(_) => write!(f, "this road type has no bridge"),
            Self::PathTooShort => write!(f, "path too short"),
            Self::BadCoordinates => write!(f, "invalid coordinates"),
            Self::DegenerateLeg { .. } => write!(f, "both ends are the same point"),
            Self::SegmentTooShort { length, .. } => {
                write!(
                    f,
                    "piece too short ({length:.1} m, min {MIN_SEGMENT_LENGTH_BUILD:.0} m)"
                )
            }
            Self::SharpAngle { degrees, .. } => {
                write!(
                    f,
                    "angle too sharp ({degrees:.0}°, min {MIN_JUNCTION_ANGLE_DEG:.0}°)"
                )
            }
            Self::TooSteep { slope, max, .. } => {
                write!(
                    f,
                    "too steep ({:.0}%, max {:.0}%)",
                    slope * 100.0,
                    max * 100.0
                )
            }
            Self::WaterWithoutBridge { .. } => write!(f, "water: raise the road to bridge it"),
            Self::TooManySegments { .. } => write!(f, "too many roads meet here"),
            Self::TileNotOwned { .. } => write!(f, "tile not owned"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RoadBuildError {
    UnknownRoad(RoadDefId),
    /// `Elevated` requested but the road type has no elevated variant.
    NoElevatedVariant(RoadDefId),
    /// The elevated path is too short to ramp up to its height and down again within the
    /// road's `max_slope`.
    RampTooShort {
        pos: Vec3,
    },
    /// A ramp (or a free path end) would sit over water.
    RampStartsOverWater {
        pos: Vec3,
    },
    /// Part of an `Elevated` path is over water but the road type has no bridge variant.
    NoBridgeVariant(RoadDefId),
    /// Fewer than two distinct points.
    PathTooShort,
    /// A coordinate or the height is NaN, infinite or absurdly large.
    BadCoordinates,
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
            // Over water the deck is measured from the water surface.
            ElevationMode::Elevated(h) => y + self.terrain.water_depth(p).max(0.0) + h,
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

    /// `any_level`: an elevated path's end vertex may snap to ground nodes too.
    fn nearest_node(&self, p: Vec2, any_level: bool) -> Option<NodeId> {
        let mut best: Option<(f32, NodeId)> = None;
        let raised = matches!(self.elev, ElevationMode::Elevated(_));
        for (id, n) in self.graph.nodes() {
            if !(raised && any_level) && (n.elevation != Elevation::Ground) != raised {
                continue;
            }
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

/// Whether any sample along a planar-projected piece is over water.
fn piece_over_water(terrain: &Terrain, curve: &CubicBezier3, length: f32) -> bool {
    let steps = ((length / SAMPLE_STEP).ceil() as usize).max(1);
    (0..=steps).any(|k| terrain.is_water(to_xz(curve.point(k as f32 / steps as f32))))
}

/// Pier positions of an elevated or bridge segment: points on the deck every
/// [`PIER_SPACING`] metres of arc length from the start, excluding the ends (nodes).
/// Empty for ground segments. A pure function of the segment, so nothing is stored; the
/// renderer drops each pier to the terrain.
pub fn pier_positions(segment: &Segment) -> Vec<Vec3> {
    if segment.elevation == Elevation::Ground {
        return Vec::new();
    }
    let table = segment.curve.arc_table();
    let mut out = Vec::new();
    let mut s = PIER_SPACING;
    while s < segment.length - 1.0 {
        out.push(segment.curve.point(table.s_to_t(s)));
        s += PIER_SPACING;
    }
    out
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
    let points_ok = match &req.path {
        RoadPath::Straight { from, to } => cp_core::sane_point(*from) && cp_core::sane_point(*to),
        RoadPath::Curve { from, control, to } => {
            [from, control, to].iter().all(|p| cp_core::sane_point(**p))
        }
        RoadPath::Free(pts) => pts.iter().all(|p| cp_core::sane_point(*p)),
    };
    let height_ok = match req.elevation {
        ElevationMode::Ground => true,
        ElevationMode::Elevated(h) => cp_core::sane_coord(h),
    };
    if !points_ok || !height_ok {
        return Err(vec![RoadBuildError::BadCoordinates]);
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
            && let Some(n) = b.nearest_node(p, i == 0 || i + 1 == distinct.len())
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
        // Fixed split points first: crossings with existing ground segments (each becomes a
        // node). Then every span between fixed points (leg ends included) is cut into equal
        // pieces of at most MAX_SEGMENT_LENGTH, so an automatic cut can never land a few metres
        // from a crossing; a span that really is shorter than MIN_SEGMENT_LENGTH_BUILD is
        // reported by the per-piece check below.
        let mut fixed: Vec<(f32, NodeRef)> = Vec::new();
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
                    if !fixed.iter().any(|(_, nd)| *nd == r) {
                        fixed.push((tc, r));
                    }
                }
            }
        }
        fixed.sort_by(|a, c| a.0.total_cmp(&c.0));
        let mut bounds: Vec<(f32, Option<NodeRef>)> = vec![(0.0, Some(v0.node))];
        let mut span_start = 0.0f32; // arc length of the previous fixed point
        let ends = fixed
            .iter()
            .map(|(t, nd)| (tab.t_to_s(*t), Some(*t), *nd))
            .chain(std::iter::once((len, None, v1.node)));
        for (s_end, t_end, node) in ends {
            let span = s_end - span_start;
            let pieces = (span / MAX_SEGMENT_LENGTH).ceil().max(1.0) as usize;
            for k in 1..pieces {
                let s = span_start + span * k as f32 / pieces as f32;
                bounds.push((tab.s_to_t(s), None));
            }
            bounds.push((t_end.unwrap_or(1.0), Some(node)));
            span_start = s_end;
        }
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
            let mut elevation = seg_elevation;
            let mut price = cost_per_m;
            if elevated && piece_over_water(terrain, &curve, length) {
                elevation = Elevation::Bridge;
                match &def.bridge {
                    Some(v) => price = v.cost_per_m,
                    None => {
                        if !errors.contains(&RoadBuildError::NoBridgeVariant(req.road)) {
                            errors.push(RoadBuildError::NoBridgeVariant(req.road));
                        }
                    }
                }
            }
            segments.push(PlannedSegment {
                start: na,
                end: nb,
                curve,
                elevation,
                length,
                cost: segment_cost(length, price),
            });
        }
    }

    // --- ramps: an elevated path starts and ends at ground level ---
    if elevated {
        let slope = def.max_slope * 0.98;
        let price = |e: Elevation| match e {
            Elevation::Bridge => def.bridge.as_ref().map_or(cost_per_m, |v| v.cost_per_m),
            _ => cost_per_m,
        };
        let chain_ok = segments.windows(2).all(|w| w[0].end == w[1].start);
        if chain_ok && !segments.is_empty() {
            let mut nodes = vec![segments[0].start];
            let mut at_s = vec![0.0f32];
            for sg in &segments {
                nodes.push(sg.end);
                at_s.push(at_s.last().copied().unwrap_or(0.0) + sg.length);
            }
            let last = nodes.len() - 1;
            let target: Vec<f32> = nodes.iter().map(|&n| b.node_pos(n).y).collect();
            let fixed: Vec<Option<f32>> = nodes
                .iter()
                .enumerate()
                .map(|(k, &n)| match n {
                    NodeRef::Existing(id) => Some(graph.node(id).expect("exists").pos.y),
                    NodeRef::New(i) if k == 0 || k == last => {
                        Some(terrain.height(to_xz(b.new_nodes[i])))
                    }
                    _ => None,
                })
                .collect();
            let mut ys = vec![0.0f32; nodes.len()];
            let mut reached = false;
            for k in 0..nodes.len() {
                if let Some(y) = fixed[k] {
                    ys[k] = y;
                    continue;
                }
                let mut y = target[k];
                for (f, fy) in fixed.iter().enumerate() {
                    if let Some(fy) = fy {
                        y = y.min(fy + slope * (at_s[k] - at_s[f]).abs());
                    }
                }
                ys[k] = y;
                reached |= y >= target[k] - 1e-3;
            }
            if !reached {
                errors.push(RoadBuildError::RampTooShort {
                    pos: b.node_pos(nodes[0]),
                });
            }
            for k in 0..nodes.len() {
                if let NodeRef::New(i) = nodes[k] {
                    let p = to_xz(b.new_nodes[i]);
                    let on_ramp = fixed[k].is_some() || ys[k] < target[k] - 1e-3;
                    if on_ramp && terrain.water_depth(p) > 0.0 {
                        errors.push(RoadBuildError::RampStartsOverWater {
                            pos: Vec3::new(p.x, ys[k], p.y),
                        });
                    }
                    b.new_nodes[i].y = ys[k];
                }
            }
            for sg in &mut segments {
                let (pa, pb) = (b.node_pos(sg.start), b.node_pos(sg.end));
                sg.curve = lift(sg.curve, pa, pb);
                sg.length = sg.curve.arc_table().length();
                sg.cost = segment_cost(sg.length, price(sg.elevation));
            }
        }
    }

    // --- per-piece checks ---
    for s in &segments {
        let raised = s.elevation != Elevation::Ground;
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
            if water.is_none() && !raised && terrain.is_water(p) {
                water = Some(p3);
            }
            if foreign.is_none() && !tiles.contains_point(p) {
                foreign = Some(p3);
            }
            if !raised {
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
        if raised {
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
