//! Parking bays along the kerbs (T-276; T-331: also the kerb places of the sim, §18.3): the kerb-side [`BAY_W`] of a sidewalk of a
//! local or collector street is drawn as an asphalt bay with a thin white line, the kerb moved to
//! its outer edge, between a margin from each end of the segment (a cell from a junction, a
//! little at the joint of two pieces of one street). Parked cars stand in it; lamps and poles
//! keep to the rest of the sidewalk. The lanes, the road's width, the cells and the lots are
//! the same.
//!
//! One function, [`bays`], says where the bays of a segment are; the road mesh, the street
//! furniture, the parked cars, the pedestrians' spread clear of the bays and the places of
//! `cp_traffic::ParkingIndex` all read it. A road has bays only when its def says
//! `kerb_parking` (T-331).

use cp_content::{ContentDb, LaneKind};

use crate::graph::NetGraph;
use crate::junction::JunctionGeometry;
use crate::model::{Elevation, NodeId, SegmentId};

/// Width of a parking bay, metres (a car is 1.8 m wide).
pub const BAY_W: f32 = 2.4;
/// Width of the white line on the carriageway side of a bay, metres.
pub const BAY_LINE_W: f32 = 0.12;
/// Length of one parking place, metres.
pub const SLOT_LEN: f32 = 6.0;
/// A bay ends this far from a junction or a dead end (a cell: the crossing and the corner stay
/// clear), metres.
const JUNCTION_MARGIN: f32 = 8.0;
/// ...and this far from the joint of two pieces of one street.
const JOINT_MARGIN: f32 = 2.0;
/// A sidewalk at least this wide gets a bay (the T-275 local and collector streets).
const MIN_SIDEWALK: f32 = 4.5;
/// Streets with more car lanes than this (arterials, highways) get none.
const MAX_CAR_LANES: usize = 4;

/// One side's bay: `kerb` is the lateral offset of the carriageway's edge (positive left of the
/// segment's direction), the bay runs from it outwards by `sign`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BaySide {
    pub kerb: f32,
    pub sign: f32,
}

impl BaySide {
    /// The bay's lateral range `(lo, hi)`.
    pub fn range(&self) -> (f32, f32) {
        let outer = self.kerb + self.sign * BAY_W;
        (self.kerb.min(outer), self.kerb.max(outer))
    }

    /// Lateral offset of a parked car's centre.
    pub fn car_offset(&self) -> f32 {
        self.kerb + self.sign * (BAY_LINE_W + (BAY_W - BAY_LINE_W) * 0.5)
    }
}

/// The bays of a segment: its sides with one, and the stretch `[a, b]` of the trimmed curve
/// (arc length) they run along.
#[derive(Clone, Debug, PartialEq)]
pub struct Bays {
    pub sides: Vec<BaySide>,
    pub a: f32,
    pub b: f32,
}

impl Bays {
    /// Whether arc length `s` is inside the bays' stretch.
    pub fn covers(&self, s: f32) -> bool {
        s > self.a && s < self.b
    }

    /// The parking places of the stretch: their centres (arc length), [`SLOT_LEN`] apart,
    /// centred in the stretch.
    pub fn slots(&self) -> Vec<f32> {
        let room = self.b - self.a;
        let n = (room / SLOT_LEN).floor() as usize;
        let first = self.a + (room - n as f32 * SLOT_LEN) * 0.5 + SLOT_LEN * 0.5;
        (0..n).map(|k| first + k as f32 * SLOT_LEN).collect()
    }
}

fn margin(geo: &JunctionGeometry, node: NodeId) -> f32 {
    if geo.get(node).is_some_and(|j| j.arms.len() == 2) {
        JOINT_MARGIN
    } else {
        JUNCTION_MARGIN
    }
}

/// Where the parking bays of segment `id` are, on the curve the road mesh draws
/// (`JunctionGeometry::trimmed_curve`). `None` for a street without them: not on the ground, a
/// footpath, a road without `kerb_parking`, more than [`MAX_CAR_LANES`] car lanes, no sidewalk of [`MIN_SIDEWALK`] beside the
/// carriageway, or too short for one parking place.
pub fn bays(
    graph: &NetGraph,
    geo: &JunctionGeometry,
    db: &ContentDb,
    id: SegmentId,
) -> Option<Bays> {
    let seg = graph.segment(id)?;
    if seg.elevation != Elevation::Ground
        || seg.road.index() >= db.roads().count()
        || !db.road(seg.road).kerb_parking
        || is_path(graph, seg)
    {
        return None;
    }
    let lanes: Vec<_> = seg.lanes.iter().filter_map(|&l| graph.lane(l)).collect();
    let cars: Vec<_> = lanes.iter().filter(|l| l.kind.is_road_vehicle()).collect();
    if cars.is_empty() || cars.len() > MAX_CAR_LANES {
        return None;
    }
    let lo = cars
        .iter()
        .map(|l| l.offset - l.width * 0.5)
        .fold(f32::MAX, f32::min);
    let hi = cars
        .iter()
        .map(|l| l.offset + l.width * 0.5)
        .fold(f32::MIN, f32::max);
    let mut sides = Vec::new();
    for l in lanes
        .iter()
        .filter(|l| l.kind == LaneKind::Pedestrian && l.width >= MIN_SIDEWALK - 1e-3)
    {
        let (pl, ph) = (l.offset - l.width * 0.5, l.offset + l.width * 0.5);
        if (pl - hi).abs() < 1e-3 {
            sides.push(BaySide {
                kerb: hi,
                sign: 1.0,
            });
        } else if (ph - lo).abs() < 1e-3 {
            sides.push(BaySide {
                kerb: lo,
                sign: -1.0,
            });
        }
    }
    if sides.is_empty() {
        return None;
    }
    sides.sort_by(|a, b| a.sign.total_cmp(&b.sign));
    let length = geo.trimmed_curve(graph, id)?.arc_table().length();
    let (a, b) = (margin(geo, seg.start), length - margin(geo, seg.end));
    (b - a >= SLOT_LEN).then_some(Bays { sides, a, b })
}

/// Width of a driveway across the sidewalk, metres.
pub const DRIVEWAY_W: f32 = 3.0;
/// A parking place whose centre is closer than this to a yard gate overlaps the 3 m driveway
/// (half a place plus half the driveway), metres.
pub const GATE_CLEAR: f32 = SLOT_LEN * 0.5 + DRIVEWAY_W * 0.5;

fn is_path(graph: &NetGraph, seg: &crate::model::Segment) -> bool {
    let mut lanes = seg.lanes.iter().filter_map(|&l| graph.lane(l)).peekable();
    lanes.peek().is_some() && lanes.all(|l| l.kind == LaneKind::Pedestrian)
}
