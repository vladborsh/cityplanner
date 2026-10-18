//! `cp_zone` — zone blocks and cells (`architecture/system-design.md` §7).

pub mod command;
pub mod geom;
pub mod grid;
pub mod layout;
pub mod legacy;
mod roads;

pub use command::{ZoneCommand, ZoneError, apply, paint};
pub use grid::{
    BLOCK_DEPTH, CELL_SIZE, MAX_CELL_SLOPE, PENETRATION_EPS, RoadZoning, Side, StripPreview,
    ZoneBlock, ZoneBlockId, ZoneCellRef, ZoneGrid,
};
pub use layout::{
    BlockDraft, FrontEnd, Frontage, Mark, StreetTick, fit_cells, frontage_of, frontages, street_ticks,
};

use bevy_ecs::change_detection::DetectChanges;
use bevy_ecs::system::{Res, ResMut};
use cp_content::SimContent;
use cp_net::NetGraph;

/// `Network`-phase system: keeps [`ZoneGrid`] in line with the road network.
///
/// It runs in `SimPhase::Network`, after `cp_net::update_junctions` (blocks start at the
/// junction setbacks, so they need this tick's geometry) and before `Buildings` (which read valid cells). It does
/// nothing until the net changed (or after a load, until the grid's index is rebuilt) and
/// does nothing without content (road widths and `allows_zoning` come from `SimContent`).
pub fn update_zones(
    net: Res<NetGraph>,
    content: Option<Res<SimContent>>,
    terrain: Option<Res<cp_terrain::SimTerrain>>,
    mut grid: ResMut<ZoneGrid>,
) {
    let Some(content) = content else {
        return;
    };
    if let Some(t) = &terrain
        && !grid.has_terrain(&t.0)
    {
        grid.set_terrain(std::sync::Arc::clone(&t.0));
    }
    if !net.is_changed() && grid.index_ready() && !grid.terrain_dirty() {
        return;
    }
    sync_grid(&mut grid, &net, &content.0);
}

/// Brings `grid` in line with `net` (what [`update_zones`] does once the net changed). Also used
/// by `cp_sim` on a clone while the sim is paused, so a road built then shows its cells.
pub fn sync_grid(grid: &mut ZoneGrid, net: &NetGraph, db: &cp_content::ContentDb) {
    grid.set_block_fit(db.balance().zoning_block_fit_m);
    grid.sync(net, &road_zoning(db));
}

/// What the zoning needs to know about each road type of `db`.
pub fn road_zoning(
    db: &cp_content::ContentDb,
) -> impl Fn(cp_content::RoadDefId) -> RoadZoning + '_ {
    move |id| {
        let d = db.road(id);
        RoadZoning {
            allows_zoning: d.allows_zoning,
            half_width: d.width / 2.0,
            footpath: d.class == cp_content::RoadClass::Path,
        }
    }
}

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_zone";

#[cfg(test)]
mod tests;
