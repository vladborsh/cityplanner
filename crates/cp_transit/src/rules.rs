//! Where a stop may stand (§16 M3 A "Остановка на дороге"). Pure functions of the network and
//! the content, shared by the command and the tool's ghost.

use bevy_math::Vec2;
use cp_content::{ContentDb, LaneKind, RoadClass};
use cp_core::Arena;
use cp_core::curve::to_xz;
use cp_net::{NetGraph, Segment, SegmentId};
use cp_traffic::Side;

use crate::model::{Stop, StopId, StopKind};

/// Metres a road stop keeps from either end of its segment.
pub const STOP_NODE_CLEARANCE: f32 = 12.0;
/// Metres between two stops of one side of a segment.
pub const STOP_SPACING: f32 = 24.0;
/// How far a stop may sit from where its side's pavement lies and still be re-attached to a
/// segment after the network changed.
pub const STOP_SNAP: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopError {
    NoSuchSegment,
    /// `s` is outside the segment.
    OffSegment,
    /// The road has no pavement on that side.
    NoPavement,
    Highway,
    /// Closer than [`STOP_NODE_CLEARANCE`] to an end of the segment.
    NearNode,
    /// Closer than [`STOP_SPACING`] to another stop on the same side.
    NearStop(StopId),
}

impl std::fmt::Display for StopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchSegment => write!(f, "no such road"),
            Self::OffSegment => write!(f, "not on the road"),
            Self::NoPavement => write!(f, "the road has no pavement on that side"),
            Self::Highway => write!(f, "no stops on highways"),
            Self::NearNode => write!(
                f,
                "a stop is at least {STOP_NODE_CLEARANCE} m from a junction"
            ),
            Self::NearStop(_) => write!(f, "a stop is at least {STOP_SPACING} m from the next"),
        }
    }
}

/// Lateral offset of the outermost pavement lane on `side` of the road, metres (positive is
/// left of the segment direction); `None` when that side has none.
pub fn pavement_offset(db: &ContentDb, seg: &Segment, side: Side) -> Option<f32> {
    let def = db.road(seg.road);
    def.lanes
        .iter()
        .filter(|l| l.kind == LaneKind::Pedestrian)
        .map(|l| l.offset)
        .filter(|&o| match side {
            Side::Left => o > 0.0,
            Side::Right => o < 0.0,
        })
        .max_by(|a, b| a.abs().total_cmp(&b.abs()))
}

/// The pavement point `s` metres along `seg`, `offset` to the left of its direction.
pub fn pavement_point(seg: &Segment, s: f32, offset: f32) -> Vec2 {
    let t = seg.curve.arc_table().s_to_t(s);
    let p = to_xz(seg.curve.point(t));
    let d = to_xz(seg.curve.derivative(t)).normalize_or_zero();
    // Left of the direction in the ground plane (see `CubicBezier3::offset_xz`).
    p + Vec2::new(d.y, -d.x) * offset
}

/// Whether a bus stop may stand `s` metres along `seg` on `side`, given the stops that are
/// there already (`ignore` leaves one out: the stop being moved). Gives the pavement point.
pub fn check_road_stop(
    net: &NetGraph,
    db: &ContentDb,
    stops: &Arena<Stop>,
    seg: SegmentId,
    s: f32,
    side: Side,
    ignore: Option<StopId>,
) -> Result<Vec2, StopError> {
    let segment = net.segment(seg).ok_or(StopError::NoSuchSegment)?;
    if !s.is_finite() || s < 0.0 || s > segment.length {
        return Err(StopError::OffSegment);
    }
    if db.road(segment.road).class == RoadClass::Highway {
        return Err(StopError::Highway);
    }
    let offset = pavement_offset(db, segment, side).ok_or(StopError::NoPavement)?;
    if s < STOP_NODE_CLEARANCE || segment.length - s < STOP_NODE_CLEARANCE {
        return Err(StopError::NearNode);
    }
    for (id, other) in stops.iter() {
        if Some(id) == ignore || other.side != side {
            continue;
        }
        if let StopKind::Road { seg: o, s: os } = other.kind
            && o == seg
            && (os - s).abs() < STOP_SPACING
        {
            return Err(StopError::NearStop(id));
        }
    }
    Ok(pavement_point(segment, s, offset))
}
