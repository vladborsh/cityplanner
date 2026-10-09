//! Tile ownership: 25 flags for the 5×5 tile grid (`architecture/system-design.md` §5).

use bevy_ecs::resource::Resource;
use bevy_math::Vec2;
use serde::{Deserialize, Serialize};

pub const TILES_PER_SIDE: usize = 5;
pub const TILE_COUNT: usize = TILES_PER_SIDE * TILES_PER_SIDE;

/// Which tiles the player may build on. Bit `tz * 5 + tx` of a `u32`.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileOwnership(u32);

impl TileOwnership {
    /// Nothing owned.
    pub fn none() -> Self {
        Self(0)
    }

    /// M1 start: the `start` tile and its neighbours, clipped at the map edge (4 tiles
    /// in a corner, 6 on an edge, 9 inside). `start` is `MapMeta::start_tile`.
    pub fn m1_around(start: (u8, u8)) -> Self {
        let (sx, sz) = (start.0 as usize, start.1 as usize);
        assert!(
            sx < TILES_PER_SIDE && sz < TILES_PER_SIDE,
            "start tile out of range"
        );
        let mut o = Self::none();
        for tz in sz.saturating_sub(1)..=(sz + 1).min(TILES_PER_SIDE - 1) {
            for tx in sx.saturating_sub(1)..=(sx + 1).min(TILES_PER_SIDE - 1) {
                o.set(tx, tz, true);
            }
        }
        o
    }

    /// M2 start: only the `start` tile (`MapMeta::start_tile`).
    pub fn only(start: (u8, u8)) -> Self {
        let mut o = Self::none();
        o.set(start.0 as usize, start.1 as usize, true);
        o
    }

    /// Whether tile `(tx, tz)` is open; out-of-range tiles are not.
    pub fn is_open(&self, tx: usize, tz: usize) -> bool {
        tx < TILES_PER_SIDE && tz < TILES_PER_SIDE && self.0 >> (tz * TILES_PER_SIDE + tx) & 1 == 1
    }

    /// Opens or closes a tile. Panics if out of range.
    pub fn set(&mut self, tx: usize, tz: usize, open: bool) {
        assert!(
            tx < TILES_PER_SIDE && tz < TILES_PER_SIDE,
            "tile out of range"
        );
        let bit = 1u32 << (tz * TILES_PER_SIDE + tx);
        if open {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
    }

    pub fn open_count(&self) -> usize {
        (self.0 & ((1 << TILE_COUNT) - 1)).count_ones() as usize
    }

    /// Whether the world point lies on an open tile (out of bounds is clamped to the edge tile).
    pub fn contains_point(&self, p: Vec2) -> bool {
        let (tx, tz) = crate::Terrain::tile_of(p);
        self.is_open(tx, tz)
    }
}
