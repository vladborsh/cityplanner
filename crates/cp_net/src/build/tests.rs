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
            RoadBuildError::PathTooShort => "PathTooShort",
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
    r.elevation = ElevationMode::Elevated(8.0);
    let plan = w.run(&r).unwrap();
    assert!(
        plan.segments
            .iter()
            .all(|s| s.elevation == Elevation::Elevated)
    );
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
    r.elevation = ElevationMode::Elevated(6.0);
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
fn long_legs_are_cut_into_equal_pieces() {
    let w = W::new();
    let mut r = w.straight(at(0.0, 0.0), at(250.0, 0.0));
    r.snap = no_snap();
    let plan = w.run(&r).unwrap();
    assert_eq!(plan.segments.len(), 3);
    assert_eq!(plan.new_nodes.len(), 4);
    for s in &plan.segments {
        assert!(
            (MIN_SEGMENT_LENGTH_BUILD..=MAX_SEGMENT_LENGTH).contains(&s.length),
            "{}",
            s.length
        );
        assert!((s.length - 250.0 / 3.0).abs() < 0.1);
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

    r.elevation = ElevationMode::Elevated(7.0);
    let plan = w.run(&r).unwrap();
    assert!(plan.splits.is_empty());
    assert_eq!(plan.segments.len(), 1);
    // Elevated heights: ground (100) + 7.
    assert!((plan.segments[0].curve.p0.y - 107.0).abs() < 0.05);
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
    // Elevated uses the variant price.
    r.elevation = ElevationMode::Elevated(6.0);
    let v = w.db.road(r.road).elevated.as_ref().unwrap().cost_per_m;
    assert_eq!(w.run(&r).unwrap().cost, segment_cost(50.0, v));
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
