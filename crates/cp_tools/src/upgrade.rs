//! The road upgrade tool (T-081): hovering a road highlights the piece the upgrade takes, the
//! preview says what changing it to the chosen road type would cost, and a click sends one
//! `UpgradeSegment` (a whole segment) or `UpgradePart` (part of one) per segment touched that is
//! not of that type yet, all in the same frame (so the same tick). The piece (BUG-103) runs
//! between the nearest stops on either side of the cursor: a junction, a corner, a dead end, a
//! change of road type or height, or a section mark (every 10 cells from either end of the
//! stretch, as the road tool shows them); with Shift it is the whole road run (as the bulldozer).
//! Nothing here changes the sim.

use bevy_math::Vec2;
use cp_content::{ContentDb, RoadDefId};
use cp_core::Cost;
use cp_locale::{tr, trf};
use cp_net::{
    Elevation, NetCommand, NetGraph, PART_SNAP, SegmentId, curve_outside_owned, locate, road_run,
    segment_cost, span, stretch,
};
use cp_sim::SimCommand;
use cp_terrain::TileOwnership;
use cp_zone::{ZoneCellRef, ZoneGrid};

use crate::bulldozer::{Span, pick_segment};
use crate::lattice::section_marks;

/// Why a run that leaves the bought tiles cannot be changed (§5.1).
pub const OUTSIDE_LAND: &str = "outside your land";

/// What an upgrade of one road run would do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpgradePreview {
    /// The segments of the piece under the cursor.
    pub run: Vec<SegmentId>,
    /// The piece itself (highlighted): `(segment, s0, s1)`, arc lengths from each segment's start.
    pub spans: Vec<Span>,
    /// The part of the run that changes: segments not already of the target type.
    pub changes: Vec<SegmentId>,
    /// What the commands would charge: the target type's price for the changed lengths.
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
            trf!(
                plural "no zoning on the new type: {n} built cells lose their road (NO_ROAD)",
                n = self.dropped_building_cells
            )
        })
    }
}

/// The piece of road the upgrade takes at `p` (see the module docs): `whole_run` (Shift) takes
/// the road run of the segment under the cursor. Ends within [`PART_SNAP`] of a segment end
/// move onto it, as `UpgradePart` moves them, so what is highlighted is what changes.
pub fn upgrade_piece(net: &NetGraph, p: Vec2, radius: f32, whole_run: bool) -> Vec<Span> {
    let Some(seg) = pick_segment(net, p, radius) else {
        return Vec::new();
    };
    if whole_run {
        return whole(net, &road_run(net, seg));
    }
    let (Some(st), Some(s)) = (stretch(net, seg), net.segment(seg)) else {
        return Vec::new();
    };
    let (t, _) = s.curve.nearest_xz(p);
    let Some(along) = locate(net, seg, s.curve.arc_table().t_to_s(t)) else {
        return Vec::new();
    };
    let mut stops = vec![0.0];
    stops.extend(section_marks(net, seg).into_iter().map(|m| m.0));
    stops.push(st.length);
    // The first stop beyond the cursor closes the piece, the one before it opens it.
    let hi = stops
        .iter()
        .position(|&a| a > along + 1e-3)
        .unwrap_or(stops.len() - 1)
        .max(1);
    span(net, &st, stops[hi - 1], stops[hi])
        .into_iter()
        .filter_map(|(id, s0, s1)| {
            let len = net.segment(id)?.length;
            let s0 = if s0 < PART_SNAP { 0.0 } else { s0 };
            let s1 = if s1 > len - PART_SNAP { len } else { s1 };
            (s1 - s0 >= cp_net::MIN_SEGMENT_LENGTH).then_some((id, s0, s1))
        })
        .collect()
}

fn whole(net: &NetGraph, run: &[SegmentId]) -> Vec<Span> {
    run.iter()
        .filter_map(|&id| net.segment(id).map(|s| (id, 0.0, s.length)))
        .collect()
}

/// The preview of upgrading the whole road run through `seg` to `target`.
pub fn upgrade_preview(
    net: &NetGraph,
    tiles: &TileOwnership,
    zones: &ZoneGrid,
    content: &ContentDb,
    seg: SegmentId,
    target: RoadDefId,
) -> UpgradePreview {
    let spans = whole(net, &road_run(net, seg));
    piece_preview(net, tiles, zones, content, &spans, target)
}

/// The preview of upgrading the piece `spans` (from [`upgrade_piece`]) to `target`.
pub fn piece_preview(
    net: &NetGraph,
    tiles: &TileOwnership,
    zones: &ZoneGrid,
    content: &ContentDb,
    spans: &[Span],
    target: RoadDefId,
) -> UpgradePreview {
    let mut out = UpgradePreview {
        run: spans.iter().map(|s| s.0).collect(),
        spans: spans.to_vec(),
        ..Default::default()
    };
    if target.index() >= content.roads().count() {
        out.errors.push(tr("pick a road type first"));
        return out;
    }
    let def = content.road(target);
    if spans.iter().any(|&(id, ..)| {
        net.segment(id)
            .is_some_and(|s| curve_outside_owned(tiles, &s.curve).is_some())
    }) {
        out.errors.push(tr(OUTSIDE_LAND));
        return out;
    }
    for &(id, s0, s1) in spans {
        let Some(s) = net.segment(id) else { continue };
        if s.road == target {
            continue;
        }
        let cost_per_m = if s.elevation == Elevation::Elevated {
            match def.elevated.as_ref() {
                Some(v) => v.cost_per_m,
                None => {
                    out.errors
                        .push(trf!("{name} has no elevated variant", name = tr(&def.name)));
                    continue;
                }
            }
        } else {
            def.cost_per_m
        };
        out.cost = Cost(out.cost.0 + segment_cost(s1 - s0, cost_per_m).0);
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

/// Per changed segment, in order: `UpgradeSegment` when the piece takes all of it,
/// `UpgradePart` otherwise.
pub fn upgrade_commands(
    net: &NetGraph,
    preview: &UpgradePreview,
    target: RoadDefId,
) -> Vec<SimCommand> {
    if !preview.errors.is_empty() {
        return Vec::new();
    }
    preview
        .spans
        .iter()
        .filter(|s| preview.changes.contains(&s.0))
        .map(|&(seg, s0, s1)| {
            let whole = net.segment(seg).is_none_or(|s| s1 >= s.length);
            SimCommand::Net(if s0 <= 0.0 && whole {
                NetCommand::UpgradeSegment { seg, road: target }
            } else {
                NetCommand::UpgradePart {
                    seg,
                    s0,
                    s1,
                    road: target,
                }
            })
        })
        .collect()
}
