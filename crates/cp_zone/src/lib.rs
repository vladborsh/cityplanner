//! `cp_zone` — zone blocks and cells (`architecture/system-design.md` §7).

pub mod command;
pub mod geom;
pub mod grid;

pub use command::{ZoneCommand, ZoneError, apply};
pub use grid::{
    BLOCK_DEPTH, CELL_SIZE, PENETRATION_EPS, RoadZoning, Side, ZoneBlock, ZoneBlockId, ZoneCellRef,
    ZoneGrid,
};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_zone";

#[cfg(test)]
mod tests;
