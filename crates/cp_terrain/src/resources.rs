//! The resource map of a map (`resources.png`, T-205) and the ore and oil left in it.
//! See `architecture/system-design.md` §16 M5 (A).
//!
//! `resources.png` is RGBA, 8 bit, one pixel per 64 m (so [`RES_GRID`] = 160 per side for the
//! 10 240 m map): R fertile land, G forest, B ore, A oil, `0..=255` → richness `0..=1`. The
//! image has the same orientation as the other maps: pixel `(c, r)` covers
//! `x = c * 64 .. (c + 1) * 64`, `z = r * 64 .. (r + 1) * 64`. A map without the file has an
//! empty [`ResourceMap`] (no resources); it is not part of [`crate::Terrain::map_hash`].

use std::path::Path;

use bevy_ecs::resource::Resource;
use bevy_math::Vec2;
use cp_content::ResourceKind;
use serde::{Deserialize, Serialize};

use crate::{MAP_SIZE, TerrainError};

/// Side of a resource cell in metres.
pub const RES_CELL: f32 = 64.0;
/// Resource cells per map side.
pub const RES_GRID: usize = (MAP_SIZE / RES_CELL) as usize;

/// Richness of every resource per 64 m cell, immutable for the lifetime of a game.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceMap {
    /// `RES_GRID * RES_GRID` RGBA pixels, row-major; empty = no resources on this map.
    cells: Vec<[u8; 4]>,
}

impl ResourceMap {
    /// A map with no resources.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Builds a map from `RES_GRID * RES_GRID` RGBA pixels, row-major.
    pub fn from_pixels(cells: Vec<[u8; 4]>) -> Result<Self, String> {
        if cells.len() != RES_GRID * RES_GRID {
            return Err(format!(
                "expected {RES_GRID}x{RES_GRID} resource cells, got {}",
                cells.len()
            ));
        }
        Ok(Self { cells })
    }

    /// Decodes `resources.png` (8-bit RGBA, `RES_GRID` square); `name` is for messages.
    pub fn decode(input: impl std::io::Read, name: &Path) -> Result<Self, TerrainError> {
        let mut reader = png::Decoder::new(input)
            .read_info()
            .map_err(|e| TerrainError::Png(name.into(), e.to_string()))?;
        let info = reader.info();
        if info.width as usize != RES_GRID
            || info.height as usize != RES_GRID
            || info.color_type != png::ColorType::Rgba
            || info.bit_depth != png::BitDepth::Eight
        {
            return Err(TerrainError::BadImage(
                name.into(),
                format!(
                    "expected 8-bit RGBA {RES_GRID}x{RES_GRID}, got {:?} {:?} {}x{}",
                    info.color_type, info.bit_depth, info.width, info.height
                ),
            ));
        }
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let frame = reader
            .next_frame(&mut buf)
            .map_err(|e| TerrainError::Png(name.into(), e.to_string()))?;
        let (px, _) = buf[..frame.buffer_size()].as_chunks::<4>();
        Self::from_pixels(px.to_vec()).map_err(|e| TerrainError::BadImage(name.into(), e))
    }

    /// Encodes the map as `resources.png` (what `tools/mapgen` writes).
    pub fn to_png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, RES_GRID as u32, RES_GRID as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("png header");
        let bytes: Vec<u8> = if self.cells.is_empty() {
            vec![0; RES_GRID * RES_GRID * 4]
        } else {
            self.cells.iter().flatten().copied().collect()
        };
        w.write_image_data(&bytes).expect("png data");
        w.finish().expect("png end");
        out
    }

    /// The map has no resource data.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Cell `(cx, cz)` containing `p` (clamped to the map).
    pub fn cell_of(p: Vec2) -> (usize, usize) {
        let c = |v: f32| ((v / RES_CELL).floor().max(0.0) as usize).min(RES_GRID - 1);
        (c(p.x), c(p.y))
    }

    /// Centre of cell `(cx, cz)` in world `(x, z)`.
    pub fn cell_centre(cx: usize, cz: usize) -> Vec2 {
        Vec2::new((cx as f32 + 0.5) * RES_CELL, (cz as f32 + 0.5) * RES_CELL)
    }

    /// Richness `0..=1` of `kind` in cell `(cx, cz)` (indices clamped).
    pub fn richness(&self, kind: ResourceKind, cx: usize, cz: usize) -> f32 {
        if self.cells.is_empty() {
            return 0.0;
        }
        let i = cz.min(RES_GRID - 1) * RES_GRID + cx.min(RES_GRID - 1);
        f32::from(self.cells[i][kind.index()]) / 255.0
    }

    /// Richness `0..=1` of `kind` at world `(x, z)`: the value of the pixel containing `p`.
    pub fn richness_at(&self, kind: ResourceKind, p: Vec2) -> f32 {
        let (cx, cz) = Self::cell_of(p);
        self.richness(kind, cx, cz)
    }
}

/// Ore and oil left in every resource cell (T-205). Land and forest are not used up and have no
/// entry. Saved and in the checksum. Starts as `richness × deposit_per_cell`.
///
/// `Default` is *unfilled* (no lists): a game that has not been through
/// [`ResourceDeposits::from_map`] yet, or a save older than v41 that has to be filled from the
/// map on load. An unfilled value reads as zero everywhere.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceDeposits {
    ore: Vec<u16>,
    oil: Vec<u16>,
}

impl ResourceDeposits {
    /// The full deposits of `map`: `richness × per_cell` rounded, clamped to `u16`.
    pub fn from_map(map: &ResourceMap, per_cell: f32) -> Self {
        let fill = |kind: ResourceKind| -> Vec<u16> {
            (0..RES_GRID * RES_GRID)
                .map(|i| {
                    let r = map.richness(kind, i % RES_GRID, i / RES_GRID);
                    (r * per_cell).round().clamp(0.0, f32::from(u16::MAX)) as u16
                })
                .collect()
        };
        Self {
            ore: fill(ResourceKind::Ore),
            oil: fill(ResourceKind::Oil),
        }
    }

    /// Not filled from a map yet (see the type docs).
    pub fn is_unfilled(&self) -> bool {
        self.ore.is_empty()
    }

    fn index(cx: usize, cz: usize) -> usize {
        cz.min(RES_GRID - 1) * RES_GRID + cx.min(RES_GRID - 1)
    }

    /// What is left of `kind` in cell `(cx, cz)`; `0` for land and forest and for unfilled.
    pub fn remaining(&self, kind: ResourceKind, cx: usize, cz: usize) -> u16 {
        let list = match kind {
            ResourceKind::Ore => &self.ore,
            ResourceKind::Oil => &self.oil,
            _ => return 0,
        };
        list.get(Self::index(cx, cz)).copied().unwrap_or(0)
    }

    /// What is left of `kind` in the cell containing `p`.
    pub fn remaining_at(&self, kind: ResourceKind, p: Vec2) -> u16 {
        let (cx, cz) = ResourceMap::cell_of(p);
        self.remaining(kind, cx, cz)
    }

    /// Takes up to `amount` of `kind` from cell `(cx, cz)`; returns what was taken.
    pub fn take(&mut self, kind: ResourceKind, cx: usize, cz: usize, amount: u16) -> u16 {
        let list = match kind {
            ResourceKind::Ore => &mut self.ore,
            ResourceKind::Oil => &mut self.oil,
            _ => return 0,
        };
        let Some(left) = list.get_mut(Self::index(cx, cz)) else {
            return 0;
        };
        let got = amount.min(*left);
        *left -= got;
        got
    }
}
