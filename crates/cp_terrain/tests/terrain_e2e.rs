//! Acceptance tests for `m0-data` terrain: committed maps, boundaries, ownership.

use bevy_math::Vec2;
use cp_terrain::*;
use std::path::{Path, PathBuf};

fn map(name: &str) -> PathBuf {
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

/// Flat terrain at `h` metres everywhere, no water mask.
fn flat(h: f32, sea: f32) -> Terrain {
    let s = (h / MAX_HEIGHT * 65535.0).round() as u16;
    Terrain::from_samples(meta(sea), &vec![s; GRID * GRID], &vec![0; GRID * GRID]).unwrap()
}

fn all_finite(t: &Terrain, p: Vec2) {
    assert!(t.height(p).is_finite(), "height {p:?}");
    assert!(t.water_depth(p).is_finite(), "depth {p:?}");
    let n = t.normal(p);
    assert!(
        n.is_finite() && (n.length() - 1.0).abs() < 1e-4,
        "normal {p:?} {n:?}"
    );
}

#[test]
fn committed_maps_are_valid_and_loading_is_deterministic() {
    for name in ["flatlands", "river_valley"] {
        let a = Terrain::load(&map(name)).unwrap();
        let b = Terrain::load(&map(name)).unwrap();
        assert_eq!(a.meta(), b.meta());
        for iz in (0..GRID).step_by(7) {
            for ix in (0..GRID).step_by(7) {
                assert_eq!(
                    a.node_height(ix, iz).to_bits(),
                    b.node_height(ix, iz).to_bits()
                );
            }
        }
        let m = a.meta();
        assert_eq!(m.name, name);
        assert!(
            usize::from(m.start_tile.0) < TILES_PER_SIDE
                && usize::from(m.start_tile.1) < TILES_PER_SIDE
        );
        for &(x, z) in &m.external_connections {
            assert!(
                (0.0..=MAP_SIZE).contains(&x) && (0.0..=MAP_SIZE).contains(&z),
                "{name}: connection {x},{z} outside the map"
            );
        }
        // The default M1 area must contain dry, buildable land.
        let (c0, c1) = (TILE_SIZE, TILE_SIZE * 4.0);
        let mut dry = 0;
        let mut p = c0;
        while p < c1 {
            let mut q = c0;
            while q < c1 {
                if !a.is_water(Vec2::new(p, q)) {
                    dry += 1
                }
                q += 100.0;
            }
            p += 100.0;
        }
        assert!(dry > 100, "{name}: only {dry} dry samples in the M1 tiles");
    }
}

#[test]
fn queries_never_misbehave_at_the_boundaries() {
    let t = Terrain::load(&map("river_valley")).unwrap();
    let e = MAP_SIZE;
    let pts = [
        Vec2::ZERO,
        Vec2::new(e, e),
        Vec2::new(e, 0.0),
        Vec2::new(0.0, e),
        Vec2::new(-1.0, -1.0),
        Vec2::new(e + 1.0, e + 1.0),
        Vec2::new(-1e30, 1e30),
        Vec2::new(f32::INFINITY, f32::NEG_INFINITY),
        Vec2::new(f32::NAN, 5.0),
        Vec2::new(5.0, f32::NAN),
        Vec2::new(SPACING, SPACING),
        Vec2::new(SPACING * 1023.0, SPACING * 1023.0),
        Vec2::new(e - 1e-3, e - 1e-3),
        Vec2::new(TILE_SIZE, TILE_SIZE),
    ];
    for p in pts {
        if p.is_nan() {
            // must not panic; value unspecified
            let _ = (
                t.height(p),
                t.water_depth(p),
                t.normal(p),
                Terrain::tile_of(p),
            );
        } else {
            all_finite(&t, p);
        }
    }
    // Continuity across a cell boundary.
    let (a, b) = (
        Vec2::new(SPACING - 1e-3, 500.0),
        Vec2::new(SPACING + 1e-3, 500.0),
    );
    assert!((t.height(a) - t.height(b)).abs() < 0.01);
    // Node values are returned exactly.
    assert!((t.height(Vec2::new(30.0, 40.0)) - t.node_height(3, 4)).abs() < 1e-3);
    assert_eq!(
        t.node_height(usize::MAX, usize::MAX),
        t.node_height(GRID - 1, GRID - 1)
    );
}

#[test]
fn tile_of_is_exact_on_tile_edges() {
    assert_eq!(Terrain::tile_of(Vec2::new(0.0, 0.0)), (0, 0));
    assert_eq!(Terrain::tile_of(Vec2::new(TILE_SIZE - 0.01, 0.0)), (0, 0));
    assert_eq!(Terrain::tile_of(Vec2::new(TILE_SIZE, 0.0)), (1, 0));
    assert_eq!(Terrain::tile_of(Vec2::new(MAP_SIZE, MAP_SIZE)), (4, 4));
    assert_eq!(Terrain::tile_of(Vec2::new(-5.0, MAP_SIZE * 2.0)), (0, 4));
    let own = TileOwnership::m1_around((2, 2));
    assert!(own.contains_point(Vec2::new(TILE_SIZE, TILE_SIZE)));
    assert!(!own.contains_point(Vec2::new(TILE_SIZE - 0.01, TILE_SIZE)));
    assert!(own.contains_point(Vec2::new(TILE_SIZE * 4.0 - 0.01, TILE_SIZE * 4.0 - 0.01)));
    assert!(!own.contains_point(Vec2::new(TILE_SIZE * 4.0, TILE_SIZE * 2.0)));
}

#[test]
fn water_is_strictly_below_the_surface() {
    let sea = 20.0;
    let level = 20.0 / MAX_HEIGHT * 65535.0;
    // ground exactly at sea level (after u16 quantisation) is dry; clearly below is wet.
    let at = flat(sea, sea);
    let g = at.node_height(0, 0);
    assert_eq!(
        at.is_water(Vec2::new(100.0, 100.0)),
        sea > g,
        "ground {g} sea {sea} ({level})"
    );
    assert!(flat(sea - 1.0, sea).is_water(Vec2::new(100.0, 100.0)));
    assert!(!flat(sea + 1.0, sea).is_water(Vec2::new(100.0, 100.0)));
    assert!(flat(sea - 1.0, sea).water_depth(Vec2::new(1.0, 1.0)) > 0.9);
    assert!(flat(sea + 1.0, sea).water_depth(Vec2::new(1.0, 1.0)) < 0.0);
}

#[test]
fn ownership_boundaries() {
    let mut o = TileOwnership::none();
    assert_eq!(o.open_count(), 0);
    for tz in 0..TILES_PER_SIDE {
        for tx in 0..TILES_PER_SIDE {
            assert!(!o.is_open(tx, tz));
        }
    }
    o.set(4, 4, true);
    assert!(o.is_open(4, 4) && !o.is_open(0, 4) && !o.is_open(4, 0) && o.open_count() == 1);
    // Out of range reads are "closed", not a wrap-around onto another tile.
    assert!(!o.is_open(5, 4) && !o.is_open(0, 5) && !o.is_open(usize::MAX, 0));
    o.set(4, 4, false);
    assert_eq!(o, TileOwnership::none());
    assert_eq!(TileOwnership::m1_around((2, 2)).open_count(), 9);
    assert_eq!(TileOwnership::only((2, 2)).open_count(), 1);
    assert!(std::panic::catch_unwind(|| TileOwnership::none().set(5, 0, true)).is_err());
}

#[test]
fn bad_input_is_an_error_not_a_panic() {
    assert!(Terrain::from_samples(meta(0.0), &[0; 4], &[0; 4]).is_err());
    let mut m = meta(0.0);
    m.start_tile = (5, 0);
    assert!(Terrain::from_samples(m, &vec![0; GRID * GRID], &vec![0; GRID * GRID]).is_err());
    assert!(Terrain::load(&map("no_such_map")).is_err());
}
