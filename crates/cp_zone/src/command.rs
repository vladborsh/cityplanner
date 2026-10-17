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
    /// road, overlapped by an older block) or unknown are skipped; a cell under a building is
    /// painted like any other (BUG-053: the building that no longer fits its zone is demolished
    /// by the sim afterwards). The command fails only if nothing at all changes. Zoning is free.
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
    NothingToPaint { invalid: usize, unchanged: usize },
}

impl fmt::Display for ZoneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "no zone grid in the sim world"),
            Self::NoCells => write!(f, "no cells to paint"),
            Self::NothingToPaint { invalid, unchanged } => write!(
                f,
                "nothing to paint ({invalid} invalid, {unchanged} already that zone)"
            ),
        }
    }
}

impl std::error::Error for ZoneError {}

/// Paints the cells; returns the cost (zero).
pub fn apply(world: &mut World, cmd: &ZoneCommand) -> Result<Cost, ZoneError> {
    match cmd {
        ZoneCommand::PaintZones { cells, zone } => {
            sync_before_paint(world);
            let mut grid = world
                .get_resource_mut::<ZoneGrid>()
                .ok_or(ZoneError::Missing)?;
            paint(&mut grid, cells, *zone)
        }
    }
}

/// Brings the grid in line with the net before cells are painted (BUG-049). The zone sync is a
/// tick system, so while the game is paused a road built a moment ago has cells in the view
/// (a synced clone, BUG-038) that the world's own grid does not have yet; a paint naming them
/// would be skipped as unknown. Syncing here is what the next tick would do anyway, and the
/// sync only touches segments whose fingerprint changed.
fn sync_before_paint(world: &mut World) {
    use cp_content::SimContent;
    use cp_net::{JunctionGeometry, NetGraph};
    let Some(content) = world.get_resource::<SimContent>().map(|c| c.0.clone()) else {
        return;
    };
    let terrain = world
        .get_resource::<cp_terrain::SimTerrain>()
        .map(|t| t.0.clone());
    world.resource_scope(|world, mut grid: bevy_ecs::prelude::Mut<ZoneGrid>| {
        let (Some(net), Some(junctions)) = (
            world.get_resource::<NetGraph>(),
            world.get_resource::<JunctionGeometry>(),
        ) else {
            return;
        };
        if let Some(t) = terrain
            && !grid.has_terrain(&t)
        {
            grid.set_terrain(t);
        }
        crate::sync_grid(&mut grid, net, &content, junctions);
    });
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
    let (mut invalid, mut unchanged, mut painted) = (0, 0, 0);
    for cell in cells {
        // A cell under a building is invalid for growth but can be zoned: the building goes.
        if !grid.is_valid(*cell) && !grid.is_occupied(*cell) {
            invalid += 1;
        } else if grid.zone_of(*cell) == Some(zone) {
            unchanged += 1;
        } else {
            grid.set_zone(*cell, zone);
            painted += 1;
        }
    }
    if painted == 0 {
        return Err(ZoneError::NothingToPaint { invalid, unchanged });
    }
    Ok(Cost::ZERO)
}
