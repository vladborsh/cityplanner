//! Road network data: [`Node`], [`Segment`], [`Lane`] (`system-design.md` §6.1).
//!
//! Save stability: these types are serialised with postcard; changing fields is a save
//! format change. Enums are append-only.

use bevy_math::Vec3;
use cp_content::{LaneDir, LaneKind, RoadDefId};
use cp_core::Id;
use cp_core::curve::CubicBezier3;
use serde::{Deserialize, Serialize};

pub type NodeId = Id<Node>;
pub type SegmentId = Id<Segment>;
pub type LaneId = Id<Lane>;

/// Hard limit of segments meeting at one node (`system-design.md` §6.3).
pub const MAX_SEGMENTS_PER_NODE: usize = 8;

/// Cubics per lane centre line.
pub const LANE_CURVE_PIECES: usize = 4;

/// Topological role of a node; derived from its segment count except `Outside`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    /// Three or more segments.
    Junction,
    /// Zero or one segment (dead end / lone node).
    End,
    /// Exactly two segments (a bend or a split point).
    Middle,
    /// Connection to the world outside the map; never auto-removed or re-derived.
    Outside,
}

/// How traffic is regulated at a node. Saved by variant number: variants are only
/// appended (no save version bump needed for that, `system-design.md` §4).
///
/// `Priority` and `TrafficLight` are the *automatic* states: the §6.5 rule decides between
/// them whenever the junction is recomputed. `ManualPriority` / `ManualTrafficLight` are
/// the player's choice (`ToggleTrafficLight`) and the rule never changes them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum JunctionControl {
    #[default]
    Priority,
    TrafficLight,
    ManualPriority,
    ManualTrafficLight,
}

impl JunctionControl {
    pub fn has_light(self) -> bool {
        matches!(self, Self::TrafficLight | Self::ManualTrafficLight)
    }
    /// Set by the player, not by the auto-install rule.
    pub fn is_manual(self) -> bool {
        matches!(self, Self::ManualPriority | Self::ManualTrafficLight)
    }
}

/// Vertical treatment of a road piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Elevation {
    Ground,
    Elevated,
    Bridge,
    Tunnel,
}

impl Elevation {
    /// How far from its centre line a segment of this height keeps the ground free, for a road
    /// of `half_width` (asphalt and sidewalks): the strip itself on the ground, one zone cell
    /// more each side under a deck or a bridge (T-332, §18.1: no lot and no building under an
    /// overpass), nothing over a tunnel (only its portals, which are ground pieces).
    pub fn ground_strip(self, half_width: f32) -> Option<f32> {
        match self {
            Elevation::Ground => Some(half_width),
            Elevation::Elevated | Elevation::Bridge => Some(half_width + crate::ZONE_CELL),
            Elevation::Tunnel => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// World position (Y = height).
    pub pos: Vec3,
    /// Attached segments in attachment order (at most [`MAX_SEGMENTS_PER_NODE`]).
    pub segments: Vec<SegmentId>,
    pub kind: NodeKind,
    pub control: JunctionControl,
    pub elevation: Elevation,
    /// The player's priority signs (T-303, §6.5a), one per arm at most, ascending by segment.
    /// They choose the main road only while the node has no light. Appended last (a save format bump).
    pub signs: Vec<ArmSign>,
    /// The player's lane rules (T-313, §18.2), sorted by `(segment, lane)`, one per lane at
    /// most. A lane without a rule connects by the default of §6.4. Appended last (a save
    /// format bump).
    pub lane_rules: Vec<LaneRule>,
}

/// Where a lane may go at a node: the arm, and the lane of that arm when it is fixed (a
/// connector); `None` leaves the lane to the turn rule of §6.4. The arm of the lane itself is a
/// U-turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Exit {
    pub arm: SegmentId,
    pub lane: Option<u8>,
}

/// The exits of one inbound lane (`lane` is its index in the segment) at a node (§18.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneRule {
    pub segment: SegmentId,
    pub lane: u8,
    /// Sorted, without repeats, never empty.
    pub exits: Vec<Exit>,
}

/// A priority sign on one arm of a junction (§6.5a). Enum variants are only appended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sign {
    /// This arm is on the main road.
    Main,
    /// Traffic from this arm gives way.
    Yield,
}

/// The sign standing on the arm `segment` of a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArmSign {
    pub segment: SegmentId,
    pub sign: Sign,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: NodeId,
    pub end: NodeId,
    pub curve: CubicBezier3,
    pub road: RoadDefId,
    pub elevation: Elevation,
    /// Lanes in `RoadDef.lanes` order.
    pub lanes: Vec<LaneId>,
    /// Arc length of `curve`, metres.
    pub length: f32,
    /// Build order (T-262, save v55): a monotonic counter ([`crate::NetGraph::next_order`]), the
    /// tie-break of the zone layout instead of the arena slot (which is reused). A piece cut from
    /// a segment keeps its order; a segment made of two takes the older one.
    pub order: u32,
    /// A row of street trees along both sides (T-323, §24, save v79). Only on ground segments
    /// of a road that is not a path or a railway. A piece cut from a segment keeps the flag; a
    /// segment made of two has it when both had it.
    pub trees: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    pub segment: SegmentId,
    /// Index in the road profile.
    pub index: u8,
    pub kind: LaneKind,
    pub dir: LaneDir,
    pub width: f32,
    /// Lateral offset of the lane centre from the segment axis; positive is left of the
    /// segment's own direction (start → end).
    pub offset: f32,
    /// Speed limit, m/s.
    pub speed_limit: f32,
    /// Centre line **in the lane's direction of travel**: the segment curve offset by
    /// `offset` as a chain of [`LANE_CURVE_PIECES`] cubics (uniform in the segment's `t`),
    /// reversed for `LaneDir::Backward`.
    ///
    /// Accuracy: the chain stays within 2 cm of the true offset (distance `|offset|` from
    /// the segment axis) for lane offsets up to 13 m on a 80 m S-bend with a minimum
    /// radius of 35 m; gentler or shorter segments are better. The build limit is 96 m
    /// per segment.
    pub curve: [CubicBezier3; LANE_CURVE_PIECES],
}
