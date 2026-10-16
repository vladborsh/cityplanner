//! Player commands of the terrain domain (T-150). The checks that need the treasury, the
//! content and the milestones run in `cp_sim`; the geometry of a purchase is here.

use serde::{Deserialize, Serialize};

use crate::{TILES_PER_SIDE, TileOwnership};

/// Commands handled for this domain. Variants are only appended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerrainCommand {
    /// Buys the tile `(tx, tz)`: it must touch an owned tile along an edge.
    BuyTile { tile: (u8, u8) },
}

impl TileOwnership {
    /// Whether `(tx, tz)` is on the map and shares an edge with an owned tile.
    pub fn touches_owned(&self, tx: usize, tz: usize) -> bool {
        tx < TILES_PER_SIDE
            && tz < TILES_PER_SIDE
            && [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)]
                .into_iter()
                .any(|(dx, dz)| {
                    let (x, z) = (tx as i32 + dx, tz as i32 + dz);
                    x >= 0 && z >= 0 && self.is_open(x as usize, z as usize)
                })
    }
}

/// Ring of a tile around `start`: the larger of the two tile distances (1 = next to it).
pub fn ring(start: (u8, u8), tile: (u8, u8)) -> u8 {
    start.0.abs_diff(tile.0).max(start.1.abs_diff(tile.1))
}
