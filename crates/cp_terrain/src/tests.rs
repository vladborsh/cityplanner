use std::path::{Path, PathBuf};

use bevy_math::Vec2;

use super::*;

fn map_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/maps")
        .join(name)
}

fn meta(sea: f32) -> MapMeta {
    MapMeta {
        name: "t".into(),
        sea_level: sea,
        start_tile: (2, 2),
        external_connections: vec![],
        entry_road: None,
    }
}

fn enc(m: f32) -> u16 {
    (m / MAX_HEIGHT * 65535.0).round() as u16
}

/// Flat field of `h` m, with a 2×2 plateau at nodes (10..=11, 10..=11) of `top` m.
fn synthetic(sea: f32, h: f32, water: u16) -> Terrain {
    let mut hs = vec![enc(h); GRID * GRID];
    for (x, z) in [(10, 10), (11, 10), (10, 11), (11, 11)] {
        hs[z * GRID + x] = enc(h + 100.0);
    }
    Terrain::from_samples(meta(sea), &hs, &vec![water; GRID * GRID]).unwrap()
}

#[test]
fn height_at_nodes_matches_samples() {
    let t = synthetic(0.0, 50.0, 0);
    let top = enc(150.0) as f32 / 65535.0 * MAX_HEIGHT;
    assert!((t.height(Vec2::new(100.0, 100.0)) - top).abs() < 1e-3);
    assert!((t.height(Vec2::new(0.0, 0.0)) - t.node_height(0, 0)).abs() < 1e-4);
    assert!((t.height(Vec2::new(500.0, 500.0)) - 50.0).abs() < 0.02);
}

#[test]
fn height_between_nodes_is_bilinear() {
    let t = synthetic(0.0, 50.0, 0);
    let (a, b) = (t.node_height(10, 10), t.node_height(9, 10));
    let mid = t.height(Vec2::new(95.0, 100.0));
    assert!((mid - (a + b) / 2.0).abs() < 1e-3);
    let q = t.height(Vec2::new(97.5, 100.0));
    assert!((q - (a * 0.75 + b * 0.25)).abs() < 1e-3);
    // Cell centre of (9,9)-(10,10): average of the four corners.
    let c = t.height(Vec2::new(95.0, 95.0));
    let want =
        (t.node_height(9, 9) + t.node_height(10, 9) + t.node_height(9, 10) + t.node_height(10, 10))
            / 4.0;
    assert!((c - want).abs() < 1e-3);
}

#[test]
fn out_of_bounds_is_clamped() {
    let t = synthetic(0.0, 50.0, 0);
    assert_eq!(
        t.height(Vec2::new(-500.0, -9.0)),
        t.height(Vec2::new(0.0, 0.0))
    );
    assert_eq!(
        t.height(Vec2::new(1e6, 1e6)),
        t.height(Vec2::new(MAP_SIZE, MAP_SIZE))
    );
    assert!(t.height(Vec2::new(f32::MAX, 0.0)).is_finite());
}

#[test]
fn normal_is_unit_and_up_on_flat_ground() {
    let t = synthetic(0.0, 50.0, 0);
    let n = t.normal(Vec2::new(5000.0, 5000.0));
    assert!((n - Vec3::Y).length() < 1e-4);
    // On the plateau slope the normal leans away from the high side and stays unit length.
    let n = t.normal(Vec2::new(95.0, 100.0));
    assert!((n.length() - 1.0).abs() < 1e-5);
    assert!(n.x < 0.0 && n.y > 0.0);
}

#[test]
fn water_from_sea_level() {
    let t = synthetic(60.0, 50.0, 0);
    assert!((t.water_depth(Vec2::new(5000.0, 5000.0)) - 10.0).abs() < 0.02);
    assert!(t.is_water(Vec2::new(5000.0, 5000.0)));
    // Plateau (150 m) is dry.
    assert!(!t.is_water(Vec2::new(100.0, 100.0)));
    let dry = synthetic(40.0, 50.0, 0);
    assert!(!dry.is_water(Vec2::new(5000.0, 5000.0)));
    assert!(dry.water_depth(Vec2::new(5000.0, 5000.0)) < 0.0);
}

#[test]
fn water_mask_overrides_sea_when_higher() {
    // Mask surface 70 m over 50 m ground, sea 60 m: surface = max = 70.
    let t = synthetic(60.0, 50.0, enc(70.0));
    assert!((t.water_depth(Vec2::new(5000.0, 5000.0)) - 20.0).abs() < 0.03);
    // Mask below sea level has no effect.
    let t = synthetic(60.0, 50.0, enc(55.0));
    assert!((t.water_depth(Vec2::new(5000.0, 5000.0)) - 10.0).abs() < 0.02);
}

#[test]
fn tile_ownership_m1_around_centre() {
    let o = TileOwnership::m1_around((2, 2));
    assert_eq!(o.open_count(), 9);
    for tz in 0..5 {
        for tx in 0..5 {
            assert_eq!(
                o.is_open(tx, tz),
                (1..=3).contains(&tx) && (1..=3).contains(&tz)
            );
        }
    }
    assert!(!o.is_open(5, 2));
    assert!(o.contains_point(Vec2::new(5120.0, 5120.0)));
    assert!(!o.contains_point(Vec2::new(100.0, 100.0)));
    assert!(!o.contains_point(Vec2::new(-5.0, 5120.0)));
}

#[test]
fn tile_ownership_m1_around_clips_at_the_edge() {
    for (start, open) in [
        ((0, 0), 4),
        ((2, 2), 9),
        ((4, 2), 6),
        ((4, 4), 4),
        ((0, 3), 6),
    ] {
        let o = TileOwnership::m1_around(start);
        assert_eq!(o.open_count(), open, "{start:?}");
        assert!(o.is_open(start.0 as usize, start.1 as usize));
    }
    assert_eq!(TileOwnership::only((3, 1)).open_count(), 1);
    assert!(TileOwnership::only((3, 1)).is_open(3, 1));
    assert!(std::panic::catch_unwind(|| TileOwnership::m1_around((5, 0))).is_err());
}

#[test]
fn committed_maps_start_with_the_3x3_centre() {
    for name in ["flatlands", "river_valley"] {
        let t = Terrain::load(&map_dir(name)).unwrap();
        assert_eq!(t.meta().start_tile, (2, 2));
        let o = TileOwnership::m1_around(t.meta().start_tile);
        assert_eq!(o.open_count(), 9);
        assert!(o.contains_point(Vec2::new(5120.0, 5120.0)));
    }
}

#[test]
fn tile_ownership_set_and_serde_roundtrip() {
    let mut o = TileOwnership::only((2, 2));
    assert_eq!(o.open_count(), 1);
    o.set(4, 4, true);
    o.set(2, 2, false);
    assert!(o.is_open(4, 4) && !o.is_open(2, 2));
    let s = ron::to_string(&o).unwrap();
    assert_eq!(ron::from_str::<TileOwnership>(&s).unwrap(), o);
    assert_eq!(Terrain::tile_of(Vec2::new(MAP_SIZE, MAP_SIZE)), (4, 4));
    assert_eq!(Terrain::tile_of(Vec2::new(2047.9, 2048.0)), (0, 1));
}

#[test]
fn committed_maps_load() {
    for name in ["flatlands", "river_valley"] {
        let t = Terrain::load(&map_dir(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(t.meta().name, name);
        let c = Vec2::new(MAP_SIZE / 2.0, MAP_SIZE / 2.0);
        assert!(t.height(c).is_finite() && t.normal(c).is_finite());
    }
}

#[test]
fn flatlands_has_sea_and_dry_land() {
    let t = Terrain::load(&map_dir("flatlands")).unwrap();
    assert!(t.is_water(Vec2::new(100.0, 5000.0)));
    assert!(t.water_depth(Vec2::new(100.0, 5000.0)) > 2.0);
    assert!(!t.is_water(Vec2::new(6000.0, 5000.0)));
    // Plains: gentle slope at the start tile.
    assert!(t.normal(Vec2::new(5120.0, 5120.0)).y > 0.99);
}

#[test]
fn river_valley_has_river_and_hills() {
    let t = Terrain::load(&map_dir("river_valley")).unwrap();
    let mut wet = 0;
    let mut min = f32::MAX;
    let mut max = f32::MIN;
    for iz in (0..GRID).step_by(8) {
        for ix in (0..GRID).step_by(8) {
            let p = Vec2::new(ix as f32 * SPACING, iz as f32 * SPACING);
            wet += t.is_water(p) as usize;
            min = min.min(t.height(p));
            max = max.max(t.height(p));
        }
    }
    assert!(wet > 50, "river should cover some samples, got {wet}");
    assert!(max - min > 80.0, "hills expected, relief {}", max - min);
    // Centre line of the river at z = 0 is wet.
    let cx = 5120.0 + 500.0 * 1.3f32.sin();
    assert!(t.is_water(Vec2::new(cx, 0.0)));
}

#[test]
fn bad_files_report_errors() {
    assert!(matches!(
        Terrain::load(&map_dir("nope")),
        Err(TerrainError::Io(..))
    ));
}

#[test]
fn map_hash_is_stable_and_sensitive() {
    let dir = map_dir("flatlands");
    let a = Terrain::load(&dir).unwrap();
    let b = Terrain::load(&dir).unwrap();
    assert_eq!(a.map_hash(), b.map_hash());
    assert_ne!(a.map_hash(), 0);

    // One height sample, one water sample, or the meta changes the hash.
    let base =
        |hs: &[u16], ws: &[u16], m: MapMeta| Terrain::from_samples(m, hs, ws).unwrap().map_hash();
    let hs = vec![enc(50.0); GRID * GRID];
    let ws = vec![0u16; GRID * GRID];
    let h0 = base(&hs, &ws, meta(0.0));
    let mut hs2 = hs.clone();
    hs2[GRID * 500 + 500] += 1;
    assert_ne!(base(&hs2, &ws, meta(0.0)), h0);
    let mut ws2 = ws.clone();
    ws2[7] = enc(60.0);
    assert_ne!(base(&hs, &ws2, meta(0.0)), h0);
    assert_ne!(base(&hs, &ws, meta(1.0)), h0);
    let mut m = meta(0.0);
    m.name = "other".into();
    assert_ne!(base(&hs, &ws, m), h0);
    assert_eq!(base(&hs, &ws, meta(0.0)), h0);
}

#[test]
fn from_bytes_matches_load() {
    for name in ["flatlands", "river_valley"] {
        let dir = map_dir(name);
        let ron = std::fs::read_to_string(dir.join("map.ron")).unwrap();
        let h = std::fs::read(dir.join("height.png")).unwrap();
        let w = std::fs::read(dir.join("water.png")).unwrap();
        let a = Terrain::from_bytes(&ron, &h, &w).unwrap();
        let b = Terrain::load(&dir).unwrap();
        assert_eq!(a.meta(), b.meta());
        for p in [
            Vec2::new(0.0, 0.0),
            Vec2::new(5120.0, 3000.0),
            Vec2::new(10240.0, 10240.0),
        ] {
            assert_eq!(a.height(p), b.height(p));
            assert_eq!(a.water_depth(p), b.water_depth(p));
        }
    }
}

#[test]
fn from_bytes_reports_errors() {
    assert!(Terrain::from_bytes("(", &[], &[]).is_err());
    let dir = map_dir("flatlands");
    let ron = std::fs::read_to_string(dir.join("map.ron")).unwrap();
    assert!(Terrain::from_bytes(&ron, b"not a png", b"not a png").is_err());
}

// ---- entry road (T-115) ----

/// A map with sea level `sea`, flat ground of 50 m, and `entry` as its entry road: the
/// connection at the north edge of tile column 2, the start tile `(2, 2)`.
fn with_entry(sea: f32, entry: Option<EntryRoad>) -> Terrain {
    let mut m = meta(sea);
    m.external_connections = vec![(5120.0, 0.0)];
    m.entry_road = entry;
    Terrain::from_samples(m, &vec![enc(50.0); GRID * GRID], &vec![0; GRID * GRID]).unwrap()
}

fn entry(path: Vec<(f32, f32)>) -> EntryRoad {
    EntryRoad {
        connection: 0,
        path,
        highway: "highway_1w_2l".into(),
        stub: "large_6l".into(),
        stub_m: 48.0,
    }
}

fn straight() -> EntryRoad {
    entry(vec![(5120.0, 0.0), (5120.0, 4300.0)])
}

#[test]
fn a_good_entry_road_validates_and_no_entry_road_is_an_error() {
    assert_eq!(with_entry(0.0, Some(straight())).validate_entry(), Ok(()));
    let e = with_entry(0.0, None).validate_entry().unwrap_err();
    assert!(e.contains("entry_road is missing"), "{e}");
}

#[test]
fn an_entry_road_that_ends_outside_the_start_tile_or_too_close_to_its_edge_is_an_error() {
    // Still in tile (2, 1).
    let e = with_entry(0.0, Some(entry(vec![(5120.0, 0.0), (5120.0, 3000.0)])))
        .validate_entry()
        .unwrap_err();
    assert!(e.contains("inside start tile"), "{e}");
    // Inside, but 30 m from the tile edge.
    let e = with_entry(0.0, Some(entry(vec![(5120.0, 0.0), (5120.0, 4126.0)])))
        .validate_entry()
        .unwrap_err();
    assert!(e.contains("64 m"), "{e}");
    // The junction is fine, but the stub leaves the tile.
    let mut r = entry(vec![(5120.0, 0.0), (5120.0, 6000.0)]);
    r.stub_m = 200.0;
    let e = with_entry(0.0, Some(r)).validate_entry().unwrap_err();
    assert!(e.contains("stub"), "{e}");
}

#[test]
fn an_entry_road_needs_its_connection_and_must_start_on_it() {
    let mut r = straight();
    r.connection = 3;
    assert!(
        with_entry(0.0, Some(r))
            .validate_entry()
            .unwrap_err()
            .contains("connection 3")
    );
    let e = with_entry(0.0, Some(entry(vec![(5200.0, 0.0), (5120.0, 4300.0)])))
        .validate_entry()
        .unwrap_err();
    assert!(e.contains("not at external connection"), "{e}");
}

#[test]
fn an_entry_road_across_water_or_with_a_sharp_corner_is_an_error() {
    // Sea level above the ground: the whole map is under water.
    let e = with_entry(60.0, Some(straight()))
        .validate_entry()
        .unwrap_err();
    assert!(e.contains("water"), "{e}");
    // A right angle at (5120, 2000).
    let e = with_entry(
        0.0,
        Some(entry(vec![
            (5120.0, 0.0),
            (5120.0, 2000.0),
            (5700.0, 2000.0),
            (5700.0, 4300.0),
        ])),
    )
    .validate_entry()
    .unwrap_err();
    assert!(e.contains("angle"), "{e}");
    // A gentle bend (20°) is fine.
    let ok = entry(vec![
        (5120.0, 0.0),
        (5120.0, 2000.0),
        (5120.0 + 2300.0 * 0.364, 4300.0),
    ]);
    assert_eq!(with_entry(0.0, Some(ok)).validate_entry(), Ok(()));
}

#[test]
fn a_steep_entry_road_is_an_error() {
    let mut hs = vec![enc(50.0); GRID * GRID];
    for z in 0..GRID {
        for x in 0..GRID {
            hs[z * GRID + x] = enc(50.0 + z as f32 * 1.0); // 10 % grade along z
        }
    }
    let mut m = meta(0.0);
    m.external_connections = vec![(5120.0, 0.0)];
    m.entry_road = Some(straight());
    let t = Terrain::from_samples(m, &hs, &vec![0; GRID * GRID]).unwrap();
    let e = t.validate_entry().unwrap_err();
    assert!(e.contains("slope"), "{e}");
}

/// Adding `entry_road` to `MapMeta` must not change any map's identity (saves would all be
/// `MapMismatch`): the values are those from before the field existed.
#[test]
fn map_hashes_of_the_committed_maps_are_unchanged() {
    let hash = |m: &str| Terrain::load(&map_dir(m)).unwrap().map_hash();
    assert_eq!(hash("flatlands"), 0x197e_0a80_bc6b_e309);
    // river_valley: re-recorded by T-180 (meadows and a new entry road: the terrain changed).
    assert_eq!(hash("river_valley"), 0x8dd1_51a9_3494_4405);
    // And the hash ignores the entry road itself.
    let a = with_entry(0.0, Some(straight())).map_hash();
    let b = with_entry(0.0, None).map_hash();
    assert_eq!(a, b);
}

#[test]
fn committed_maps_have_a_valid_entry_road() {
    for m in ["flatlands", "river_valley"] {
        let t = Terrain::load(&map_dir(m)).unwrap_or_else(|e| panic!("{m}: {e}"));
        assert_eq!(t.validate_entry(), Ok(()), "{m}");
    }
}

#[test]
fn a_map_on_disk_without_entry_road_does_not_load() {
    // Unique per process and test: slots and concurrent runs share the temp directory.
    let dir = std::env::temp_dir().join(format!(
        "cp-terrain-no-entry-map-{}-{}",
        std::process::id(),
        "a_map_on_disk_without_entry_road_does_not_load"
    ));
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["height.png", "water.png"] {
        std::fs::copy(map_dir("flatlands").join(f), dir.join(f)).unwrap();
    }
    std::fs::write(
        dir.join("map.ron"),
        "(name: \"x\", sea_level: 20.0, start_tile: (2, 2), external_connections: [(5120.0, 0.0)])",
    )
    .unwrap();
    let result = Terrain::load(&dir);
    std::fs::remove_dir_all(&dir).unwrap();
    let e = result.unwrap_err().to_string();
    assert!(e.contains("entry_road is missing"), "{e}");
}
