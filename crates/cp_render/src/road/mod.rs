//! Pure road mesh generation (no Bevy types). See `architecture/system-design.md` §13.3.
//!
//! * A **segment piece** extrudes the road profile (the segment's lanes) along the segment
//!   curve *cut at its junctions* (`JunctionGeometry::trimmed_curve`): rings every 4 m on
//!   curves, one ring pair on straights. Lane markings are baked into the surface as extra
//!   bands (and on/off dash quads) of vertex colour, so no two surfaces overlap and nothing
//!   z-fights. Sidewalks are raised bands with kerb faces. Ground segments get skirts down
//!   to the terrain; elevated and bridge segments get an underside, fascias and piers.
//! * A **junction piece** is the junction polygon, triangulated. Its arm corners are *the
//!   same points* the neighbouring segments use for their end rings, so there are no gaps.
//! * One-way segments (every car lane runs forward) get direction **chevrons** on the
//!   carriageway axis every [`CHEVRON_SPACING`] metres, baked in as vertex-coloured
//!   triangles that replace the asphalt under them (no overlay). Near mesh only.
//! * Every piece has a `near` mesh (markings, kerbs, chevrons) and a `far` mesh (none).
//! * [`RoadSet`] keeps the pieces and, given a new snapshot, rebuilds only pieces whose
//!   inputs changed (segment fingerprint / junction revision).
//!
//! Positions are world space. Surfaces sit [`LIFT`] above the curve to stay above the
//! terrain mesh.

use std::collections::{BTreeMap, BTreeSet};

use bevy_math::{Vec2, Vec3};
use cp_content::{LaneDir, LaneKind};
use cp_core::curve::{CubicBezier3, to_xz};
use cp_net::{
    Arm, Elevation, Junction, JunctionGeometry, NetGraph, NodeId, Segment, SegmentId,
    pier_positions,
};
use cp_terrain::Terrain;

use crate::chunk::MeshData;

/// Ring spacing on curved segments, metres.
pub const STEP: f32 = 4.0;
/// Road surface height above the curve, metres.
pub const LIFT: f32 = 0.06;
/// Sidewalk height above the road, metres.
pub const CURB: f32 = 0.15;
/// Deck thickness of elevated and bridge pieces, metres.
pub const DECK: f32 = 1.0;
/// Half side of a pier, metres.
pub const PIER_HALF: f32 = 1.0;
/// Dash pattern of lane dividers: painted length and period, metres.
pub const DASH_ON: f32 = 3.0;
pub const DASH_PERIOD: f32 = 12.0;
/// Chunk side used to batch pieces into meshes, metres.
pub const ROAD_CHUNK: f32 = 256.0;
/// Distance between direction chevrons on a one-way segment, metres.
pub const CHEVRON_SPACING: f32 = 24.0;
/// Length of a chevron along the road, metres.
const CHEVRON_LEN: f32 = 3.2;
/// Half width of a chevron, metres, at most (also capped at 70% of the carriageway half).
const CHEVRON_HALF: f32 = 1.4;
/// Thickness of a chevron arm along the road, as a fraction of [`CHEVRON_LEN`].
const CHEVRON_ARM: f32 = 0.15;

const ASPHALT: [f32; 4] = [0.11, 0.11, 0.12, 1.0];
const SIDEWALK: [f32; 4] = [0.55, 0.55, 0.52, 1.0];
const KERB: [f32; 4] = [0.42, 0.42, 0.40, 1.0];
const BIKE: [f32; 4] = [0.10, 0.30, 0.16, 1.0];
const BUS: [f32; 4] = [0.30, 0.10, 0.10, 1.0];
const TRAM: [f32; 4] = [0.20, 0.20, 0.21, 1.0];
const MEDIAN: [f32; 4] = [0.30, 0.34, 0.26, 1.0];
const WHITE: [f32; 4] = [0.80, 0.80, 0.78, 1.0];
const YELLOW: [f32; 4] = [0.80, 0.62, 0.12, 1.0];
/// Direction chevrons on one-way roads (paler than the lane lines).
pub const CHEVRON: [f32; 4] = [0.62, 0.62, 0.60, 1.0];
const CONCRETE: [f32; 4] = [0.48, 0.48, 0.47, 1.0];
const EMBANKMENT: [f32; 4] = [0.28, 0.24, 0.17, 1.0];

/// A piece of the road network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PieceKey {
    Segment(SegmentId),
    Junction(NodeId),
}

/// Meshes of one piece.
#[derive(Clone, Debug, Default)]
pub struct Piece {
    /// With markings and kerbs.
    pub near: MeshData,
    /// Without markings (distant view).
    pub far: MeshData,
    pub min: Vec3,
    pub max: Vec3,
}

impl Piece {
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
}

// ---------------------------------------------------------------------------------------
// Mesh helpers

pub(crate) fn push_vertex(m: &mut MeshData, p: Vec3, n: Vec3, c: [f32; 4]) -> u32 {
    m.positions.push(p.to_array());
    m.normals.push(n.to_array());
    m.colors.push(c);
    (m.positions.len() - 1) as u32
}

/// Adds a triangle wound so that its geometric normal faces `facing`.
pub(crate) fn tri(m: &mut MeshData, a: u32, b: u32, c: u32, facing: Vec3) {
    let p = |i: u32| Vec3::from_array(m.positions[i as usize]);
    let n = (p(b) - p(a)).cross(p(c) - p(a));
    if n.dot(facing) >= 0.0 {
        m.indices.extend_from_slice(&[a, b, c]);
    } else {
        m.indices.extend_from_slice(&[a, c, b]);
    }
}

/// Quad `a b c d` in order around its boundary, facing `facing`.
pub(crate) fn quad(m: &mut MeshData, v: [u32; 4], facing: Vec3) {
    tri(m, v[0], v[1], v[2], facing);
    tri(m, v[0], v[2], v[3], facing);
}

/// A flat-coloured quad with its own vertices.
pub(crate) fn flat_quad(m: &mut MeshData, p: [Vec3; 4], n: Vec3, c: [f32; 4]) {
    let v = p.map(|p| push_vertex(m, p, n, c));
    quad(m, v, n);
}

pub(crate) fn append(dst: &mut MeshData, src: &MeshData) {
    let base = dst.positions.len() as u32;
    dst.positions.extend_from_slice(&src.positions);
    dst.normals.extend_from_slice(&src.normals);
    dst.colors.extend_from_slice(&src.colors);
    dst.indices.extend(src.indices.iter().map(|i| i + base));
}

pub(crate) fn bounds(m: &MeshData) -> (Vec3, Vec3) {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &m.positions {
        let p = Vec3::from_array(*p);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    (lo, hi)
}

fn finish(near: MeshData, far: MeshData) -> Piece {
    let (min, max) = bounds(&near);
    Piece {
        near,
        far,
        min,
        max,
    }
}

// ---------------------------------------------------------------------------------------
// Cross-section

#[derive(Clone, Copy, Debug)]
struct Band {
    lo: f32,
    hi: f32,
    color: [f32; 4],
    raise: f32,
    /// Alternates `color` and the road colour along the segment.
    dashed: bool,
}

fn lane_color(kind: LaneKind) -> ([f32; 4], f32) {
    match kind {
        LaneKind::Car => (ASPHALT, 0.0),
        LaneKind::Pedestrian => (SIDEWALK, CURB),
        LaneKind::Bike => (BIKE, 0.0),
        LaneKind::Tram => (TRAM, 0.0),
        LaneKind::Bus => (BUS, 0.0),
        LaneKind::Rail => (TRAM, 0.0),
    }
}

fn is_vehicle(kind: LaneKind) -> bool {
    matches!(kind, LaneKind::Car | LaneKind::Bus)
}

/// Half width of the road, from the lanes (what `cp_net::junction` uses for its arms).
fn half_width(graph: &NetGraph, seg: &Segment) -> f32 {
    let half = seg
        .lanes
        .iter()
        .filter_map(|&l| graph.lane(l))
        .map(|l| l.offset.abs() + l.width * 0.5)
        .fold(0.0f32, f32::max);
    if half > 0.0 { half } else { 4.0 }
}

/// Bands from right (most negative lateral) to left. `markings` splits lane borders into
/// painted lines.
fn cross_section(graph: &NetGraph, seg: &Segment, markings: bool) -> Vec<Band> {
    let mut lanes: Vec<_> = seg.lanes.iter().filter_map(|&l| graph.lane(l)).collect();
    lanes.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    if lanes.is_empty() {
        return vec![Band {
            lo: -4.0,
            hi: 4.0,
            color: ASPHALT,
            raise: 0.0,
            dashed: false,
        }];
    }
    let mut bands: Vec<Band> = Vec::new();
    for (i, l) in lanes.iter().enumerate() {
        let (color, raise) = lane_color(l.kind);
        let (lo, hi) = (l.offset - l.width * 0.5, l.offset + l.width * 0.5);
        if let Some(prev) = bands.last().copied() {
            if lo - prev.hi > 1e-3 {
                bands.push(Band {
                    lo: prev.hi,
                    hi: lo,
                    color: MEDIAN,
                    raise: CURB,
                    dashed: false,
                });
            }
            let prev_lane = lanes[i - 1];
            if markings && is_vehicle(prev_lane.kind) && is_vehicle(l.kind) && lo - prev.hi <= 1e-3
            {
                let (mw, color, dashed) = if prev_lane.dir != l.dir {
                    (0.10, YELLOW, false)
                } else {
                    (0.06, WHITE, true)
                };
                let x = (prev.hi + lo) * 0.5;
                bands.last_mut().expect("prev").hi = x - mw;
                bands.push(Band {
                    lo: x - mw,
                    hi: x + mw,
                    color,
                    raise: 0.0,
                    dashed,
                });
                bands.push(Band {
                    lo: x + mw,
                    hi,
                    color: lane_color(l.kind).0,
                    raise,
                    dashed: false,
                });
                continue;
            }
        }
        bands.push(Band {
            lo,
            hi,
            color,
            raise,
            dashed: false,
        });
    }
    bands
}

// ---------------------------------------------------------------------------------------
// Rings

/// A cross-section line: where lateral offset `x` lands in the world.
#[derive(Clone, Copy, Debug)]
struct Ring {
    /// Centre point on the curve (with [`LIFT`] applied to y).
    center: Vec3,
    /// Unit left direction in XZ (lateral +).
    left: Vec2,
    /// For end rings: the exact world XZ of the right and left outer corners.
    corners: Option<(Vec2, Vec2)>,
    half: f32,
}

impl Ring {
    fn xz(&self, x: f32) -> Vec2 {
        match self.corners {
            Some((r, l)) => r + (l - r) * ((x + self.half) / (2.0 * self.half)),
            None => to_xz(self.center) + self.left * x,
        }
    }
    fn at(&self, x: f32, raise: f32) -> Vec3 {
        let p = self.xz(x);
        Vec3::new(p.x, self.center.y + raise, p.y)
    }
}

fn left_of(tangent: Vec3) -> Vec2 {
    // Same convention as the lane offsets in `cp_net` (positive offset = left of travel).
    Vec2::new(tangent.z, -tangent.x)
        .try_normalize()
        .unwrap_or(Vec2::Y)
}

/// The arm of `segment` at its start or end node.
fn arm_of(geo: &JunctionGeometry, node: NodeId, seg: SegmentId, at_start: bool) -> Option<&Arm> {
    geo.get(node)?
        .arms
        .iter()
        .find(|a| a.segment == seg && a.at_start == at_start)
}

/// World XZ of the corners of the segment's end, `(right, left)` in the segment's own
/// travel direction. `Arm::left/right` are named for the outward direction in the
/// counter-clockwise (x, z) rotation, which is the mirror of the lane-offset convention.
fn end_corners(arm: &Arm) -> (Vec2, Vec2) {
    if arm.at_start {
        // Outward = travel direction: the arm's `right` is on the lane-offset left.
        (arm.left, arm.right)
    } else {
        (arm.right, arm.left)
    }
}

struct Frame<'a> {
    curve: CubicBezier3,
    table: cp_core::curve::ArcTable,
    length: f32,
    half: f32,
    start: Option<&'a Arm>,
    end: Option<&'a Arm>,
}

impl Frame<'_> {
    fn ring(&self, s: f32) -> Ring {
        let t = self.table.s_to_t(s);
        let mut p = self.curve.point(t);
        p.y += LIFT;
        let corners = if s <= 1e-4 {
            self.start.map(end_corners)
        } else if s >= self.length - 1e-4 {
            self.end.map(end_corners)
        } else {
            None
        };
        Ring {
            center: p,
            left: left_of(self.curve.tangent(t)),
            corners,
            half: self.half,
        }
    }
}

fn is_straight(c: &CubicBezier3) -> bool {
    let d = c.p3 - c.p0;
    let len = d.length();
    if len < 1e-3 {
        return true;
    }
    let dir = d / len;
    [c.p1, c.p2]
        .iter()
        .all(|p| (*p - c.p0).cross(dir).length() < 0.01)
}

/// Every car lane of the segment runs forward, and there is one (`RoadDef::is_one_way`,
/// read from the segment's lanes so the mesh needs no content).
fn is_one_way(graph: &NetGraph, seg: &Segment) -> bool {
    let mut cars = seg
        .lanes
        .iter()
        .filter_map(|&l| graph.lane(l))
        .filter(|l| l.kind == LaneKind::Car)
        .peekable();
    cars.peek().is_some() && cars.all(|l| l.dir == LaneDir::Forward)
}

/// The lateral extent `(lo, hi)` of the car lanes, when everything between is plain
/// asphalt or a white lane line at road level (so chevrons can replace it).
fn carriageway(graph: &NetGraph, seg: &Segment, bands: &[Band]) -> Option<(f32, f32)> {
    let cars = || {
        seg.lanes
            .iter()
            .filter_map(|&l| graph.lane(l))
            .filter(|l| l.kind == LaneKind::Car)
    };
    let lo = cars()
        .map(|l| l.offset - l.width * 0.5)
        .fold(f32::MAX, f32::min);
    let hi = cars()
        .map(|l| l.offset + l.width * 0.5)
        .fold(f32::MIN, f32::max);
    let inside = bands.iter().filter(|b| in_range(b, lo, hi));
    inside
        .into_iter()
        .all(|b| b.raise == 0.0 && (b.color == ASPHALT || b.color == WHITE))
        .then_some((lo, hi))
}

fn in_range(b: &Band, lo: f32, hi: f32) -> bool {
    b.lo >= lo - 1e-3 && b.hi <= hi + 1e-3
}

/// The arc-length intervals of the chevrons of a segment of `length` metres: one centred in
/// each [`CHEVRON_SPACING`] cell, `floor(length / 24)` of them.
pub fn chevron_rows(length: f32) -> Vec<(f32, f32)> {
    let n = (length / CHEVRON_SPACING).floor().max(0.0) as usize;
    (0..n)
        .map(|i| {
            let c = (i as f32 + 0.5) * CHEVRON_SPACING;
            (c - CHEVRON_LEN * 0.5, c + CHEVRON_LEN * 0.5)
        })
        .collect()
}

/// Arc-length positions of the rings: ends, curve steps, dash boundaries, and the two
/// boundaries of every chevron row (which displace other rings within it).
fn ring_positions(length: f32, straight: bool, dashes: bool, chevrons: &[(f32, f32)]) -> Vec<f32> {
    let mut s = vec![0.0, length];
    if !straight {
        let n = (length / STEP).ceil().max(1.0) as usize;
        s.extend((1..n).map(|i| length * i as f32 / n as f32));
    }
    if dashes {
        let mut d = DASH_PERIOD * 0.5;
        while d < length - 0.5 {
            s.push(d);
            if d + DASH_ON < length - 0.5 {
                s.push(d + DASH_ON);
            }
            d += DASH_PERIOD;
        }
    }
    s.sort_by(f32::total_cmp);
    s.dedup_by(|a, b| (*a - *b).abs() < 0.25);
    // Keep the exact end value.
    if let Some(last) = s.last_mut() {
        *last = length;
    }
    if !chevrons.is_empty() {
        s.retain(|&x| {
            x <= 1e-4
                || x >= length - 1e-4
                || chevrons.iter().all(|&(a, b)| x < a - 0.25 || x > b + 0.25)
        });
        s.extend(chevrons.iter().flat_map(|&(a, b)| [a, b]));
        s.sort_by(f32::total_cmp);
    }
    s
}

/// Whether the dash band is painted between `s0` and `s1`.
fn dash_on(s0: f32, s1: f32) -> bool {
    let mid = (s0 + s1) * 0.5;
    ((mid - DASH_PERIOD * 0.5) % DASH_PERIOD + DASH_PERIOD) % DASH_PERIOD < DASH_ON
}

/// Skirt depth below `y`, so the terrain never shows through under the road edge.
fn skirt_depth(terrain: &Terrain, p: Vec2, y: f32) -> f32 {
    (y - terrain.height(p) + 0.4).clamp(0.4, 8.0)
}

fn extrude(
    graph: &NetGraph,
    geo: &JunctionGeometry,
    terrain: &Terrain,
    id: SegmentId,
    markings: bool,
) -> Option<MeshData> {
    let seg = graph.segment(id)?;
    if seg.elevation == Elevation::Tunnel {
        return None;
    }
    let curve = geo.trimmed_curve(graph, id)?;
    let table = curve.arc_table();
    let length = table.length();
    if length < 0.05 {
        return Some(MeshData::default());
    }
    let frame = Frame {
        curve,
        table,
        length,
        half: half_width(graph, seg),
        start: arm_of(geo, seg.start, id, true),
        end: arm_of(geo, seg.end, id, false),
    };
    let bands = cross_section(graph, seg, markings);
    let has_dashes = bands.iter().any(|b| b.dashed);
    // Chevrons (near mesh, one-way roads): `(lo, hi)` is the carriageway they replace.
    let chevron = (markings && is_one_way(graph, seg))
        .then(|| carriageway(graph, seg, &bands))
        .flatten();
    let chevrons = chevron.map_or_else(Vec::new, |_| chevron_rows(length));
    let base = ring_positions(length, is_straight(&curve), has_dashes, &chevrons);
    let (s_list, rings) = ground::rings(&frame, graph, geo, terrain, seg, base);
    // Ring pairs `k - 1, k` that hold a chevron.
    let chevron_row = |k: usize| {
        k > 0
            && chevrons
                .iter()
                .any(|&(a, b)| (s_list[k - 1] - a).abs() < 1e-4 && (s_list[k] - b).abs() < 1e-4)
    };
    let up = Vec3::Y;
    let mut m = MeshData::default();

    // Surface bands.
    for b in &bands {
        let mut prev: Option<(u32, u32)> = None;
        // Under a chevron the carriageway is drawn by `chevron_quads` instead.
        let replaced = chevron.is_some_and(|(lo, hi)| in_range(b, lo, hi));
        for (k, r) in rings.iter().enumerate() {
            let (lo, hi) = (r.at(b.lo, b.raise), r.at(b.hi, b.raise));
            let skip = replaced && chevron_row(k);
            if b.dashed {
                if k == 0 || skip {
                    continue;
                }
                let on = dash_on(s_list[k - 1], s_list[k]);
                let color = if on { b.color } else { ASPHALT };
                let q = &rings[k - 1];
                flat_quad(
                    &mut m,
                    [q.at(b.lo, b.raise), q.at(b.hi, b.raise), hi, lo],
                    up,
                    color,
                );
            } else {
                let v = (
                    push_vertex(&mut m, lo, up, b.color),
                    push_vertex(&mut m, hi, up, b.color),
                );
                if let Some(p) = prev
                    && !skip
                {
                    quad(&mut m, [p.0, p.1, v.1, v.0], up);
                }
                prev = Some(v);
            }
        }
    }
    if let Some((lo, hi)) = chevron {
        for k in (1..rings.len()).filter(|&k| chevron_row(k)) {
            chevron_quads(&mut m, &rings[k - 1], &rings[k], lo, hi);
        }
    }

    // Kerb faces where neighbouring bands differ in height.
    if markings {
        for pair in bands.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if (a.raise - b.raise).abs() < 1e-3 {
                continue;
            }
            let (low, high) = if a.raise < b.raise { (a, b) } else { (b, a) };
            let x = a.hi;
            // Face towards the lower band.
            let towards = if a.raise < b.raise { -1.0 } else { 1.0 };
            for w in rings.windows(2) {
                let n3 = |r: &Ring| {
                    let l = r.left * towards;
                    Vec3::new(l.x, 0.0, l.y)
                };
                let nrm = n3(&w[0]);
                flat_quad(
                    &mut m,
                    [
                        w[0].at(x, low.raise),
                        w[1].at(x, low.raise),
                        w[1].at(x, high.raise),
                        w[0].at(x, high.raise),
                    ],
                    nrm,
                    KERB,
                );
            }
        }
    }

    // End caps of raised bands (sidewalks): they meet the flat junction polygon.
    if markings {
        for (ring, sign) in [(rings.first(), -1.0f32), (rings.last(), 1.0)] {
            let Some(r) = ring else { continue };
            let tangent = Vec2::new(-r.left.y, r.left.x) * sign;
            let n3 = Vec3::new(tangent.x, 0.0, tangent.y);
            for b in bands.iter().filter(|b| b.raise > 1e-3) {
                flat_quad(
                    &mut m,
                    [
                        r.at(b.lo, 0.0),
                        r.at(b.hi, 0.0),
                        r.at(b.hi, b.raise),
                        r.at(b.lo, b.raise),
                    ],
                    n3,
                    KERB,
                );
            }
        }
    }

    // Edges: skirts to the terrain, or fascias and underside for elevated pieces.
    let (first, last) = (bands.first().expect("bands"), bands.last().expect("bands"));
    let raised = seg.elevation != Elevation::Ground;
    for (x, raise, side) in [(first.lo, first.raise, -1.0f32), (last.hi, last.raise, 1.0)] {
        for w in rings.windows(2) {
            let (a, b) = (w[0].at(x, raise), w[1].at(x, raise));
            let (da, db) = if raised {
                (DECK + raise, DECK + raise)
            } else {
                (
                    skirt_depth(terrain, Vec2::new(a.x, a.z), a.y),
                    skirt_depth(terrain, Vec2::new(b.x, b.z), b.y),
                )
            };
            let l = w[0].left * side;
            flat_quad(
                &mut m,
                [a, b, b - Vec3::Y * db, a - Vec3::Y * da],
                Vec3::new(l.x, 0.0, l.y),
                if raised { CONCRETE } else { EMBANKMENT },
            );
        }
    }
    if raised {
        let mut prev = None::<(u32, u32)>;
        for r in &rings {
            let (a, b) = (r.at(first.lo, 0.0), r.at(last.hi, 0.0));
            let v = (
                push_vertex(&mut m, a - Vec3::Y * DECK, -Vec3::Y, CONCRETE),
                push_vertex(&mut m, b - Vec3::Y * DECK, -Vec3::Y, CONCRETE),
            );
            if let Some(p) = prev {
                quad(&mut m, [p.0, p.1, v.1, v.0], -Vec3::Y);
            }
            prev = Some(v);
        }
        piers(&mut m, terrain, seg);
    }
    Some(m)
}

fn flat_tri(m: &mut MeshData, p: [Vec3; 3], c: [f32; 4]) {
    let v = p.map(|p| push_vertex(m, p, Vec3::Y, c));
    tri(m, v[0], v[1], v[2], Vec3::Y);
}

/// The carriageway `[lo, hi]` between rings `a` and `b`, as asphalt with one chevron (an
/// arrowhead pointing along the road) cut out of it: the chevron is four triangles of
/// [`CHEVRON`] colour, the rest of the rectangle is tiled around it, nothing overlaps.
fn chevron_quads(m: &mut MeshData, a: &Ring, b: &Ring, lo: f32, hi: f32) {
    let c = (lo + hi) * 0.5;
    let w = CHEVRON_HALF.min((hi - lo) * 0.5 * 0.7);
    let th = CHEVRON_ARM;
    // A point at lateral `x`, `t` (0..1) of the way from ring `a` to ring `b`.
    let at = |x: f32, t: f32| a.at(x, 0.0).lerp(b.at(x, 0.0), t);
    let (q0, q1, q2, q3) = (at(lo, 0.0), at(hi, 0.0), at(hi, 1.0), at(lo, 1.0));
    // Chevron outline: outer edge p0 - p1 - p2 (tip p1), inner edge p5 - p4 - p3.
    let (p0, p1, p2) = (at(c - w, th), at(c, 1.0), at(c + w, th));
    let (p3, p4, p5) = (at(c + w, 0.0), at(c, 1.0 - th), at(c - w, 0.0));
    for t in [[p5, p0, p1], [p5, p1, p4], [p4, p1, p2], [p4, p2, p3]] {
        flat_tri(m, t, CHEVRON);
    }
    for t in [
        // Under the arrowhead.
        [p5, p3, p4],
        // Left of it (fan from the lower left corner).
        [q0, p5, p0],
        [q0, p0, p1],
        [q0, p1, q3],
        // Right of it.
        [q1, q2, p1],
        [q1, p1, p2],
        [q1, p2, p3],
    ] {
        flat_tri(m, t, ASPHALT);
    }
}

/// Square columns from the underside of the deck down to the terrain.
fn piers(m: &mut MeshData, terrain: &Terrain, seg: &Segment) {
    for p in pier_positions(seg) {
        let top = p.y + LIFT - DECK;
        let bottom = terrain.height(to_xz(p));
        if top - bottom < 0.5 {
            continue;
        }
        pier(m, Vec3::new(p.x, bottom, p.z), top);
    }
}

/// An axis-aligned square column from `base` up to height `top`.
fn pier(m: &mut MeshData, base: Vec3, top: f32) {
    let h = PIER_HALF;
    let c = [(-h, -h), (h, -h), (h, h), (-h, h)];
    for k in 0..4 {
        let (a, b) = (c[k], c[(k + 1) % 4]);
        let n = Vec3::new((a.0 + b.0) * 0.5, 0.0, (a.1 + b.1) * 0.5).normalize();
        flat_quad(
            m,
            [
                base + Vec3::new(a.0, 0.0, a.1),
                base + Vec3::new(b.0, 0.0, b.1),
                Vec3::new(base.x + b.0, top, base.z + b.1),
                Vec3::new(base.x + a.0, top, base.z + a.1),
            ],
            n,
            CONCRETE,
        );
    }
}

/// Builds the mesh pair of one segment. `None` for unknown or tunnel segments.
pub fn build_segment_piece(
    graph: &NetGraph,
    geo: &JunctionGeometry,
    terrain: &Terrain,
    id: SegmentId,
) -> Option<Piece> {
    let near = extrude(graph, geo, terrain, id, true)?;
    let far = extrude(graph, geo, terrain, id, false)?;
    Some(finish(near, far))
}

// ---------------------------------------------------------------------------------------
// Junctions

/// `y` of the end of `arm`'s segment (curve height at the cut).
fn arm_y(graph: &NetGraph, arm: &Arm) -> f32 {
    graph
        .segment(arm.segment)
        .map_or(0.0, |s| s.curve.point(arm.trim_t).y)
        + LIFT
}

/// Ear-clipping triangulation of a simple polygon; returns index triples.
fn triangulate(poly: &[Vec2]) -> Vec<[usize; 3]> {
    let n = poly.len();
    if n < 3 {
        return Vec::new();
    }
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let mut idx: Vec<usize> = (0..n).collect();
    if area < 0.0 {
        idx.reverse();
    }
    let cross = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - a);
    let inside = |p: Vec2, a: Vec2, b: Vec2, c: Vec2| {
        cross(a, b, p) >= -1e-6 && cross(b, c, p) >= -1e-6 && cross(c, a, p) >= -1e-6
    };
    let mut out = Vec::new();
    let mut guard = 0;
    while idx.len() > 3 && guard < 10_000 {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            if cross(a, b, c) <= 1e-6 {
                continue;
            }
            let ear_free = idx
                .iter()
                .filter(|&&j| j != ia && j != ib && j != ic)
                .all(|&j| !inside(poly[j], a, b, c));
            if ear_free {
                out.push([ia, ib, ic]);
                idx.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            // Degenerate input: fan the rest rather than loop.
            for k in 1..idx.len() - 1 {
                out.push([idx[0], idx[k], idx[k + 1]]);
            }
            return out;
        }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    }
    out
}

/// Height of each polygon vertex: the end height of the arm whose corner it is, else the
/// node's.
fn polygon_heights(graph: &NetGraph, j: &Junction) -> Vec<f32> {
    j.polygon
        .iter()
        .map(|p| {
            j.arms
                .iter()
                .find(|a| a.right.distance(*p) < 1e-4 || a.left.distance(*p) < 1e-4)
                .map_or(j.pos.y + LIFT, |a| arm_y(graph, a))
        })
        .collect()
}

/// Builds the mesh of one junction. `None` for nodes without a polygon.
pub fn build_junction_piece(
    graph: &NetGraph,
    geo: &JunctionGeometry,
    terrain: &Terrain,
    node: NodeId,
) -> Option<Piece> {
    let j = geo.get(node)?;
    if j.polygon.len() < 3 {
        return None;
    }
    let n = graph.node(node)?;
    let raised = n.elevation != Elevation::Ground;
    if n.elevation == Elevation::Tunnel {
        return None;
    }
    let ys = ground::heights(graph, geo, terrain, j);
    let mut m = MeshData::default();
    let verts: Vec<u32> = j
        .polygon
        .iter()
        .zip(&ys)
        .map(|(p, &y)| push_vertex(&mut m, Vec3::new(p.x, y, p.y), Vec3::Y, ASPHALT))
        .collect();
    for [a, b, c] in triangulate(&j.polygon) {
        tri(&mut m, verts[a], verts[b], verts[c], Vec3::Y);
    }
    // The pavement around the outside of the junction joins the arms' sidewalks.
    let walks = corner::walks(graph, j);
    let mut kerb = MeshData::default();
    corner::build(j, &walks, &ys, &mut m, &mut kerb);
    corner::build_patches(&corner::patches(graph, j), &ys, &mut m, &mut kerb);

    // Edges between corners of *different* arms are the outside of the junction: skirt them.
    let k = j.polygon.len();
    let corner_of = |p: Vec2| {
        j.arms
            .iter()
            .position(|a| a.right.distance(p) < 1e-4 || a.left.distance(p) < 1e-4)
    };
    for i in 0..k {
        let (pa, pb) = (j.polygon[i], j.polygon[(i + 1) % k]);
        if let (Some(a), Some(b)) = (corner_of(pa), corner_of(pb))
            && a == b
        {
            continue; // the end line of an arm: the segment's ring is attached here
        }
        let (ya, yb) = (ys[i], ys[(i + 1) % k]);
        // Under a sidewalk the wall starts at its top.
        let lift = if walks.iter().any(|w| w.from == i) {
            CURB
        } else {
            0.0
        };
        let (a, b) = (
            Vec3::new(pa.x, ya + lift, pa.y),
            Vec3::new(pb.x, yb + lift, pb.y),
        );
        let out = (pb - pa).perp().try_normalize().unwrap_or(Vec2::Y);
        // Polygon is counter-clockwise in (x, z): the right-hand perpendicular points out.
        let outward = Vec3::new(-out.x, 0.0, -out.y);
        let (da, db) = if raised {
            (DECK + lift, DECK + lift)
        } else {
            (
                skirt_depth(terrain, pa, ya) + lift,
                skirt_depth(terrain, pb, yb) + lift,
            )
        };
        let v = [a, b, b - Vec3::Y * db, a - Vec3::Y * da].map(|p| {
            push_vertex(
                &mut m,
                p,
                outward,
                if raised { CONCRETE } else { EMBANKMENT },
            )
        });
        quad(&mut m, v, outward);
    }
    if raised {
        // Underside and a pier below the node.
        let bottom: Vec<u32> = j
            .polygon
            .iter()
            .zip(&ys)
            .map(|(p, &y)| push_vertex(&mut m, Vec3::new(p.x, y - DECK, p.y), -Vec3::Y, CONCRETE))
            .collect();
        for [a, b, c] in triangulate(&j.polygon) {
            tri(&mut m, bottom[a], bottom[b], bottom[c], -Vec3::Y);
        }
        let ground = terrain.height(to_xz(n.pos));
        let top = n.pos.y + LIFT - DECK;
        if top - ground > 0.5 {
            pier(&mut m, Vec3::new(n.pos.x, ground, n.pos.z), top);
        }
    }
    let far = m.clone();
    append(&mut m, &kerb);
    Some(finish(m, far))
}

// ---------------------------------------------------------------------------------------
// Incremental set

fn fnv(h: &mut u64, bits: u32) {
    for b in bits.to_le_bytes() {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn fv(h: &mut u64, v: Vec3) {
    for c in v.to_array() {
        fnv(h, c.to_bits());
    }
}

/// Hash of everything a segment's mesh depends on except the (static) terrain.
pub fn segment_fingerprint(graph: &NetGraph, geo: &JunctionGeometry, id: SegmentId) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let Some(seg) = graph.segment(id) else {
        return h;
    };
    for p in [seg.curve.p0, seg.curve.p1, seg.curve.p2, seg.curve.p3] {
        fv(&mut h, p);
    }
    fnv(&mut h, seg.elevation as u32);
    for &l in &seg.lanes {
        if let Some(l) = graph.lane(l) {
            fnv(&mut h, l.kind as u32);
            fnv(&mut h, l.dir as u32);
            fnv(&mut h, l.offset.to_bits());
            fnv(&mut h, l.width.to_bits());
        }
    }
    for (node, at_start) in [(seg.start, true), (seg.end, false)] {
        // The ground level at a junction depends on all its arms.
        fnv(&mut h, geo.get(node).map_or(0, |j| j.revision as u32));
        match arm_of(geo, node, id, at_start) {
            Some(a) => {
                fnv(&mut h, a.trim_t.to_bits());
                for v in [a.right, a.left] {
                    fnv(&mut h, v.x.to_bits());
                    fnv(&mut h, v.y.to_bits());
                }
            }
            None => fnv(&mut h, 0xdead),
        }
    }
    h
}

/// Grid cell of a point.
pub fn chunk_of(p: Vec3) -> (i32, i32) {
    (
        (p.x / ROAD_CHUNK).floor() as i32,
        (p.z / ROAD_CHUNK).floor() as i32,
    )
}

struct Entry {
    /// Segment fingerprint or junction revision.
    stamp: u64,
    chunk: (i32, i32),
    piece: Piece,
}

/// What a [`RoadSet::update`] changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Update {
    /// Pieces that were (re)built.
    pub rebuilt: usize,
    /// Pieces that disappeared.
    pub removed: usize,
    /// Chunks whose merged meshes must be regenerated.
    pub dirty_chunks: BTreeSet<(i32, i32)>,
}

/// All road pieces, batched by chunk.
#[derive(Default)]
pub struct RoadSet {
    pieces: BTreeMap<PieceKey, Entry>,
    by_chunk: BTreeMap<(i32, i32), BTreeSet<PieceKey>>,
}

impl RoadSet {
    pub fn piece_count(&self) -> usize {
        self.pieces.len()
    }

    pub fn chunks(&self) -> impl Iterator<Item = &(i32, i32)> {
        self.by_chunk.keys()
    }

    pub fn piece(&self, key: PieceKey) -> Option<&Piece> {
        self.pieces.get(&key).map(|e| &e.piece)
    }

    /// Rebuilds pieces whose inputs changed since the last call and drops vanished ones.
    ///
    /// Cost scales with the *change*, not the network: every network edit dirties the end
    /// nodes of the segments involved, so a junction whose `revision` differs from the one
    /// we built names exactly the segments that may need a rebuild (its arms), besides new
    /// segment ids.
    pub fn update(
        &mut self,
        graph: &NetGraph,
        geo: &JunctionGeometry,
        terrain: &Terrain,
    ) -> Update {
        let mut up = Update::default();

        // Vanished pieces.
        let gone: Vec<PieceKey> = self
            .pieces
            .keys()
            .filter(|k| match k {
                PieceKey::Segment(s) => graph.segment(*s).is_none(),
                PieceKey::Junction(n) => geo.get(*n).is_none(),
            })
            .copied()
            .collect();
        for key in gone {
            self.replace(key, 0, None, &mut up);
        }

        // Junctions whose revision changed, and the segments around them.
        let mut candidates: BTreeSet<SegmentId> = BTreeSet::new();
        for (&node, j) in geo.iter() {
            let key = PieceKey::Junction(node);
            if self.pieces.get(&key).is_some_and(|e| e.stamp == j.revision) {
                continue;
            }
            candidates.extend(j.arms.iter().map(|a| a.segment));
            self.replace(
                key,
                j.revision,
                build_junction_piece(graph, geo, terrain, node),
                &mut up,
            );
        }
        // Segments never built (also covers a segment whose junction data is missing).
        candidates.extend(
            graph
                .segments()
                .map(|(id, _)| id)
                .filter(|id| !self.pieces.contains_key(&PieceKey::Segment(*id))),
        );
        for id in candidates {
            let key = PieceKey::Segment(id);
            let stamp = segment_fingerprint(graph, geo, id);
            if self.pieces.get(&key).is_some_and(|e| e.stamp == stamp) {
                continue;
            }
            self.replace(
                key,
                stamp,
                build_segment_piece(graph, geo, terrain, id),
                &mut up,
            );
        }
        up
    }

    fn replace(&mut self, key: PieceKey, stamp: u64, piece: Option<Piece>, up: &mut Update) {
        if let Some(old) = self.pieces.remove(&key) {
            if let Some(set) = self.by_chunk.get_mut(&old.chunk) {
                set.remove(&key);
                if set.is_empty() {
                    self.by_chunk.remove(&old.chunk);
                }
            }
            up.dirty_chunks.insert(old.chunk);
            if piece.is_none() {
                up.removed += 1;
            }
        }
        if let Some(piece) = piece {
            let chunk = chunk_of(piece.center());
            self.by_chunk.entry(chunk).or_default().insert(key);
            up.dirty_chunks.insert(chunk);
            up.rebuilt += 1;
            self.pieces.insert(
                key,
                Entry {
                    stamp,
                    chunk,
                    piece,
                },
            );
        }
    }

    /// Merged mesh of one chunk, `far` selecting the marking-free variant. `None` if the
    /// chunk has no pieces.
    pub fn chunk_mesh(&self, chunk: (i32, i32), far: bool) -> Option<MeshData> {
        let keys = self.by_chunk.get(&chunk)?;
        let mut m = MeshData::default();
        for k in keys {
            let p = &self.pieces[k].piece;
            append(&mut m, if far { &p.far } else { &p.near });
        }
        Some(m)
    }

    /// World bounds of a chunk's pieces.
    pub fn chunk_bounds(&self, chunk: (i32, i32)) -> Option<(Vec3, Vec3)> {
        let keys = self.by_chunk.get(&chunk)?;
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for k in keys {
            let p = &self.pieces[k].piece;
            lo = lo.min(p.min);
            hi = hi.max(p.max);
        }
        Some((lo, hi))
    }

    pub fn triangle_count(&self, far: bool) -> usize {
        self.pieces
            .values()
            .map(|e| {
                let m = if far { &e.piece.far } else { &e.piece.near };
                m.triangle_count()
            })
            .sum()
    }
}

pub mod band;
mod corner;
mod ground;
pub use band::{BAND_LIFT, BAND_STEP, band_data, band_mesh, band_polyline, to_mesh};
pub use ground::{CLEARANCE, lod_height, max_mesh_height};

#[cfg(test)]
mod tests;
