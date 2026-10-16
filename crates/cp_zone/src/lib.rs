//! `cp_zone` — zone blocks and cells (`architecture/system-design.md` §7).

pub mod command;
pub mod geom;
pub mod grid;
pub mod legacy;

pub use command::{ZoneCommand, ZoneError, apply, paint};
pub use grid::{
    BLOCK_DEPTH, CELL_SIZE, MAX_CELL_SLOPE, PENETRATION_EPS, RoadZoning, Side, ZoneBlock,
    ZoneBlockId, ZoneCellRef, ZoneGrid, strip_layout,
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
    sync_grid(&mut grid, &net, &content.0, &junctions);
}

/// Brings `grid` in line with `net` (what [`update_zones`] does once the net changed). Also used
/// by `cp_sim` on a clone while the sim is paused, so a road built then shows its cells.
pub fn sync_grid(
    grid: &mut ZoneGrid,
    net: &NetGraph,
    db: &cp_content::ContentDb,
    junctions: &JunctionGeometry,
) {
    grid.set_block_fit(db.balance().zoning_block_fit_m);
    grid.sync(
        net,
        &|id| {
            let d = db.road(id);
            RoadZoning {
                allows_zoning: d.allows_zoning,
                half_width: d.width / 2.0,
            }
        },
        &|id, seg| zoned_curve(junctions, id, seg),
    );
}

/// The stretch of a segment that is zoned: its curve cut at the setback of every end that
/// meets a real junction (three or more arms). Dead ends and plain bends (degree <= 2) have
/// no setback, so blocks run to the end of the segment (BUG-016, §7 `floor(len / 8)`).
fn zoned_curve(
    junctions: &JunctionGeometry,
    id: cp_net::SegmentId,
    seg: &cp_net::Segment,
) -> cp_core::curve::CubicBezier3 {
    let t_of = |node, at_start: bool, default: f32| {
        junctions
            .get(node)
            .filter(|j| j.arms.len() >= 3)
            .and_then(|j| {
                j.arms
                    .iter()
                    .find(|a| a.segment == id && a.at_start == at_start)
            })
            .map_or(default, |a| a.trim_t)
    };
    let t0 = t_of(seg.start, true, 0.0);
    let t1 = t_of(seg.end, false, 1.0);
    if t1 > t0 {
        seg.curve.sub_curve(t0, t1)
    } else {
        seg.curve
    }
}

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_zone";

#[cfg(test)]
mod tests;
