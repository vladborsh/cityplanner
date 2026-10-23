//! Stepped heat layers for noise and pollution (BUG-119): what the inspector calls "low"
//! (below `cp_sim::inspect::LEVEL_LOW`, 15 of 100) is not tinted at all; the rest falls into
//! four bands 15 points wide (60 and up is the last) of distinct colour with alpha rising to the
//! loud cores, and a thin dark line runs along every edge where the band steps up, so loud areas
//! read as contour rings around their sources. The steps sit where a city's values are: a
//! factory block reaches 40-60, a stadium 30-50 on the 64 m grid. The grid is sampled
//! bilinearly at [`SUB`] points per cell side, so the bands follow round shapes instead of 64 m
//! squares.

use cp_sim::ScalarGrid;
use cp_sim::inspect::LEVEL_LOW;
use cp_terrain::{MAP_SIZE, Terrain};

use super::{LIFT, MeshData, flat};

/// Number of bands.
pub const BANDS: usize = 4;
/// Fine cells per land-grid cell side.
pub const SUB: usize = 4;
/// Width of a contour line, as a share of a fine cell.
const LINE: f32 = 0.3;
/// The contour lines sit this much above the fill, so they never fight it.
const LINE_LIFT: f32 = 0.05;

/// A stepped ramp: band `i` covers `[edges[i], edges[i + 1])` of the normalised value (the last
/// one up to 1); below `edges[0]` the cell is quiet and not drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bands {
    pub edges: [f32; BANDS],
    pub colors: [[f32; 4]; BANDS],
}

impl Bands {
    /// The band of `v`, `None` when quiet (or NaN).
    pub fn of(&self, v: f32) -> Option<usize> {
        if v.is_nan() || v < self.edges[0] {
            return None;
        }
        Some(self.edges.iter().rposition(|e| v >= *e).unwrap_or(0))
    }

    /// The colour of the contour line on the edge of band `b`: the band's colour, darker and
    /// nearly opaque.
    pub fn line_color(&self, b: usize) -> [f32; 4] {
        let c = self.colors[b];
        [c[0] * 0.4, c[1] * 0.4, c[2] * 0.4, 1.0]
    }
}

/// Noise: yellow → orange → red → deep magenta at the hot cores.
pub const NOISE: Bands = Bands {
    edges: [LEVEL_LOW / 100.0, 0.30, 0.45, 0.60],
    colors: [
        [1.00, 0.88, 0.15, 0.50],
        [1.00, 0.52, 0.05, 0.62],
        [0.92, 0.10, 0.08, 0.74],
        [0.62, 0.00, 0.52, 0.88],
    ],
};

/// Pollution: pale olive → ochre → brown → dark purple at the cores.
pub const POLLUTION: Bands = Bands {
    edges: NOISE.edges,
    colors: [
        [0.74, 0.78, 0.25, 0.50],
        [0.80, 0.52, 0.08, 0.62],
        [0.52, 0.24, 0.06, 0.74],
        [0.36, 0.05, 0.32, 0.88],
    ],
};

/// Bilinear value of `grid` at map position `(x, z)`, sampling the cell centres.
fn sample(grid: &ScalarGrid, x: f32, z: f32) -> f32 {
    let n = grid.side;
    let step = MAP_SIZE / n as f32;
    let at = |c: f32| {
        let u = (c / step - 0.5).clamp(0.0, (n - 1) as f32);
        let i = (u.floor() as usize).min(n.saturating_sub(2));
        (i, u - i as f32)
    };
    let ((ix, fx), (iz, fz)) = (at(x), at(z));
    let v = |i: usize, k: usize| grid.values[k.min(n - 1) * n + i.min(n - 1)];
    let top = v(ix, iz) * (1.0 - fx) + v(ix + 1, iz) * fx;
    let bottom = v(ix, iz + 1) * (1.0 - fx) + v(ix + 1, iz + 1) * fx;
    top * (1.0 - fz) + bottom * fz
}

/// The band of every fine cell of `grid`, row-major, `side * SUB` per side.
pub fn fine_bands(grid: &ScalarGrid, bands: &Bands) -> Vec<Option<usize>> {
    let side = grid.side * SUB;
    let step = MAP_SIZE / side as f32;
    (0..side * side)
        .map(|i| {
            let (x, z) = ((i % side) as f32 + 0.5, (i / side) as f32 + 0.5);
            bands.of(sample(grid, x * step, z * step))
        })
        .collect()
}

/// The banded layer of `grid` (a missing grid is a quiet map: nothing drawn), draped
/// [`LIFT`] above the terrain.
pub fn band_mesh(grid: Option<&ScalarGrid>, bands: &Bands, terrain: &Terrain) -> MeshData {
    let mut m = MeshData::default();
    let Some(grid) = grid else {
        return m;
    };
    let side = grid.side * SUB;
    let step = MAP_SIZE / side as f32;
    let cells = fine_bands(grid, bands);
    let at = |x: f32, z: f32, lift: f32| [x, terrain.height(bevy::math::Vec2::new(x, z)) + lift, z];
    let rect = |m: &mut MeshData, x0: f32, z0: f32, x1: f32, z1: f32, lift: f32, c: [f32; 4]| {
        flat(
            m,
            [
                at(x0, z0, lift),
                at(x1, z0, lift),
                at(x1, z1, lift),
                at(x0, z1, lift),
            ],
            c,
        );
    };
    let band_at = |ix: isize, iz: isize| -> Option<Option<usize>> {
        (ix >= 0 && iz >= 0 && (ix as usize) < side && (iz as usize) < side)
            .then(|| cells[iz as usize * side + ix as usize])
    };
    let w = step * LINE;
    for iz in 0..side {
        for ix in 0..side {
            let Some(b) = cells[iz * side + ix] else {
                continue;
            };
            let (x0, z0) = (ix as f32 * step, iz as f32 * step);
            let (x1, z1) = (x0 + step, z0 + step);
            rect(&mut m, x0, z0, x1, z1, LIFT, bands.colors[b]);
            // A contour where the neighbour is quieter: on this cell's side of the edge.
            let line = bands.line_color(b);
            let lower = |dx: isize, dz: isize| {
                band_at(ix as isize + dx, iz as isize + dz).is_some_and(|n| n.is_none_or(|n| n < b))
            };
            let lift = LIFT + LINE_LIFT;
            if lower(-1, 0) {
                rect(&mut m, x0, z0, x0 + w, z1, lift, line);
            }
            if lower(1, 0) {
                rect(&mut m, x1 - w, z0, x1, z1, lift, line);
            }
            if lower(0, -1) {
                rect(&mut m, x0, z0, x1, z0 + w, lift, line);
            }
            if lower(0, 1) {
                rect(&mut m, x0, z1 - w, x1, z1, lift, line);
            }
        }
    }
    m
}
