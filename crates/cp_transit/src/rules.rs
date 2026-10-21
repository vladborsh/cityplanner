//! Where a stop may stand (§16 M3 A "Остановка на дороге"). Pure functions of the network and
//! the content, shared by the command and the tool's ghost.

use bevy_math::Vec2;
use cp_building::{BuildingGrid, BuildingState};
use cp_content::{
    ContentDb, LaneKind, RoadClass, ServiceKind, TransitKind, VehicleClass, lane_allows,
};
use cp_core::Arena;
use cp_core::curve::to_xz;
use cp_net::{Elevation, NetGraph, Segment, SegmentId};
use cp_traffic::Side;

use crate::model::{Stop, StopId, StopKind};

/// Metres a road stop keeps from either end of its segment.
pub const STOP_NODE_CLEARANCE: f32 = 12.0;
/// Metres between two stops of one side of a segment.
pub const STOP_SPACING: f32 = 24.0;
/// How far a stop may sit from where its side's pavement lies and still be re-attached to a
/// segment after the network changed.
pub const STOP_SNAP: f32 = 4.0;

/// How far a station's platform may lie from the station and still carry its stop after the
/// network changed (T-162).
pub const STATION_SNAP: f32 = 2.0;

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
    /// The road has no lane the line's vehicles may drive (T-302: a tram stop needs rails, a
    /// trolleybus stop wires, a bus stop a car or bus lane).
    NoLane,
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
            Self::NoLane => write!(f, "the road has no lane for this line's vehicles"),
        }
    }
}

/// Whether `segment` has a lane a vehicle of `class` may drive (T-302, `lane_allows`).
pub fn segment_has_lane_for(net: &NetGraph, segment: &Segment, class: VehicleClass) -> bool {
    segment
        .lanes
        .iter()
        .filter_map(|&l| net.lane(l))
        .any(|l| lane_allows(l.kind, class))
}

/// What a public transport mode needs before its line tool works (T-302, "station first"):
/// `ok` when it is there, else `missing` says what to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ready {
    pub mode: TransitKind,
    pub ok: bool,
    /// The English key of the hint (translated by the UI); empty when `ok`.
    pub missing: &'static str,
}

/// The readiness of every mode (`TransitKind::ALL` order): a bus line needs an active bus
/// depot; a tram line a tram depot and a road with rails; a trolleybus line a trolleybus
/// depot and a road with wires; a metro line a metro station and its track; a train line a
/// railway station and a railway. Pure over the content, the buildings and the graph; the
/// same rule gates `CreateLine` (`TransitError::NotReady`) and greys the tool's button.
pub fn prerequisites(db: &ContentDb, buildings: &BuildingGrid, net: &NetGraph) -> [Ready; 5] {
    let mut depot = [false; 5];
    let mut station = [false; 5];
    for (_, b) in buildings.iter() {
        // Standing or being built, like the demand counts them (§11).
        if !matches!(
            b.state,
            BuildingState::Active | BuildingState::UnderConstruction
        ) {
            continue;
        }
        let def = db.building(b.def);
        if let Some(track) = &def.track {
            let k = if track.depth > 0.0 {
                TransitKind::Metro
            } else {
                TransitKind::Train
            };
            station[k as usize] = true;
        } else if let Some(svc) = def
            .service
            .as_deref()
            .and_then(|s| db.service_id(s))
            .map(|s| db.service(s))
            && svc.kind == ServiceKind::Transit
        {
            depot[svc.transit_mode as usize] = true;
        }
    }
    let mut lane = [false; 5];
    for (_, seg) in net.segments() {
        for k in TransitKind::ALL {
            if !lane[k as usize] && segment_has_lane_for(net, seg, k.class()) {
                lane[k as usize] = true;
            }
        }
    }
    // Metro and train tracks are both `Rail` lanes: tell them apart by the elevation.
    let (mut metro_track, mut rail_track) = (false, false);
    for (_, seg) in net.segments() {
        if segment_has_lane_for(net, seg, VehicleClass::Rail) {
            if seg.elevation == Elevation::Tunnel {
                metro_track = true;
            } else {
                rail_track = true;
            }
        }
    }
    TransitKind::ALL.map(|k| {
        let (ok, missing) = match k {
            TransitKind::Bus => (depot[0], "Build a bus depot first"),
            TransitKind::Metro => match (station[1], metro_track) {
                (false, _) => (false, "Build a metro station first"),
                (true, false) => (false, "Lay a metro track first"),
                (true, true) => (true, ""),
            },
            TransitKind::Train => match (station[2], rail_track) {
                (false, _) => (false, "Build a railway station first"),
                (true, false) => (false, "Lay a railway first"),
                (true, true) => (true, ""),
            },
            TransitKind::Tram => match (depot[3], lane[3]) {
                (false, _) => (false, "Build a tram depot first"),
                (true, false) => (false, "Lay tram rails first"),
                (true, true) => (true, ""),
            },
            TransitKind::Trolley => match (depot[4], lane[4]) {
                (false, _) => (false, "Build a trolleybus depot first"),
                (true, false) => (false, "Build a road with trolleybus wires first"),
                (true, true) => (true, ""),
            },
        };
        Ready {
            mode: k,
            ok,
            missing: if ok { "" } else { missing },
        }
    })
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
#[allow(clippy::too_many_arguments)]
pub fn check_road_stop(
    net: &NetGraph,
    db: &ContentDb,
    stops: &Arena<Stop>,
    class: VehicleClass,
    seg: SegmentId,
    s: f32,
    side: Side,
    ignore: Option<StopId>,
) -> Result<Vec2, StopError> {
    let segment = net.segment(seg).ok_or(StopError::NoSuchSegment)?;
    if !s.is_finite() || s < 0.0 || s > segment.length {
        return Err(StopError::OffSegment);
    }
    if !segment_has_lane_for(net, segment, class) {
        return Err(StopError::NoLane);
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
