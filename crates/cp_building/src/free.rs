//! Placement of free-standing service buildings (`architecture/system-design.md` §8.1).
//!
//! [`validate_free_placement`] is the pure check the ghost previews with and `Place` commits
//! with for a def whose service has `road_access: false`: a rectangle of `w` × `d` cells (8 m)
//! centred on `pos` and turned by `rot`, not tied to a zone block or a road.
//!
//! [`sync_blockers`] hands the rectangles of the placed free-standing buildings to the
//! [`ZoneGrid`] as obstacles, so the zone cells under them are invalid.

use bevy_math::Vec2;
use cp_content::{BuildingDefId, ContentDb};
use cp_core::Cost;
use cp_core::curve::to_xz;
use cp_net::{Elevation, NetGraph};
use cp_terrain::{Terrain, TileOwnership};
use cp_zone::geom::{Quad, quad_aabb, quad_segment_distance, quads_overlap};
use cp_zone::{MAX_CELL_SLOPE, ZoneGrid};

use crate::command::{BuildingError, PlacementPlan};
use crate::grid::BuildingGrid;
use crate::model::{FOOTPRINT_CELL_M, Footprint, footprint_quad};

/// Spacing of the sample points inside the rectangle, metres (the sides are multiples of
/// 8 m, so the samples include the corners).
const SAMPLE_M: f32 = 4.0;
/// Clear distance between the rectangle and the edge of any road, metres.
pub const ROAD_CLEARANCE_M: f32 = 1.0;
/// Spacing of the road centre-line samples, metres.
const ROAD_SAMPLE_M: f32 = 4.0;

/// The pure placement check for a free-standing def. `terrain` and `tiles` are skipped when
/// `None` (bare test worlds); the sim always has both.
#[allow(clippy::too_many_arguments)]
pub fn validate_free_placement(
    db: &ContentDb,
    zones: &ZoneGrid,
    net: &NetGraph,
    buildings: &BuildingGrid,
    terrain: Option<&Terrain>,
    tiles: Option<&TileOwnership>,
    def: BuildingDefId,
    pos: Vec2,
    rot: f32,
) -> Result<PlacementPlan, BuildingError> {
    validate_free_placement_except(
        db, zones, net, buildings, terrain, tiles, def, pos, rot, None,
    )
}

/// [`validate_free_placement`] for moving a building: `ignore` is the building being moved,
/// which is not an obstacle to itself (T-177).
#[allow(clippy::too_many_arguments)]
pub fn validate_free_placement_except(
    db: &ContentDb,
    zones: &ZoneGrid,
    net: &NetGraph,
    buildings: &BuildingGrid,
    terrain: Option<&Terrain>,
    tiles: Option<&TileOwnership>,
    def: BuildingDefId,
    pos: Vec2,
    rot: f32,
    ignore: Option<crate::model::BuildingId>,
) -> Result<PlacementPlan, BuildingError> {
    validate_free_placement_ignoring(
        db,
        zones,
        net,
        buildings,
        terrain,
        tiles,
        def,
        pos,
        rot,
        ignore.as_slice(),
        &[],
    )
}

/// [`validate_free_placement`] where the buildings `ignore` and the cells `free_cells` are no
/// obstacle: the building being moved (T-177), or the zoned buildings a service is placed over
/// (T-252).
#[allow(clippy::too_many_arguments)]
pub fn validate_free_placement_ignoring(
    db: &ContentDb,
    zones: &ZoneGrid,
    net: &NetGraph,
    buildings: &BuildingGrid,
    terrain: Option<&Terrain>,
    tiles: Option<&TileOwnership>,
    def: BuildingDefId,
    pos: Vec2,
    rot: f32,
    ignore: &[crate::model::BuildingId],
    free_cells: &[cp_zone::ZoneCellRef],
) -> Result<PlacementPlan, BuildingError> {
    if !cp_core::sane_point(pos) || !rot.is_finite() || rot.abs() > 1.0e4 {
        return Err(BuildingError::BadCoordinates);
    }
    let def_ref = db
        .buildings()
        .find(|(id, _)| *id == def)
        .map(|(_, d)| d)
        .ok_or(BuildingError::UnknownDef)?;
    let service = def_ref
        .service
        .as_deref()
        .and_then(|s| db.service_id(s))
        .map(|s| db.service(s))
        .ok_or(BuildingError::NotPlaceable)?;
    if service.road_access {
        return Err(BuildingError::NotPlaceable);
    }
    let (w, d) = def_ref.lot;
    if w == 0 || d == 0 {
        return Err(BuildingError::DoesNotFit);
    }
    let quad = footprint_quad(pos, rot, w, d);

    // Sample lattice in the rectangle's own frame, `SAMPLE_M` apart, corners included.
    let (nu, nv) = (
        (f32::from(w) * FOOTPRINT_CELL_M / SAMPLE_M).round() as usize + 1,
        (f32::from(d) * FOOTPRINT_CELL_M / SAMPLE_M).round() as usize + 1,
    );
    let (ax, ay) = (quad[1] - quad[0], quad[3] - quad[0]);
    let at = |i: usize, j: usize| {
        quad[0] + ax * (i as f32 / (nu - 1) as f32) + ay * (j as f32 / (nv - 1) as f32)
    };
    let samples: Vec<Vec2> = (0..nv)
        .flat_map(|j| (0..nu).map(move |i| (i, j)))
        .map(|(i, j)| at(i, j))
        .collect();

    if let Some(tiles) = tiles
        && !samples.iter().all(|&p| tiles.contains_point(p))
    {
        return Err(BuildingError::NotOwned);
    }
    if let Some(terrain) = terrain {
        if samples.iter().any(|&p| terrain.is_water(p)) {
            return Err(BuildingError::Water);
        }
        let h: Vec<f32> = samples.iter().map(|&p| terrain.height(p)).collect();
        let steep = |a: usize, b: usize, dist: f32| (h[a] - h[b]).abs() > MAX_CELL_SLOPE * dist;
        for j in 0..nv {
            for i in 0..nu {
                let k = j * nu + i;
                if i + 1 < nu && steep(k, k + 1, ax.length() / (nu - 1) as f32) {
                    return Err(BuildingError::TooSteep);
                }
                if j + 1 < nv && steep(k, k + nu, ay.length() / (nv - 1) as f32) {
                    return Err(BuildingError::TooSteep);
                }
            }
        }
    }

    let (lo, hi) = quad_aabb(&quad);
    for (_, seg) in net.segments() {
        if seg.elevation != Elevation::Ground {
            continue;
        }
        let reach = db.road(seg.road).width / 2.0 + ROAD_CLEARANCE_M;
        let c = &seg.curve;
        let ctrl = [c.p0, c.p1, c.p2, c.p3].map(to_xz);
        let (clo, chi) = (
            ctrl[0].min(ctrl[1]).min(ctrl[2]).min(ctrl[3]) - Vec2::splat(reach),
            ctrl[0].max(ctrl[1]).max(ctrl[2]).max(ctrl[3]) + Vec2::splat(reach),
        );
        if chi.x < lo.x || clo.x > hi.x || chi.y < lo.y || clo.y > hi.y {
            continue;
        }
        let n = ((seg.length / ROAD_SAMPLE_M).ceil() as usize).max(1);
        let pts: Vec<Vec2> = (0..=n)
            .map(|i| to_xz(c.point(i as f32 / n as f32)))
            .collect();
        if pts
            .windows(2)
            .any(|p| quad_segment_distance(&quad, p[0], p[1]) < reach)
        {
            return Err(BuildingError::Road);
        }
    }

    for (id, b) in buildings.iter() {
        if ignore.contains(&id) {
            continue;
        }
        let other = b.footprint_polygon();
        let (olo, ohi) = quad_aabb(&other);
        if ohi.x < lo.x || olo.x > hi.x || ohi.y < lo.y || olo.y > hi.y {
            continue;
        }
        if quads_overlap(&quad, &other) {
            return Err(BuildingError::Blocked);
        }
    }
    // A cell of a lot that is held by a building no longer in the list (rebinding lots).
    let reach = Vec2::new(w.into(), d.into()).length() * FOOTPRINT_CELL_M * 0.5 + 6.0;
    for cell in zones.cells_within(pos, reach) {
        if zones.is_occupied(cell)
            && !free_cells.contains(&cell)
            && zones
                .cell_footprint(cell)
                .is_some_and(|o| quads_overlap(&quad, &o.corners()))
        {
            return Err(BuildingError::Blocked);
        }
    }

    Ok(PlacementPlan {
        footprint: Footprint::Free { w, d },
        pos,
        rot,
        access: None,
        cost: Cost::from_units(def_ref.cost),
    })
}

/// Hands the rectangles of the free-standing buildings to the zone grid as obstacles. Does
/// nothing when they are unchanged, so it is cheap to call every tick.
pub fn sync_blockers(zones: &mut ZoneGrid, buildings: &BuildingGrid) {
    let quads: Vec<Quad> = buildings
        .iter()
        .filter(|(_, b)| matches!(b.footprint, Footprint::Free { .. }))
        .map(|(_, b)| b.footprint_polygon())
        .collect();
    zones.set_blockers(quads);
}

/// Whether a def stands free (its service needs no road, §8.1).
pub fn is_free_standing(db: &ContentDb, def: BuildingDefId) -> bool {
    db.buildings()
        .find(|(id, _)| *id == def)
        .and_then(|(_, d)| d.service.as_deref())
        .and_then(|s| db.service_id(s))
        .is_some_and(|s| !db.service(s).road_access)
}
