//! The build plan (`architecture/system-design.md` §6.8.3 item 1, T-262): what the tool sends
//! once it has resolved every snap. The sim applies it as written and does not snap again.
//!
//! Step 1 (this file): the contract types. [`NetCommand::BuildPlan`](crate::NetCommand) applies
//! a plan through the existing `BuildRoad` path (each piece is turned into a [`RoadPath`] at the
//! resolved positions; a cubic piece with bent control points becomes the quadratic through the
//! intersection of its end tangents). Step 2 replaces that with the literal application.

use bevy_math::Vec2;
use cp_content::RoadDefId;
use serde::{Deserialize, Serialize};

use crate::build::{ElevationMode, RoadPath};
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
}

/// How far control points may stand off the chord for a piece to count as straight, metres.
const STRAIGHT_TOL_M: f32 = 0.05;

impl PlanPiece {
    /// A straight piece between two anchors, the control points on the chord thirds.
    pub fn straight(from: Anchor, to: Anchor, a: Vec2, b: Vec2) -> Self {
        Self {
            from,
            c1: a.lerp(b, 1.0 / 3.0),
            c2: a.lerp(b, 2.0 / 3.0),
            to,
        }
    }

    /// The drawn path of this piece between the resolved end points `a` and `b`.
    pub fn road_path(&self, a: Vec2, b: Vec2) -> RoadPath {
        let chord = b - a;
        let len = chord.length();
        let off = |p: Vec2| {
            if len < 1.0e-3 {
                (p - a).length()
            } else {
                (chord.perp_dot(p - a) / len).abs()
            }
        };
        if off(self.c1) <= STRAIGHT_TOL_M && off(self.c2) <= STRAIGHT_TOL_M {
            return RoadPath::Straight { from: a, to: b };
        }
        // The quadratic through the meeting point of the end tangents; parallel tangents (an S
        // bend) fall back to the middle of the control points.
        let (d1, d2) = (self.c1 - a, self.c2 - b);
        let denom = d1.perp_dot(d2);
        let control = if denom.abs() > 1.0e-6 {
            let t = (b - a).perp_dot(d2) / denom;
            a + d1 * t
        } else {
            (self.c1 + self.c2) * 0.5
        };
        RoadPath::Curve {
            from: a,
            control,
            to: b,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_pieces_stay_straight_and_bent_ones_get_the_tangent_meeting_point() {
        let (a, b) = (Vec2::new(0.0, 0.0), Vec2::new(90.0, 0.0));
        let line = PlanPiece::straight(Anchor::Free(a), Anchor::Free(b), a, b);
        assert_eq!(line.road_path(a, b), RoadPath::Straight { from: a, to: b });
        // Tangents rise at both ends: they meet over the middle.
        let bent = PlanPiece {
            from: Anchor::Free(a),
            c1: Vec2::new(30.0, 30.0),
            c2: Vec2::new(60.0, 30.0),
            to: Anchor::Free(b),
        };
        let RoadPath::Curve { control, .. } = bent.road_path(a, b) else {
            panic!("a curve");
        };
        assert!(
            control.distance(Vec2::new(45.0, 45.0)) < 1.0e-3,
            "{control}"
        );
    }
}
