//! Player commands of this domain (`architecture/system-design.md` §4, level 2).
//! New variants are only appended (save stability rule).

use std::fmt;

use bevy_ecs::world::World;
use cp_content::ZoneType;
use cp_core::Cost;
use serde::{Deserialize, Serialize};

use crate::grid::{ZoneCellRef, ZoneGrid};

/// Commands handled by this crate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ZoneCommand {
    /// Paints `zone` onto `cells` (`ZoneType::None` erases). Cells that are invalid (under a
    /// road, overlapped by an older block), covered by a building, or unknown are skipped; the
    /// command fails only if nothing at all changes. Zoning is free in M1.
    PaintZones {
        cells: Vec<ZoneCellRef>,
        zone: ZoneType,
    },
}

/// Why a `PaintZones` changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZoneError {
    /// The grid resource is missing from the world.
    Missing,
    /// No cells were given.
    NoCells,
    /// Every cell was skipped or already had that type.
    NothingToPaint {
        invalid: usize,
        occupied: usize,
        unchanged: usize,
    },
}

impl fmt::Display for ZoneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "no zone grid in the sim world"),
            Self::NoCells => write!(f, "no cells to paint"),
            Self::NothingToPaint {
                invalid,
                occupied,
                unchanged,
            } => write!(
                f,
                "nothing to paint ({invalid} invalid, {occupied} built on, {unchanged} already that zone)"
            ),
        }
    }
}

impl std::error::Error for ZoneError {}

/// Paints the cells; returns the cost (zero).
pub fn apply(world: &mut World, cmd: &ZoneCommand) -> Result<Cost, ZoneError> {
    match cmd {
        ZoneCommand::PaintZones { cells, zone } => {
            let mut grid = world
                .get_resource_mut::<ZoneGrid>()
                .ok_or(ZoneError::Missing)?;
            paint(&mut grid, cells, *zone)
        }
    }
}

/// The pure part of [`apply`], for tools and tests.
pub fn paint(
    grid: &mut ZoneGrid,
    cells: &[ZoneCellRef],
    zone: ZoneType,
) -> Result<Cost, ZoneError> {
    if cells.is_empty() {
        return Err(ZoneError::NoCells);
    }
    let (mut invalid, mut occupied, mut unchanged, mut painted) = (0, 0, 0, 0);
    for cell in cells {
        // A built-on cell is invalid too; report it as built on.
        if grid.is_occupied(*cell) {
            occupied += 1;
        } else if !grid.is_valid(*cell) {
            invalid += 1;
        } else if grid.zone_of(*cell) == Some(zone) {
            unchanged += 1;
        } else {
            grid.set_zone(*cell, zone);
            painted += 1;
        }
    }
    if painted == 0 {
        return Err(ZoneError::NothingToPaint {
            invalid,
            occupied,
            unchanged,
        });
    }
    Ok(Cost::ZERO)
}
