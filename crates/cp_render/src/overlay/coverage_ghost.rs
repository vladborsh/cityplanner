//! The coverage a service building would give, drawn on the roads while its ghost is under the
//! cursor (`architecture/system-design.md` §13.5 v6.2, T-254): light blue where it would raise the
//! coverage (brighter by the gain), thin grey where the roads are already covered as well. The
//! values come from `cp_service::site_coverage`, the function the sim folds into its coverage, so
//! the preview is what the building gives once it stands.

use bevy::prelude::*;
use cp_building::{AccessPoint, BuildingGrid};
use cp_content::{ContentDb, ServiceKind};
use cp_net::NetGraph;
use cp_service::{RoadGraph, ServiceCoverage, site_coverage};
use cp_sim::SimView;

use super::{OverlayLayers, road_ribbons_with};
use crate::chunk::MeshData;

/// What the building tool's ghost would cover, written by `cp_tools` (default: nothing).
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct CoverageGhost {
    pub site: Option<CoverageSite>,
}

/// A service building's ghost on a road: its kind, access point and road radius.
#[derive(Clone, Debug, PartialEq)]
pub struct CoverageSite {
    pub kind: ServiceKind,
    pub access: AccessPoint,
    pub radius_m: f32,
    /// The service's daily capacity (shown with the summary).
    pub capacity: u32,
    /// The corners of the ghost's lot cells: drawn filled with a ring round them so the site
    /// reads from the height the whole reach needs.
    pub lot: Vec<[Vec2; 4]>,
}

/// The numbers next to the cursor, written by the renderer for the UI (`None` without a site).
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoverageGhostSummary(pub Option<CoverageCount>);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoverageCount {
    /// Buildings whose access segment the ghost would cover at `cover_min` or more.
    pub reached: u32,
    /// Of those, the ones the kind does not cover at `cover_min` yet.
    pub newly: u32,
    pub capacity: u32,
}

/// New coverage, alpha and brightness growing with the gain.
pub fn gain_color(gain: f32) -> [f32; 4] {
    let g = gain.clamp(0.0, 1.0);
    [0.10, 0.40 + 0.30 * g, 1.0, 0.65 + 0.3 * g]
}

/// A road the ghost reaches but the kind already covers at least as well.
pub const ALREADY_COVERED: [f32; 4] = [0.92, 0.92, 0.96, 0.85];
/// The ribbons sit above the service overlay's (its lift is 0.3 m).
const GHOST_LIFT: f32 = 0.5;
/// Width of the grey ribbon relative to the road (the blue one is full width).
const ALREADY_WIDTH: f32 = 0.35;
/// The ghost's lot, filled.
pub const SITE_FILL: [f32; 4] = [0.15, 0.95, 0.35, 0.9];
/// The ring round the lot: inner and outer radius beyond the lot's half diagonal, metres.
const RING: (f32, f32) = (14.0, 22.0);
const RING_COLOR: [f32; 4] = [0.10, 0.55, 1.0, 0.95];
const SITE_LIFT: f32 = 0.8;

/// The ghost's lot filled and a ring round it, at `height(p)` + a lift.
pub fn site_marker_mesh(lot: &[[Vec2; 4]], height: impl Fn(Vec2) -> f32) -> MeshData {
    let mut m = MeshData::default();
    if lot.is_empty() {
        return m;
    }
    let at = |p: Vec2| [p.x, height(p) + SITE_LIFT, p.y];
    let n = (lot.len() * 4) as f32;
    let centre = lot.iter().flatten().fold(Vec2::ZERO, |a, &p| a + p) / n;
    let half = lot
        .iter()
        .flatten()
        .map(|p| p.distance(centre))
        .fold(0.0f32, f32::max);
    for cell in lot {
        super::flat(&mut m, cell.map(at), SITE_FILL);
    }
    const SEGMENTS: usize = 48;
    let (r0, r1) = (half + RING.0, half + RING.1);
    for i in 0..SEGMENTS {
        let a = |k: usize| {
            let t = k as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            Vec2::new(t.cos(), t.sin())
        };
        let (u, v) = (a(i), a(i + 1));
        super::flat(
            &mut m,
            [
                at(centre + u * r0),
                at(centre + u * r1),
                at(centre + v * r1),
                at(centre + v * r0),
            ],
            RING_COLOR,
        );
    }
    m
}

/// The ghost's ribbons: per segment of `site` (`(index, value)` from `site_coverage`), blue by
/// `value − existing` where that is positive, else the thin grey.
pub fn coverage_ghost_mesh(
    net: &NetGraph,
    site: &[(u32, f32)],
    existing: impl Fn(u32) -> f32,
) -> MeshData {
    let gain = |index: u32| {
        site.binary_search_by_key(&index, |&(i, _)| i)
            .ok()
            .map(|k| site[k].1 - existing(index))
    };
    let mut m = road_ribbons_with(net, GHOST_LIFT, 1.0, |id| {
        gain(id.index).filter(|&g| g > 0.0).map(gain_color)
    });
    let thin = road_ribbons_with(net, GHOST_LIFT, ALREADY_WIDTH, |id| {
        gain(id.index)
            .filter(|&g| g <= 0.0)
            .map(|_| ALREADY_COVERED)
    });
    m.append(thin);
    m
}

/// The summary of `site` against the kind's current coverage: buildings (any kind) whose access
/// segment the ghost covers at `cover_min` or more, and how many of them are not covered so yet.
pub fn coverage_count(
    site: &[(u32, f32)],
    existing: impl Fn(u32) -> f32,
    buildings: &BuildingGrid,
    db: &ContentDb,
    kind: ServiceKind,
    cover_min: f32,
    capacity: u32,
) -> CoverageCount {
    let mut out = CoverageCount {
        capacity,
        ..Default::default()
    };
    for (_, b) in buildings.iter() {
        let Some(a) = b.access() else { continue };
        // The buildings of the service itself are not what it serves.
        if db
            .building(b.def)
            .service
            .as_deref()
            .and_then(|s| db.service_id(s))
            .is_some_and(|s| db.service(s).kind == kind)
        {
            continue;
        }
        let Ok(k) = site.binary_search_by_key(&a.segment.index, |&(i, _)| i) else {
            continue;
        };
        if site[k].1 >= cover_min {
            out.reached += 1;
            if existing(a.segment.index) < cover_min {
                out.newly += 1;
            }
        }
    }
    out
}

#[derive(Resource, Default)]
pub(super) struct CoverageGhostLayer {
    /// The road graph and the pointer of the network it was built from.
    graph: Option<(usize, RoadGraph)>,
    key: Option<(CoverageGhost, usize, usize, usize)>,
    entity: Option<Entity>,
}

/// Marks the ghost's ribbons.
#[derive(Component)]
pub struct CoverageGhostRibbons;

pub(super) fn sync_coverage_ghost(
    view: Option<Res<SimView>>,
    overlay: Res<OverlayLayers>,
    ghost: Res<CoverageGhost>,
    mut layer: ResMut<CoverageGhostLayer>,
    mut summary: ResMut<CoverageGhostSummary>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let (Some(view), Some(material)) = (view, overlay.material.as_ref()) else {
        return;
    };
    let net_key = std::sync::Arc::as_ptr(&view.net) as usize;
    let key = (
        ghost.clone(),
        net_key,
        std::sync::Arc::as_ptr(&view.service) as usize,
        std::sync::Arc::as_ptr(&view.buildings) as usize,
    );
    if layer.key.as_ref() == Some(&key) {
        return;
    }
    layer.key = Some(key);
    if let Some(e) = layer.entity.take() {
        commands.entity(e).despawn();
    }
    let (Some(site), Some(db)) = (ghost.site.as_ref(), view.content.as_deref()) else {
        summary.set_if_neq(CoverageGhostSummary(None));
        return;
    };
    if layer.graph.as_ref().is_none_or(|(k, _)| *k != net_key) {
        layer.graph = Some((net_key, RoadGraph::new(&view.net)));
    }
    let Some((_, graph)) = layer.graph.as_ref() else {
        return;
    };
    let kind = site.kind;
    let values = site_coverage(graph, site.access, site.radius_m);
    let existing = |i: u32| existing_value(&view.service, kind, i);
    summary.set_if_neq(CoverageGhostSummary(Some(coverage_count(
        &values,
        existing,
        &view.buildings,
        db,
        site.kind,
        db.balance().res_service_cover,
        site.capacity,
    ))));
    let mut mesh = coverage_ghost_mesh(&view.net, &values, existing);
    if let Some(terrain) = view.terrain.as_deref() {
        mesh.append(site_marker_mesh(&site.lot, |p| {
            crate::chunk::terrain_mesh_height(terrain, p)
        }));
    }
    layer.entity = super::transit::spawn(
        &mut commands,
        &mut meshes,
        material,
        mesh,
        "coverage ghost",
        CoverageGhostRibbons,
    );
}

fn existing_value(cov: &ServiceCoverage, kind: ServiceKind, index: u32) -> f32 {
    cov.on_segment_index(kind, index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::{Vec2, Vec3};
    use cp_building::{Building, BuildingFlags, BuildingState, Footprint, Lot};
    use cp_core::curve::CubicBezier3;
    use cp_core::{Id, Tick};
    use cp_net::{Elevation, SegmentId};

    fn db() -> ContentDb {
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap()
    }

    /// A straight street of four 100 m segments.
    fn street(db: &ContentDb) -> (NetGraph, Vec<SegmentId>) {
        let road = db.road_id("basic_2l").unwrap();
        let mut net = NetGraph::default();
        let segs = (0..4)
            .map(|k| {
                let (a, b) = (k as f32 * 100.0, (k + 1) as f32 * 100.0);
                net.add_segment(
                    &db.road(road).clone(),
                    road,
                    CubicBezier3::line(Vec3::new(a, 0.0, 0.0), Vec3::new(b, 0.0, 0.0)),
                    Elevation::Ground,
                )
                .unwrap()
            })
            .collect();
        (net, segs)
    }

    fn put(grid: &mut BuildingGrid, db: &ContentDb, def: &str, seg: SegmentId) {
        grid.insert(Building {
            def: db.building_id(def).unwrap(),
            footprint: Footprint::Lot(Lot {
                block: Id::new(0, 0),
                col: 0,
                w: 1,
                d: 1,
            }),
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos: Vec2::ZERO,
            rot: 0.0,
            access: Some(AccessPoint {
                segment: seg,
                t: 0.5,
            }),
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
    }

    #[test]
    fn the_ribbons_are_blue_where_coverage_rises_and_grey_where_it_is_already_there() {
        let db = db();
        let (net, segs) = street(&db);
        // The ghost reaches the first three segments; the second is already fully covered.
        let site = vec![
            (segs[0].index, 1.0),
            (segs[1].index, 0.7),
            (segs[2].index, 0.3),
        ];
        let existing = |i: u32| if i == segs[1].index { 1.0 } else { 0.0 };
        let m = coverage_ghost_mesh(&net, &site, existing);
        assert!(m.triangle_count() > 0);
        let blue = m.colors.iter().filter(|c| c[2] == 1.0).count();
        let grey = m.colors.iter().filter(|&&c| c == ALREADY_COVERED).count();
        assert!(blue > 0 && grey > 0 && blue + grey == m.colors.len());
        // Two blue segments, one grey: the same ring count per 100 m segment.
        assert_eq!(blue, 2 * grey);
        // The fourth segment is not drawn at all: every vertex lies before x = 300 m.
        assert!(m.positions.iter().all(|p| p[0] <= 300.0 + 1e-3));
        assert!(gain_color(1.0)[3] > gain_color(0.1)[3]);
        assert!(coverage_ghost_mesh(&net, &[], existing).indices.is_empty());
    }

    #[test]
    fn the_site_marker_fills_the_lot_and_rings_it() {
        let lot = [[
            Vec2::new(0.0, 0.0),
            Vec2::new(8.0, 0.0),
            Vec2::new(8.0, 8.0),
            Vec2::new(0.0, 8.0),
        ]];
        let m = site_marker_mesh(&lot, |_| 10.0);
        assert_eq!(m.colors.iter().filter(|&&c| c == SITE_FILL).count(), 4);
        assert!(m.colors.contains(&RING_COLOR));
        assert!(m.positions.iter().all(|p| p[1] > 10.0));
        assert!(site_marker_mesh(&[], |_| 0.0).indices.is_empty());
    }

    #[test]
    fn the_count_takes_the_buildings_on_reached_roads_and_skips_the_service_itself() {
        let db = db();
        let (_, segs) = street(&db);
        let mut grid = BuildingGrid::default();
        for &s in &segs {
            put(&mut grid, &db, "spec_self_sufficient_m", s);
        }
        put(&mut grid, &db, "fire_station", segs[0]);
        let site = vec![
            (segs[0].index, 1.0),
            (segs[1].index, 0.5),
            (segs[2].index, 0.2),
        ];
        // Segment 0 is covered by another station already.
        let existing = |i: u32| if i == segs[0].index { 0.8 } else { 0.0 };
        let c = coverage_count(&site, existing, &grid, &db, ServiceKind::Fire, 0.3, 3);
        assert_eq!(
            c,
            CoverageCount {
                reached: 2,
                newly: 1,
                capacity: 3
            }
        );
    }
}
