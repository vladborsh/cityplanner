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
        entry_rail: None,
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
            RoadBuildError::TooTight { .. } => "TooTight",
            RoadBuildError::TooSteep { .. } => "TooSteep",
            RoadBuildError::WaterWithoutBridge { .. } => "WaterWithoutBridge",
            RoadBuildError::TooManySegments { .. } => "TooManySegments",
            RoadBuildError::TileNotOwned { .. } => "TileNotOwned",
            RoadBuildError::NodeOnRoad { .. } => "NodeOnRoad",
            RoadBuildError::RoadsOverlap { .. } => "RoadsOverlap",
            RoadBuildError::NoTunnelVariant(_) => "NoTunnelVariant",
            RoadBuildError::TunnelOnly(_) => "TunnelOnly",
            RoadBuildError::RailCrossesRoad { .. } => "RailCrossesRoad",
            RoadBuildError::TunnelClearance { .. } => "TunnelClearance",
            RoadBuildError::ElevatedClearance { .. } => "ElevatedClearance",
            RoadBuildError::LevelMismatch { .. } => "LevelMismatch",
            RoadBuildError::LevelsCrossGround => "LevelsCrossGround",
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

// ---------- track and tunnels (T-161) ----------

impl W {
    /// An existing straight piece of `road` at height `y`.
    fn add_as(&mut self, road: &str, a: Vec2, b: Vec2, y: f32, level: Elevation) -> SegmentId {
        let id = self.road(road);
        let def = self.db.road(id).clone();
        let v = |p: Vec2| Vec3::new(p.x, y, p.y);
        self.graph
            .add_segment(&def, id, CubicBezier3::line(v(a), v(b)), level)
            .unwrap()
    }

    fn rail(&self, road: &str, from: Vec2, to: Vec2, elevation: ElevationMode) -> RoadPathRequest {
        RoadPathRequest {
            road: self.road(road),
            path: RoadPath::Straight { from, to },
            elevation,
            snap: no_snap(),
        }
    }
}

#[test]
fn a_metro_tunnel_passes_under_a_river_and_needs_no_bridge() {
    let mut w = W::new();
    w.terrain = terrain(98.0, |x, _| 97.0 + 0.04 * (x - 5045.0).abs());
    let depth =
        w.db.road(w.road("metro_track"))
            .tunnel
            .as_ref()
            .unwrap()
            .depth;
    let r = w.rail(
        "metro_track",
        at(-60.0, 0.0),
        at(150.0, 0.0),
        ElevationMode::Tunnel(depth),
    );
    let plan = w.run(&r).unwrap();
    assert!(
        plan.segments
            .iter()
            .all(|s| s.elevation == Elevation::Tunnel)
    );
    // The axis follows the terrain at the given depth, the river bed included.
    for (i, p) in plan.new_nodes.iter().enumerate() {
        let ground = w.terrain.height(to_xz(*p));
        assert!((ground - p.y - depth).abs() < 0.01, "node {i}");
    }
    let per_m = w.db.road(r.road).tunnel.as_ref().unwrap().cost_per_m;
    for s in &plan.segments {
        assert_eq!(s.cost, segment_cost(s.length, per_m));
    }
    // The tunnel is the only version of the metro: not on the ground, not raised.
    let mut ground = r.clone();
    ground.elevation = ElevationMode::Ground;
    assert_eq!(kinds(&w.run(&ground).unwrap_err()), ["TunnelOnly"]);
    ground.elevation = ElevationMode::Elevated(4.0);
    assert_eq!(kinds(&w.run(&ground).unwrap_err()), ["NoElevatedVariant"]);
    // A track that is only built on the ground has no tunnel (roads have one since T-173).
    let road = w.rail(
        "rail_2t",
        at(0.0, 0.0),
        at(80.0, 0.0),
        ElevationMode::Tunnel(12.0),
    );
    assert_eq!(kinds(&w.run(&road).unwrap_err()), ["NoTunnelVariant"]);
}

#[test]
fn a_tunnel_ignores_the_street_above_it_but_not_other_tunnels() {
    let mut w = W::new();
    w.add(at(45.0, -60.0), at(45.0, 60.0));
    let r = w.rail(
        "metro_track",
        at(0.0, 0.0),
        at(90.0, 0.0),
        ElevationMode::Tunnel(12.0),
    );
    let plan = w.run(&r).unwrap();
    assert!(plan.splits.is_empty(), "no node with the street above");

    // Another tunnel on the same depth: a node.
    let t = w.add_as(
        "metro_track",
        at(45.0, -60.0),
        at(45.0, 60.0),
        88.0,
        Elevation::Tunnel,
    );
    let plan = w.run(&r).unwrap();
    assert_eq!(plan.splits.len(), 1);
    assert_eq!(plan.splits[0].segment, t);

    // 8 m deeper (gap >= 5 m): they pass each other, no node.
    let deep = w.rail(
        "metro_track",
        at(0.0, 0.0),
        at(90.0, 0.0),
        ElevationMode::Tunnel(20.0),
    );
    let plan = w.run(&deep).unwrap();
    assert!(plan.splits.is_empty());
    // 3 m deeper: neither a node nor room to pass.
    let near = w.rail(
        "metro_track",
        at(0.0, 0.0),
        at(90.0, 0.0),
        ElevationMode::Tunnel(15.0),
    );
    assert_eq!(kinds(&w.run(&near).unwrap_err()), ["TunnelClearance"]);
}

#[test]
fn track_never_crosses_a_road_in_the_level_but_may_pass_over_it() {
    let mut w = W::new();
    w.add(at(0.0, -150.0), at(0.0, 150.0));
    let r = w.rail(
        "rail_2t",
        at(-200.0, 0.0),
        at(200.0, 0.0),
        ElevationMode::Ground,
    );
    assert_eq!(kinds(&w.run(&r).unwrap_err()), ["RailCrossesRoad"]);
    // And a road does not cross a track at ground level either.
    let mut w2 = W::new();
    w2.add_as(
        "rail_2t",
        at(0.0, -150.0),
        at(0.0, 150.0),
        100.0,
        Elevation::Ground,
    );
    let road = w2.straight(at(-100.0, 0.0), at(100.0, 0.0));
    assert_eq!(kinds(&w2.run(&road).unwrap_err()), ["RailCrossesRoad"]);
    // Raised, the track passes over the street without a node.
    let mut over = r.clone();
    over.elevation = ElevationMode::Elevated(4.0);
    let plan = w.run(&over).unwrap();
    assert!(plan.splits.is_empty());
    // Two tracks at ground level meet in a node as roads do.
    let mut w3 = W::new();
    let t = w3.add_as(
        "rail_2t",
        at(0.0, -150.0),
        at(0.0, 150.0),
        100.0,
        Elevation::Ground,
    );
    let plan = w3.run(&r).unwrap();
    assert_eq!(plan.splits.len(), 1);
    assert_eq!(plan.splits[0].segment, t);
}

#[test]
fn track_and_road_do_not_share_a_node() {
    let mut w = W::new();
    let road = w.add(at(0.0, 0.0), at(80.0, 0.0));
    let r = w.rail(
        "rail_2t",
        at(82.0, 0.0),
        at(82.0, 90.0),
        ElevationMode::Ground,
    );
    let mut r = r;
    r.snap = SnapOptions::default();
    let plan = w.run(&r).unwrap();
    assert!(
        plan.snaps
            .iter()
            .all(|s| !matches!(s.kind, SnapKind::Node(_) | SnapKind::Segment(_))),
        "{:?}",
        plan.snaps
    );
    assert!(plan.splits.is_empty());
    let _ = road;
}

// ----- T-064: crossings of raised roads, ramps, short raised pieces -----

fn raised(w: &W, road: &str, from: Vec2, to: Vec2, h: f32) -> RoadPathRequest {
    RoadPathRequest {
        road: w.road(road),
        path: RoadPath::Straight { from, to },
        elevation: ElevationMode::Elevated(h),
        snap: no_snap(),
    }
}

/// Raised roads that cross: the same level meets in a node, a gap under the clearance is
/// refused, a bigger one passes.
#[test]
fn raised_roads_cross_in_a_node_at_one_level_and_are_refused_or_pass_at_others() {
    let mut w = W::new();
    // An existing deck 5 m above the 100 m ground, along x.
    w.add_as(
        "basic_2l",
        at(-150.0, 0.0),
        at(150.0, 0.0),
        105.0,
        Elevation::Elevated,
    );
    // The same level: a node on the old deck.
    let same = w
        .run(&raised(
            &w,
            "basic_2l",
            at(0.0, -200.0),
            at(0.0, 200.0),
            5.0,
        ))
        .unwrap();
    assert_eq!(same.splits.len(), 1, "the crossing is a node");
    // 7.5 m: 2.5 m under the old deck is neither one level nor clear of it.
    let close = w
        .run(&raised(
            &w,
            "basic_2l",
            at(0.0, -200.0),
            at(0.0, 200.0),
            7.5,
        ))
        .unwrap_err();
    assert_eq!(kinds(&close), ["ElevatedClearance"]);
    // 10 m: 5 m over the old deck, clear.
    let clear = w
        .run(&raised(
            &w,
            "basic_2l",
            at(0.0, -200.0),
            at(0.0, 200.0),
            10.0,
        ))
        .unwrap();
    assert!(clear.splits.is_empty());
}

/// Property: random pairs of raised roads never cross closer than the clearance without a node
/// (the plan splits the old deck where they meet).
#[test]
fn random_raised_pairs_never_cross_without_a_node_under_the_clearance() {
    let mut seed = 0x2545_F491u32;
    let mut rnd = |lo: f32, hi: f32| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        lo + (seed >> 8) as f32 / (1u32 << 24) as f32 * (hi - lo)
    };
    let mut crossings = 0;
    for _ in 0..400 {
        let mut w = W::new();
        let h1 = [5.0f32, 10.0][(rnd(0.0, 2.0) as usize).min(1)];
        let (a, b) = (
            at(rnd(-250.0, -150.0), rnd(-60.0, 60.0)),
            at(rnd(150.0, 250.0), rnd(-60.0, 60.0)),
        );
        let old = w.add_as("basic_2l", a, b, 100.0 + h1, Elevation::Elevated);
        let h2 = [5.0f32, 7.5, 10.0][(rnd(0.0, 3.0) as usize).min(2)];
        let (c, d) = (
            at(rnd(-60.0, 60.0), rnd(-250.0, -150.0)),
            at(rnd(-60.0, 60.0), rnd(150.0, 250.0)),
        );
        let Ok(plan) = w.run(&raised(&w, "basic_2l", c, d, h2)) else {
            continue;
        };
        let old_curve = w.graph.segment(old).unwrap().curve;
        for s in &plan.segments {
            for (tc, to) in s.curve.intersect_xz(&old_curve) {
                if !(0.002..=0.998).contains(&tc) || !(0.002..=0.998).contains(&to) {
                    continue;
                }
                crossings += 1;
                let gap = (s.curve.point(tc).y - old_curve.point(to).y).abs();
                if gap < ELEVATED_CLEARANCE {
                    assert!(
                        plan.splits
                            .iter()
                            .any(|sp| sp.segment == old && (sp.t - to).abs() < 0.02),
                        "a crossing with a gap of {gap} m and no node"
                    );
                }
            }
        }
    }
    assert!(crossings > 10, "{crossings} crossings tried");
}

/// A ramp from the ground up to a raised node: 100 m up 6 m is the highway's 6 %, 100 m up 12 m
/// is too steep.
#[test]
fn a_highway_ramp_of_6_percent_builds_and_12_percent_is_refused() {
    for (h, ok) in [(6.0f32, true), (12.0, false)] {
        let mut w = W::new();
        // A raised highway deck standing at `h`, from x = 200 to 400.
        w.add_as(
            "highway_1w_2l",
            at(200.0, 0.0),
            at(400.0, 0.0),
            100.0 + h,
            Elevation::Elevated,
        );
        let ramp = RoadPathRequest {
            road: w.road("highway_1w_2l"),
            path: RoadPath::Straight {
                from: at(100.0, 0.0),
                to: at(200.0, 0.0),
            },
            elevation: ElevationMode::Elevated(h),
            snap: SnapOptions::default(),
        };
        let r = w.run(&ramp);
        assert_eq!(r.is_ok(), ok, "{h} m over 100 m: {r:?}");
        if !ok {
            assert!(kinds(&r.unwrap_err()).contains(&"TooSteep"));
        }
    }
}

/// A raised path that ends a few metres past a crossing: the short last piece is cut off and the
/// path ends on the crossing node instead of being refused (on the ground it is refused).
#[test]
fn a_short_raised_tail_past_a_crossing_is_cut_off() {
    let mut w = W::new();
    w.add_as(
        "basic_2l",
        at(200.0, -150.0),
        at(200.0, 150.0),
        105.0,
        Elevation::Elevated,
    );
    let plan = w
        .run(&raised(&w, "basic_2l", at(-60.0, 0.0), at(205.0, 0.0), 5.0))
        .unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(plan.splits.len(), 1, "a node on the old deck");
    let last = plan.segments.last().unwrap();
    assert!(
        (last.curve.p3.x - at(200.0, 0.0).x).abs() < 0.5,
        "the path ends on the crossing, not 5 m after it: {:?}",
        last.curve.p3
    );
}

// ---------- pedestrian paths (T-222) ----------

impl W {
    fn path(
        &self,
        from: Vec2,
        to: Vec2,
        elevation: ElevationMode,
        snap: SnapOptions,
    ) -> RoadPathRequest {
        RoadPathRequest {
            road: self.road("park_path"),
            path: RoadPath::Straight { from, to },
            elevation,
            snap,
        }
    }
}

#[test]
fn a_path_shares_a_node_with_a_road_and_splits_it() {
    let mut w = W::new();
    let s = w.add(at(0.0, 0.0), at(0.0, 80.0));
    let node = w.graph.segment(s).unwrap().end;
    let r = w.path(
        at(60.0, 70.0),
        at(9.0, 80.0),
        ElevationMode::Ground,
        SnapOptions::default(),
    );
    let plan = w.run(&r).unwrap();
    assert_eq!(plan.segments[0].end, NodeRef::Existing(node));
    // Across the middle of the road: a junction is made in it.
    let r = w.path(
        at(-40.0, 40.0),
        at(40.0, 40.0),
        ElevationMode::Ground,
        SnapOptions::default(),
    );
    assert_eq!(w.run(&r).unwrap().splits.len(), 1);
}

#[test]
fn a_path_never_meets_track_lies_only_on_the_ground() {
    let mut w = W::new();
    let t = w.add_as(
        "rail_2t",
        at(0.0, -150.0),
        at(0.0, 150.0),
        100.0,
        Elevation::Ground,
    );
    let node = w.graph.segment(t).unwrap().end;
    // Across the track: refused, as for a road.
    let r = w.path(
        at(-100.0, 0.0),
        at(100.0, 0.0),
        ElevationMode::Ground,
        SnapOptions::default(),
    );
    assert_eq!(kinds(&w.run(&r).unwrap_err()), ["RailCrossesRoad"]);
    // Ending next to a track node does not snap to it.
    let r = w.path(
        at(60.0, 140.0),
        at(5.0, 150.0),
        ElevationMode::Ground,
        SnapOptions::default(),
    );
    let plan = w.run(&r).unwrap();
    assert_ne!(plan.segments[0].end, NodeRef::Existing(node));
    // No raised or underground version.
    let r = w.path(
        at(-100.0, 0.0),
        at(-50.0, 0.0),
        ElevationMode::Elevated(4.0),
        no_snap(),
    );
    assert_eq!(kinds(&w.run(&r).unwrap_err()), ["NoElevatedVariant"]);
    let r = w.path(
        at(-100.0, 0.0),
        at(-50.0, 0.0),
        ElevationMode::Tunnel(12.0),
        no_snap(),
    );
    assert_eq!(kinds(&w.run(&r).unwrap_err()), ["NoTunnelVariant"]);
}

// ---------- roads between heights (T-332, §18.1) ----------

impl W {
    /// Builds `req` into the graph as planned (no splits expected), giving its segments.
    fn build(&mut self, req: &RoadPathRequest) -> Vec<SegmentId> {
        let plan = self.run(req).unwrap();
        assert!(plan.splits.is_empty(), "{:?}", plan.splits);
        let def = self.db.road(req.road).clone();
        let ids = plan
            .segments
            .iter()
            .map(|s| {
                self.graph
                    .add_segment(&def, req.road, s.curve, s.elevation)
                    .unwrap()
            })
            .collect();
        for (&p, &e) in plan.new_nodes.iter().zip(&plan.node_elevations) {
            let id = self.graph.find_node(p).unwrap();
            self.graph.set_node_elevation(id, e);
        }
        ids
    }

    fn levels(&self, road: &str, from: Vec2, to: Vec2, h0: f32, h1: f32) -> RoadPathRequest {
        RoadPathRequest {
            road: self.road(road),
            path: RoadPath::Straight { from, to },
            elevation: ElevationMode::Levels { from: h0, to: h1 },
            snap: SnapOptions::default(),
        }
    }
}

fn ends(plan: &RoadBuildPlan) -> (f32, f32) {
    let first = plan.segments.first().unwrap().curve.p0.y;
    let last = plan.segments.last().unwrap().curve.p3.y;
    (first, last)
}

/// Decision 9: a highway (6 %) climbs 6 m in 120 m (5 %) but not in 60 m; a street (12 %) does.
#[test]
fn a_ramp_of_6_m_needs_120_m_of_highway_and_60_m_of_street() {
    let w = W::new();
    let long = w.levels("highway_1w_2l", at(0.0, 0.0), at(120.0, 0.0), 0.0, 6.0);
    let (first, last) = ends(&w.run(&long).unwrap());
    assert!((first - 100.0).abs() < 0.01, "{first}");
    assert!((last - 106.0).abs() < 0.01, "{last}");
    let short = w.levels("highway_1w_2l", at(0.0, 0.0), at(60.0, 0.0), 0.0, 6.0);
    let errs = kinds(&w.run(&short).unwrap_err());
    assert!(
        !errs.is_empty() && errs.iter().all(|k| *k == "TooSteep"),
        "{errs:?}"
    );
    let street = w.levels("basic_2l", at(0.0, 0.0), at(60.0, 0.0), 0.0, 6.0);
    assert!(w.run(&street).is_ok());
}

/// A9 ends: a new end stands at its height, an end on a node takes the node's height.
#[test]
fn a_new_end_stands_at_its_height_and_an_old_one_keeps_the_nodes() {
    let mut w = W::new();
    w.add(at(-200.0, 0.0), at(0.0, 0.0));
    // From the end of a street to free ground: ends 6 m up, in the air.
    let up = w.levels("basic_2l", at(0.0, 0.0), at(200.0, 0.0), 6.0, 6.0);
    let plan = w.run(&up).unwrap();
    assert!(matches!(plan.segments[0].start, NodeRef::Existing(_)));
    let (first, last) = ends(&plan);
    assert!(
        (first - 100.0).abs() < 0.01 && (last - 106.0).abs() < 0.01,
        "{first} {last}"
    );
    assert_eq!(*plan.node_elevations.last().unwrap(), Elevation::Elevated);
    w.build(&up);
    // On from that node to 12 m.
    let on = w.levels("basic_2l", at(200.0, 0.0), at(400.0, 0.0), 6.0, 12.0);
    let (first, last) = ends(&w.run(&on).unwrap());
    assert!(
        (first - 106.0).abs() < 0.01 && (last - 112.0).abs() < 0.01,
        "{first} {last}"
    );
    // Down from 12 m onto the node: it ends at the node's 6 m.
    let back = w.levels("basic_2l", at(200.0, 200.0), at(200.0, 0.0), 12.0, 6.0);
    let plan = w.run(&back).unwrap();
    assert!(matches!(
        plan.segments.last().unwrap().end,
        NodeRef::Existing(_)
    ));
    let (first, last) = ends(&plan);
    assert!(
        (first - 112.0).abs() < 0.01 && (last - 106.0).abs() < 0.01,
        "{first} {last}"
    );
    // Asked to end 12 m up on the 6 m node, it still ends at 6 m: never a mismatch.
    let high = w.levels("basic_2l", at(200.0, -200.0), at(200.0, 0.0), 12.0, 12.0);
    let (_, last) = ends(&w.run(&high).unwrap());
    assert!((last - 106.0).abs() < 0.01, "{last}");
}

#[test]
fn heights_on_two_sides_of_the_ground_are_refused_and_zero_is_the_ground() {
    let w = W::new();
    let cross = w.levels("basic_2l", at(0.0, 0.0), at(300.0, 0.0), 6.0, -6.0);
    assert_eq!(kinds(&w.run(&cross).unwrap_err()), ["LevelsCrossGround"]);
    let bad = w.levels("basic_2l", at(0.0, 0.0), at(300.0, 0.0), 0.0, f32::NAN);
    assert_eq!(kinds(&w.run(&bad).unwrap_err()), ["BadCoordinates"]);
    let ground = w.levels("basic_2l", at(0.0, 0.0), at(200.0, 0.0), 0.0, 0.0);
    let plan = w.run(&ground).unwrap();
    assert!(
        plan.segments
            .iter()
            .all(|s| s.elevation == Elevation::Ground)
    );
}

/// A4: the pieces and nodes of a ramp take their class from their height over the ground.
#[test]
fn a_ramp_is_ground_where_it_is_low_and_raised_where_it_is_high() {
    let w = W::new();
    // 0 → 6 m over 200 m of street, cut in three: the end node is raised, the start node is a
    // ground node; a deck that goes down below the ground is a tunnel.
    let up = w.levels("basic_2l", at(0.0, 0.0), at(200.0, 0.0), 0.0, 6.0);
    let plan = w.run(&up).unwrap();
    assert_eq!(plan.node_elevations.first(), Some(&Elevation::Ground));
    assert_eq!(plan.node_elevations.last(), Some(&Elevation::Elevated));
    assert!(
        plan.segments
            .iter()
            .all(|s| s.elevation == Elevation::Elevated)
    );
    let down = w.levels("basic_2l", at(0.0, 0.0), at(200.0, 0.0), 0.0, -6.0);
    let plan = w.run(&down).unwrap();
    assert_eq!(plan.node_elevations.first(), Some(&Elevation::Ground));
    assert_eq!(plan.node_elevations.last(), Some(&Elevation::Tunnel));
    assert!(
        plan.segments
            .iter()
            .all(|s| s.elevation == Elevation::Tunnel)
    );
    // A ramp that crosses a street where it is still low meets it in a node; the short piece
    // up to it is a ground piece.
    let mut w = W::new();
    w.add(at(40.0, -100.0), at(40.0, 100.0));
    let mut r = w.levels("highway_1w_2l", at(0.0, 0.0), at(300.0, 0.0), 0.0, 6.0);
    r.snap = no_snap();
    let plan = w.run(&r).unwrap();
    assert_eq!(plan.splits.len(), 1, "a node on the street");
    assert_eq!(plan.segments[0].elevation, Elevation::Ground);
    assert!(plan.segments.last().unwrap().elevation == Elevation::Elevated);
}

/// A3: a node takes a segment end only at its own height.
#[test]
fn the_graph_refuses_an_end_off_the_nodes_height() {
    let mut w = W::new();
    w.add(at(0.0, 0.0), at(100.0, 0.0));
    let id = w.road("basic_2l");
    let def = w.db.road(id).clone();
    let p = |y: f32| Vec3::new(at(100.0, 0.0).x, y, at(100.0, 0.0).y);
    let q = Vec3::new(at(100.0, 100.0).x, 103.0, at(100.0, 100.0).y);
    let line = |y: f32| CubicBezier3::line(p(y), q);
    assert_eq!(
        w.graph.check_build(&[], &[line(103.0)]),
        Err(crate::NetOpError::LevelMismatch(
            w.graph.find_node(p(100.0)).unwrap()
        ))
    );
    assert!(matches!(
        w.graph
            .add_segment(&def, id, line(103.0), Elevation::Elevated),
        Err(crate::NetOpError::LevelMismatch(_))
    ));
    // On the node, or a level above it, is fine.
    assert!(w.graph.check_build(&[], &[line(100.2)]).is_ok());
    assert!(w.graph.check_build(&[], &[line(106.0)]).is_ok());
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(32))]
    /// Decision 9: flat decks at two heights of {−12, −6, 6, 12, 18} crossing at 30…90°: on
    /// different heights no node and no error, on one height one node.
    #[test]
    fn decks_on_one_height_meet_and_on_two_pass(
        a in 0usize..5,
        b in 0usize..5,
        deg in 30.0f32..90.0,
        off in -40.0f32..40.0,
    ) {
        const H: [f32; 5] = [-12.0, -6.0, 6.0, 12.0, 18.0];
        let (ha, hb) = (H[a], H[b]);
        let mut w = W::new();
        let mut first = w.levels("basic_2l", at(-200.0, off), at(200.0, off), ha, ha);
        first.snap = no_snap();
        w.build(&first);
        let d = Vec2::from_angle(deg.to_radians()) * 200.0;
        let mut second = w.levels("basic_2l", C - d, C + d, hb, hb);
        second.snap = no_snap();
        let plan = w.run(&second);
        proptest::prop_assert!(plan.is_ok(), "{} / {}: {:?}", ha, hb, plan.err());
        let plan = plan.unwrap();
        proptest::prop_assert_eq!(plan.splits.len(), usize::from(a == b), "{} / {}", ha, hb);
    }
}

/// T-332 (§18.1 item 5): a deck's command strips reach one zone cell past its sidewalks, so the
/// buildings under an overpass come down as under a street (BUG-096); a ground road's do not.
#[test]
fn a_deck_strip_is_one_cell_wider_than_its_road() {
    let w = W::new();
    let road = w.road("basic_2l");
    let hw = w.db.road(road).width * 0.5;
    let strips = |elevation| {
        crate::command_strips(
            &w.graph,
            &w.terrain,
            &w.tiles,
            &w.db,
            &crate::NetCommand::BuildRoad {
                road,
                path: RoadPath::Straight {
                    from: at(0.0, 0.0),
                    to: at(300.0, 0.0),
                },
                elevation,
            },
        )
    };
    let ground = strips(ElevationMode::Ground);
    assert!(!ground.is_empty() && ground.iter().all(|(_, h)| (*h - hw).abs() < 1e-4));
    let deck = strips(ElevationMode::Levels { from: 6.0, to: 6.0 });
    assert!(!deck.is_empty());
    assert!(
        deck.iter()
            .all(|(_, h)| (*h - (hw + ZONE_CELL)).abs() < 1e-4)
    );
}
