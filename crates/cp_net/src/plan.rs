//! The build plan (`architecture/system-design.md` §6.8.3 item 1, T-262): what the tool sends
//! once it has resolved every snap. The sim applies it as written and does not snap again.
//!
//! [`NetCommand::BuildPlan`](crate::NetCommand) applies a plan literally
//! ([`crate::build::validate_build_plan`]): anchors are used as given, only real errors refuse
//! it, and the graph keeps its invariants (`NetGraph::check_invariants`).

use bevy_math::Vec2;
use cp_content::RoadDefId;
use serde::{Deserialize, Serialize};

use crate::build::ElevationMode;
use crate::model::{NodeId, SegmentId};

/// Where a piece starts or ends. Resolved by the tool; the sim never looks for a better place.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Anchor {
    /// An existing node.
    Node(NodeId),
    /// A point of an existing segment at curve parameter `t` (the segment is cut there).
    OnSegment(SegmentId, f32),
    /// Open ground.
    Free(Vec2),
}

/// One cubic piece of a plan, in world XZ: from `from` through the control points `c1`, `c2`
/// to `to`. A straight piece has its control points on the chord.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanPiece {
    pub from: Anchor,
    pub c1: Vec2,
    pub c2: Vec2,
    pub to: Anchor,
}

/// A road to build: its type, its height and the pieces in drawing order. Consecutive pieces
/// share their end anchor (`to` of one is `from` of the next).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuildPlan {
    pub road: RoadDefId,
    pub elevation: ElevationMode,
    pub pieces: Vec<PlanPiece>,
    /// Plant street trees on the new ground segments (T-323, §24.2): the price is added to the
    /// plan's. Last field: the command log replays exactly.
    pub trees: bool,
}

impl PlanPiece {
    /// A straight piece between two points, the control points on the chord thirds.
    pub fn straight(from: Anchor, to: Anchor, a: Vec2, b: Vec2) -> Self {
        Self {
            from,
            c1: a.lerp(b, 1.0 / 3.0),
            c2: a.lerp(b, 2.0 / 3.0),
            to,
        }
    }
}
