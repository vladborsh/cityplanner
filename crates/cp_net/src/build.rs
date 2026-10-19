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
use cp_content::{ContentDb, LaneDir, LaneKind, RoadClass, RoadDefId};
use cp_core::Cost;
use cp_core::curve::{CubicBezier3, to_xz};
use cp_terrain::{Terrain, TileOwnership};
use serde::{Deserialize, Serialize};

use crate::graph::NetGraph;
use crate::model::{Elevation, MAX_SEGMENTS_PER_NODE, NodeId, NodeKind, Segment, SegmentId};
use crate::plan::Anchor;

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
/// Tolerance of the key angles (45°, 90°, 135° from a neighbour) and of the guides; wider than
/// the 15° step tolerance so that a right angle is caught early.
pub const KEY_ANGLE_TOL_DEG: f32 = 8.0;
/// Ground segments within this distance of the cursor give parallel / perpendicular guides.
pub const GUIDE_RADIUS: f32 = 60.0;
/// Half length of the guide line reported to the tool, metres.
pub const GUIDE_HALF_LENGTH: f32 = 120.0;
/// The key angles, degrees from a reference direction.
const KEY_ANGLES_DEG: [f32; 6] = [45.0, -45.0, 90.0, -90.0, 135.0, -135.0];
/// Spacing of terrain/ownership samples along a piece, metres.
const SAMPLE_STEP: f32 = 4.0;
/// Distance between bridge/elevated piers, metres.
pub const PIER_SPACING: f32 = 48.0;
/// One zoning cell, metres: the build step and the tick spacing of a stretch (§6.7).
pub const ZONE_CELL: f32 = 8.0;
/// Distance between the control points of a stretch (8 cells), metres (§6.7).
pub const CONTROL_SPACING: f32 = 64.0;
/// Tunnels whose axes differ by at most this many metres at a crossing are on the same depth:
/// they meet in a node (T-161).
pub const SAME_DEPTH_TOL: f32 = 1.5;
/// Smallest vertical gap (axis to axis, metres) between two tunnels that cross without a node.
/// A crossing with a gap between [`SAME_DEPTH_TOL`] and this is refused.
pub const TUNNEL_CLEARANCE: f32 = 5.0;
/// Deepest tunnel axis below the terrain, metres.
pub const MAX_TUNNEL_DEPTH: f32 = 60.0;
/// Smallest vertical gap (deck to deck, metres) between two raised roads that cross without a
/// node (T-064); a crossing at the same level (within [`SAME_DEPTH_TOL`]) is a node, one with
/// a gap between the two is refused.
pub const ELEVATED_CLEARANCE: f32 = 5.0;
/// Spacing of the water samples under a raised piece, metres (T-064; was 4).
const WATER_SAMPLE_STEP: f32 = 2.0;
/// The share of a road's `max_slope` its ramps use (a hair under, so rounding never makes a ramp
/// steeper than the road may be), and the slack of the piece check.
const RAMP_SLOPE_SHARE: f32 = 0.9995;
const SLOPE_EPS: f32 = 1e-4;
/// The reach of a node's junction, metres (§6.8.3 items 1 and 2, T-264): a point of a plan this
/// close (along a segment) to the node is the node. A dead end and an invisible cut of a straight
/// street (degree 2, same road and level, a corner of at most [`crate::CORNER_DEG`]) have no
/// junction: [`MIN_SEGMENT_LENGTH_BUILD`], the smallest piece. A junction (degree 3 and more), a
/// bend or a change of road at degree 2 takes the room of its corners: the half width of the
/// widest road there, until the junction geometry of T-263 gives the real setback.
pub fn junction_reach(graph: &NetGraph, content: &ContentDb, node: NodeId) -> f32 {
    let Some(n) = graph.node(node) else {
        return MIN_SEGMENT_LENGTH_BUILD;
    };
    if is_plain_cut_or_end(graph, node) {
        return MIN_SEGMENT_LENGTH_BUILD;
    }
    n.segments
        .iter()
        .filter_map(|&s| graph.segment(s))
        .map(|s| content.road(s.road).width / 2.0)
        .fold(MIN_SEGMENT_LENGTH_BUILD, f32::max)
}

/// A node that is no junction at all: a dead end, or an invisible cut of a straight street
/// (degree 2, same road and level, a corner of at most [`crate::CORNER_DEG`]).
fn is_plain_cut_or_end(graph: &NetGraph, node: NodeId) -> bool {
    let Some(n) = graph.node(node) else {
        return true;
    };
    let segs: Vec<&Segment> = n
        .segments
        .iter()
        .filter_map(|&s| graph.segment(s))
        .collect();
    match segs.as_slice() {
        [] | [_] => true,
        [a, b] => {
            let away = |s: &Segment| {
                if s.start == node {
                    dir_xz(s.curve.tangent(0.0))
                } else {
                    -dir_xz(s.curve.tangent(1.0))
                }
            };
            a.road == b.road
                && a.elevation == b.elevation
                && away(a).dot(away(b)) <= -crate::CORNER_DEG.to_radians().cos()
        }
        _ => false,
    }
}

/// An invisible cut of a straight street: degree 2, same road and level, no bend. The tool does
/// not snap to one (it is not there for the player); a plan that meets one dissolves it.
pub fn is_plain_cut(graph: &NetGraph, node: NodeId) -> bool {
    graph.node(node).is_some_and(|n| n.segments.len() == 2) && is_plain_cut_or_end(graph, node)
}

/// Whether `p` lies inside the junction of `node` (§6.8.3 items 1 and 5, T-264): within
/// [`MIN_SEGMENT_LENGTH_BUILD`] of it, or on the asphalt of one of its arms, no farther along that
/// arm than [`junction_reach`]. At an acute junction a point 14 m out on one arm lies on the other
/// arm's asphalt, which a radius does not see.
pub fn in_junction(graph: &NetGraph, content: &ContentDb, node: NodeId, p: Vec2) -> bool {
    let Some(n) = graph.node(node) else {
        return false;
    };
    let c = to_xz(n.pos);
    if c.distance(p) <= MIN_SEGMENT_LENGTH_BUILD {
        return true;
    }
    let reach = junction_reach(graph, content, node);
    n.segments
        .iter()
        .filter_map(|&s| graph.segment(s))
        .any(|s| {
            let away = if s.start == node {
                dir_xz(s.curve.tangent(0.0))
            } else {
                -dir_xz(s.curve.tangent(1.0))
            };
            let rel = p - c;
            let (along, off) = (rel.dot(away), rel.perp_dot(away).abs());
            away != Vec2::ZERO
                && (0.0..=reach).contains(&along)
                && off < content.road(s.road).width / 2.0
        })
}

/// A remnant this much under 8 m still counts as 8 m (the arc length of a cut is a float: a
/// point aimed at 8.000 m from a node must stay a cut).
const REMNANT_EPS: f32 = 0.02;
/// Crossings closer than this (metres) to an auto-cut point replace it.
const CUT_MERGE: f32 = 0.5;

/// Where an automatic cut splits a span of `span` metres (from its start, ascending): on the
/// control points `64·k` (§6.7) for as long as more than [`MAX_SEGMENT_LENGTH`] is left, so
/// the tail stays one piece of 32…96 m. A control point then lies on a cut node or at least
/// 8 m from every one, and the bulldozer's stop is never snapped onto a piece's end.
pub fn auto_cuts(span: f32) -> Vec<f32> {
    let mut cuts = Vec::new();
    let mut at = 0.0;
    while span - at > MAX_SEGMENT_LENGTH {
        at += CONTROL_SPACING;
        cuts.push(at);
    }
    cuts
}

/// Height mode of a build request (the command-facing type of §4; segments store the
/// data type [`Elevation`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ElevationMode {
    Ground,
    /// Raised by `h` metres above the ground at each node.
    Elevated(f32),
    /// Underground, the axis `d` metres below the terrain at each node (T-161). Appended last:
    /// older command logs read as before.
    Tunnel(f32),
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
    /// Direction snapped to a key angle (45, 90 or 135) from a neighbouring direction.
    KeyAngle(i32),
    /// Direction snapped parallel (or perpendicular) to a nearby ground segment.
    Guide {
        perpendicular: bool,
    },
    /// TODO(T-026): never produced yet.
    ZoneGrid,
}

/// What snapped the vertex at the cursor (the last path vertex), for the tool's label.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SnapReportKind {
    #[default]
    None,
    Node,
    Segment,
    KeyAngle(i32),
    Guide {
        perpendicular: bool,
    },
    Step(i32),
    /// The length went to the perpendicular through a control point of a neighbouring stretch
    /// that lies this many metres from its anchor (label `⊥ 64`, §6.7).
    ControlLine(i32),
    /// The length was rounded to this many metres (a multiple of 8).
    CellStep(i32),
}

/// The snap at the cursor and, for angle and guide snaps, the guide line through the snapped
/// point (±[`GUIDE_HALF_LENGTH`] m along the chosen direction).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SnapReport {
    pub kind: SnapReportKind,
    pub guide: Option<(Vec2, Vec2)>,
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
    /// The snap at the last vertex (the cursor), with its guide line.
    pub report: SnapReport,
    /// Buildings to demolish (with player confirmation). TODO(T-028): always empty now.
    pub demolish: Vec<DemolishRef>,
    pub cost: Cost,
    /// Made by [`validate_build_plan`] (T-264): applied as it is, no joint merged afterwards.
    pub literal: bool,
    /// Invisible cuts of straight streets that the plan dissolves before anything is split: a
    /// junction or an end within 8 m of such a cut takes its place on the street's lattice, and
    /// the street is cut again where it needs it (T-264, what BUG-069 did after the fact).
    pub dissolve: Vec<NodeId>,
    /// The streets those merges make, in order: what the apply must find again (checked in
    /// debug builds; the ids come from a clone of the graph).
    pub merged: Vec<SegmentId>,
}

impl RoadBuildError {
    /// Where on the map the error is, in the plane, when it has a place (the tool draws a mark
    /// there, §6.8.3 item 9).
    pub fn pos(&self) -> Option<Vec2> {
        match self {
            Self::DegenerateLeg { pos } => Some(*pos),
            Self::RampTooShort { pos }
            | Self::RampStartsOverWater { pos }
            | Self::SegmentTooShort { pos, .. }
            | Self::SharpAngle { pos, .. }
            | Self::TooSteep { pos, .. }
            | Self::WaterWithoutBridge { pos }
            | Self::TooManySegments { pos }
            | Self::TileNotOwned { pos }
            | Self::NodeOnRoad { pos }
            | Self::RoadsOverlap { pos }
            | Self::RailCrossesRoad { pos }
            | Self::ElevatedClearance { pos }
            | Self::TunnelClearance { pos } => Some(Vec2::new(pos.x, pos.z)),
            _ => None,
        }
    }
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
            Self::RoadsOverlap { .. } => write!(f, "this road would lie on another road"),
            Self::NodeOnRoad { .. } => {
                write!(f, "a junction would stand on a road it does not join")
            }
            Self::NoTunnelVariant(_) => write!(f, "this road type cannot go underground"),
            Self::TunnelOnly(_) => write!(f, "this track can only be built in a tunnel"),
            Self::RailCrossesRoad { .. } => write!(f, "track and road cannot cross at one level"),
            Self::ElevatedClearance { .. } => {
                write!(
                    f,
                    "roads crossing here need {ELEVATED_CLEARANCE:.0} m between the decks (or one level)"
                )
            }
            Self::TunnelClearance { .. } => {
                write!(
                    f,
                    "tunnels crossing here need {TUNNEL_CLEARANCE:.0} m between them"
                )
            }
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
    /// A literal plan (T-262, T-264): a node of the plan lies on the asphalt of a ground road it
    /// does not join, or an old node lies under a new road that does not join it: a collision.
    NodeOnRoad {
        pos: Vec3,
    },
    /// A literal plan (T-264): a new piece runs side by side with an old road and their asphalt
    /// overlaps along more than 8 m.
    RoadsOverlap {
        pos: Vec3,
    },
    /// `Tunnel` requested but the road type has no tunnel variant (T-161).
    NoTunnelVariant(RoadDefId),
    /// The road type has no version on the ground and was not asked for in a tunnel.
    TunnelOnly(RoadDefId),
    /// A path at ground level would cross a segment of the other kind (track against road)
    /// without a bridge: they never share a node and never cross in the level.
    RailCrossesRoad {
        pos: Vec3,
    },
    /// Two raised roads cross closer than [`ELEVATED_CLEARANCE`] but not on one level (T-064).
    ElevatedClearance {
        pos: Vec3,
    },
    /// Two tunnels cross closer than [`TUNNEL_CLEARANCE`] but not on one depth.
    TunnelClearance {
        pos: Vec3,
    },
}

/// Money for one segment: `metres × cost_per_m` as cents, rounded once ([`Cost::scaled`], §11).
pub fn segment_cost(length_m: f32, cost_per_m: f32) -> Cost {
    Cost::scaled(cost_per_m, f64::from(length_m))
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
    content: &'a ContentDb,
    /// The road being built is track: it only meets track.
    rail: bool,
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
            // Under water too: a tunnel is measured from the terrain (the river bed).
            ElevationMode::Tunnel(d) => y - d,
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

    /// The vertex a plan anchor stands for (§6.8.3 item 2): a cut that would leave a remnant
    /// under [`MIN_SEGMENT_LENGTH_BUILD`] is the near end node instead.
    fn anchor_node(&mut self, anchor: Anchor, free: Vec2) -> (Vec2, NodeRef) {
        match anchor {
            Anchor::Free(_) => match self.node_under(free) {
                // A free point on an existing node is that node: its roads count for the
                // junction checks (`add_segment` would join them by position anyway).
                Some(id) => (free, NodeRef::Existing(id)),
                None => (free, self.new_node(free)),
            },
            Anchor::Node(id) => (
                to_xz(self.graph.node(id).expect("resolved").pos),
                NodeRef::Existing(id),
            ),
            Anchor::OnSegment(sid, t) => {
                let r = self.cross_node(sid, t.clamp(0.0, 1.0), true);
                (to_xz(self.node_pos(r)), r)
            }
        }
    }

    /// An existing node of the plan's level within [`crate::NODE_REUSE_EPS`] of `p` in the plane.
    fn node_under(&self, p: Vec2) -> Option<NodeId> {
        let want = match self.elev {
            ElevationMode::Ground => |e: Elevation| e == Elevation::Ground,
            ElevationMode::Elevated(_) => {
                |e: Elevation| matches!(e, Elevation::Elevated | Elevation::Bridge)
            }
            ElevationMode::Tunnel(_) => |e: Elevation| e == Elevation::Tunnel,
        };
        self.graph
            .nodes()
            .filter(|(_, n)| want(n.elevation) && to_xz(n.pos).distance(p) < crate::NODE_REUSE_EPS)
            .min_by(|a, b| {
                let da = to_xz(a.1.pos).distance(p);
                da.total_cmp(&to_xz(b.1.pos).distance(p))
            })
            .map(|(id, _)| id)
    }

    /// The node a road crossing `segment` at `t` makes. Legacy: a split at `t`. Literal: when
    /// the cut would leave a piece under [`MIN_SEGMENT_LENGTH_BUILD`], the near end of the
    /// segment (the crossing passes through that node).
    fn cross_node(&mut self, segment: SegmentId, t: f32, literal: bool) -> NodeRef {
        if literal {
            let s = self.graph.segment(segment).expect("segment exists");
            let at = s.curve.arc_table().t_to_s(t);
            let at_point = s.curve.point(t);
            let at_xz = to_xz(at_point);
            let (near0, near1) = (
                at < MIN_SEGMENT_LENGTH_BUILD - REMNANT_EPS
                    || in_junction(self.graph, self.content, s.start, at_xz),
                s.length - at < MIN_SEGMENT_LENGTH_BUILD - REMNANT_EPS
                    || in_junction(self.graph, self.content, s.end, at_xz),
            );
            if near0 || near1 {
                let near = match (near0, near1) {
                    (true, false) => s.start,
                    (false, true) => s.end,
                    _ if at <= s.length - at => s.start,
                    _ => s.end,
                };
                return NodeRef::Existing(near);
            }
        }
        self.split_at(segment, t)
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

    fn is_rail_road(&self, road: RoadDefId) -> bool {
        self.content.road(road).class == RoadClass::Rail
    }

    /// Whether the node belongs to the track network (a segment of class `Rail` ends in it).
    fn is_rail_node(&self, n: &crate::model::Node) -> bool {
        n.segments
            .iter()
            .filter_map(|&s| self.graph.segment(s))
            .any(|s| self.is_rail_road(s.road))
    }

    /// `any_level`: an elevated path's end vertex may snap to ground nodes too. Track meets
    /// track and roads meet roads; a tunnel meets tunnel nodes of its own depth.
    fn nearest_node(&self, p: Vec2, any_level: bool) -> Option<NodeId> {
        let mut best: Option<(f32, NodeId)> = None;
        for (id, n) in self.graph.nodes() {
            let level_ok = match self.elev {
                ElevationMode::Ground => n.elevation == Elevation::Ground,
                ElevationMode::Elevated(_) => match n.elevation {
                    Elevation::Elevated | Elevation::Bridge => true,
                    Elevation::Ground => any_level,
                    Elevation::Tunnel => false,
                },
                ElevationMode::Tunnel(_) => {
                    // A road tunnel starts and ends on the surface (its portals): its end
                    // vertices may join a ground node (T-173).
                    (any_level && !self.rail && n.elevation == Elevation::Ground)
                        || (n.elevation == Elevation::Tunnel
                            && (n.pos.y - self.ground_y(p)).abs() <= SAME_DEPTH_TOL)
                }
            };
            if !level_ok || self.is_rail_node(n) != self.rail {
                continue;
            }
            let d = to_xz(n.pos).distance(p);
            // A point exactly on a street (a tick of the road tool) stays there: a through
            // node of that street (a split of a long road) does not pull it (BUG-073).
            if d > 0.05 && n.segments.len() == 2 && self.on_street_through(p, n) {
                continue;
            }
            if d <= NODE_SNAP_RADIUS && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// `p` lies on the axis of one of the two segments of `n`, which run on through it.
    fn on_street_through(&self, p: Vec2, n: &crate::model::Node) -> bool {
        let dirs: Vec<Vec2> = n
            .segments
            .iter()
            .filter_map(|&sid| self.graph.segment(sid).map(|s| (sid, s)))
            .map(|(_, s)| {
                if to_xz(n.pos).distance(to_xz(s.curve.p0)) < 0.05 {
                    to_xz(s.curve.tangent(0.0))
                } else {
                    -to_xz(s.curve.tangent(1.0))
                }
                .normalize_or_zero()
            })
            .collect();
        if dirs.len() != 2 || dirs[0].dot(dirs[1]) > -0.9996 {
            return false;
        }
        n.segments.iter().any(|&sid| {
            self.graph
                .segment(sid)
                .is_some_and(|s| s.curve.nearest_xz(p).1 < 0.05)
        })
    }

    fn nearest_segment(&self, p: Vec2) -> Option<(SegmentId, f32)> {
        let mut best: Option<(f32, SegmentId, f32)> = None;
        for (id, s) in self.graph.segments() {
            if s.elevation != Elevation::Ground || self.is_rail_road(s.road) != self.rail {
                continue;
            }
            // The box of the control points bounds the curve: far segments are not searched.
            if s.curve.bounds_distance_xz(p) > SEGMENT_SNAP_RADIUS + 1e-3 {
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

/// Result of the angle snaps of one vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AngleSnap {
    pub pos: Vec2,
    pub kind: SnapKind,
    pub guide: Option<(Vec2, Vec2)>,
}

/// Signed angle in degrees from `a` to `b`, in `(-180, 180]`.
fn signed_deg(a: Vec2, b: Vec2) -> f32 {
    let ang = (b.y.atan2(b.x) - a.y.atan2(a.x)).to_degrees();
    (ang + 180.0).rem_euclid(360.0) - 180.0
}

/// The direction snaps of a leg from `prev` towards the cursor `p`, in priority order:
/// key angles (45°/90°/135° from a neighbouring direction) and guides (parallel or perpendicular
/// to a ground segment within [`GUIDE_RADIUS`] of the cursor), both within
/// [`KEY_ANGLE_TOL_DEG`] — the smaller deviation wins, a key angle on a tie — then the
/// [`ANGLE_STEP_DEG`] step. Segments are visited in id order, so the result is deterministic.
/// Key angles keep the leg's length; a guide uses the projection of the cursor on its line.
fn angle_snap(graph: &NetGraph, p: Vec2, prev: Vec2, refs: &[Vec2]) -> Option<AngleSnap> {
    let curves: Vec<&CubicBezier3> = graph
        .segments()
        .filter(|(_, s)| s.elevation == Elevation::Ground)
        .map(|(_, s)| &s.curve)
        .collect();
    angle_snap_guides(curves, p, prev, refs)
}

/// [`angle_snap`] against the guide `curves` given (the ground roads of the graph, or the pipes
/// and roads of the utility tool, T-264), visited in the order given.
pub fn angle_snap_guides<'a>(
    guides: impl IntoIterator<Item = &'a CubicBezier3>,
    p: Vec2,
    prev: Vec2,
    refs: &[Vec2],
) -> Option<AngleSnap> {
    let d = p - prev;
    if d.length() < 1e-3 {
        return None;
    }
    let len = d.length();
    // (deviation, new offset from `prev`, unit direction, kind)
    let mut best: Option<(f32, Vec2, Vec2, SnapKind)> = None;
    for &r in refs {
        let rel = signed_deg(r, d);
        for target in KEY_ANGLES_DEG {
            let dev = (rel - target).abs();
            if dev <= KEY_ANGLE_TOL_DEG && best.is_none_or(|(bd, ..)| dev < bd) {
                let dir = rotate(r, target);
                best = Some((dev, dir * len, dir, SnapKind::KeyAngle(target.abs() as i32)));
            }
        }
    }
    for curve in guides {
        if curve.bounds_distance_xz(p) > GUIDE_RADIUS + 1e-3 {
            continue;
        }
        let (t, dist) = curve.nearest_xz(p);
        if dist > GUIDE_RADIUS {
            continue;
        }
        let tan = dir_xz(curve.tangent(t));
        if tan == Vec2::ZERO {
            continue;
        }
        for (dir, perpendicular) in [(tan, false), (Vec2::new(-tan.y, tan.x), true)] {
            // A guide is a line: take the end of it the cursor points to.
            let dir = if d.dot(dir) >= 0.0 { dir } else { -dir };
            let dev = signed_deg(dir, d).abs();
            if dev <= KEY_ANGLE_TOL_DEG && best.is_none_or(|(bd, ..)| dev < bd) {
                let along = d.dot(dir);
                best = Some((dev, dir * along, dir, SnapKind::Guide { perpendicular }));
            }
        }
    }
    if let Some((_, off, dir, kind)) = best {
        let pos = prev + off;
        return Some(AngleSnap {
            pos,
            kind,
            guide: Some((pos - dir * GUIDE_HALF_LENGTH, pos + dir * GUIDE_HALF_LENGTH)),
        });
    }
    snap_angle(d, refs).map(|(v, step)| AngleSnap {
        pos: prev + v,
        kind: SnapKind::Angle(step),
        guide: None,
    })
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
    let steps = ((length / WATER_SAMPLE_STEP).ceil() as usize).max(1);
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

/// The XZ positions of a plan's anchors, `pieces + 1` of them; the end of a piece must be the
/// start of the next. `BadCoordinates` for an empty plan, a node or segment that is not in the
/// graph, or pieces that do not join.
fn plan_points(
    graph: &NetGraph,
    plan: &crate::plan::BuildPlan,
) -> Result<Vec<Vec2>, Vec<RoadBuildError>> {
    let bad = || vec![RoadBuildError::BadCoordinates];
    let at = |a: &Anchor| -> Result<Vec2, Vec<RoadBuildError>> {
        match a {
            Anchor::Free(p) => Ok(*p),
            Anchor::Node(n) => graph.node(*n).map(|n| to_xz(n.pos)).ok_or_else(bad),
            Anchor::OnSegment(s, t) if t.is_finite() => graph
                .segment(*s)
                .map(|s| to_xz(s.curve.point(t.clamp(0.0, 1.0))))
                .ok_or_else(bad),
            Anchor::OnSegment(..) => Err(bad()),
        }
    };
    let first = plan.pieces.first().ok_or_else(bad)?;
    let mut pts = vec![at(&first.from)?];
    for (k, pc) in plan.pieces.iter().enumerate() {
        if k > 0 && at(&pc.from)?.distance(pts[k]) > 1e-3 {
            return Err(bad());
        }
        pts.push(at(&pc.to)?);
    }
    Ok(pts)
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
    validate_road_path_report(graph, terrain, tiles, content, req).0
}

/// [`validate_road_path`] plus the [`SnapReport`], which is also available when the path is
/// invalid (the tool still draws the guide under a red ghost).
pub fn validate_road_path_report(
    graph: &NetGraph,
    terrain: &Terrain,
    tiles: &TileOwnership,
    content: &ContentDb,
    req: &RoadPathRequest,
) -> (Result<RoadBuildPlan, Vec<RoadBuildError>>, SnapReport) {
    let mut report = SnapReport::default();
    let result = validate_inner(graph, terrain, tiles, content, req, None, &mut report);
    (result, report)
}

/// Validates a resolved [`BuildPlan`] (T-262): the anchors are used as given, nothing is snapped.
/// Only real errors refuse it (water, slope, ownership, a track meeting a road, too many roads or
/// an angle under 30° at a node, a piece shorter than [`MIN_SEGMENT_LENGTH_BUILD`]). The plan
/// then gets the graph invariants of §6.8.3 item 2: a cut that would leave a remnant under 8 m
/// is a node at the near end, a crossing within 8 m of a node goes through that node, a road
/// across a dead end joins it, and every span is sliced into `floor(L/96)+1` equal pieces.
pub fn validate_build_plan(
    graph: &NetGraph,
    terrain: &Terrain,
    tiles: &TileOwnership,
    content: &ContentDb,
    plan: &crate::plan::BuildPlan,
) -> Result<RoadBuildPlan, Vec<RoadBuildError>> {
    let req = RoadPathRequest {
        road: plan.road,
        path: RoadPath::Free(Vec::new()),
        elevation: plan.elevation,
        snap: SnapOptions {
            node: false,
            segment: false,
            angle: false,
            zone_grid: false,
        },
    };
    // The invisible cuts the plan meets close by are dissolved first; the plan is then checked on
    // the graph as it will be.
    let cuts = dissolve_cuts(graph, content, plan);
    let (g, p) = match &cuts {
        Some(c) => (&c.graph, &c.plan),
        None => (graph, plan),
    };
    let mut built = validate_inner(
        g,
        terrain,
        tiles,
        content,
        &req,
        Some(p),
        &mut SnapReport::default(),
    )?;
    if let Some(c) = &cuts {
        built.dissolve = c.dissolved.clone();
        built.merged = c.merged.clone();
    }
    cut_long_remnants(&mut built, g, cuts.as_ref().map_or(&[][..], |c| &c.merged));
    Ok(built)
}

/// The graph with the invisible cuts of a plan dissolved, and the plan with its anchors moved
/// onto the merged streets.
struct Dissolved {
    graph: NetGraph,
    plan: crate::plan::BuildPlan,
    /// The cuts (nodes of the original graph) that were merged away, in order.
    dissolved: Vec<NodeId>,
    /// The streets they made.
    merged: Vec<SegmentId>,
}

/// Where a ground plan meets an old street within [`MIN_SEGMENT_LENGTH_BUILD`] (along it) of a
/// plain cut — an anchor on the street, or a crossing — the cut is dissolved: the two pieces
/// become one straight street (`NetGraph::merge_joint`). `None` when nothing is dissolved.
fn dissolve_cuts(
    graph: &NetGraph,
    content: &ContentDb,
    plan: &crate::plan::BuildPlan,
) -> Option<Dissolved> {
    if !matches!(plan.elevation, ElevationMode::Ground) {
        return None;
    }
    let pts = plan_points(graph, plan).ok()?;
    // Where the plan meets a street: (segment, parameter on it).
    let mut meets: Vec<(SegmentId, f32)> = Vec::new();
    for pc in &plan.pieces {
        for a in [pc.from, pc.to] {
            if let Anchor::OnSegment(sid, t) = a {
                meets.push((sid, t.clamp(0.0, 1.0)));
            }
        }
    }
    let v3 = |p: Vec2| Vec3::new(p.x, 0.0, p.y);
    for (i, pc) in plan.pieces.iter().enumerate() {
        let leg = CubicBezier3::new(v3(pts[i]), v3(pc.c1), v3(pc.c2), v3(pts[i + 1]));
        for (sid, seg) in graph.segments() {
            if seg.elevation != Elevation::Ground || seg.curve.bounds_distance_xz(pts[i]) > 400.0 {
                continue;
            }
            for (tc, ts) in leg.intersect_xz(&seg.curve) {
                if (0.002..=0.998).contains(&tc) {
                    meets.push((sid, ts));
                }
            }
        }
    }
    let mut cuts: Vec<NodeId> = Vec::new();
    for (sid, t) in meets {
        let Some(seg) = graph.segment(sid) else {
            continue;
        };
        let at = seg.curve.arc_table().t_to_s(t);
        for (node, along) in [(seg.start, at), (seg.end, seg.length - at)] {
            if is_plain_cut(graph, node)
                && along < MIN_SEGMENT_LENGTH_BUILD - REMNANT_EPS
                && !cuts.contains(&node)
            {
                cuts.push(node);
            }
        }
    }
    if cuts.is_empty() {
        return None;
    }
    let mut work = graph.clone();
    let (mut dissolved, mut merged) = (Vec::new(), Vec::new());
    // (the piece consumed, the street it became, how far along it its start now is)
    let mut moves: Vec<(SegmentId, SegmentId, f32)> = Vec::new();
    for node in cuts {
        let Some(pieces) = joint_pieces(&work, node) else {
            continue;
        };
        let (first, second) = pieces;
        let (f, g) = (work.segment(first)?.clone(), work.segment(second)?.clone());
        // A cut that sits off the straight line between its ends (a crest or a dip: the pieces
        // followed the ground) is not dissolved: the merged street would be the chord in 3D and
        // the re-cuts would stand off the ground.
        let (from, to, joint) = (f.curve.p0, g.curve.p3, f.curve.p3);
        let run = Vec2::new(to.x - from.x, to.z - from.z);
        let along = if run.length_squared() > 1e-6 {
            (Vec2::new(joint.x - from.x, joint.z - from.z).dot(run) / run.length_squared())
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        if (from.y + (to.y - from.y) * along - joint.y).abs() > DISSOLVE_HEIGHT_TOL {
            continue;
        }
        let Some(m) = work.merge_joint_planar(content.road(f.road), node) else {
            continue;
        };
        dissolved.push(node);
        merged.push(m);
        moves.push((first, m, 0.0));
        moves.push((second, m, f.length));
    }
    if dissolved.is_empty() {
        return None;
    }
    // An anchor on a street that was merged away is the same place on the merged street: its
    // distance along the piece, plus the length of what came before it in the merge.
    let moved = |a: Anchor| match a {
        Anchor::OnSegment(sid, t) if work.segment(sid).is_none() => {
            let Some(old) = graph.segment(sid) else {
                return a;
            };
            let (mut id, mut s) = (sid, old.curve.arc_table().t_to_s(t));
            while work.segment(id).is_none() {
                let Some(&(_, into, offset)) = moves.iter().find(|(from, ..)| *from == id) else {
                    return a;
                };
                (id, s) = (into, s + offset);
            }
            let curve = &work.segment(id).expect("checked").curve;
            let table = curve.arc_table();
            Anchor::OnSegment(id, table.s_to_t(s.min(table.length())))
        }
        other => other,
    };
    let mut plan = plan.clone();
    for pc in &mut plan.pieces {
        pc.from = moved(pc.from);
        pc.to = moved(pc.to);
    }
    Some(Dissolved {
        graph: work,
        plan,
        dissolved,
        merged,
    })
}

/// The two pieces that meet in the plain cut `node`: the one that ends in it and the one that
/// starts in it.
fn joint_pieces(graph: &NetGraph, node: NodeId) -> Option<(SegmentId, SegmentId)> {
    let n = graph.node(node)?;
    let [a, b] = n.segments.as_slice() else {
        return None;
    };
    let (sa, sb) = (graph.segment(*a)?, graph.segment(*b)?);
    if sa.end == node && sb.start == node {
        Some((*a, *b))
    } else if sb.end == node && sa.start == node {
        Some((*b, *a))
    } else {
        None
    }
}

/// How far (metres, vertically) a cut may stand off the straight line between the ends of the
/// street it is a cut of for the plan to dissolve it.
const DISSOLVE_HEIGHT_TOL: f32 = 0.25;

/// The pieces an old street is left in after the plan cut it (or after cuts were merged into
/// it) are at most [`MAX_SEGMENT_LENGTH`] long: a longer span gets equal cuts, like a new road.
fn cut_long_remnants(built: &mut RoadBuildPlan, graph: &NetGraph, merged: &[SegmentId]) {
    let mut ids: Vec<SegmentId> = built.splits.iter().map(|s| s.segment).collect();
    ids.extend_from_slice(merged);
    ids.sort();
    ids.dedup();
    for id in ids {
        let Some(seg) = graph.segment(id) else {
            continue;
        };
        let table = seg.curve.arc_table();
        let mut at: Vec<f32> = built
            .splits
            .iter()
            .filter(|s| s.segment == id)
            .map(|s| table.t_to_s(s.t))
            .collect();
        at.sort_by(f32::total_cmp);
        let mut bounds = vec![0.0];
        bounds.extend(at);
        bounds.push(seg.length);
        for w in bounds.windows(2) {
            let span = w[1] - w[0];
            for cut in even_cuts(span) {
                let t = table.s_to_t(w[0] + cut);
                built.splits.push(PlannedSplit {
                    segment: id,
                    t,
                    pos: seg.curve.point(t),
                });
            }
        }
    }
}

/// Equal pieces of a span (§6.8.3 item 2): `floor(span/96)+1` of them, so none is longer than
/// [`MAX_SEGMENT_LENGTH`]; the cuts, ascending, from the span start.
pub fn even_cuts(span: f32) -> Vec<f32> {
    let n = (span / MAX_SEGMENT_LENGTH).floor() as usize + 1;
    (1..n).map(|k| span * k as f32 / n as f32).collect()
}

fn validate_inner(
    graph: &NetGraph,
    terrain: &Terrain,
    tiles: &TileOwnership,
    content: &ContentDb,
    req: &RoadPathRequest,
    literal: Option<&crate::plan::BuildPlan>,
    report: &mut SnapReport,
) -> Result<RoadBuildPlan, Vec<RoadBuildError>> {
    if req.road.index() >= content.roads().count() {
        return Err(vec![RoadBuildError::UnknownRoad(req.road)]);
    }
    let points_ok = if let Some(plan) = literal {
        plan.pieces.iter().all(|pc| {
            cp_core::sane_point(pc.c1)
                && cp_core::sane_point(pc.c2)
                && [pc.from, pc.to]
                    .iter()
                    .all(|a| !matches!(a, Anchor::Free(p) if !cp_core::sane_point(*p)))
        })
    } else {
        match &req.path {
            RoadPath::Straight { from, to } => {
                cp_core::sane_point(*from) && cp_core::sane_point(*to)
            }
            RoadPath::Curve { from, control, to } => {
                [from, control, to].iter().all(|p| cp_core::sane_point(**p))
            }
            RoadPath::Free(pts) => pts.iter().all(|p| cp_core::sane_point(*p)),
        }
    };
    let height_ok = match req.elevation {
        ElevationMode::Ground => true,
        ElevationMode::Elevated(h) => cp_core::sane_coord(h),
        ElevationMode::Tunnel(d) => cp_core::sane_coord(d) && d > 0.0 && d <= MAX_TUNNEL_DEPTH,
    };
    if !points_ok || !height_ok {
        return Err(vec![RoadBuildError::BadCoordinates]);
    }
    let def = content.road(req.road);
    let mut errors = Vec::new();
    let elevated = matches!(req.elevation, ElevationMode::Elevated(_));
    let tunnel = matches!(req.elevation, ElevationMode::Tunnel(_));
    let variant = if elevated {
        match &def.elevated {
            Some(v) => Some(v),
            None => return Err(vec![RoadBuildError::NoElevatedVariant(req.road)]),
        }
    } else {
        None
    };
    let cost_per_m = if tunnel {
        match &def.tunnel {
            Some(v) => v.cost_per_m,
            None => return Err(vec![RoadBuildError::NoTunnelVariant(req.road)]),
        }
    } else if !elevated && !def.ground {
        return Err(vec![RoadBuildError::TunnelOnly(req.road)]);
    } else {
        variant.map_or(def.cost_per_m, |v| v.cost_per_m)
    };
    let seg_elevation = if elevated {
        Elevation::Elevated
    } else if tunnel {
        Elevation::Tunnel
    } else {
        Elevation::Ground
    };
    let rail = def.class == RoadClass::Rail;

    // Raw vertices and the control point of a curve leg.
    let (raw, control): (Vec<Vec2>, Option<Vec2>) = if let Some(plan) = literal {
        (plan_points(graph, plan)?, None)
    } else {
        match &req.path {
            RoadPath::Straight { from, to } => (vec![*from, *to], None),
            RoadPath::Curve { from, control, to } => (vec![*from, *to], Some(*control)),
            RoadPath::Free(pts) => (pts.clone(), None),
        }
    };
    let mut distinct = raw.clone();
    if literal.is_none() {
        distinct.dedup_by(|a, b| a.distance(*b) < 1e-3);
    }
    if distinct.len() < 2
        || (literal.is_some() && distinct.windows(2).any(|w| w[0].distance(w[1]) < 1e-3))
    {
        return Err(vec![RoadBuildError::PathTooShort]);
    }

    let mut b = Builder {
        graph,
        terrain,
        content,
        rail,
        elev: req.elevation,
        new_nodes: Vec::new(),
        splits: Vec::new(),
    };
    let angle_ok = req.snap.angle && control.is_none();

    // --- snap the vertices ---
    let mut snaps = Vec::new();
    let mut verts: Vec<Vertex> = Vec::new();
    for (i, &p) in distinct.iter().enumerate() {
        if let Some(plan) = literal {
            let anchor = if i == 0 {
                plan.pieces[0].from
            } else {
                plan.pieces[i - 1].to
            };
            let (pos, node) = b.anchor_node(anchor, p);
            verts.push(Vertex {
                pos,
                node,
                refs: Vec::new(),
            });
            continue;
        }
        let mut pos = p;
        let mut node = None;
        *report = SnapReport::default();
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
            report.kind = SnapReportKind::Node;
        }
        if node.is_none()
            && req.snap.segment
            && matches!(req.elevation, ElevationMode::Ground)
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
            report.kind = SnapReportKind::Segment;
        }
        if node.is_none() && angle_ok && i > 0 {
            let prev = &verts[i - 1];
            if let Some(a) = angle_snap(graph, p, prev.pos, &prev.refs) {
                pos = a.pos;
                snaps.push(AppliedSnap {
                    vertex: i,
                    kind: a.kind,
                    pos,
                });
                *report = SnapReport {
                    kind: match a.kind {
                        SnapKind::KeyAngle(d) => SnapReportKind::KeyAngle(d),
                        SnapKind::Guide { perpendicular } => {
                            SnapReportKind::Guide { perpendicular }
                        }
                        SnapKind::Angle(d) => SnapReportKind::Step(d),
                        _ => SnapReportKind::None,
                    },
                    guide: a.guide,
                };
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
    // For the height of a raised road at a crossing: how far along the path it is and the
    // ramp it climbs (the ends stand on the ground), see below.
    let path_len: f32 = verts.windows(2).map(|w| w[0].pos.distance(w[1].pos)).sum();
    let ramp_slope = def.max_slope * RAMP_SLOPE_SHARE;
    let (ground0, ground1) = (
        verts.first().map_or(0.0, |v| terrain.height(v.pos)),
        verts.last().map_or(0.0, |v| terrain.height(v.pos)),
    );
    let mut done_len = 0.0f32;
    for (li, w) in verts.windows(2).enumerate() {
        let (v0, v1) = (&w[0], &w[1]);
        if v0.node == v1.node {
            errors.push(RoadBuildError::DegenerateLeg { pos: v0.pos });
            continue;
        }
        let leg = match literal {
            Some(plan) => {
                let pc = &plan.pieces[li];
                let v3 = |p: Vec2| Vec3::new(p.x, 0.0, p.y);
                CubicBezier3::new(v3(v0.pos), v3(pc.c1), v3(pc.c2), v3(v1.pos))
            }
            None => flat_curve(v0.pos, v1.pos, control.filter(|_| li == 0)),
        };
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
        // pieces of at most MAX_SEGMENT_LENGTH (see `auto_cuts`: on the control points of §6.7),
        // so an automatic cut can never land a few metres from a crossing; a span that really
        // is shorter than MIN_SEGMENT_LENGTH_BUILD is reported by the per-piece check below.
        let mut fixed: Vec<(f32, NodeRef)> = Vec::new();
        if elevated {
            // Two raised roads that cross: the same level meets in a node, a gap under the
            // clearance is refused, a bigger one passes (T-064).
            for (sid, seg) in graph.segments() {
                if !matches!(seg.elevation, Elevation::Elevated | Elevation::Bridge) {
                    continue;
                }
                for (tc, ts) in leg.intersect_xz(&seg.curve) {
                    // A literal plan also meets the ends of a segment (a road across a dead end
                    // joins it, §6.8.3 item 2).
                    let ts_ok = literal.is_some() || (0.002..=0.998).contains(&ts);
                    if !(0.002..=0.998).contains(&tc) || !ts_ok {
                        continue;
                    }
                    let at = seg.curve.point(ts);
                    // The deck there: its height over the ground, less where it still climbs
                    // its ramp from the ends of the path.
                    let s_here = done_len + tab.t_to_s(tc);
                    let mine = b
                        .ground_y(to_xz(leg.point(tc)))
                        .min(ground0 + ramp_slope * s_here)
                        .min(ground1 + ramp_slope * (path_len - s_here).max(0.0));
                    let mut gap = (mine - at.y).abs();
                    // A path that stops a few metres past (or starts a few metres before) an old
                    // deck ends on it: the short piece is cut off, so the deck is where it joins.
                    if s_here < MIN_SEGMENT_LENGTH_BUILD
                        || path_len - s_here < MIN_SEGMENT_LENGTH_BUILD
                    {
                        gap = 0.0;
                    }
                    if gap > SAME_DEPTH_TOL {
                        if gap < ELEVATED_CLEARANCE {
                            errors.push(RoadBuildError::ElevatedClearance { pos: at });
                        }
                        continue;
                    }
                    let r = b.cross_node(sid, ts, literal.is_some());
                    if r != v0.node && r != v1.node && !fixed.iter().any(|(_, nd)| *nd == r) {
                        fixed.push((tc, r));
                    }
                }
            }
        }
        if !elevated {
            let level = if tunnel {
                Elevation::Tunnel
            } else {
                Elevation::Ground
            };
            for (sid, seg) in graph.segments() {
                if seg.elevation != level {
                    continue;
                }
                for (tc, ts) in leg.intersect_xz(&seg.curve) {
                    // A literal plan also meets the ends of a segment (a road across a dead end
                    // joins it, §6.8.3 item 2).
                    let ts_ok = literal.is_some() || (0.002..=0.998).contains(&ts);
                    if !(0.002..=0.998).contains(&tc) || !ts_ok {
                        continue;
                    }
                    let at = seg.curve.point(ts);
                    if tunnel {
                        // Same depth: a node. Far enough apart: they pass each other.
                        let mine = b.ground_y(to_xz(leg.point(tc)));
                        let gap = (mine - at.y).abs();
                        if gap > SAME_DEPTH_TOL {
                            if gap < TUNNEL_CLEARANCE {
                                errors.push(RoadBuildError::TunnelClearance { pos: at });
                            }
                            continue;
                        }
                    } else if b.is_rail_road(seg.road) != rail {
                        errors.push(RoadBuildError::RailCrossesRoad { pos: at });
                        continue;
                    }
                    let r = b.cross_node(sid, ts, literal.is_some());
                    if r != v0.node && r != v1.node && !fixed.iter().any(|(_, nd)| *nd == r) {
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
            let cuts = if literal.is_some() {
                even_cuts(span)
            } else {
                auto_cuts(span)
            };
            for cut in cuts {
                bounds.push((tab.s_to_t(span_start + cut), None));
            }
            bounds.push((t_end.unwrap_or(1.0), Some(node)));
            span_start = s_end;
        }
        // A raised piece shorter than the minimum is not a reason to refuse the path: the
        // automatic cut beside it goes, and the neighbour grows to take its place (T-064).
        if elevated && literal.is_none() {
            while let Some(i) = (1..bounds.len().saturating_sub(1)).find(|&i| {
                bounds[i].1.is_none()
                    && (tab.t_to_s(bounds[i].0) - tab.t_to_s(bounds[i - 1].0)
                        < MIN_SEGMENT_LENGTH_BUILD
                        || tab.t_to_s(bounds[i + 1].0) - tab.t_to_s(bounds[i].0)
                            < MIN_SEGMENT_LENGTH_BUILD)
            }) {
                bounds.remove(i);
            }
            // The first or last piece of the whole path that is shorter than the minimum, next to
            // a crossing, is cut off: the path then starts or ends on the crossing node.
            let piece = |bs: &[(f32, Option<NodeRef>)], a: usize| {
                tab.t_to_s(bs[a + 1].0) - tab.t_to_s(bs[a].0)
            };
            if li + 2 == verts.len()
                && bounds.len() >= 3
                && piece(&bounds, bounds.len() - 2) < MIN_SEGMENT_LENGTH_BUILD
                && bounds[bounds.len() - 2].1.is_some()
            {
                bounds.pop();
            }
            if li == 0
                && bounds.len() >= 3
                && piece(&bounds, 0) < MIN_SEGMENT_LENGTH_BUILD
                && bounds[1].1.is_some()
            {
                bounds.remove(0);
            }
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
            let mut planar = leg.sub_curve(pair[0].0, pair[1].0);
            if literal.is_some() {
                // An end moved onto an existing node (a crossing within 8 m of it): its control
                // point goes along, so the piece keeps the direction it had there.
                let d = |a: Vec3, b: Vec3| Vec3::new(a.x - b.x, 0.0, a.z - b.z);
                planar.p1 += d(pa, planar.p0);
                planar.p2 += d(pb, planar.p3);
            }
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
        done_len += len;
    }

    // --- ramps: an elevated path starts and ends at ground level; so does a road tunnel (its
    // portals, T-173), dipping down to its depth instead of rising ---
    let road_tunnel = tunnel && !rail;
    if elevated || road_tunnel {
        let slope = ramp_slope;
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
                    // A node made on a crossing road is where that road is.
                    NodeRef::Split(i) => Some(b.splits[i].pos.y),
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
                let (mut lo, mut hi) = (f32::NEG_INFINITY, f32::INFINITY);
                for (f, fy) in fixed.iter().enumerate() {
                    if let Some(fy) = fy {
                        let reach = slope * (at_s[k] - at_s[f]).abs();
                        if road_tunnel {
                            y = y.max(fy - reach);
                            hi = hi.min(fy + reach);
                        } else {
                            y = y.min(fy + reach);
                            lo = lo.max(fy - reach);
                        }
                    }
                }
                // Within what every fixed node allows (a ramp to a raised node that already
                // stands, T-064); when nothing fits the piece is steeper than the road may be and
                // the piece check below says so.
                y = if road_tunnel { y.min(hi) } else { y.max(lo) };
                ys[k] = y;
                reached |= (y - target[k]).abs() <= 1e-3;
            }
            // A path that ends on a raised node that already stands is a ramp up to it: the
            // heights in between follow the nodes, the slope of every piece is checked below.
            let ramp_to_raised = elevated
                && nodes.iter().zip(&fixed).any(|(&n, f)| {
                    matches!(n, NodeRef::Existing(_))
                        && f.is_some_and(|y| y > terrain.height(to_xz(b.node_pos(n))) + 0.5)
                });
            if !reached && !ramp_to_raised {
                errors.push(RoadBuildError::RampTooShort {
                    pos: b.node_pos(nodes[0]),
                });
            }
            for k in 0..nodes.len() {
                if let NodeRef::New(i) = nodes[k] {
                    let p = to_xz(b.new_nodes[i]);
                    // A tunnel may run under water at its depth; its portals and the ramp down
                    // to it must be dry.
                    let on_ramp = if road_tunnel {
                        fixed[k].is_some() || ys[k] > target[k] + 1.0
                    } else {
                        fixed[k].is_some() || ys[k] < target[k] - 1e-3
                    };
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

    // --- raised crossings, once the ramps have set the heights: whatever crosses an old deck
    // closer than the clearance without a node is refused (T-064) ---
    if elevated {
        for s in &segments {
            for (sid, old) in graph.segments() {
                if !matches!(old.elevation, Elevation::Elevated | Elevation::Bridge) {
                    continue;
                }
                for (tc, ts) in s.curve.intersect_xz(&old.curve) {
                    if !(0.002..=0.998).contains(&tc) || !(0.002..=0.998).contains(&ts) {
                        continue;
                    }
                    let (mine, theirs) = (s.curve.point(tc), old.curve.point(ts));
                    let noded = b
                        .splits
                        .iter()
                        .any(|sp| sp.segment == sid && (sp.t - ts).abs() < 0.02);
                    if !noded
                        && (mine.y - theirs.y).abs() < ELEVATED_CLEARANCE
                        && !errors
                            .iter()
                            .any(|e| matches!(e, RoadBuildError::ElevatedClearance { .. }))
                    {
                        errors.push(RoadBuildError::ElevatedClearance { pos: theirs });
                    }
                }
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
            && slope > def.max_slope + SLOPE_EPS
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

    // --- a literal plan on the asphalt of a road it does not join (§6.8.3 item 1) ---
    if literal.is_some() && matches!(req.elevation, ElevationMode::Ground) {
        const TOL: f32 = 0.3;
        let ground = |e: Elevation| e == Elevation::Ground;
        let half = |road: RoadDefId| content.road(road).width / 2.0;
        // A node the plan makes (a new end, a cut of a road) on another road's asphalt.
        let mut checked: Vec<NodeRef> = Vec::new();
        for n in segments.iter().flat_map(|s| [s.start, s.end]) {
            if checked.contains(&n) || matches!(n, NodeRef::Existing(_)) {
                continue;
            }
            checked.push(n);
            let p = to_xz(b.node_pos(n));
            let own = match n {
                NodeRef::Split(i) => Some(b.splits[i].segment),
                _ => None,
            };
            // The street the cut is made in runs on through its plain cuts: the next pieces are
            // the same road, not another one.
            let runs_on = |seg: &Segment| {
                own.and_then(|o| graph.segment(o)).is_some_and(|o| {
                    [o.start, o.end].iter().any(|&m| {
                        (seg.start == m || seg.end == m)
                            && junction_reach(graph, content, m) <= MIN_SEGMENT_LENGTH_BUILD
                            && graph.node(m).is_some_and(|n| n.segments.len() == 2)
                    })
                })
            };
            for (sid, seg) in graph.segments() {
                if !ground(seg.elevation)
                    || Some(sid) == own
                    || b.is_rail_road(seg.road) != rail
                    || runs_on(seg)
                {
                    continue;
                }
                let hw = half(seg.road);
                if seg.curve.bounds_distance_xz(p) > hw + 1e-3 {
                    continue;
                }
                if seg.curve.nearest_xz(p).1 < hw - TOL {
                    errors.push(RoadBuildError::NodeOnRoad { pos: b.node_pos(n) });
                    break;
                }
            }
        }
        // An old node under the asphalt of a new piece that does not end in it.
        let joined: Vec<NodeId> = segments
            .iter()
            .flat_map(|s| [s.start, s.end])
            .filter_map(|n| match n {
                NodeRef::Existing(id) => Some(id),
                _ => None,
            })
            .collect();
        let hw_new = half(req.road);
        for (nid, node) in graph.nodes() {
            if !ground(node.elevation) || joined.contains(&nid) || b.is_rail_node(node) != rail {
                continue;
            }
            let p = to_xz(node.pos);
            if segments.iter().any(|s| {
                s.elevation == Elevation::Ground
                    && s.curve.bounds_distance_xz(p) <= hw_new
                    && s.curve.nearest_xz(p).1 < hw_new - TOL
            }) {
                errors.push(RoadBuildError::NodeOnRoad { pos: node.pos });
            }
        }
    }

    if literal.is_some() && matches!(req.elevation, ElevationMode::Ground) && !rail {
        for w in clearance::overlap_report(graph, content, req.road, &segments) {
            let y = terrain.height(w.pos);
            errors.push(RoadBuildError::RoadsOverlap {
                pos: Vec3::new(w.pos.x, y, w.pos.y),
            });
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
        report: *report,
        demolish: Vec::new(),
        cost,
        literal: literal.is_some(),
        dissolve: Vec::new(),
        merged: Vec::new(),
    })
}

/// A warning about the direction of a one-way plan (§6.6). Warnings never block the command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OneWayWarning {
    /// A one-way piece ends at this node, which has other roads but no car lane leaving it.
    /// (A node with no other road is an unfinished end, not a dead end.)
    DeadEnd { at: NodeRef },
    /// A node of degree two where both roads are one-way and both lead into it.
    HeadOn { at: NodeRef },
}

impl std::fmt::Display for OneWayWarning {
    /// Short player-facing text (shown next to the ghost).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeadEnd { .. } => write!(f, "one-way road ends where its traffic cannot leave"),
            Self::HeadOn { .. } => write!(f, "two one-way roads meet head-on"),
        }
    }
}

/// What a road end at a node carries, as seen from the node.
#[derive(Clone, Copy, Debug, Default)]
struct ArmDirs {
    /// Has a car lane that arrives at the node.
    inbound: bool,
    /// Has a car lane that leaves the node.
    outbound: bool,
}

impl ArmDirs {
    /// The end of a road with these car lanes (`fwd` / `bwd` = has a Forward / Backward car
    /// lane) at the segment's `end` (`at_end`) or `start` node.
    fn of(fwd: bool, bwd: bool, at_end: bool) -> Self {
        if at_end {
            Self {
                inbound: fwd,
                outbound: bwd,
            }
        } else {
            Self {
                inbound: bwd,
                outbound: fwd,
            }
        }
    }
    fn one_way_in(self) -> bool {
        self.inbound && !self.outbound
    }
}

/// The car-lane directions of an existing segment: `(has Forward, has Backward)`.
fn segment_car_dirs(graph: &NetGraph, seg: &Segment) -> (bool, bool) {
    let (mut fwd, mut bwd) = (false, false);
    for lane in seg.lanes.iter().filter_map(|&l| graph.lane(l)) {
        if lane.kind == LaneKind::Car {
            match lane.dir {
                LaneDir::Forward => fwd = true,
                LaneDir::Backward => bwd = true,
            }
        }
    }
    (fwd, bwd)
}

/// Direction warnings for `plan` (§6.6): pure, reads the graph the plan was made against.
///
/// `content` gives the car lanes of the plan's road (the plan only holds its id).
/// A node that is a `HeadOn` is not also reported as a `DeadEnd`. Nodes are visited in plan
/// order, so the result is deterministic.
pub fn one_way_report(
    graph: &NetGraph,
    content: &ContentDb,
    plan: &RoadBuildPlan,
) -> Vec<OneWayWarning> {
    let def = content.road(plan.road);
    let (fwd, bwd) =
        def.lanes
            .iter()
            .filter(|l| l.kind == LaneKind::Car)
            .fold((false, false), |(f, b), l| match l.dir {
                LaneDir::Forward => (true, b),
                LaneDir::Backward => (f, true),
            });
    let plan_one_way = def.is_one_way();

    // The distinct nodes the plan touches, in plan order.
    let mut touched: Vec<NodeRef> = Vec::new();
    for s in &plan.segments {
        for r in [s.start, s.end] {
            if !touched.contains(&r) {
                touched.push(r);
            }
        }
    }

    let mut out = Vec::new();
    for at in touched {
        let mut arms: Vec<ArmDirs> = Vec::new();
        let mut leaves_map = false;
        // Roads that already exist at the node.
        match at {
            NodeRef::Existing(n) => {
                if let Some(node) = graph.node(n) {
                    leaves_map = node.kind == NodeKind::Outside;
                    for seg in node.segments.iter().filter_map(|&id| graph.segment(id)) {
                        let (f, b) = segment_car_dirs(graph, seg);
                        if seg.end == n {
                            arms.push(ArmDirs::of(f, b, true));
                        }
                        if seg.start == n {
                            arms.push(ArmDirs::of(f, b, false));
                        }
                    }
                }
            }
            NodeRef::Split(i) => {
                // The two halves of the split segment: one ends at the node, one starts there.
                if let Some(seg) = plan.splits.get(i).and_then(|sp| graph.segment(sp.segment)) {
                    let (f, b) = segment_car_dirs(graph, seg);
                    arms.push(ArmDirs::of(f, b, true));
                    arms.push(ArmDirs::of(f, b, false));
                }
            }
            NodeRef::New(_) => {}
        }
        // The plan's own pieces.
        let mut plan_arm_in = false;
        for s in &plan.segments {
            if s.end == at {
                let a = ArmDirs::of(fwd, bwd, true);
                plan_arm_in |= plan_one_way && a.one_way_in();
                arms.push(a);
            }
            if s.start == at {
                arms.push(ArmDirs::of(fwd, bwd, false));
            }
        }
        if leaves_map {
            continue;
        }
        let head_on = arms.len() == 2 && arms.iter().all(|a| a.one_way_in());
        if head_on {
            out.push(OneWayWarning::HeadOn { at });
        } else if plan_arm_in && arms.len() >= 2 && !arms.iter().any(|a| a.outbound) {
            out.push(OneWayWarning::DeadEnd { at });
        }
    }
    out
}

mod clearance;
pub use clearance::{ClearanceWarning, clearance_report};
mod quantize;
pub use quantize::{CONTROL_LINE_TOL, ControlMark, StepContext, quantize_path};

#[cfg(test)]
mod tests;
