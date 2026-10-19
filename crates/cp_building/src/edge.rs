//! Placement of a service building at the road edge (`architecture/system-design.md` §6.8.3
//! item 8, T-267).
//!
//! A service or a unique building does not stand on zone cells. [`validate_edge_placement`] puts
//! its rectangle (`w` cells along the road, `d` deep, 8 m each) with its front on the edge of a
//! road, facing it, and slides it along the edge to where it fits: the cursor picks the road
//! (any ground road within [`EDGE_REACH_M`]), the side and the place; the rectangle is checked
//! against the real obstacles — roads, other buildings, water, slope, the land the player owns —
//! and the zoned buildings under it are no obstacle (T-252 takes them down). The plan is a
//! [`Footprint::Free`] rectangle with an access point on the road it faces, so the save format
//! does not change and the zone cells under the rectangle go invalid through the blockers like
//! those under any free-standing building.
//!
//! **Order of candidates.** For every nearby road and side the lot may stand at a slide from the
//! cursor's foot of up to [`SLIDE_M`] either way, every 2 m, and at the exact slides where its
//! side stands flush against the edge of a cross street. All candidates are sorted by the
//! distance from the cursor to the rectangle's centre (millimetres), then by the build order of
//! the road, the side and the slide; they are checked in that order and the first that fits wins.
//! Two roads or sides never tie on an arena slot (`SegmentId`), which a rebuilt street may reuse.
//!
//! **Flush.** The front stands on the edge, the same as the zone cells ([`ROAD_EPS`] of slack
//! against every road), so two services four cells deep fit back to back between streets 64 m
//! apart, and the facade lines up with the houses beside it.

use bevy_math::Vec2;
use cp_content::{BuildingDefId, ContentDb, RoadClass, ServiceSite};
use cp_core::Cost;
use cp_core::curve::to_xz;
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_terrain::{Terrain, TileOwnership};
use cp_zone::{ROAD_EPS, ZoneCellRef, ZoneGrid};

use crate::command::{BuildingError, PlacementPlan, oriented_lot};
use crate::free::RectCtx;
use crate::grid::BuildingGrid;
use crate::model::{AccessPoint, BuildingId, FOOTPRINT_CELL_M, Footprint, footprint_quad};

/// How far from the cursor a road's edge may be for the building to stand there, metres (C:S
/// snaps to the nearest edge within 52 m).
pub const EDGE_REACH_M: f32 = 52.0;
/// How far along the edge the building may slide from the cursor's foot, metres.
pub const SLIDE_M: f32 = 48.0;
/// Spacing of the slides tried, metres.
const SLIDE_STEP_M: f32 = 2.0;
/// Spacing of the scan for the exact flush slides, metres.
const SCAN_STEP_M: f32 = 1.0;
/// Pushes away from the edge tried when no flush slide fits (the inside of a curve), metres.
const PUSHES_M: [f32; 3] = [1.0, 2.0, 4.0];

/// Whether `def` stands at the road edge: a service building that needs a road and stands on
/// land, or a unique building — but not an industry or park building (their sites are areas of
/// cells) and not a station (its platform is laid behind a lot, BUG-046).
pub fn is_edge_placed(db: &ContentDb, def: BuildingDefId) -> bool {
    let d = db.building(def);
    d.industry.is_none()
        && d.park.is_none()
        && d.track.is_none()
        && d.service
            .as_deref()
            .and_then(|s| db.service_id(s))
            .map(|s| db.service(s))
            .is_some_and(|s| s.road_access && s.site == ServiceSite::Land)
}

/// One place the rectangle may stand: on `seg`, `side` of it (0 left, 1 right of its direction),
/// `slide` metres from the cursor's foot, `push` metres off the edge.
struct Candidate {
    key: (i64, u32, u8, i64, i64),
    seg: SegmentId,
    t: f32,
    centre: Vec2,
    rot: f32,
}

/// The rectangle on `side` of a road at arc length `s`, `push` metres off its edge: its centre and
/// the turn that puts its front toward the road (the building's local x runs along the road, the
/// way the zone cells' do, so the render faces it the same).
fn frame(
    curve: &cp_core::curve::CubicBezier3,
    s: f32,
    side: u8,
    half: f32,
    push: f32,
    depth_m: f32,
) -> (Vec2, f32, f32) {
    let table = curve.arc_table();
    let t = table.s_to_t(s.clamp(0.0, table.length()));
    let on = to_xz(curve.point(t));
    let tan = to_xz(curve.tangent(t)).normalize_or_zero();
    let left = Vec2::new(-tan.y, tan.x);
    let (n, u) = if side == 0 {
        (left, tan)
    } else {
        (-left, -tan)
    };
    (on + n * (half + push + depth_m * 0.5), u.y.atan2(u.x), t)
}

/// The plan for `def` at the road edge nearest `cursor` (§6.8.3 item 8). `rot` only says how many
/// quarter turns the lot has (its `w` side along the road or across it); the turn of the building
/// is the road's. `ignore` are buildings that do not count (the one being moved, the zoned ones
/// a placement goes over) and `free_cells` cells held by buildings that go.
///
/// Errors: [`BuildingError::NoRoad`] when no road edge is within [`EDGE_REACH_M`]; otherwise the
/// real collision of the candidate nearest the cursor (`Road`, `Blocked`, `Water`, `TooSteep`,
/// `NotOwned`).
#[allow(clippy::too_many_arguments)]
pub fn validate_edge_placement(
    db: &ContentDb,
    zones: &ZoneGrid,
    net: &NetGraph,
    buildings: &BuildingGrid,
    terrain: Option<&Terrain>,
    tiles: Option<&TileOwnership>,
    def: BuildingDefId,
    cursor: Vec2,
    rot: f32,
    ignore: &[BuildingId],
    free_cells: &[ZoneCellRef],
) -> Result<PlacementPlan, BuildingError> {
    edge_search(
        db, zones, net, buildings, terrain, tiles, def, cursor, rot, ignore, free_cells,
    )
    .map_err(|b| b.err)
}

/// A placement that found no place: the error, the rectangle of the candidate nearest the cursor
/// (where the building would stand if it could, for the tool to draw in red) and where the
/// blocker is. Both are `None` when no road edge is within reach.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeBlocked {
    pub err: BuildingError,
    pub rect: Option<[Vec2; 4]>,
    pub at: Option<Vec2>,
}

/// [`validate_edge_placement`], with what a refusal looks like on the map.
#[allow(clippy::too_many_arguments)]
pub fn edge_search(
    db: &ContentDb,
    zones: &ZoneGrid,
    net: &NetGraph,
    buildings: &BuildingGrid,
    terrain: Option<&Terrain>,
    tiles: Option<&TileOwnership>,
    def: BuildingDefId,
    cursor: Vec2,
    rot: f32,
    ignore: &[BuildingId],
    free_cells: &[ZoneCellRef],
) -> Result<PlacementPlan, EdgeBlocked> {
    let bare = |err| EdgeBlocked {
        err,
        rect: None,
        at: None,
    };
    if !cp_core::sane_point(cursor) || !rot.is_finite() || rot.abs() > 1.0e4 {
        return Err(bare(BuildingError::BadCoordinates));
    }
    let def_ref = db.building(def);
    if !is_edge_placed(db, def) {
        return Err(bare(BuildingError::NotPlaceable));
    }
    let (w, d) = oriented_lot(def_ref, rot);
    if w == 0 || d == 0 {
        return Err(bare(BuildingError::DoesNotFit));
    }
    let (along_m, depth_m) = (
        f32::from(w) * FOOTPRINT_CELL_M,
        f32::from(d) * FOOTPRINT_CELL_M,
    );
    let radius = EDGE_REACH_M + SLIDE_M + along_m.max(depth_m) + FOOTPRINT_CELL_M;
    let ctx = RectCtx::new(
        db, zones, net, buildings, terrain, tiles, cursor, radius, ignore,
    );

    // The roads whose edge is within reach, in build order.
    let mut roads: Vec<(SegmentId, u32)> = Vec::new();
    for (id, seg) in net.segments() {
        if seg.elevation != Elevation::Ground || db.road(seg.road).class == RoadClass::Rail {
            continue;
        }
        let half = db.road(seg.road).width / 2.0;
        if seg.curve.bounds_distance_xz(cursor) > EDGE_REACH_M + half + 1.0 {
            continue;
        }
        if seg.curve.nearest_xz(cursor).1 - half > EDGE_REACH_M {
            continue;
        }
        roads.push((id, seg.order));
    }
    if roads.is_empty() {
        return Err(bare(BuildingError::NoRoad));
    }

    let mm = |v: f32| (v * 1000.0).round() as i64;
    let mut flush: Vec<Candidate> = Vec::new();
    let mut pushed: Vec<Candidate> = Vec::new();
    for &(id, order) in &roads {
        let seg = net.segment(id).expect("listed above");
        let half = db.road(seg.road).width / 2.0;
        let curve = &seg.curve;
        let table = curve.arc_table();
        let s0 = table.t_to_s(curve.nearest_xz(cursor).0);
        for side in 0..2u8 {
            let mut slides: Vec<f32> = (-(SLIDE_M / SLIDE_STEP_M) as i32
                ..=(SLIDE_M / SLIDE_STEP_M) as i32)
                .map(|k| k as f32 * SLIDE_STEP_M)
                .collect();
            // The exact slides: where the rectangle's side stands flush against another road.
            let clear = |slide: f32| {
                let s = s0 + slide;
                if !(0.0..=table.length()).contains(&s) {
                    return true;
                }
                let (c, r, _) = frame(curve, s, side, half, 0.0, depth_m);
                ctx.roads_clear(c, r, w, d, -ROAD_EPS)
            };
            let steps = (SLIDE_M / SCAN_STEP_M) as i32;
            let mut prev = clear(-(steps as f32) * SCAN_STEP_M);
            for k in -steps + 1..=steps {
                let at = k as f32 * SCAN_STEP_M;
                let now = clear(at);
                if now != prev {
                    // Bisect to the clear end of the change.
                    let (mut lo, mut hi) = (at - SCAN_STEP_M, at);
                    for _ in 0..24 {
                        let mid = (lo + hi) * 0.5;
                        if clear(mid) == clear(lo) {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    slides.push(if prev { lo } else { hi });
                }
                prev = now;
            }
            for slide in slides {
                let s = s0 + slide;
                if !(0.0..=table.length()).contains(&s) {
                    continue;
                }
                for push in std::iter::once(0.0).chain(PUSHES_M) {
                    let (centre, rot, t) = frame(curve, s, side, half, push, depth_m);
                    let list = if push == 0.0 { &mut flush } else { &mut pushed };
                    list.push(Candidate {
                        key: (
                            mm(centre.distance(cursor)),
                            order,
                            side,
                            mm(slide),
                            mm(push),
                        ),
                        seg: id,
                        t,
                        centre,
                        rot,
                    });
                }
            }
        }
    }

    let mut first: Option<EdgeBlocked> = None;
    for list in [&mut flush, &mut pushed] {
        list.sort_by_key(|c| c.key);
        for c in list.iter() {
            match ctx.check(c.centre, c.rot, w, d, -ROAD_EPS, free_cells) {
                Ok(()) => {
                    return Ok(PlacementPlan {
                        footprint: Footprint::Free { w, d },
                        pos: c.centre,
                        rot: c.rot,
                        access: Some(AccessPoint {
                            segment: c.seg,
                            t: c.t,
                        }),
                        cost: Cost::from_units(def_ref.cost),
                    });
                }
                Err(b) => {
                    first.get_or_insert(EdgeBlocked {
                        err: b.err,
                        rect: Some(footprint_quad(c.centre, c.rot, w, d)),
                        at: Some(b.at),
                    });
                }
            }
        }
    }
    Err(first.unwrap_or_else(|| bare(BuildingError::NoRoad)))
}
