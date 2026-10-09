//! Pure terrain mesh generation (no Bevy types): chunks of 64×64 cells, three LODs.
//!
//! Positions are **chunk-local** (relative to the chunk's north-west corner, at world
//! `(cx * 640, 0, cz * 640)`), so the entity transform carries the offset.
//!
//! **Seams.** Neighbouring chunks at the same LOD share their edge vertices exactly (same
//! samples), so they match. Chunks at different LODs do not (the coarse edge skips every
//! other node), so every chunk gets a *skirt*: a strip hanging below its border that hides
//! the gap. The skirt is deeper for coarser LODs.

use cp_terrain::{GRID, SPACING, Terrain};

/// Cells per chunk side.
pub const CHUNK_CELLS: usize = 64;
/// Chunks per map side.
pub const CHUNKS_PER_SIDE: usize = (GRID - 1) / CHUNK_CELLS;
/// Chunk side length in metres.
pub const CHUNK_SIZE: f32 = CHUNK_CELLS as f32 * SPACING;
/// Number of LOD levels. LOD `l` samples every `2^l`-th node.
pub const LOD_COUNT: usize = 3;

/// Vertices per side for a LOD (excluding skirt).
pub const fn verts_per_side(lod: usize) -> usize {
    CHUNK_CELLS / (1 << lod) + 1
}

/// Triangles in a LOD's grid (excluding skirt).
pub const fn grid_triangles(lod: usize) -> usize {
    let cells = CHUNK_CELLS / (1 << lod);
    cells * cells * 2
}

/// Raw mesh buffers.
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Linear RGBA.
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Colour bands for height/slope texturing, in metres above sea level.
const SAND_BAND: f32 = 4.0;
const GRASS_TOP: f32 = 180.0;
const ROCK_TOP: f32 = 420.0;

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Terrain colour from elevation above sea level and slope (`normal.y`, 1 = flat).
/// Vertex-colour "texturing": sand at the shore, grass on flats, rock on steep ground and
/// high up, snow on the peaks.
pub fn terrain_color(above_sea: f32, normal_y: f32) -> [f32; 4] {
    const SAND: [f32; 3] = [0.55, 0.50, 0.30];
    const GRASS_LOW: [f32; 3] = [0.14, 0.30, 0.08];
    const GRASS_HIGH: [f32; 3] = [0.20, 0.28, 0.10];
    const ROCK: [f32; 3] = [0.25, 0.22, 0.20];
    const SNOW: [f32; 3] = [0.80, 0.82, 0.86];
    let t = |a: f32, b: f32, v: f32| ((v - a) / (b - a)).clamp(0.0, 1.0);

    let flat = lerp3(GRASS_LOW, GRASS_HIGH, t(0.0, GRASS_TOP, above_sea));
    let mut c = lerp3(SAND, flat, t(SAND_BAND * 0.5, SAND_BAND, above_sea));
    // Steepness: rock above ~30° (normal_y 0.87), fully rock by ~50° (0.64).
    c = lerp3(c, ROCK, 1.0 - t(0.64, 0.87, normal_y));
    c = lerp3(c, ROCK, t(GRASS_TOP, ROCK_TOP, above_sea) * 0.8);
    // Snow only on flat-ish ground high up.
    let snow = t(ROCK_TOP, ROCK_TOP + 120.0, above_sea) * t(0.6, 0.85, normal_y);
    c = lerp3(c, SNOW, snow);
    [c[0], c[1], c[2], 1.0]
}

/// Normal from central differences on the node grid (global indices, clamped at the map edge).
fn node_normal(t: &Terrain, ix: usize, iz: usize) -> [f32; 3] {
    let h = |x: usize, z: usize| t.node_height(x, z);
    let (x0, x1) = (ix.saturating_sub(1), (ix + 1).min(GRID - 1));
    let (z0, z1) = (iz.saturating_sub(1), (iz + 1).min(GRID - 1));
    let dx = (h(x1, iz) - h(x0, iz)) / ((x1 - x0) as f32 * SPACING);
    let dz = (h(ix, z1) - h(ix, z0)) / ((z1 - z0) as f32 * SPACING);
    let l = (dx * dx + 1.0 + dz * dz).sqrt();
    [-dx / l, 1.0 / l, -dz / l]
}

/// Skirt depth in metres for a LOD.
pub fn skirt_depth(lod: usize) -> f32 {
    6.0 * (1 << lod) as f32
}

/// Builds the mesh of chunk `(cx, cz)` at `lod`. Grid vertices come first, row-major
/// (`z * verts_per_side + x`), followed by the skirt vertices.
pub fn build_chunk_mesh(terrain: &Terrain, cx: usize, cz: usize, lod: usize) -> MeshData {
    assert!(cx < CHUNKS_PER_SIDE && cz < CHUNKS_PER_SIDE && lod < LOD_COUNT);
    let step = 1usize << lod;
    let n = verts_per_side(lod);
    let (gx0, gz0) = (cx * CHUNK_CELLS, cz * CHUNK_CELLS);
    let sea = terrain.sea_level();

    let mut m = MeshData::default();
    let skirt_verts = 4 * n;
    m.positions.reserve(n * n + skirt_verts);
    m.normals.reserve(n * n + skirt_verts);
    m.colors.reserve(n * n + skirt_verts);
    for j in 0..n {
        for i in 0..n {
            let (ix, iz) = (gx0 + i * step, gz0 + j * step);
            let y = terrain.node_height(ix, iz);
            let nrm = node_normal(terrain, ix, iz);
            m.positions.push([
                i as f32 * step as f32 * SPACING,
                y,
                j as f32 * step as f32 * SPACING,
            ]);
            m.normals.push(nrm);
            m.colors.push(terrain_color(y - sea, nrm[1]));
        }
    }
    let idx = |i: usize, j: usize| (j * n + i) as u32;
    m.indices.reserve(grid_triangles(lod) * 3 + (n - 1) * 4 * 6);
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            let (a, b, c, d) = (idx(i, j), idx(i + 1, j), idx(i, j + 1), idx(i + 1, j + 1));
            // Counter-clockwise seen from above (+Y); z grows south (towards the viewer
            // looking north).
            m.indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }

    // Skirts: one duplicated, lowered vertex row per edge, wound outward.
    let depth = skirt_depth(lod);
    let mut add_skirt = |edge: &[(usize, usize)], flip: bool| {
        let base = m.positions.len() as u32;
        for &(i, j) in edge {
            let src = j * n + i;
            let mut p = m.positions[src];
            p[1] -= depth;
            m.positions.push(p);
            m.normals.push(m.normals[src]);
            m.colors.push(m.colors[src]);
        }
        for k in 0..edge.len() as u32 - 1 {
            let (t0, t1) = (
                idx(edge[k as usize].0, edge[k as usize].1),
                idx(edge[k as usize + 1].0, edge[k as usize + 1].1),
            );
            let (b0, b1) = (base + k, base + k + 1);
            if flip {
                m.indices.extend_from_slice(&[t0, b0, t1, t1, b0, b1]);
            } else {
                m.indices.extend_from_slice(&[t0, t1, b0, t1, b1, b0]);
            }
        }
    };
    let last = n - 1;
    add_skirt(&(0..n).map(|i| (i, 0)).collect::<Vec<_>>(), false); // north
    add_skirt(&(0..n).map(|i| (i, last)).collect::<Vec<_>>(), true); // south
    add_skirt(&(0..n).map(|j| (0, j)).collect::<Vec<_>>(), true); // west
    add_skirt(&(0..n).map(|j| (last, j)).collect::<Vec<_>>(), false); // east
    m
}

/// River/lake surface for chunk `(cx, cz)`: LOD0 quads over every cell with at least one
/// wet corner whose surface is above sea level (the sea itself is a separate plane).
/// Vertex alpha encodes depth, so shallow edges fade out. `None` if the chunk has no
/// inland water.
pub fn build_water_mesh(terrain: &Terrain, cx: usize, cz: usize) -> Option<MeshData> {
    use bevy_math::Vec2;
    let sea = terrain.sea_level();
    let (gx0, gz0) = (cx * CHUNK_CELLS, cz * CHUNK_CELLS);
    let n = CHUNK_CELLS + 1;
    // Per node: (surface, depth).
    let node = |i: usize, j: usize| {
        let (ix, iz) = (gx0 + i, gz0 + j);
        let depth = terrain.water_depth(Vec2::new(ix as f32 * SPACING, iz as f32 * SPACING));
        (terrain.node_height(ix, iz) + depth, depth)
    };
    let nodes: Vec<(f32, f32)> = (0..n * n).map(|k| node(k % n, k / n)).collect();
    let inland = |(s, d): (f32, f32)| d > 0.0 && s > sea + 0.01;

    let mut m = MeshData::default();
    let mut remap = vec![u32::MAX; n * n];
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            let c = [(i, j), (i + 1, j), (i, j + 1), (i + 1, j + 1)];
            if !c.iter().any(|&(x, y)| inland(nodes[y * n + x])) {
                continue;
            }
            let mut v = [0u32; 4];
            for (k, &(x, y)) in c.iter().enumerate() {
                let slot = &mut remap[y * n + x];
                if *slot == u32::MAX {
                    let (s, d) = nodes[y * n + x];
                    // Dry corners sit at the neighbours' level, fully transparent.
                    let alpha = (d / 3.0).clamp(0.0, 1.0) * 0.85;
                    *slot = m.positions.len() as u32;
                    m.positions
                        .push([x as f32 * SPACING, s.max(sea), y as f32 * SPACING]);
                    m.normals.push([0.0, 1.0, 0.0]);
                    m.colors.push([0.10, 0.28, 0.42, alpha]);
                }
                v[k] = *slot;
            }
            m.indices
                .extend_from_slice(&[v[0], v[2], v[1], v[1], v[2], v[3]]);
        }
    }
    (!m.indices.is_empty()).then_some(m)
}

/// LOD for a chunk at `distance` metres from the camera. Hysteresis: a chunk currently at
/// `current` only changes once the distance leaves its band by 10 %.
pub fn select_lod(distance: f32, current: Option<usize>) -> usize {
    const LIMITS: [f32; LOD_COUNT - 1] = [1500.0, 3500.0];
    let band = |d: f32| LIMITS.iter().position(|&l| d < l).unwrap_or(LOD_COUNT - 1);
    let fresh = band(distance);
    match current {
        Some(c) if c != fresh => {
            let (lo, hi) = (
                if c == 0 { 0.0 } else { LIMITS[c - 1] * 0.9 },
                if c == LOD_COUNT - 1 {
                    f32::MAX
                } else {
                    LIMITS[c] * 1.1
                },
            );
            if (lo..hi).contains(&distance) {
                c
            } else {
                fresh
            }
        }
        _ => fresh,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_terrain::{MapMeta, Terrain};
    use std::path::PathBuf;

    fn map(name: &str) -> Terrain {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/maps")
            .join(name);
        Terrain::load(&dir).unwrap()
    }

    fn synthetic(f: impl Fn(usize, usize) -> u16, water: impl Fn(usize, usize) -> u16) -> Terrain {
        let meta = MapMeta {
            name: "t".into(),
            sea_level: 5.0,
            start_tile: (2, 2),
            external_connections: vec![],
        };
        let mut h = vec![0u16; GRID * GRID];
        let mut w = vec![0u16; GRID * GRID];
        for z in 0..GRID {
            for x in 0..GRID {
                h[z * GRID + x] = f(x, z);
                w[z * GRID + x] = water(x, z);
            }
        }
        Terrain::from_samples(meta, &h, &w).unwrap()
    }

    #[test]
    fn vertex_and_triangle_counts_per_lod() {
        let t = synthetic(|x, z| (1000 + (x * 7 + z * 3) % 400) as u16, |_, _| 0);
        for (lod, side) in [(0, 65), (1, 33), (2, 17)] {
            let m = build_chunk_mesh(&t, 3, 4, lod);
            assert_eq!(verts_per_side(lod), side);
            assert_eq!(m.positions.len(), side * side + 4 * side);
            assert_eq!(m.positions.len(), m.normals.len());
            assert_eq!(m.positions.len(), m.colors.len());
            assert_eq!(m.triangle_count(), grid_triangles(lod) + 4 * (side - 1) * 2);
            assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
        }
        assert_eq!(grid_triangles(0), 8192);
        assert_eq!(grid_triangles(2), 512);
    }

    /// Edge vertices of a chunk (grid part), as world-space heights along one side.
    fn edge(m: &MeshData, lod: usize, side: char) -> Vec<f32> {
        let n = verts_per_side(lod);
        (0..n)
            .map(|k| match side {
                'e' => m.positions[k * n + n - 1][1],
                'w' => m.positions[k * n][1],
                's' => m.positions[(n - 1) * n + k][1],
                _ => m.positions[k][1],
            })
            .collect()
    }

    #[test]
    fn same_lod_neighbours_share_edge_heights_on_real_maps() {
        for name in ["flatlands", "river_valley"] {
            let t = map(name);
            for lod in 0..LOD_COUNT {
                let a = build_chunk_mesh(&t, 5, 7, lod);
                let east = build_chunk_mesh(&t, 6, 7, lod);
                let south = build_chunk_mesh(&t, 5, 8, lod);
                assert_eq!(
                    edge(&a, lod, 'e'),
                    edge(&east, lod, 'w'),
                    "{name} lod {lod}"
                );
                assert_eq!(
                    edge(&a, lod, 's'),
                    edge(&south, lod, 'n'),
                    "{name} lod {lod}"
                );
            }
        }
    }

    #[test]
    fn mixed_lod_edges_agree_at_shared_nodes_and_skirts_cover_the_rest() {
        let t = synthetic(|x, z| (2000 + ((x * 13) ^ (z * 7)) % 900) as u16, |_, _| 0);
        let fine = build_chunk_mesh(&t, 2, 2, 0);
        let coarse = build_chunk_mesh(&t, 3, 2, 2);
        let fe = edge(&fine, 0, 'e');
        let ce = edge(&coarse, 2, 'w');
        // Every 4th fine node is a coarse node: identical heights there.
        for (k, h) in ce.iter().enumerate() {
            assert_eq!(*h, fe[k * 4]);
        }
        // Between them the surfaces may differ, but the skirt reaches below both.
        let max_gap = (0..fe.len())
            .map(|i| {
                let (a, b) = (ce[i / 4], ce[(i / 4 + 1).min(ce.len() - 1)]);
                let lerp = a + (b - a) * (i % 4) as f32 / 4.0;
                (fe[i] - lerp).abs()
            })
            .fold(0.0f32, f32::max);
        assert!(max_gap < skirt_depth(2), "gap {max_gap} exceeds skirt");
        let n = verts_per_side(2);
        let grid_min_y = ce.iter().copied().fold(f32::MAX, f32::min);
        // First west-skirt vertex sits `depth` below its source edge vertex.
        let west_skirt = coarse.positions[n * n + 2 * n];
        assert!(west_skirt[1] <= grid_min_y - skirt_depth(2) + 1e-3 + (ce[0] - grid_min_y));
    }

    #[test]
    fn grid_faces_up_and_skirt_faces_out() {
        let t = synthetic(|_, _| 1500, |_, _| 0);
        let m = build_chunk_mesh(&t, 0, 0, 1);
        let n = verts_per_side(1);
        let normal = |tri: usize| {
            let p = |k: usize| m.positions[m.indices[tri * 3 + k] as usize];
            let (a, b, c) = (p(0), p(1), p(2));
            let (u, v) = (
                [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
            );
            [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ]
        };
        for tri in 0..grid_triangles(1) {
            assert!(normal(tri)[1] > 0.0, "grid tri {tri} faces down");
        }
        // Skirt triangles follow the grid in north, south, west, east order.
        let per_edge = (n - 1) * 2;
        let first = grid_triangles(1);
        for (e, (axis, sign)) in [(2usize, -1.0f32), (2, 1.0), (0, -1.0), (0, 1.0)]
            .into_iter()
            .enumerate()
        {
            for k in 0..per_edge {
                let nrm = normal(first + e * per_edge + k);
                assert!(nrm[axis] * sign > 0.0, "edge {e} tri {k}: {nrm:?}");
            }
        }
    }

    #[test]
    fn colours_follow_height_and_slope() {
        let shore = terrain_color(0.5, 1.0);
        let meadow = terrain_color(60.0, 1.0);
        let cliff = terrain_color(60.0, 0.4);
        let peak = terrain_color(600.0, 1.0);
        assert!(
            shore[0] > meadow[0] && shore[1] > meadow[0],
            "sand is lighter/yellower"
        );
        assert!(
            meadow[1] > meadow[0] && meadow[1] > meadow[2],
            "meadow is green"
        );
        assert!(
            (cliff[0] - cliff[1]).abs() < 0.05,
            "cliff is grey/brown, not green"
        );
        assert!(peak[0] > 0.6 && peak[2] > 0.6, "snow");
        assert!(
            terrain_color(0.0, 1.0)
                .iter()
                .all(|c| (0.0..=1.0).contains(c))
        );
    }

    #[test]
    fn water_mesh_covers_inland_water_only() {
        // Sea level 5 m. Flat ground at 2 m (under the sea), plus a river at surface 30 m
        // over ground at 25 m, spanning x = 100..110 nodes.
        let enc = |m: f32| (m / 1024.0 * 65535.0).round() as u16;
        let t = synthetic(
            |x, _| {
                if (100..110).contains(&x) {
                    enc(25.0)
                } else {
                    enc(2.0)
                }
            },
            |x, _| {
                if (100..110).contains(&x) {
                    enc(30.0)
                } else {
                    0
                }
            },
        );
        // Chunk (1, 0) holds nodes 64..128.
        let river = build_water_mesh(&t, 1, 0).expect("river chunk");
        assert!(river.triangle_count() > 0);
        let max_y = river.positions.iter().map(|p| p[1]).fold(0.0f32, f32::max);
        assert!(
            (max_y - 30.0).abs() < 0.1,
            "surface at river level, got {max_y}"
        );
        // Chunk (4, 0) has only sea: no river mesh.
        assert!(build_water_mesh(&t, 4, 0).is_none());
    }

    #[test]
    fn real_maps_have_expected_water() {
        let flat = map("flatlands");
        let valley = map("river_valley");
        let count = |t: &Terrain| {
            (0..CHUNKS_PER_SIDE)
                .flat_map(|z| (0..CHUNKS_PER_SIDE).map(move |x| (x, z)))
                .filter(|&(x, z)| build_water_mesh(t, x, z).is_some())
                .count()
        };
        assert!(count(&valley) > 10, "river valley has a river");
        let _ = count(&flat); // lakes optional
    }

    #[test]
    fn lod_selection_has_hysteresis() {
        assert_eq!(select_lod(100.0, None), 0);
        assert_eq!(select_lod(2000.0, None), 1);
        assert_eq!(select_lod(9000.0, None), 2);
        // Just past the 1500 m boundary a LOD0 chunk stays LOD0 ...
        assert_eq!(select_lod(1550.0, Some(0)), 0);
        // ... until it is clearly past it.
        assert_eq!(select_lod(1700.0, Some(0)), 1);
        // And a LOD1 chunk stays LOD1 just inside the boundary.
        assert_eq!(select_lod(1400.0, Some(1)), 1);
        assert_eq!(select_lod(1300.0, Some(1)), 0);
    }
}
