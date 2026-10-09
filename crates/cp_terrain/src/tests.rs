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
fn tile_ownership_m1_default() {
    let o = TileOwnership::m1_default();
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
fn tile_ownership_set_and_serde_roundtrip() {
    let mut o = TileOwnership::only(2, 2);
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
