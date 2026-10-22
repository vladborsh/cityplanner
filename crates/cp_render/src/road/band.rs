//! Flat bands the width of a road (`architecture/system-design.md` §13.5): the road ghost and the
//! bulldozer highlight. The full profile extrusion needs lanes and junction geometry, which a
//! preview does not have; a band is the same footprint without them.
//!
//! Rings every [`BAND_STEP`] m along the centre line, the left edge on the left of the tangent
//! exactly like the extrusion (`left_of`), draped on the terrain and lifted by `lift`.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy_math::{Vec2, Vec3};
use cp_core::curve::{CubicBezier3, to_xz};
use cp_terrain::Terrain;

use super::{left_of, push_vertex, quad};
use crate::chunk::MeshData;

/// Distance between rings along a curve, metres.
pub const BAND_STEP: f32 = 4.0;
/// Default height above the ground, metres.
pub const BAND_LIFT: f32 = 0.3;

/// A band of `half_width` either side of the polyline `points`, draped on the terrain but never
/// below the polyline's own height (elevated and bridge pieces). `colour` is linear RGBA for every vertex.
pub fn band_polyline(
    points: &[Vec3],
    half_width: f32,
    terrain: Option<&Terrain>,
    lift: f32,
    colour: [f32; 4],
) -> MeshData {
    strip_polyline(points, -half_width, half_width, terrain, lift, colour)
}

/// The strip between lateral offsets `lo` and `hi` (positive = left of travel, as lane
/// offsets) of the polyline `points`: one lane of a road ghost (T-302d).
pub fn strip_polyline(
    points: &[Vec3],
    lo: f32,
    hi: f32,
    terrain: Option<&Terrain>,
    lift: f32,
    colour: [f32; 4],
) -> MeshData {
    let mut m = MeshData::default();
    if points.len() < 2 {
        return m;
    }
    let mut prev: Option<[u32; 2]> = None;
    for (i, p) in points.iter().enumerate() {
        let a = points[i.saturating_sub(1)];
        let b = points[(i + 1).min(points.len() - 1)];
        let left = left_of(b - a);
        let c = to_xz(*p);
        let at = |q: Vec2| {
            // Ground pieces follow the terrain; an elevated piece keeps its own height (the
            // polyline already carries it), so it never drops to the ground.
            let y = terrain.map_or(p.y, |t| t.height(q).max(p.y));
            Vec3::new(q.x, y + lift, q.y)
        };
        let l = push_vertex(&mut m, at(c + left * hi), Vec3::Y, colour);
        let r = push_vertex(&mut m, at(c + left * lo), Vec3::Y, colour);
        if let Some([pl, pr]) = prev {
            quad(&mut m, [pl, l, r, pr], Vec3::Y);
        }
        prev = Some([l, r]);
    }
    m
}

/// The band along `curve`, rings about every [`BAND_STEP`] m.
pub fn band_data(
    curve: &CubicBezier3,
    half_width: f32,
    terrain: Option<&Terrain>,
    lift: f32,
    colour: [f32; 4],
) -> MeshData {
    let n = (curve.arc_table().length() / BAND_STEP).ceil().max(1.0) as usize;
    let pts: Vec<Vec3> = (0..=n).map(|i| curve.point(i as f32 / n as f32)).collect();
    band_polyline(&pts, half_width, terrain, lift, colour)
}

pub fn to_mesh(m: MeshData) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, m.colors)
    .with_inserted_indices(Indices::U32(m.indices))
}

/// [`band_data`] as a Bevy mesh.
pub fn band_mesh(
    curve: &CubicBezier3,
    half_width: f32,
    terrain: Option<&Terrain>,
    lift: f32,
    colour: [f32; 4],
) -> Mesh {
    to_mesh(band_data(curve, half_width, terrain, lift, colour))
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: [f32; 4] = [1.0; 4];

    #[test]
    fn a_straight_band_is_as_wide_as_the_road_on_both_sides() {
        let c = CubicBezier3::line(Vec3::new(500.0, 0.0, 500.0), Vec3::new(500.0, 0.0, 600.0));
        let m = band_data(&c, 6.0, None, BAND_LIFT, C);
        assert_eq!(m.positions.len(), 2 * 26, "rings every 4 m: 25 spans + 1");
        assert!(
            m.positions
                .iter()
                .all(|p| ((p[0] - 500.0).abs() - 6.0).abs() < 1e-3),
            "every vertex is 6 m off the axis"
        );
        assert!(m.positions.iter().all(|p| (p[1] - BAND_LIFT).abs() < 1e-4));
        // Faces up.
        for t in m.indices.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(m.positions[i as usize]));
            assert!((b - a).cross(c - a).y > 0.0);
        }
    }

    #[test]
    fn the_left_edge_is_on_the_left_of_the_tangent() {
        // Heading +X, left in the extrusion's convention is (tz, -tx) = -Z.
        let c = CubicBezier3::line(Vec3::ZERO, Vec3::new(40.0, 0.0, 0.0));
        let m = band_data(&c, 3.0, None, 0.0, C);
        assert!((m.positions[0][2] + 3.0).abs() < 1e-4);
        assert!((m.positions[1][2] - 3.0).abs() < 1e-4);
    }

    #[test]
    fn a_curve_of_radius_30_has_rings_that_do_not_fold_over() {
        // A quarter circle of radius 30 as a cubic, band half-width 6 (large_6l is 14).
        let k = 0.5523 * 30.0;
        let c = CubicBezier3::new(
            Vec3::new(30.0, 0.0, 0.0),
            Vec3::new(30.0, 0.0, k),
            Vec3::new(k, 0.0, 30.0),
            Vec3::new(0.0, 0.0, 30.0),
        );
        let m = band_data(&c, 6.0, None, 0.0, C);
        let ring = |i: usize| {
            let (l, r) = (
                Vec3::from(m.positions[2 * i]),
                Vec3::from(m.positions[2 * i + 1]),
            );
            (l, r)
        };
        let rings = m.positions.len() / 2;
        assert!(rings > 5);
        for i in 0..rings - 1 {
            let (l0, r0) = ring(i);
            let (l1, r1) = ring(i + 1);
            let axis = ((l1 + r1) - (l0 + r0)) * 0.5;
            // Both edges advance along the axis direction: no fold, no self-intersection.
            assert!(
                (l1 - l0).dot(axis) > 0.0 && (r1 - r0).dot(axis) > 0.0,
                "ring {i}"
            );
        }
    }

    #[test]
    fn a_band_drapes_on_the_terrain_and_short_input_is_empty() {
        assert!(
            band_polyline(&[Vec3::ZERO], 3.0, None, 0.0, C)
                .indices
                .is_empty()
        );
        let mesh = band_mesh(
            &CubicBezier3::line(Vec3::ZERO, Vec3::new(20.0, 0.0, 0.0)),
            3.0,
            None,
            0.3,
            C,
        );
        assert!(mesh.count_vertices() > 0);
    }

    #[test]
    fn an_elevated_polyline_keeps_its_height_and_a_ground_one_follows_the_terrain() {
        use cp_terrain::{GRID, MapMeta};
        let meta = MapMeta {
            name: "t".into(),
            sea_level: 1.0,
            start_tile: (2, 2),
            external_connections: vec![],
            entry_road: None,
            entry_rail: None,
        };
        let n = GRID * GRID;
        let terrain = Terrain::from_samples(meta, &vec![1000; n], &vec![0; n]).unwrap();
        let ground = terrain.height(Vec2::new(500.0, 500.0));
        let line = |y: f32| {
            [
                Vec3::new(500.0, y, 500.0),
                Vec3::new(500.0, y, 540.0),
                Vec3::new(500.0, y, 580.0),
            ]
        };
        let low = band_polyline(&line(ground), 6.0, Some(&terrain), BAND_LIFT, C);
        assert!(
            low.positions
                .iter()
                .all(|p| (p[1] - ground - BAND_LIFT).abs() < 1e-3)
        );
        let high = band_polyline(&line(ground + 5.0), 6.0, Some(&terrain), BAND_LIFT, C);
        assert!(
            high.positions
                .iter()
                .all(|p| (p[1] - (ground + 5.0 + BAND_LIFT)).abs() < 1e-3),
            "an elevated piece keeps y + lift"
        );
    }
}
