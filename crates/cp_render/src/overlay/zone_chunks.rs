//! The zone grid (fill + outlines) built per chunk (T-119). Before, any change — a camera
//! shift of 250 m, the zoning mode, one painted cell — rebuilt the mesh of every cell (~14.5 ms in
//! release on 30,720 cells). Now the cells are split by the chunk that holds their centre
//! ([`SIZE`] square, half a terrain chunk), and a chunk is rebuilt only when what it was
//! built from changed:
//!
//! * its cells (compared by value, so painting one cell dirties one chunk);
//! * the zoning mode (fill) and the line width step (outlines);
//! * what the camera does to it: the terrain LODs the fill must clear and the outline range depend
//!   on the distance to the camera. Where the whole chunk lies in one distance band the camera
//!   does not matter, so only chunks straddling a band edge follow the camera's 250 m cell.
//!
//! Dirty chunks are rebuilt nearest to the camera first, within a per-frame time budget
//! ([`BUILD_BUDGET`]); the rest wait for the next frames and keep showing their old mesh.
//!
//! This module is pure (no ECS): [`ZoneChunks::update`] returns the meshes to swap.

use std::time::{Duration, Instant};

use bevy::math::Vec3;
use cp_sim::{ZoneCellView, ZoneOverlay};
use cp_terrain::Terrain;

use super::{OUTLINE_DISTANCE, outline_width_step, zone_mesh_with, zone_outline_mesh};
use crate::chunk::{CHUNK_CELLS, CHUNK_SIZE, CHUNKS_PER_SIDE, MeshData, select_lod};

/// A zone chunk is a quarter of a terrain chunk (half its side): a fully built-up one holds at most
/// 1,600 cells, so one chunk is a small unit of work.
pub const SPLIT: usize = 2;
/// Zone chunks per map side.
pub const SIDE: usize = CHUNKS_PER_SIDE * SPLIT;
/// Zone chunk side length, metres.
pub const SIZE: f32 = CHUNK_SIZE / SPLIT as f32;
/// Terrain nodes per zone chunk side.
const NODES: usize = CHUNK_CELLS / SPLIT;

/// Time a frame spends building chunks: it builds the nearest dirty ones until this has passed, so
/// it overshoots by at most one chunk (≤ 1,600 cells, about 1 ms in release). Cost per chunk
/// varies too much with the terrain and the style for a count of cells or triangles to bound it.
pub const BUILD_BUDGET: Duration = Duration::from_micros(2500);
/// Edge of the camera cell that chunks straddling a distance band follow, metres.
pub const CAMERA_CELL: f32 = 250.0;
/// A cell sits within this of its chunk's square (its centre is inside, its corners may not be).
const CELL_REACH: f32 = 16.0;

/// How the zone layers are drawn now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoneStyle {
    pub zoning: bool,
    pub cam: Option<Vec3>,
}

/// What the camera does to a chunk's mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CamSig {
    /// The whole chunk is in this distance band, wherever the camera stands.
    Band(usize),
    /// It straddles an edge: the mesh follows the camera's [`CAMERA_CELL`].
    Cell(i32, i32, i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FillSig {
    version: u32,
    terrain: usize,
    zoning: bool,
    cam: Option<CamSig>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OutlineSig {
    version: u32,
    terrain: usize,
    zoning: bool,
    width_step: i32,
    cam: Option<CamSig>,
}

/// The meshes of one chunk to swap in; an empty mesh removes the layer.
#[derive(Debug)]
pub struct ChunkUpdate {
    pub chunk: usize,
    pub fill: Option<MeshData>,
    pub outline: Option<MeshData>,
}

/// Per-chunk cells, what each chunk's meshes were built from, and the rebuild queue logic.
pub struct ZoneChunks {
    cells: Vec<ZoneOverlay>,
    version: Vec<u32>,
    fill: Vec<Option<FillSig>>,
    outline: Vec<Option<OutlineSig>>,
    /// Lowest and highest terrain node under the chunk (for the distance bands).
    heights: Vec<Option<(f32, f32)>>,
    terrain_key: usize,
}

impl Default for ZoneChunks {
    fn default() -> Self {
        let n = SIDE * SIDE;
        Self {
            cells: vec![ZoneOverlay::default(); n],
            version: vec![0; n],
            fill: vec![None; n],
            outline: vec![None; n],
            heights: vec![None; n],
            terrain_key: 0,
        }
    }
}

/// The chunk (`cz * SIDE + cx`) holding the centre of `cell`.
pub fn chunk_of(cell: &ZoneCellView) -> usize {
    let (mut x, mut z) = (0.0, 0.0);
    for [cx, cz] in cell.corners {
        x += cx * 0.25;
        z += cz * 0.25;
    }
    let at = |v: f32| ((v / SIZE).floor().max(0.0) as usize).min(SIDE - 1);
    at(z) * SIDE + at(x)
}

impl ZoneChunks {
    /// Takes a new zone snapshot: only the chunks whose cells differ are marked for rebuild.
    pub fn set_zones(&mut self, zones: &ZoneOverlay) {
        let mut next = vec![Vec::new(); self.cells.len()];
        for cell in &zones.cells {
            next[chunk_of(cell)].push(*cell);
        }
        for (i, cells) in next.into_iter().enumerate() {
            if self.cells[i].cells != cells {
                self.cells[i].cells = cells;
                self.version[i] += 1;
            }
        }
    }

    /// Number of chunks that would be rebuilt now, ignoring the budget.
    pub fn dirty(&mut self, terrain: &Terrain, terrain_key: usize, style: ZoneStyle) -> usize {
        self.sync_terrain(terrain, terrain_key);
        (0..self.cells.len())
            .filter(|&i| {
                let (f, o) = self.wanted(i, terrain_key, style);
                self.fill[i] != Some(f) || self.outline[i] != Some(o)
            })
            .count()
    }

    /// Drops the height ranges of another terrain and (re)computes those of chunks with cells.
    fn sync_terrain(&mut self, terrain: &Terrain, terrain_key: usize) {
        if self.terrain_key != terrain_key {
            self.terrain_key = terrain_key;
            self.heights.fill(None);
        }
        for i in 0..self.cells.len() {
            if self.cells[i].cells.is_empty() || self.heights[i].is_some() {
                continue;
            }
            let (nx, nz) = ((i % SIDE) * NODES, (i / SIDE) * NODES);
            let mut r = (f32::MAX, f32::MIN);
            for dz in 0..=NODES {
                for dx in 0..=NODES {
                    let h = terrain.node_height(nx + dx, nz + dz);
                    r = (r.0.min(h), r.1.max(h));
                }
            }
            self.heights[i] = Some(r);
        }
    }

    /// The signatures chunk `i` should carry for `style`.
    fn wanted(&self, i: usize, terrain_key: usize, style: ZoneStyle) -> (FillSig, OutlineSig) {
        let cam = style.cam.map(|c| self.cam_sig(i, c));
        let fill = FillSig {
            version: self.version[i],
            terrain: terrain_key,
            zoning: style.zoning,
            cam: cam.map(|s| s.0),
        };
        let outline = OutlineSig {
            version: self.version[i],
            terrain: terrain_key,
            zoning: style.zoning,
            width_step: style.cam.map_or(0, |c| outline_width_step(c.y)),
            cam: cam.map(|s| s.1),
        };
        (fill, outline)
    }

    /// `(fill, outline)` camera signatures of chunk `i`.
    fn cam_sig(&self, i: usize, cam: Vec3) -> (CamSig, CamSig) {
        // A chunk with no cells draws nothing whatever the camera does.
        let Some((lo_h, hi_h)) = self.heights[i] else {
            return (CamSig::Band(0), CamSig::Band(0));
        };
        let (cx, cz) = ((i % SIDE) as f32, (i / SIDE) as f32);
        let lo = Vec3::new(cx * SIZE - CELL_REACH, lo_h - 1.0, cz * SIZE - CELL_REACH);
        let hi = Vec3::new(
            (cx + 1.0) * SIZE + CELL_REACH,
            hi_h + 1.0,
            (cz + 1.0) * SIZE + CELL_REACH,
        );
        let near = cam.distance(cam.clamp(lo, hi));
        let far = Vec3::new(
            (cam.x - lo.x).abs().max((cam.x - hi.x).abs()),
            (cam.y - lo.y).abs().max((cam.y - hi.y).abs()),
            (cam.z - lo.z).abs().max((cam.z - hi.z).abs()),
        )
        .length();
        let cell = CamSig::Cell(
            (cam.x / CAMERA_CELL).floor() as i32,
            (cam.y / CAMERA_CELL).floor() as i32,
            (cam.z / CAMERA_CELL).floor() as i32,
        );
        let (lod_near, lod_far) = (select_lod(near, None), select_lod(far, None));
        let fill = if lod_near == lod_far {
            CamSig::Band(lod_near)
        } else {
            cell
        };
        let outline = if far <= OUTLINE_DISTANCE {
            CamSig::Band(0)
        } else if near > OUTLINE_DISTANCE {
            CamSig::Band(1)
        } else {
            cell
        };
        (fill, outline)
    }

    /// Rebuilds the dirty chunks, nearest to the camera first, until `budget` has passed.
    /// Chunks left over stay dirty: call again next frame.
    pub fn update(
        &mut self,
        terrain: &Terrain,
        terrain_key: usize,
        style: ZoneStyle,
        budget: Duration,
    ) -> Vec<ChunkUpdate> {
        self.sync_terrain(terrain, terrain_key);
        let mut dirty = Vec::new();
        for i in 0..self.cells.len() {
            let (f, o) = self.wanted(i, terrain_key, style);
            let (df, dof) = (self.fill[i] != Some(f), self.outline[i] != Some(o));
            if df || dof {
                dirty.push((i, f, o, df, dof));
            }
        }
        let from = style.cam.map_or(Vec3::ZERO, |c| Vec3::new(c.x, 0.0, c.z));
        let centre = |i: usize| {
            Vec3::new(
                ((i % SIDE) as f32 + 0.5) * SIZE,
                0.0,
                ((i / SIDE) as f32 + 0.5) * SIZE,
            )
        };
        dirty.sort_by(|a, b| {
            from.distance_squared(centre(a.0))
                .total_cmp(&from.distance_squared(centre(b.0)))
                .then(a.0.cmp(&b.0))
        });
        let start = Instant::now();
        let mut built_any = false;
        let mut out = Vec::new();
        for (i, f, o, df, dof) in dirty {
            if built_any && start.elapsed() >= budget {
                break;
            }
            if self.cells[i].cells.is_empty() && self.fill[i].is_none() {
                // Nothing drawn and nothing to take down.
                self.fill[i] = Some(f);
                self.outline[i] = Some(o);
                continue;
            }
            built_any |= !self.cells[i].cells.is_empty();
            let zones = &self.cells[i];
            let fill = df.then(|| {
                if zones.cells.is_empty() {
                    MeshData::default()
                } else {
                    zone_mesh_with(zones, terrain, style.zoning, style.cam)
                }
            });
            let outline = dof.then(|| match style.cam {
                Some(cam) if !zones.cells.is_empty() => {
                    zone_outline_mesh(zones, terrain, cam, outline_width_step(cam.y), style.zoning)
                }
                _ => MeshData::default(),
            });
            self.fill[i] = Some(f);
            self.outline[i] = Some(o);
            out.push(ChunkUpdate {
                chunk: i,
                fill,
                outline,
            });
        }
        out
    }
}
