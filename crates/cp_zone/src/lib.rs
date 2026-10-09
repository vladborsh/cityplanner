//! `cp_zone` — zone blocks and cells (`architecture/system-design.md` §7).

pub mod command;
pub mod geom;
pub mod grid;

pub use command::{ZoneCommand, ZoneError, apply};
pub use grid::{
    BLOCK_DEPTH, CELL_SIZE, PENETRATION_EPS, RoadZoning, Side, ZoneBlock, ZoneBlockId, ZoneCellRef,
    ZoneGrid,
};

use bevy_ecs::change_detection::DetectChanges;
use bevy_ecs::system::{Res, ResMut};
use cp_content::SimContent;
use cp_net::{JunctionGeometry, NetGraph};

/// `Network`-phase system: keeps [`ZoneGrid`] in line with the road network.
///
/// It runs in `SimPhase::Network`, after `cp_net::update_junctions` (blocks start at the
/// junction setbacks, so they need this tick's geometry) and before `Buildings` (which read valid cells). It does
/// nothing until the net changed (or after a load, until the grid's index is rebuilt) and
/// does nothing without content (road widths and `allows_zoning` come from `SimContent`).
pub fn update_zones(
    net: Res<NetGraph>,
    content: Option<Res<SimContent>>,
    junctions: Res<JunctionGeometry>,
    mut grid: ResMut<ZoneGrid>,
) {
    let Some(content) = content else {
        return;
    };
    if !net.is_changed() && grid.index_ready() {
        return;
    }
    let db = &content.0;
    grid.sync(
        &net,
        &|id| {
            let d = db.road(id);
            RoadZoning {
                allows_zoning: d.allows_zoning,
                half_width: d.width / 2.0,
            }
        },
        &|id, seg| junctions.trimmed_curve(&net, id).unwrap_or(seg.curve),
    );
}

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_zone";

#[cfg(test)]
mod tests;
