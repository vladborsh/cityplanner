//! The flip tool (T-091): hovering a one-way road highlights its run, a click turns every
//! segment of the run round with one `FlipSegment` each, all in the same frame. A two-way road
//! has nothing to flip.

use cp_content::ContentDb;
use cp_net::{NetCommand, NetGraph, SegmentId, curve_outside_owned, road_run};
use cp_sim::SimCommand;
use cp_terrain::TileOwnership;

use crate::upgrade::OUTSIDE_LAND;

/// Why a two-way road is not flipped.
pub const NOT_ONE_WAY: &str = "only a one-way road can be turned round";

/// What flipping the run under the cursor would do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlipPreview {
    /// The whole run (highlighted); empty when there is nothing to flip.
    pub run: Vec<SegmentId>,
    /// Why it cannot be done, one line each.
    pub errors: Vec<String>,
}

/// The preview of flipping the run through `seg`.
pub fn flip_preview(
    net: &NetGraph,
    tiles: &TileOwnership,
    content: &ContentDb,
    seg: SegmentId,
) -> FlipPreview {
    let mut out = FlipPreview::default();
    let Some(s) = net.segment(seg) else {
        return out;
    };
    out.run = road_run(net, seg);
    if !content.road(s.road).is_one_way() {
        out.errors.push(NOT_ONE_WAY.to_string());
        return out;
    }
    if out.run.iter().any(|&id| {
        net.segment(id)
            .is_some_and(|s| curve_outside_owned(tiles, &s.curve).is_some())
    }) {
        out.errors.push(OUTSIDE_LAND.to_string());
    }
    out
}

/// One `FlipSegment` per segment of the run, in run order.
pub fn flip_commands(preview: &FlipPreview) -> Vec<SimCommand> {
    if !preview.errors.is_empty() {
        return Vec::new();
    }
    preview
        .run
        .iter()
        .map(|&seg| SimCommand::Net(NetCommand::FlipSegment { seg }))
        .collect()
}
