//! The road upgrade tool (T-081): hovering a segment highlights its road run, the preview says
//! what changing the run to the chosen road type would cost, and a click sends one
//! `UpgradeSegment` per segment that is not of that type yet, all in the same frame (so the
//! same tick). The command itself exists since T-018; nothing here changes the sim.

use cp_content::{ContentDb, RoadDefId};
use cp_core::Cost;
use cp_net::{Elevation, NetCommand, NetGraph, SegmentId, road_run, segment_cost};
use cp_sim::SimCommand;
use cp_zone::{ZoneCellRef, ZoneGrid};

/// What an upgrade of one road run would do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpgradePreview {
    /// The whole run under the cursor (highlighted).
    pub run: Vec<SegmentId>,
    /// The part of the run that changes: segments not already of the target type.
    pub changes: Vec<SegmentId>,
    /// What the commands would charge: the target type's price for the changed segments.
    pub cost: Cost,
    /// Cells under buildings in blocks the new type would remove (they get `NO_ROAD`).
    pub dropped_building_cells: usize,
    /// Why the upgrade cannot be done, one line each.
    pub errors: Vec<String>,
}

impl UpgradePreview {
    /// The run is already of the target type.
    pub fn is_noop(&self) -> bool {
        self.changes.is_empty() && self.errors.is_empty()
    }

    /// The warning for the verdict, if the new type drops zoned blocks that have buildings.
    pub fn warning(&self) -> Option<String> {
        (self.dropped_building_cells > 0).then(|| {
            format!(
                "no zoning on the new type: {} built cells lose their road (NO_ROAD)",
                self.dropped_building_cells
            )
        })
    }
}

/// The preview of upgrading the run through `seg` to `target`.
pub fn upgrade_preview(
    net: &NetGraph,
    zones: &ZoneGrid,
    content: &ContentDb,
    seg: SegmentId,
    target: RoadDefId,
) -> UpgradePreview {
    let run = road_run(net, seg);
    let mut out = UpgradePreview {
        run: run.clone(),
        ..Default::default()
    };
    if target.index() >= content.roads().count() {
        out.errors.push("pick a road type first".to_string());
        return out;
    }
    let def = content.road(target);
    for id in run {
        let Some(s) = net.segment(id) else { continue };
        if s.road == target {
            continue;
        }
        let cost_per_m = if s.elevation == Elevation::Elevated {
            match def.elevated.as_ref() {
                Some(v) => v.cost_per_m,
                None => {
                    out.errors
                        .push(format!("{} has no elevated variant", def.name));
                    continue;
                }
            }
        } else {
            def.cost_per_m
        };
        out.cost = Cost(out.cost.0 + segment_cost(s.length, cost_per_m).0);
        if !def.allows_zoning && content.road(s.road).allows_zoning {
            out.dropped_building_cells += built_cells(zones, id);
        }
        out.changes.push(id);
    }
    out
}

/// Cells of the segment's blocks that a building covers.
fn built_cells(zones: &ZoneGrid, seg: SegmentId) -> usize {
    zones
        .blocks_of(seg)
        .into_iter()
        .flatten()
        .flat_map(|b| zones.block_cells(b))
        .filter(|c: &ZoneCellRef| zones.is_occupied(*c))
        .count()
}

/// One `UpgradeSegment` per changed segment, in run order.
pub fn upgrade_commands(preview: &UpgradePreview, target: RoadDefId) -> Vec<SimCommand> {
    if !preview.errors.is_empty() {
        return Vec::new();
    }
    preview
        .changes
        .iter()
        .map(|&seg| SimCommand::Net(NetCommand::UpgradeSegment { seg, road: target }))
        .collect()
}
