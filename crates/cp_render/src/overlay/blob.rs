//! Smooth blobs for the district and area overlays (BUG-112). The maps are 32 m cells, so a
//! painted district along a diagonal street was a staircase of world-axis steps. The data stays
//! on the grid (a building belongs to the cell its position is in, as before); only the drawing
//! changes:
//!
//! * the contour: the oriented cell sides of a district are linked into closed loops; each loop
//!   is resampled every [`SAMPLE_M`] along its length, averaged twice along the loop over a
//!   window of up to [`MAX_HALF_WINDOW`] samples each side (a triangle filter of about 200 m,
//!   capped at a quarter of the loop so a small brush shape keeps its size) and finished with
//!   [`CHAIKIN_ITERATIONS`] of Chaikin's corner cutting. Chaikin alone is not enough: on a
//!   staircase its limit curve still passes every step's side with the side's direction, a
//!   scallop with a 45° swing; the average over several steps is what straightens it. Steps up
//!   to 1:3 (a street at 18°) come out within a few degrees of straight, a 90° corner is
//!   rounded by about a cell and a disk brush is a disk;
//! * the fill: a soft-edged field. Every cell in or next to the district gets a quad whose
//!   corner alpha is the share of the four cells round that corner that belong to the district,
//!   so the tint fades over one cell across the border instead of stepping.
//!
//! Pure functions over a cell predicate; the district and the area overlays both use them.

use std::collections::BTreeMap;

use bevy::math::Vec2;

use super::{push, ribbon_with};
use crate::chunk::MeshData;

/// Spacing of the samples along a loop before smoothing, metres.
pub const SAMPLE_M: f32 = 8.0;
/// Most samples each side of a sample that the box filter averages (each of its two passes).
pub const MAX_HALF_WINDOW: usize = 6;
/// Rounds of corner cutting on a contour loop after the averaging.
pub const CHAIKIN_ITERATIONS: usize = 1;

/// The oriented sides of the cells where `inside` holds that border a cell where it does not
/// (clockwise round each cell in `(x, z)`), `(a, b)` in world metres.
pub fn border_edges(
    inside: &dyn Fn(i32, i32) -> bool,
    side: usize,
    cell: f32,
) -> Vec<(Vec2, Vec2)> {
    let mut out = Vec::new();
    for r in 0..side as i32 {
        for c in 0..side as i32 {
            if !inside(c, r) {
                continue;
            }
            let (x0, z0) = (c as f32 * cell, r as f32 * cell);
            let (x1, z1) = (x0 + cell, z0 + cell);
            for ((nc, nr), a, b) in [
                ((c, r - 1), Vec2::new(x0, z0), Vec2::new(x1, z0)),
                ((c + 1, r), Vec2::new(x1, z0), Vec2::new(x1, z1)),
                ((c, r + 1), Vec2::new(x1, z1), Vec2::new(x0, z1)),
                ((c - 1, r), Vec2::new(x0, z1), Vec2::new(x0, z0)),
            ] {
                if !inside(nc, nr) {
                    out.push((a, b));
                }
            }
        }
    }
    out
}

/// Links oriented edges that meet end to start into closed loops (each a list of corners, the
/// last joined to the first). Where two loops touch at a corner, the first unused edge out of
/// it is taken; every corner has as many edges out as in, so every edge ends up in a loop.
pub fn link_loops(edges: &[(Vec2, Vec2)], cell: f32) -> Vec<Vec<Vec2>> {
    let key = |p: Vec2| ((p.x / cell).round() as i32, (p.y / cell).round() as i32);
    let mut from: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
    for (i, (a, _)) in edges.iter().enumerate() {
        from.entry(key(*a)).or_default().push(i);
    }
    let mut used = vec![false; edges.len()];
    let mut loops = Vec::new();
    for start in 0..edges.len() {
        if used[start] {
            continue;
        }
        let mut ring = Vec::new();
        let mut i = start;
        loop {
            used[i] = true;
            ring.push(edges[i].0);
            let next = from
                .get(&key(edges[i].1))
                .and_then(|v| v.iter().copied().find(|&j| !used[j]));
            match next {
                Some(j) => i = j,
                None => break,
            }
        }
        if ring.len() >= 3 {
            loops.push(ring);
        }
    }
    loops
}

/// Chaikin's corner cutting on a closed polyline, `n` rounds: each corner is replaced by the
/// points a quarter and three quarters along its two sides.
pub fn chaikin(ring: &[Vec2], n: usize) -> Vec<Vec2> {
    let mut cur = ring.to_vec();
    for _ in 0..n {
        if cur.len() < 3 {
            break;
        }
        let mut next = Vec::with_capacity(cur.len() * 2);
        for i in 0..cur.len() {
            let (a, b) = (cur[i], cur[(i + 1) % cur.len()]);
            next.push(a.lerp(b, 0.25));
            next.push(a.lerp(b, 0.75));
        }
        cur = next;
    }
    cur
}

/// The closed polyline `ring` resampled into evenly spaced points about `step` apart (at
/// least 4 of them).
pub fn resample(ring: &[Vec2], step: f32) -> Vec<Vec2> {
    let n = ring.len();
    let sides: Vec<f32> = (0..n)
        .map(|i| ring[i].distance(ring[(i + 1) % n]))
        .collect();
    let perimeter: f32 = sides.iter().sum();
    let count = ((perimeter / step).round() as usize).max(4);
    let spacing = perimeter / count as f32;
    let mut out = Vec::with_capacity(count);
    let (mut i, mut along) = (0, 0.0f32);
    for k in 0..count {
        let target = k as f32 * spacing;
        while along + sides[i] < target && i + 1 < n {
            along += sides[i];
            i += 1;
        }
        let t = if sides[i] > 0.0 {
            ((target - along) / sides[i]).clamp(0.0, 1.0)
        } else {
            0.0
        };
        out.push(ring[i].lerp(ring[(i + 1) % n], t));
    }
    out
}

/// Each point of the closed polyline replaced by the mean of itself and `half` neighbours each
/// way round the loop.
pub fn box_smooth(ring: &[Vec2], half: usize) -> Vec<Vec2> {
    let n = ring.len();
    if half == 0 || n < 3 {
        return ring.to_vec();
    }
    let half = half.min((n - 1) / 2);
    let w = (2 * half + 1) as f32;
    (0..n)
        .map(|i| {
            let sum: Vec2 = (0..=2 * half).map(|j| ring[(i + n + j - half) % n]).sum();
            sum / w
        })
        .collect()
}

/// The smooth contour loops of the cells where `inside` holds (see the module doc).
pub fn smooth_loops(inside: &dyn Fn(i32, i32) -> bool, side: usize, cell: f32) -> Vec<Vec<Vec2>> {
    link_loops(&border_edges(inside, side, cell), cell)
        .iter()
        .map(|ring| {
            let samples = resample(ring, SAMPLE_M);
            // The window (4 × half + 1 samples over the two passes) stays within a quarter of
            // the loop, so a small shape is rounded, not shrunk to a dot.
            let half = (samples.len() / 16).clamp(1, MAX_HALF_WINDOW);
            let smooth = box_smooth(&box_smooth(&samples, half), half);
            chaikin(&smooth, CHAIKIN_ITERATIONS)
        })
        .collect()
}

/// A ribbon of width `w` along every smooth loop, in `color`, `piece` metres a quad.
pub fn contour_ribbons(
    m: &mut MeshData,
    loops: &[Vec<Vec2>],
    w: f32,
    color: [f32; 4],
    piece: f32,
    height: &dyn Fn(Vec2) -> f32,
) {
    for ring in loops {
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            let d = (b - a).normalize_or_zero();
            // Each piece overlaps its neighbours by half a width so the joints close.
            ribbon_with(
                m,
                a - d * (w * 0.5),
                b + d * (w * 0.5),
                w,
                color,
                piece,
                height,
            );
        }
    }
}

/// The soft-edged fill of the cells where `inside` holds: a quad per cell in or beside them,
/// each corner's alpha `alpha × (cells round the corner that are inside) / 4`, at `height`.
pub fn soft_fill(
    m: &mut MeshData,
    inside: &dyn Fn(i32, i32) -> bool,
    side: usize,
    cell: f32,
    rgb: [f32; 3],
    alpha: f32,
    height: &dyn Fn(Vec2) -> f32,
) {
    let corner = |c: i32, r: i32| -> f32 {
        let n = [(c - 1, r - 1), (c, r - 1), (c - 1, r), (c, r)]
            .into_iter()
            .filter(|&(x, z)| inside(x, z))
            .count();
        alpha * n as f32 / 4.0
    };
    for r in 0..side as i32 {
        for c in 0..side as i32 {
            // In the district, or touching it at a corner.
            let near = (-1..=1).any(|dz| (-1..=1).any(|dx| inside(c + dx, r + dz)));
            if !near {
                continue;
            }
            let a = [
                corner(c, r),
                corner(c + 1, r),
                corner(c + 1, r + 1),
                corner(c, r + 1),
            ];
            if a.iter().all(|&v| v <= 0.0) {
                continue;
            }
            let (x0, z0) = (c as f32 * cell, r as f32 * cell);
            let ids = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
                .into_iter()
                .zip(a)
                .map(|((i, j), alpha)| {
                    let p = Vec2::new(x0 + i * cell, z0 + j * cell);
                    push(m, [p.x, height(p), p.y], [rgb[0], rgb[1], rgb[2], alpha])
                })
                .collect::<Vec<_>>();
            // (x, z) with z down: this winding faces up.
            m.indices
                .extend_from_slice(&[ids[0], ids[2], ids[1], ids[0], ids[3], ids[2]]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::drape_height;
    use cp_terrain::Terrain;

    const CELL: f32 = 32.0;

    /// The cells whose centre is within `radius` cells of `(cx, cz)` (the round brush).
    fn disk(cx: i32, cz: i32, radius: f32) -> impl Fn(i32, i32) -> bool {
        move |c, r| {
            let (dx, dz) = ((c - cx) as f32, (r - cz) as f32);
            (dx * dx + dz * dz).sqrt() <= radius + 1e-3
        }
    }

    /// Degrees the direction turns at each corner of a closed loop.
    fn turns(ring: &[Vec2]) -> Vec<f32> {
        (0..ring.len())
            .map(|i| {
                let a = ring[(i + ring.len() - 1) % ring.len()];
                let (b, c) = (ring[i], ring[(i + 1) % ring.len()]);
                (b - a).angle_to(c - b).abs().to_degrees()
            })
            .collect()
    }

    #[test]
    fn a_disk_brush_gives_one_smooth_loop() {
        let inside = disk(20, 20, 3.0);
        let raw = link_loops(&border_edges(&inside, 64, CELL), CELL);
        assert_eq!(raw.len(), 1, "one loop");
        assert!(
            turns(&raw[0]).iter().any(|&t| t > 89.0),
            "the raw loop has right angles"
        );
        let loops = smooth_loops(&inside, 64, CELL);
        assert_eq!(loops.len(), 1);
        let max = turns(&loops[0]).into_iter().fold(0.0f32, f32::max);
        assert!(max < 25.0, "max turn {max}° after smoothing");
        // The loop still goes round the disk: every corner within a cell of its radius.
        let centre = Vec2::new(20.5 * CELL, 20.5 * CELL);
        for p in &loops[0] {
            let d = p.distance(centre) / CELL;
            assert!((2.5..=4.2).contains(&d), "{d} cells from the centre");
        }
    }

    /// The worst angle, degrees, by which a piece of the long sides of a stroke of the
    /// 3-cell brush along the line through `(10, 10)` with slope `dz/dx` leaves that line.
    fn worst_slant(dx: f32, dz: f32) -> f32 {
        let dir = Vec2::new(dx, dz).normalize();
        let (a, b) = (Vec2::splat(10.5), Vec2::splat(10.5) + dir * 40.0);
        let inside = move |c: i32, r: i32| {
            let p = Vec2::new(c as f32 + 0.5, r as f32 + 0.5);
            let t = (p - a).dot(dir).clamp(0.0, 40.0);
            p.distance(a + dir * t) <= 3.0 + 1e-3
        };
        let loops = smooth_loops(&inside, 64, CELL);
        assert_eq!(loops.len(), 1);
        let ring = &loops[0];
        let mut worst = 0.0f32;
        for i in 0..ring.len() {
            let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
            // The ends of the stroke are round; only the long sides count.
            if p.distance(a * CELL) < 5.0 * CELL || p.distance(b * CELL) < 5.0 * CELL {
                continue;
            }
            let along = (q - p).normalize_or_zero().dot(dir).abs().min(1.0);
            worst = worst.max(along.acos().to_degrees());
        }
        worst
    }

    #[test]
    fn a_diagonal_stroke_has_straight_slanted_sides_not_axis_steps() {
        let raw = {
            let inside = |c: i32, r: i32| (c - r).abs() <= 1 && (10..=40).contains(&c);
            let ring = &link_loops(&border_edges(&inside, 64, CELL), CELL)[0];
            turns(ring).into_iter().fold(0.0f32, f32::max)
        };
        assert!(raw > 89.0, "the raw staircase turns by {raw}°");
        for (dx, dz) in [(1.0, 1.0), (3.0, 1.0), (1.0, 3.0), (2.0, 1.0)] {
            let worst = worst_slant(dx, dz);
            assert!(
                worst < 10.0,
                "a stroke along ({dx}, {dz}) leaves its line by {worst}°"
            );
        }
    }

    #[test]
    fn a_small_shape_is_rounded_not_shrunk() {
        // The small brush: a plus of 5 cells. Its loop must still reach out to about the
        // arms' ends, not collapse into a dot.
        let inside = |c: i32, r: i32| (c - 20).abs() + (r - 20).abs() <= 1;
        let loops = smooth_loops(&inside, 64, CELL);
        assert_eq!(loops.len(), 1);
        let centre = Vec2::splat(20.5 * CELL);
        let reach = loops[0]
            .iter()
            .map(|p| p.distance(centre) / CELL)
            .fold(0.0f32, f32::max);
        assert!((1.0..=1.6).contains(&reach), "reach {reach} cells");
    }

    #[test]
    fn the_soft_fill_fades_over_the_border_and_covers_nothing_far_away() {
        let inside = disk(20, 20, 3.0);
        let t = Terrain::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands"),
        )
        .unwrap();
        let mut m = MeshData::default();
        soft_fill(&mut m, &inside, 64, CELL, [1.0, 0.0, 0.0], 0.4, &|p| {
            drape_height(&t, None, p) + 0.3
        });
        let alphas: Vec<f32> = m.colors.iter().map(|c| c[3]).collect();
        assert!(
            alphas.iter().any(|&a| (a - 0.4).abs() < 1e-5),
            "full inside"
        );
        assert!(
            alphas.iter().any(|&a| (a - 0.1).abs() < 1e-5),
            "a quarter at a corner"
        );
        assert!(alphas.iter().all(|&a| (0.0..=0.4 + 1e-5).contains(&a)));
        // Only cells in or beside the disk: 29 inside plus a ring, well under 10 x 10.
        assert!(m.indices.len() / 6 < 100 && m.indices.len() / 6 >= 29);
    }
}
