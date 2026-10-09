//! `cp_terrain` — map loading, height field, water and tile ownership.
//! See `architecture/system-design.md` §5.
//!
//! # World coordinates
//!
//! The ground is the XZ plane with Y up (same as `cp_core::curve`). The origin is the
//! **north-west corner** of the map: x grows east, z grows south, and image row `r`,
//! column `c` of the PNGs sits at `x = c * 10 m`, `z = r * 10 m`. The map spans
//! `0..=10 240 m` on both axes. Queries are given as `Vec2 (x, z)`.
//!
//! **Out-of-bounds queries are clamped** to the map rectangle (so the field is
//! constant-extrapolated beyond the edge); they never panic.
//!
//! # Encoding
//!
//! `height.png` and `water.png` are 16-bit greyscale, 1025×1025. A sample `v` maps to
//! `v / 65535 * 1024` metres. In `water.png` a value of `0` means "no inland water"; any
//! other value is the absolute elevation of the water surface at that node. The surface
//! is `max(sea_level, mask)` (water is static). `water_depth` is the bilinear
//! interpolation of the per-node depth `surface - ground`; `<= 0` means dry.

mod ownership;

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy_ecs::resource::Resource;
use bevy_math::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

pub use ownership::{TILE_COUNT, TILES_PER_SIDE, TileOwnership};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_terrain";

/// Heightmap samples per side.
pub const GRID: usize = 1025;
/// Distance between samples in metres.
pub const SPACING: f32 = 10.0;
/// Map side length in metres.
pub const MAP_SIZE: f32 = (GRID - 1) as f32 * SPACING;
/// Tile side length in metres.
pub const TILE_SIZE: f32 = MAP_SIZE / TILES_PER_SIDE as f32;
/// Height (metres) that the maximum `u16` sample encodes.
pub const MAX_HEIGHT: f32 = 1024.0;

/// The loaded map as sim-world state (immutable for the lifetime of a game; not in saves).
/// Lives here so domain crates (`cp_net`, ...) can read it from the world.
#[derive(Resource, Clone)]
pub struct SimTerrain(pub Arc<Terrain>);

/// `map.ron` contents.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapMeta {
    pub name: String,
    /// Sea level in metres.
    pub sea_level: f32,
    /// Starting tile as `(tx, tz)`, each in `0..5`.
    pub start_tile: (u8, u8),
    /// Outside connection points, world `(x, z)` in metres.
    pub external_connections: Vec<(f32, f32)>,
}

#[derive(Debug)]
pub enum TerrainError {
    Io(PathBuf, std::io::Error),
    Ron(PathBuf, String),
    Png(PathBuf, String),
    /// PNG is not 16-bit greyscale 1025×1025.
    BadImage(PathBuf, String),
    BadMeta(String),
}

impl std::fmt::Display for TerrainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Ron(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Png(p, e) => write!(f, "{}: {e}", p.display()),
            Self::BadImage(p, e) => write!(f, "{}: {e}", p.display()),
            Self::BadMeta(e) => write!(f, "map.ron: {e}"),
        }
    }
}

impl std::error::Error for TerrainError {}

/// Loaded, immutable terrain.
#[derive(Clone, Debug)]
pub struct Terrain {
    meta: MapMeta,
    /// Ground elevation per node, metres, row-major (`z * GRID + x`).
    ground: Vec<f32>,
    /// Water depth per node, metres (`surface - ground`, may be negative).
    depth: Vec<f32>,
    /// Lazily computed [`Terrain::map_hash`].
    hash: std::sync::OnceLock<u64>,
}

fn sample_to_m(v: u16) -> f32 {
    v as f32 / 65535.0 * MAX_HEIGHT
}

fn read_png16(path: &Path) -> Result<Vec<u16>, TerrainError> {
    let file = File::open(path).map_err(|e| TerrainError::Io(path.into(), e))?;
    decode_png16(BufReader::new(file), path)
}

/// Decodes a 16-bit greyscale `GRID`×`GRID` PNG; `name` is only used in error messages.
fn decode_png16(input: impl std::io::Read, name: &Path) -> Result<Vec<u16>, TerrainError> {
    let mut reader = png::Decoder::new(input)
        .read_info()
        .map_err(|e| TerrainError::Png(name.into(), e.to_string()))?;
    let info = reader.info();
    if info.width as usize != GRID
        || info.height as usize != GRID
        || info.color_type != png::ColorType::Grayscale
        || info.bit_depth != png::BitDepth::Sixteen
    {
        return Err(TerrainError::BadImage(
            name.into(),
            format!(
                "expected 16-bit greyscale {GRID}x{GRID}, got {:?} {:?} {}x{}",
                info.color_type, info.bit_depth, info.width, info.height
            ),
        ));
    }
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| TerrainError::Png(name.into(), e.to_string()))?;
    let (pairs, _) = buf[..frame.buffer_size()].as_chunks::<2>();
    Ok(pairs.iter().map(|&c| u16::from_be_bytes(c)).collect())
}

impl Terrain {
    /// Loads `map.ron`, `height.png` and `water.png` from a map directory.
    pub fn load(dir: &Path) -> Result<Self, TerrainError> {
        let ron_path = dir.join("map.ron");
        let text = std::fs::read_to_string(&ron_path)
            .map_err(|e| TerrainError::Io(ron_path.clone(), e))?;
        let meta: MapMeta =
            ron::from_str(&text).map_err(|e| TerrainError::Ron(ron_path, e.to_string()))?;
        let height = read_png16(&dir.join("height.png"))?;
        let water = read_png16(&dir.join("water.png"))?;
        Self::from_samples(meta, &height, &water)
    }

    /// Same as [`Terrain::load`] but from file contents already in memory (`map.ron` text,
    /// `height.png` and `water.png` bytes); used by the web build, which fetches over HTTP.
    pub fn from_bytes(
        map_ron: &str,
        height_png: &[u8],
        water_png: &[u8],
    ) -> Result<Self, TerrainError> {
        let meta: MapMeta = ron::from_str(map_ron)
            .map_err(|e| TerrainError::Ron(PathBuf::from("map.ron"), e.to_string()))?;
        let height = decode_png16(height_png, Path::new("height.png"))?;
        let water = decode_png16(water_png, Path::new("water.png"))?;
        Self::from_samples(meta, &height, &water)
    }

    /// Builds terrain from raw `u16` samples (row-major, `GRID * GRID` each).
    pub fn from_samples(
        meta: MapMeta,
        height: &[u16],
        water: &[u16],
    ) -> Result<Self, TerrainError> {
        if height.len() != GRID * GRID || water.len() != GRID * GRID {
            return Err(TerrainError::BadMeta(
                "sample arrays must be GRID*GRID".into(),
            ));
        }
        let (tx, tz) = meta.start_tile;
        if tx as usize >= TILES_PER_SIDE || tz as usize >= TILES_PER_SIDE {
            return Err(TerrainError::BadMeta(format!(
                "start_tile {tx},{tz} out of range"
            )));
        }
        let ground: Vec<f32> = height.iter().map(|&v| sample_to_m(v)).collect();
        let depth = ground
            .iter()
            .zip(water)
            .map(|(&g, &w)| {
                let surface = if w == 0 {
                    meta.sea_level
                } else {
                    meta.sea_level.max(sample_to_m(w))
                };
                surface - g
            })
            .collect();
        Ok(Self {
            meta,
            ground,
            depth,
            hash: std::sync::OnceLock::new(),
        })
    }

    /// Identity of the map: FNV-1a (64 bit) over the postcard encoding of [`MapMeta`] and the
    /// bit patterns of every height and water-depth sample. Stable across runs and
    /// platforms; any change to the meta or a single sample changes it. Never `0` (that
    /// value means "unknown" in save headers). Computed on first use and cached.
    pub fn map_hash(&self) -> u64 {
        *self.hash.get_or_init(|| {
            const PRIME: u64 = 0x0000_0100_0000_01b3;
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            let mut feed = |bytes: &[u8]| {
                for &b in bytes {
                    h ^= u64::from(b);
                    h = h.wrapping_mul(PRIME);
                }
            };
            let meta = postcard::to_stdvec(&self.meta).expect("MapMeta serialises");
            feed(&(meta.len() as u64).to_le_bytes());
            feed(&meta);
            for field in [&self.ground, &self.depth] {
                feed(&(field.len() as u64).to_le_bytes());
                for v in field {
                    feed(&v.to_bits().to_le_bytes());
                }
            }
            if h == 0 { 1 } else { h }
        })
    }

    pub fn meta(&self) -> &MapMeta {
        &self.meta
    }

    pub fn sea_level(&self) -> f32 {
        self.meta.sea_level
    }

    /// Ground elevation at a node (indices clamped).
    pub fn node_height(&self, ix: usize, iz: usize) -> f32 {
        self.ground[iz.min(GRID - 1) * GRID + ix.min(GRID - 1)]
    }

    /// Cell index, in-cell fraction for one axis, after clamping to the map.
    fn cell(v: f32) -> (usize, f32) {
        let g = (v / SPACING).clamp(0.0, (GRID - 1) as f32);
        let i = (g.floor() as usize).min(GRID - 2);
        (i, g - i as f32)
    }

    fn bilerp(field: &[f32], p: Vec2) -> (f32, f32, f32) {
        let (ix, fx) = Self::cell(p.x);
        let (iz, fz) = Self::cell(p.y);
        let at = |dx: usize, dz: usize| field[(iz + dz) * GRID + ix + dx];
        let (a, b, c, d) = (at(0, 0), at(1, 0), at(0, 1), at(1, 1));
        let top = a + (b - a) * fx;
        let bot = c + (d - c) * fx;
        let value = top + (bot - top) * fz;
        let dx = ((b - a) * (1.0 - fz) + (d - c) * fz) / SPACING;
        let dz = (bot - top) / SPACING;
        (value, dx, dz)
    }

    /// Ground elevation (m) at world `(x, z)`, bilinear; clamped out of bounds.
    pub fn height(&self, p: Vec2) -> f32 {
        Self::bilerp(&self.ground, p).0
    }

    /// Unit surface normal (Y up) of the bilinear ground at `p`.
    pub fn normal(&self, p: Vec2) -> Vec3 {
        let (_, dx, dz) = Self::bilerp(&self.ground, p);
        Vec3::new(-dx, 1.0, -dz).normalize()
    }

    /// Water depth (m) at `p`: `max(sea, mask) - ground`; `<= 0` is dry.
    pub fn water_depth(&self, p: Vec2) -> f32 {
        Self::bilerp(&self.depth, p).0
    }

    pub fn is_water(&self, p: Vec2) -> bool {
        self.water_depth(p) > 0.0
    }

    /// Tile `(tx, tz)` containing `p` (clamped to the map).
    pub fn tile_of(p: Vec2) -> (usize, usize) {
        let t = |v: f32| ((v / TILE_SIZE).floor().max(0.0) as usize).min(TILES_PER_SIDE - 1);
        (t(p.x), t(p.y))
    }
}

#[cfg(test)]
mod tests;
