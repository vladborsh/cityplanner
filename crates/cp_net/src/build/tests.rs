//! One case per error and per snap kind, plus plan shape, cost and purity.

use cp_content::ContentDb;
use cp_terrain::{GRID, MAX_HEIGHT, MapMeta, SPACING, Terrain, TileOwnership};

use super::*;

fn content() -> ContentDb {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
    ContentDb::load_dir(root).unwrap_or_else(|e| panic!("{e}"))
}

fn enc(m: f32) -> u16 {
    (m / MAX_HEIGHT * 65535.0).round() as u16
}

/// Terrain with ground height `f(x, z)` (metres) and the given sea level.
fn terrain(sea: f32, f: impl Fn(f32, f32) -> f32) -> Terrain {
    let mut hs = vec![0u16; GRID * GRID];
    for iz in 0..GRID {
        for ix in 0..GRID {
            hs[iz * GRID + ix] = enc(f(ix as f32 * SPACING, iz as f32 * SPACING));
        }
    }
    let meta = MapMeta {
        name: "t".into(),
        sea_level: sea,
        start_tile: (2, 2),
        external_connections: vec![],
        entry_road: None,
    };
    Terrain::from_samples(meta, &hs, &vec![0; GRID * GRID]).unwrap()
}

fn flat() -> Terrain {
    terrain(0.0, |_, _| 100.0)
}

const C: Vec2 = Vec2::new(5000.0, 5000.0);

fn at(dx: f32, dz: f32) -> Vec2 {
    C + Vec2::new(dx, dz)
}

struct W {
    db: ContentDb,
    graph: NetGraph,
    terrain: Terrain,
    tiles: TileOwnership,
}

impl W {
    fn new() -> Self {
        Self {
            db: content(),
            graph: NetGraph::new(),
            terrain: flat(),
            tiles: TileOwnership::m1_around((2, 2)),
        }
    }

    fn road(&self, name: &str) -> RoadDefId {
        self.db.road_id(name).unwrap()
    }

    /// Existing flat straight segment between two XZ points at the ground height.
    fn add(&mut self, a: Vec2, b: Vec2) -> SegmentId {
        let id = self.road("basic_2l");
        let def = self.db.road(id).clone();
        let v = |p: Vec2| Vec3::new(p.x, 100.0, p.y);
        self.graph
            .add_segment(&def, id, CubicBezier3::line(v(a), v(b)), Elevation::Ground)
            .unwrap()
    }

    fn run(&self, req: &RoadPathRequest) -> Result<RoadBuildPlan, Vec<RoadBuildError>> {
        validate_road_path(&self.graph, &self.terrain, &self.tiles, &self.db, req)
    }

    fn straight(&self, from: Vec2, to: Vec2) -> RoadPathRequest {
        RoadPathRequest {
            road: self.road("basic_2l"),
            path: RoadPath::Straight { from, to },
            elevation: ElevationMode::Ground,
            snap: SnapOptions::default(),
        }
    }
}

fn no_snap() -> SnapOptions {
    SnapOptions {
        node: false,
        segment: false,
        angle: false,
        zone_grid: false,
    }
}

fn kinds(e: &[RoadBuildError]) -> Vec<&'static str> {
    e.iter()
        .map(|e| match e {
            RoadBuildError::UnknownRoad(_) => "UnknownRoad",
            RoadBuildError::NoElevatedVariant(_) => "NoElevatedVariant",
            RoadBuildError::NoBridgeVariant(_) => "NoBridgeVariant",
            RoadBuildError::RampTooShort { .. } => "RampTooShort",
            RoadBuildError::RampStartsOverWater { .. } => "RampStartsOverWater",
            RoadBuildError::PathTooShort => "PathTooShort",
            RoadBuildError::BadCoordinates => "BadCoordinates",
            RoadBuildError::DegenerateLeg { .. } => "DegenerateLeg",
            RoadBuildError::SegmentTooShort { .. } => "SegmentTooShort",
            RoadBuildError::SharpAngle { .. } => "SharpAngle",
            RoadBuildError::TooSteep { .. } => "TooSteep",
            RoadBuildError::WaterWithoutBridge { .. } => "WaterWithoutBridge",
            RoadBuildError::TooManySegments { .. } => "TooManySegments",
            RoadBuildError::TileNotOwned { .. } => "TileNotOwned",
        })
        .collect()
}

// ---------- errors: one case each ----------

#[test]
fn error_unknown_road() {
    let w = W::new();
    let mut r = w.straight(at(0.0, 0.0), at(50.0, 0.0));
    r.road = RoadDefId(99);
    assert_eq!(
        w.run(&r).unwrap_err(),
        vec![RoadBuildError::UnknownRoad(RoadDefId(99))]
    );
}

#[test]
fn error_no_elevated_variant() {
    let src = r#"Road((id: "r", name: "R", width: 10.0,
        lanes: [(kind: Car, dir: Forward, width: 3.0, offset: 0.0)],
        speed_kmh: 40.0, max_slope: 0.12, cost_per_m: 1.0, upkeep_per_m_week: 0.1,
        allows_zoning: true, rank: 1, mesh: "m", material: "x"))"#;
    let mut w = W::new();
    w.db = ContentDb::from_sources(vec![("r.ron".into(), src.into())]).unwrap();
    let r = RoadPathRequest {
        road: RoadDefId(0),
        path: RoadPath::Straight {
            from: at(0.0, 0.0),
            to: at(50.0, 0.0),
        },
        elevation: ElevationMode::Elevated(6.0),
        snap: SnapOptions::default(),
    };
    assert_eq!(kinds(&w.run(&r).unwrap_err()), ["NoElevatedVariant"]);
}

#[test]
fn error_path_too_short() {
    let w = W::new();
    let r = w.straight(at(0.0, 0.0), at(0.0, 0.0));
    assert_eq!(w.run(&r).unwrap_err(), vec![RoadBuildError::PathTooShort]);
    let mut r = w.straight(at(0.0, 0.0), at(10.0, 0.0));
    r.path = RoadPath::Free(vec![at(0.0, 0.0)]);
    assert_eq!(w.run(&r).unwrap_err(), vec![RoadBuildError::PathTooShort]);
}

#[test]
fn error_degenerate_leg() {
    let mut w = W::new();
    w.add(at(0.0, 0.0), at(0.0, 60.0));
    // Both ends are within 12 m of the same node and snap onto it.
    let r = w.straight(at(2.0, 1.0), at(-3.0, 4.0));
    assert_eq!(kinds(&w.run(&r).unwrap_err()), ["DegenerateLeg"]);
}

#[test]
fn error_segment_too_short() {
    let w = W::new();
    let mut r = w.straight(at(0.0, 0.0), at(6.0, 0.0));
    r.snap = no_snap();
    assert_eq!(kinds(&w.run(&r).unwrap_err()), ["SegmentTooShort"]);
    r.path = RoadPath::Straight {
        from: at(0.0, 0.0),
        to: at(8.5, 0.0),
    };
    assert!(w.run(&r).is_ok());
}

#[test]
fn error_sharp_angle_at_existing_node() {
    let mut w = W::new();
    w.add(at(0.0, 0.0), at(60.0, 0.0));
    // From the node at (0,0) at 20 degrees to the existing segment.
    let a = 20f32.to_radians();
    let mut r = w.straight(at(0.0, 0.0), at(60.0 * a.cos(), 60.0 * a.sin()));
    r.snap = SnapOptions {
        node: true,
        ..no_snap()
    };
    assert_eq!(kinds(&w.run(&r).unwrap_err()), ["SharpAngle"]);
    // 35 degrees is fine.
    let a = 35f32.to_radians();
    r.path = RoadPath::Straight {
        from: at(0.0, 0.0),
        to: at(60.0 * a.cos(), 60.0 * a.sin()),
    };
    assert!(w.run(&r).is_ok());
}

#[test]
fn error_sharp_angle_inside_a_path() {
    let w = W::new();
    let mut r = w.straight(at(0.0, 0.0), at(60.0, 0.0));
    r.snap = no_snap();
    r.path = RoadPath::Free(vec![at(0.0, 0.0), at(60.0, 0.0), at(0.0, 10.0)]);
    assert_eq!(kinds(&w.run(&r).unwrap_err()), ["SharpAngle"]);
}

#[test]
fn error_too_steep_and_highway_limit() {
    let mut w = W::new();
    // 9% grade along x.
    w.terrain = terrain(0.0, |x, _| 100.0 + 0.09 * (x - 4000.0).max(0.0));
    let mut r = w.straight(at(0.0, 0.0), at(80.0, 0.0));
    r.snap = no_snap();
    assert!(w.run(&r).is_ok(), "9% is fine for a street");
    r.road = w.road("highway_1w_3l");
    let e = w.run(&r).unwrap_err();
    assert_eq!(kinds(&e), ["TooSteep"]);
    match e[0] {
        RoadBuildError::TooSteep { slope, max, .. } => {
            assert!((slope - 0.09).abs() < 0.005 && (max - 0.06).abs() < 1e-6);
        }
        _ => unreachable!(),
    }
    // 15% fails even for a street.
    w.terrain = terrain(0.0, |x, _| 100.0 + 0.15 * (x - 4000.0).max(0.0));
    r.road = w.road("basic_2l");
    assert_eq!(kinds(&w.run(&r).unwrap_err()), ["TooSteep"]);
}

#[test]
fn error_water_without_bridge_but_elevated_is_allowed() {
    let mut w = W::new();
    // A shallow bowl (4% sides) whose bottom is 1 m under the sea: water for |x - 5045| < 25.
    w.terrain = terrain(98.0, |x, _| 97.0 + 0.04 * (x - 5045.0).abs());
    let mut r = w.straight(at(0.0, 0.0), at(90.0, 0.0));
    r.snap = no_snap();
    let e = w.run(&r).unwrap_err();
    assert_eq!(kinds(&e), ["WaterWithoutBridge"], "{e:?}");
    // A long elevated path (it must ramp up from ground level and back down).
    r.path = RoadPath::Straight {
        from: at(-120.0, 0.0),
        to: at(210.0, 0.0),
    };
    r.elevation = ElevationMode::Elevated(3.0);
    let plan = w.run(&r).unwrap();
    // The piece over the water is a bridge (and priced as one), the rest is elevated.
    let levels: Vec<_> = plan.segments.iter().map(|s| s.elevation).collect();
    assert!(levels.contains(&Elevation::Bridge), "{levels:?}");
    let def = w.db.road(r.road);
    for s in &plan.segments {
        let per_m = match s.elevation {
            Elevation::Bridge => def.bridge.as_ref().unwrap().cost_per_m,
            _ => def.elevated.as_ref().unwrap().cost_per_m,
        };
        assert_eq!(s.cost, segment_cost(s.length, per_m));
    }
    // Deck height over the water is measured from the water surface (98 m) + 3.
    let mid = plan
        .segments
        .iter()
        .find(|s| s.elevation == Elevation::Bridge)
        .unwrap();
    let wet_y = |p: Vec3| p.y;
    assert!(wet_y(mid.curve.point(0.5)) >= 98.0 + 3.0 - 0.5);
}

#[test]
fn bridge_needs_a_bridge_variant() {
    let mut w = W::new();
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/content/roads/basic_2l.ron"
    ))
    .unwrap();
    let stripped: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("bridge:"))
        .collect::<Vec<_>>()
        .join("\n");
    w.db = ContentDb::from_sources(vec![("basic_2l.ron".into(), stripped)]).unwrap();
    w.terrain = terrain(98.0, |x, _| 97.0 + 0.04 * (x - 5045.0).abs());
    let mut r = w.straight(at(-120.0, 0.0), at(210.0, 0.0));
    r.snap = no_snap();
    r.elevation = ElevationMode::Elevated(3.0);
    let e = w.run(&r).unwrap_err();
    assert_eq!(kinds(&e), ["NoBridgeVariant"], "{e:?}");
    // Dry ground needs no bridge variant.
    w.terrain = flat();
    assert!(w.run(&r).is_ok());
}

#[test]
fn elevated_ends_snap_to_ground_nodes_but_interior_vertices_do_not() {
    let mut w = W::new();
    let s = w.add(at(0.0, 0.0), at(0.0, 60.0));
    let node = w.graph.segment(s).unwrap().end;
    // A ground path ending 5 m from the node snaps to it.
    let r = w.straight(at(60.0, 60.0), at(5.0, 60.0));
    assert_eq!(
        w.run(&r).unwrap().segments.last().unwrap().end,
        NodeRef::Existing(node)
    );
    // An elevated path's END snaps to it too, and stays at ground level there.
    let mut r = w.straight(at(300.0, 60.0), at(5.0, 60.0));
    r.elevation = ElevationMode::Elevated(4.0);
    let plan = w.run(&r).unwrap();
    let last = plan.segments.last().unwrap();
    assert_eq!(last.end, NodeRef::Existing(node));
    assert!((last.curve.p3.y - 100.0).abs() < 0.01);
    // An interior vertex 2 m from the ground node does not snap (grade separation).
    r.path = RoadPath::Free(vec![at(-250.0, 62.0), at(2.0, 62.0), at(250.0, 62.0)]);
    let plan = w.run(&r).unwrap();
    assert!(
        plan.segments
            .iter()
            .all(|s| s.start != NodeRef::Existing(node) && s.end != NodeRef::Existing(node))
    );
}

#[test]
fn elevated_paths_ramp_from_ground_within_max_slope() {
    let w = W::new();
    let mut r = w.straight(at(-200.0, 0.0), at(200.0, 0.0));
    r.snap = no_snap();
    r.elevation = ElevationMode::Elevated(5.0);
    let plan = w.run(&r).unwrap();
    let max = w.db.road(r.road).max_slope;
    let first = plan.segments.first().unwrap().curve.p0.y;
    let last = plan.segments.last().unwrap().curve.p3.y;
    assert!((first - 100.0).abs() < 0.01 && (last - 100.0).abs() < 0.01);
    // Heights rise, plateau at 105, then fall; no piece exceeds max_slope.
    let ys: Vec<f32> = std::iter::once(first)
        .chain(plan.segments.iter().map(|s| s.curve.p3.y))
        .collect();
    assert!(ys.iter().any(|&y| (y - 105.0).abs() < 0.01), "{ys:?}");
    let peak = ys.iter().position(|&y| (y - 105.0).abs() < 0.01).unwrap();
    assert!(ys[..=peak].windows(2).all(|w| w[1] >= w[0] - 1e-4));
    for s in &plan.segments {
        let run = to_xz(s.curve.p3).distance(to_xz(s.curve.p0));
        assert!((s.curve.p3.y - s.curve.p0.y).abs() / run <= max + 1e-4);
        assert_eq!(s.elevation, Elevation::Elevated);
    }
}

#[test]
fn error_ramp_too_short() {
    let w = W::new();
    // 90 m cannot go up 7 m and down again at 12 %.
    let mut r = w.straight(at(0.0, 0.0), at(90.0, 0.0));
    r.snap = no_snap();
    r.elevation = ElevationMode::Elevated(7.0);
    let e = w.run(&r).unwrap_err();
    assert_eq!(kinds(&e), ["RampTooShort"], "{e:?}");
    // A single piece between two ground ends never lifts off, however low.
    r.elevation = ElevationMode::Elevated(0.5);
    assert_eq!(kinds(&w.run(&r).unwrap_err()), ["RampTooShort"]);
    // 300 m has interior nodes but still cannot climb 40 m at 12 %.
    r.path = RoadPath::Straight {
        from: at(-150.0, 0.0),
        to: at(150.0, 0.0),
    };
    r.elevation = ElevationMode::Elevated(40.0);
    assert_eq!(kinds(&w.run(&r).unwrap_err()), ["RampTooShort"]);
    r.elevation = ElevationMode::Elevated(10.0);
    assert!(w.run(&r).is_ok());
}

#[test]
fn error_ramp_over_water() {
    let mut w = W::new();
    w.terrain = terrain(98.0, |x, _| 97.0 + 0.04 * (x - 5045.0).abs());
    // Starts at x = 5030: in the water.
    let mut r = w.straight(at(30.0, 0.0), at(400.0, 0.0));
    r.snap = no_snap();
    r.elevation = ElevationMode::Elevated(3.0);
    let e = w.run(&r).unwrap_err();
    assert!(kinds(&e).contains(&"RampStartsOverWater"), "{e:?}");
}

#[test]
fn piers_every_48_m_on_raised_segments_only() {
    use crate::{PIER_SPACING, pier_positions};
    let mut w = W::new();
    let mut r = w.straight(at(-200.0, 0.0), at(200.0, 0.0));
    r.snap = no_snap();
    r.elevation = ElevationMode::Elevated(3.0);
    let plan = w.run(&r).unwrap();
    let id = w.road("basic_2l");
    let def = w.db.road(id).clone();
    // A middle (plateau) piece of the deck.
    let piece = plan.segments[plan.segments.len() / 2].curve;
    let seg = w
        .graph
        .add_segment(&def, id, piece, Elevation::Elevated)
        .unwrap();
    let segment = w.graph.segment(seg).unwrap();
    assert!(segment.length > 48.0 && segment.length < 96.0);
    let piers = pier_positions(segment);
    assert_eq!(piers.len(), 1, "{piers:?}");
    assert!((piers[0].distance(piece.p0) - PIER_SPACING).abs() < 1.0);
    // A longer deck gets more; ground segments none.
    let mut long = segment.clone();
    long.length = 200.0;
    long.curve = CubicBezier3::line(Vec3::new(0.0, 5.0, 0.0), Vec3::new(200.0, 5.0, 0.0));
    assert_eq!(pier_positions(&long).len(), 4);
    long.elevation = Elevation::Ground;
    assert!(pier_positions(&long).is_empty());
}

#[test]
fn error_too_many_segments_at_a_node() {
    let mut w = W::new();
    for i in 0..8 {
        let a = (i as f32) * 45f32.to_radians();
        w.add(at(0.0, 0.0), at(60.0 * a.cos(), 60.0 * a.sin()));
    }
    // A ninth road from the hub, between two existing ones (22.5 degrees from each).
    let a = 22.5f32.to_radians();
    let mut r = w.straight(at(0.0, 0.0), at(60.0 * a.cos(), 60.0 * a.sin()));
    r.snap = SnapOptions {
        node: true,
        ..no_snap()
    };
    let e = kinds(&w.run(&r).unwrap_err());
    assert!(e.contains(&"TooManySegments"), "{e:?}");
}

#[test]
fn error_tile_not_owned() {
    let mut w = W::new();
    w.tiles = TileOwnership::only((2, 2));
    // Centre tile spans x 4096..6144; this leg leaves it.
    let mut r = w.straight(at(900.0, 0.0), at(1200.0, 0.0));
    r.snap = no_snap();
    assert!(!w.tiles.contains_point(at(1200.0, 0.0)));
    let e = kinds(&w.run(&r).unwrap_err());
    assert!(
        e.iter().all(|k| *k == "TileNotOwned") && !e.is_empty(),
        "{e:?}"
    );
    r.path = RoadPath::Straight {
        from: at(100.0, 0.0),
        to: at(190.0, 0.0),
    };
    assert!(w.run(&r).is_ok());
}

#[test]
fn all_errors_are_reported_together() {
    let mut w = W::new();
    w.tiles = TileOwnership::only((2, 2));
    w.terrain = terrain(10.0, |x, _| {
        if (6000.0..6100.0).contains(&x) {
            0.0
        } else {
            100.0
        }
    });
    // From inside the tile into water and out of the tile: water + tile (+ nothing else).
    let mut r = w.straight(at(900.0, 0.0), at(1200.0, 0.0));
    r.snap = no_snap();
    let k = kinds(&w.run(&r).unwrap_err());
    assert!(
        k.contains(&"WaterWithoutBridge") && k.contains(&"TileNotOwned"),
        "{k:?}"
    );
}

// ---------- snaps: one case each ----------

#[test]
fn snap_node_wins_over_everything() {
    let mut w = W::new();
    let s = w.add(at(0.0, 0.0), at(0.0, 80.0));
    let node = w.graph.segment(s).unwrap().end;
    // 9 m from the end node and also within reach of the segment: the node wins.
    let r = w.straight(at(60.0, 70.0), at(9.0, 80.0));
    let plan = w.run(&r).unwrap();
    assert_eq!(plan.splits.len(), 0);
    assert_eq!(plan.segments[0].end, NodeRef::Existing(node));
    assert!(
        plan.snaps
            .iter()
            .any(|s| s.kind == SnapKind::Node(node) && s.vertex == 1)
    );
    assert_eq!(plan.segments[0].curve.p3, w.graph.node(node).unwrap().pos);
    // Beyond 12 m: no node snap.
    let r = w.straight(at(60.0, 70.0), at(13.0, 100.0));
    let plan = w.run(&r).unwrap();
    assert!(
        !plan
            .snaps
            .iter()
            .any(|s| matches!(s.kind, SnapKind::Node(_)))
    );
}

#[test]
fn snap_segment_splits_it() {
    let mut w = W::new();
    let s = w.add(at(0.0, 0.0), at(0.0, 100.0));
    // 4 m from the middle of the segment, far from both nodes.
    let r = w.straight(at(60.0, 52.0), at(4.0, 50.0));
    let plan = w.run(&r).unwrap();
    assert_eq!(plan.splits.len(), 1);
    let sp = plan.splits[0];
    assert_eq!(sp.segment, s);
    assert!((sp.t - 0.5).abs() < 0.02);
    assert_eq!(plan.segments.last().unwrap().end, NodeRef::Split(0));
    assert!(plan.snaps.iter().any(|a| a.kind == SnapKind::Segment(s)));
    // Elevated roads do not snap onto ground segments.
    let mut r = r;
    r.path = RoadPath::Straight {
        from: at(300.0, 52.0),
        to: at(4.0, 50.0),
    };
    r.elevation = ElevationMode::Elevated(1.0);
    let plan = w.run(&r).unwrap();
    assert!(plan.splits.is_empty());
}

#[test]
fn snap_angle_to_15_degree_steps() {
    let w = W::new();
    // 13 degrees off the X axis snaps to 15.
    let a = 13f32.to_radians();
    let to = at(80.0 * a.cos(), 80.0 * a.sin());
    let plan = w.run(&w.straight(at(0.0, 0.0), to)).unwrap();
    let end = plan.segments.last().unwrap().curve.p3;
    let ang = ((end.z - C.y).atan2(end.x - C.x)).to_degrees();
    assert!((ang - 15.0).abs() < 0.01, "{ang}");
    assert!(plan.snaps.iter().any(|s| s.kind == SnapKind::Angle(15)));
    // Length is preserved.
    assert!((to_xz(end).distance(C) - 80.0).abs() < 0.01);
    // 22 degrees is outside the tolerance: unchanged.
    let a = 22f32.to_radians();
    let to = at(80.0 * a.cos(), 80.0 * a.sin());
    let plan = w.run(&w.straight(at(0.0, 0.0), to)).unwrap();
    assert!(plan.snaps.is_empty());
    // Disabled.
    let mut r = w.straight(
        at(0.0, 0.0),
        at(
            80.0 * 13f32.to_radians().cos(),
            80.0 * 13f32.to_radians().sin(),
        ),
    );
    r.snap.angle = false;
    assert!(w.run(&r).unwrap().snaps.is_empty());
}

#[test]
fn snap_angle_is_relative_to_neighbour_segments() {
    let mut w = W::new();
    // Existing segment at 30 degrees from the world X axis.
    let a = 30f32.to_radians();
    w.add(at(0.0, 0.0), at(60.0 * a.cos(), 60.0 * a.sin()));
    // New leg from the node at (0,0): 30 + 47 degrees; relative to the neighbour 47 -> 45.
    let b = (30.0 + 47.0f32).to_radians();
    let mut r = w.straight(at(0.0, 0.0), at(80.0 * b.cos(), 80.0 * b.sin()));
    r.snap = SnapOptions {
        node: true,
        angle: true,
        ..no_snap()
    };
    let plan = w.run(&r).unwrap();
    let end = plan.segments.last().unwrap().curve.p3;
    let ang = ((end.z - C.y).atan2(end.x - C.x)).to_degrees();
    assert!((ang - 75.0).abs() < 0.01, "{ang}");
}

#[test]
fn snap_zone_grid_is_an_inert_option_until_t026() {
    let w = W::new();
    let mut r = w.straight(at(0.0, 0.0), at(80.3, 0.0));
    r.snap = SnapOptions {
        zone_grid: true,
        ..no_snap()
    };
    let plan = w.run(&r).unwrap();
    assert!(plan.snaps.is_empty());
    assert!(!plan.snaps.iter().any(|s| s.kind == SnapKind::ZoneGrid));
}

// ---------- plan shape ----------

#[test]
fn long_legs_are_cut_on_the_control_points_with_a_tail_of_up_to_96_m() {
    let w = W::new();
    let mut r = w.straight(at(0.0, 0.0), at(250.0, 0.0));
    r.snap = no_snap();
    let plan = w.run(&r).unwrap();
    // 64, 64, 64 and a 58 m tail (T-127: cuts on the control points of §6.7).
    assert_eq!(plan.segments.len(), 4);
    assert_eq!(plan.new_nodes.len(), 5);
    for (k, s) in plan.segments.iter().enumerate() {
        assert!(
            (MIN_SEGMENT_LENGTH_BUILD..=MAX_SEGMENT_LENGTH).contains(&s.length),
            "{}",
            s.length
        );
        let want = if k < 3 { 64.0 } else { 58.0 };
        assert!((s.length - want).abs() < 0.1, "{k}: {}", s.length);
    }
    // Chain connectivity.
    for pair in plan.segments.windows(2) {
        assert_eq!(pair[0].end, pair[1].start);
    }
    // Exactly 96 m is a single piece.
    r.path = RoadPath::Straight {
        from: at(0.0, 0.0),
        to: at(96.0, 0.0),
    };
    assert_eq!(w.run(&r).unwrap().segments.len(), 1);
}

#[test]
fn curve_and_free_modes() {
    let w = W::new();
    let mut r = w.straight(at(0.0, 0.0), at(0.0, 0.0));
    r.snap = no_snap();
    r.path = RoadPath::Curve {
        from: at(0.0, 0.0),
        control: at(60.0, 50.0),
        to: at(120.0, 0.0),
    };
    let plan = w.run(&r).unwrap();
    assert!(plan.segments.len() >= 2);
    let total: f32 = plan.segments.iter().map(|s| s.length).sum();
    assert!(total > 120.0, "curve is longer than its chord: {total}");
    r.path = RoadPath::Free(vec![
        at(0.0, 0.0),
        at(50.0, 0.0),
        at(100.0, 40.0),
        at(100.0, 100.0),
    ]);
    let plan = w.run(&r).unwrap();
    assert_eq!(plan.segments.len(), 3);
}

#[test]
fn ground_crossing_creates_a_node_elevated_does_not() {
    let mut w = W::new();
    let s = w.add(at(45.0, -60.0), at(45.0, 60.0));
    let mut r = w.straight(at(0.0, 0.0), at(90.0, 0.0));
    r.snap = no_snap();
    let plan = w.run(&r).unwrap();
    assert_eq!(plan.splits.len(), 1);
    assert_eq!(plan.splits[0].segment, s);
    assert!((plan.splits[0].t - 0.5).abs() < 0.01);
    assert_eq!(plan.segments.len(), 2);
    assert_eq!(plan.segments[0].end, NodeRef::Split(0));
    assert_eq!(plan.segments[1].start, NodeRef::Split(0));

    // Long enough to ramp up and over; the road at x = 45 passes underneath.
    r.path = RoadPath::Straight {
        from: at(-100.0, 0.0),
        to: at(190.0, 0.0),
    };
    r.elevation = ElevationMode::Elevated(4.0);
    let plan = w.run(&r).unwrap();
    assert!(plan.splits.is_empty());
    // 290 m: cuts at 64, 128, 192 and 256 (T-127).
    assert_eq!(plan.segments.len(), 5);
    // Starts at ground level, reaches ground + 4 at the top.
    assert!((plan.segments[0].curve.p0.y - 100.0).abs() < 0.05);
    assert!(
        plan.segments
            .iter()
            .any(|s| (s.curve.p3.y - 104.0).abs() < 0.05)
    );
}

#[test]
fn cost_is_cents_per_segment() {
    let w = W::new();
    let mut r = w.straight(at(0.0, 0.0), at(50.0, 0.0));
    r.snap = no_snap();
    let plan = w.run(&r).unwrap();
    let per_m = w.db.road(r.road).cost_per_m;
    assert_eq!(plan.cost, segment_cost(50.0, per_m));
    assert_eq!(plan.cost.0, (50.0 * per_m as f64 * 100.0).round() as i64);
    assert_eq!(
        plan.cost,
        Cost(plan.segments.iter().map(|s| s.cost.0).sum())
    );
    // Elevated uses the variant price (on the actual, ramped lengths).
    r.path = RoadPath::Straight {
        from: at(-100.0, 0.0),
        to: at(190.0, 0.0),
    };
    r.elevation = ElevationMode::Elevated(2.0);
    let v = w.db.road(r.road).elevated.as_ref().unwrap().cost_per_m;
    let plan = w.run(&r).unwrap();
    for s in &plan.segments {
        assert_eq!(s.cost, segment_cost(s.length, v));
    }
}

#[test]
fn demolish_list_exists_and_is_empty_until_buildings_land() {
    let w = W::new();
    let plan = w.run(&w.straight(at(0.0, 0.0), at(50.0, 0.0))).unwrap();
    assert!(plan.demolish.is_empty()); // TODO(T-028)
}

#[test]
fn validation_is_pure_and_deterministic() {
    let mut w = W::new();
    w.add(at(50.0, -60.0), at(50.0, 60.0));
    w.add(at(-60.0, 20.0), at(0.0, 20.0));
    let before = w.graph.clone();
    let r = w.straight(at(0.0, 0.0), at(150.0, 5.0));
    let a = w.run(&r);
    let b = w.run(&r);
    assert_eq!(a, b);
    assert_eq!(w.graph, before);
}

// ---------- key angles and guides (T-072) ----------

fn polar(len: f32, deg: f32) -> Vec2 {
    let a = deg.to_radians();
    Vec2::new(len * a.cos(), len * a.sin())
}

/// A two-leg path 0 -> 80 m along +X -> 80 m at `deg` to the first leg; returns the plan and
/// the direction of the last piece in degrees from +X.
fn second_leg(w: &W, deg: f32) -> (RoadBuildPlan, f32) {
    let b = at(80.0, 0.0);
    let mut r = w.straight(b, b);
    r.path = RoadPath::Free(vec![at(0.0, 0.0), b, b + polar(80.0, deg)]);
    let plan = w.run(&r).unwrap();
    let seg = plan.segments.last().unwrap();
    let d = to_xz(seg.curve.p3) - to_xz(seg.curve.p0);
    (plan, d.y.atan2(d.x).to_degrees())
}

#[test]
fn key_angles_90_and_45_win_over_the_15_degree_step() {
    let w = W::new();
    let (plan, ang) = second_leg(&w, 84.0);
    assert!((ang - 90.0).abs() < 0.01, "{ang}");
    assert_eq!(plan.report.kind, SnapReportKind::KeyAngle(90));
    assert!(
        plan.snaps
            .iter()
            .any(|s| s.vertex == 2 && s.kind == SnapKind::KeyAngle(90))
    );
    let (plan, ang) = second_leg(&w, 51.0);
    assert!((ang - 45.0).abs() < 0.01, "{ang}");
    assert_eq!(plan.report.kind, SnapReportKind::KeyAngle(45));
    // Length is kept.
    let seg = plan.segments.last().unwrap();
    assert!((seg.length - 80.0).abs() < 0.01);
}

#[test]
fn the_15_degree_step_still_applies_outside_the_key_angles() {
    let w = W::new();
    // 73 is 17 off 90 and 28 off 45, but 2 off 75: the step takes it.
    let (plan, ang) = second_leg(&w, 73.0);
    assert!((ang - 75.0).abs() < 0.01, "{ang}");
    assert_eq!(plan.report.kind, SnapReportKind::Step(75));
    assert!(plan.report.guide.is_none(), "steps show no guide");
    // 70 is 5 off the step: beyond ANGLE_SNAP_TOL_DEG (4), so it stays as drawn.
    let (plan, ang) = second_leg(&w, 70.0);
    assert!((ang - 70.0).abs() < 0.01, "{ang}");
    assert_eq!(plan.report.kind, SnapReportKind::None);
}

/// A 300 m road at 30 degrees whose centre line passes `offset` metres from `cursor`, on the
/// left of its direction.
fn road_near(w: &mut W, cursor: Vec2, offset: f32) {
    let u = polar(1.0, 30.0);
    let q = cursor + Vec2::new(-u.y, u.x) * offset;
    w.add(q - u * 150.0, q + u * 150.0);
}

#[test]
fn a_nearby_road_gives_a_parallel_guide_within_60_m_only() {
    // 36 degrees from X is 9 off the 45 key and 6 off the step: nothing snaps without a road.
    let cursor = at(0.0, 0.0) + polar(80.0, 36.0);
    let draw = |w: &W| {
        let r = w.straight(at(0.0, 0.0), cursor);
        let plan = w.run(&r).unwrap();
        let seg = plan.segments.last().unwrap();
        let d = to_xz(seg.curve.p3) - to_xz(seg.curve.p0);
        (plan.report, d.y.atan2(d.x).to_degrees())
    };
    let mut w = W::new();
    assert_eq!(draw(&w).0.kind, SnapReportKind::None);

    road_near(&mut w, cursor, 30.0);
    let (report, ang) = draw(&w);
    assert!((ang - 30.0).abs() < 0.01, "parallel: {ang}");
    assert_eq!(
        report.kind,
        SnapReportKind::Guide {
            perpendicular: false
        }
    );
    let (a, b) = report.guide.expect("guide line");
    assert!((a.distance(b) - 240.0).abs() < 0.1, "±120 m");

    let mut far = W::new();
    road_near(&mut far, cursor, 80.0);
    let (report, ang) = draw(&far);
    assert!((ang - 36.0).abs() < 0.01, "80 m away is too far: {ang}");
    assert_eq!(report.kind, SnapReportKind::None);
}

#[test]
fn a_nearby_road_also_gives_a_perpendicular_guide() {
    let mut w = W::new();
    let cursor = at(0.0, 0.0) + polar(80.0, 126.0); // 6 off the perpendicular (120)
    road_near(&mut w, cursor, 30.0);
    let plan = w.run(&w.straight(at(0.0, 0.0), cursor)).unwrap();
    let seg = plan.segments.last().unwrap();
    let d = to_xz(seg.curve.p3) - to_xz(seg.curve.p0);
    assert!((d.y.atan2(d.x).to_degrees() - 120.0).abs() < 0.01);
    assert_eq!(
        plan.report.kind,
        SnapReportKind::Guide {
            perpendicular: true
        }
    );
}

#[test]
fn a_node_within_reach_beats_a_guide() {
    let mut w = W::new();
    let cursor = at(0.0, 0.0) + polar(80.0, 36.0);
    road_near(&mut w, cursor, 30.0);
    let node = cursor + Vec2::new(10.0, 0.0);
    w.add(node, node + Vec2::new(0.0, 60.0));
    let plan = w.run(&w.straight(at(0.0, 0.0), cursor)).unwrap();
    assert_eq!(plan.report.kind, SnapReportKind::Node);
    assert!(plan.report.guide.is_none());
    let end = to_xz(plan.segments.last().unwrap().curve.p3);
    assert!(end.distance(node) < 0.01, "snapped to the node");
}

#[test]
fn curve_mode_leaves_the_angle_alone() {
    let w = W::new();
    let (a, c) = (at(0.0, 0.0), at(80.0, 0.0));
    let to = c + polar(80.0, 84.0);
    let mut r = w.straight(a, to);
    r.path = RoadPath::Curve {
        from: a,
        control: c,
        to,
    };
    let plan = w.run(&r).unwrap();
    assert_eq!(plan.report.kind, SnapReportKind::None);
    assert!(
        !plan
            .snaps
            .iter()
            .any(|s| matches!(s.kind, SnapKind::KeyAngle(_) | SnapKind::Guide { .. }))
    );
    assert!(to_xz(plan.segments.last().unwrap().curve.p3).distance(to) < 0.01);
}

#[test]
fn the_report_is_available_for_an_invalid_path_and_matches_the_plan() {
    let w = W::new();
    let b = at(80.0, 0.0);
    // The second leg is too short: invalid, but the cursor snap is still reported.
    let mut r = w.straight(b, b);
    r.path = RoadPath::Free(vec![at(0.0, 0.0), b, b + polar(3.0, 84.0)]);
    let (res, report) = validate_road_path_report(&w.graph, &w.terrain, &w.tiles, &w.db, &r);
    assert!(res.is_err());
    assert_eq!(report.kind, SnapReportKind::KeyAngle(90));

    // For a valid path the plan carries the same report.
    let (plan, _) = second_leg(&w, 84.0);
    let r2 = {
        let mut r = w.straight(b, b);
        r.path = RoadPath::Free(vec![at(0.0, 0.0), b, b + polar(80.0, 84.0)]);
        r
    };
    let (res, report) = validate_road_path_report(&w.graph, &w.terrain, &w.tiles, &w.db, &r2);
    assert_eq!(res.unwrap().report, report);
    assert_eq!(plan.report, report);
}

// ---------- one-way warnings (§6.6) ----------

impl W {
    /// Existing flat straight segment of the named road type.
    fn add_road(&mut self, name: &str, a: Vec2, b: Vec2) -> SegmentId {
        let id = self.road(name);
        let def = self.db.road(id).clone();
        let v = |p: Vec2| Vec3::new(p.x, 100.0, p.y);
        self.graph
            .add_segment(&def, id, CubicBezier3::line(v(a), v(b)), Elevation::Ground)
            .unwrap()
    }

    fn straight_of(&self, name: &str, from: Vec2, to: Vec2) -> RoadPathRequest {
        RoadPathRequest {
            road: self.road(name),
            ..self.straight(from, to)
        }
    }

    fn warnings(&self, req: &RoadPathRequest) -> Vec<OneWayWarning> {
        let plan = self.run(req).unwrap_or_else(|e| panic!("{e:?}"));
        one_way_report(&self.graph, &self.db, &plan)
    }
}

#[test]
fn a_highway_entering_a_street_node_has_no_warning() {
    let mut w = W::new();
    // A street through the origin; the highway ends on its end node.
    w.add_road("basic_2l", at(0.0, 0.0), at(60.0, 0.0));
    w.add_road("basic_2l", at(60.0, 0.0), at(60.0, 60.0));
    let req = w.straight_of("highway_1w_3l", at(-60.0, 0.0), at(0.0, 0.0));
    assert_eq!(w.warnings(&req), vec![]);
}

#[test]
fn a_highway_ending_in_a_street_by_splitting_it_has_no_warning() {
    let mut w = W::new();
    w.add_road("basic_2l", at(-60.0, 0.0), at(60.0, 0.0));
    let req = w.straight_of("highway_1w_3l", at(0.0, -80.0), at(0.0, 0.0));
    let plan = w.run(&req).unwrap();
    assert_eq!(
        plan.splits.len(),
        1,
        "the street is split under the highway"
    );
    assert_eq!(one_way_report(&w.graph, &w.db, &plan), vec![]);
}

#[test]
fn a_highway_alone_has_no_warning_its_free_end_is_just_unfinished() {
    let w = W::new();
    let req = w.straight_of("highway_1w_3l", at(0.0, 0.0), at(80.0, 0.0));
    assert_eq!(w.warnings(&req), vec![]);
}

#[test]
fn a_highway_ending_where_every_other_arm_is_inbound_is_a_dead_end() {
    let mut w = W::new();
    // Two one-way highways run into the node at the origin ...
    w.add_road("highway_1w_3l", at(-70.0, -20.0), at(0.0, 0.0));
    w.add_road("highway_1w_3l", at(-70.0, 60.0), at(0.0, 0.0));
    // ... and a third one is drawn into it as well: nothing leaves.
    let req = w.straight_of("highway_1w_3l", at(60.0, 0.0), at(0.0, 0.0));
    let plan = w.run(&req).unwrap();
    let node = plan.segments[0].end;
    assert_eq!(
        one_way_report(&w.graph, &w.db, &plan),
        vec![OneWayWarning::DeadEnd { at: node }]
    );
}

#[test]
fn a_highway_continuing_out_of_the_node_is_not_a_dead_end() {
    let mut w = W::new();
    w.add_road("highway_1w_3l", at(-70.0, 0.0), at(0.0, 0.0));
    // The existing highway arrives, the new one leaves in the same direction.
    let req = w.straight_of("highway_1w_3l", at(0.0, 0.0), at(70.0, 0.0));
    assert_eq!(w.warnings(&req), vec![]);
}

#[test]
fn two_one_way_roads_head_on_in_a_degree_two_node_warn_head_on_only() {
    let mut w = W::new();
    w.add_road("highway_1w_3l", at(-70.0, 0.0), at(0.0, 0.0));
    let req = w.straight_of("highway_1w_3l", at(70.0, 0.0), at(0.0, 0.0));
    let plan = w.run(&req).unwrap();
    let node = plan.segments[0].end;
    assert_eq!(
        one_way_report(&w.graph, &w.db, &plan),
        vec![OneWayWarning::HeadOn { at: node }],
        "HeadOn, and not also DeadEnd"
    );
}

#[test]
fn a_two_way_road_never_warns() {
    let mut w = W::new();
    w.add_road("highway_1w_3l", at(-70.0, -20.0), at(0.0, 0.0));
    w.add_road("highway_1w_3l", at(-70.0, 60.0), at(0.0, 0.0));
    let req = w.straight_of("basic_2l", at(60.0, 0.0), at(0.0, 0.0));
    assert_eq!(w.warnings(&req), vec![]);
    let req = w.straight_of("medium_4l", at(60.0, 0.0), at(0.0, 0.0));
    assert_eq!(w.warnings(&req), vec![]);
}

#[test]
fn warnings_do_not_block_the_command() {
    // The same plan that warns is a valid plan: the report is separate from validation.
    let mut w = W::new();
    w.add_road("highway_1w_3l", at(-70.0, 0.0), at(0.0, 0.0));
    let req = w.straight_of("highway_1w_3l", at(70.0, 0.0), at(0.0, 0.0));
    assert!(w.run(&req).is_ok());
    assert!(!w.warnings(&req).is_empty());
}

#[test]
fn auto_cuts_follow_the_control_points_and_leave_a_tail_of_32_to_96_m() {
    assert!(auto_cuts(50.0).is_empty());
    assert!(auto_cuts(96.0).is_empty());
    assert_eq!(auto_cuts(97.0), vec![64.0]);
    assert_eq!(auto_cuts(200.0), vec![64.0, 128.0]);
    assert_eq!(auto_cuts(250.0), vec![64.0, 128.0, 192.0]);
    for k in 1..=200 {
        let span = k as f32 * 8.0;
        let cuts = auto_cuts(span);
        let tail = span - cuts.last().copied().unwrap_or(0.0);
        assert!(tail <= MAX_SEGMENT_LENGTH, "{span}");
        if !cuts.is_empty() {
            assert!(tail > 32.0, "{span}: tail {tail}");
        }
        for (i, c) in cuts.iter().enumerate() {
            assert_eq!(*c, 64.0 * (i + 1) as f32);
        }
    }
}
