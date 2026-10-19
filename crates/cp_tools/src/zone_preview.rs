//! What the road tool shows of the zone cells a road will get (T-185): the strip on both sides
//! of the ghost and a tick on every 8-cell line along it. The 8-cell block line that snaps a new
//! road to the distance that closes a block is the resolver's ([`crate::lattice`], T-266).
//!
//! The cells come from `cp_zone`'s own layout ([`cp_zone::ZoneGrid::preview_strips`]), so the
//! preview and the cells a built road gets are laid by the same code; the only difference is
//! that the junction setbacks of a road that does not exist yet are not known, so a stretch
//! that ends at a junction is previewed from its full length.

use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_net::{Elevation, RoadBuildPlan, plan_applied};
use cp_sim::SimView;
use cp_zone::{CELL_SIZE, RoadZoning};
use std::sync::Arc;

pub use crate::lattice::{BLOCK_CELLS, BLOCK_SNAP_M, BlockGuide, block_hover, block_line};

/// One cell of the zone strip of the ghost: its corners and whether it would be zonable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoneGhostCell {
    pub corners: [Vec2; 4],
    pub valid: bool,
}

/// The zone strip of a road plan and its ticks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZoneStrip {
    pub cells: Vec<ZoneGhostCell>,
    /// Across the band, every [`BLOCK_CELLS`] cells from the start node: from the road edge to
    /// the outer edge of the band.
    pub ticks: Vec<[Vec2; 2]>,
}

fn zoning(db: &ContentDb) -> impl Fn(cp_content::RoadDefId) -> RoadZoning + '_ {
    move |id| {
        let d = db.road(id);
        RoadZoning {
            allows_zoning: d.allows_zoning,
            half_width: d.width / 2.0,
            footpath: d.class == cp_content::RoadClass::Path,
        }
    }
}

type StripKey = (usize, usize, u64, RoadBuildPlan);

/// The last strip computed: the ghost is asked for every frame, the layout changes with the plan.
static CACHE: std::sync::Mutex<Option<(StripKey, ZoneStrip)>> = std::sync::Mutex::new(None);

/// The cells and ticks of the road `plan` would build: the frontages of the net after the plan
/// laid by `cp_zone` itself (the cells of the built road, BUG-047), with a tick at every 8th
/// column boundary of each frontage.
pub fn zone_strip(view: &SimView, plan: &RoadBuildPlan) -> ZoneStrip {
    let key: StripKey = (
        Arc::as_ptr(&view.net) as usize,
        Arc::as_ptr(&view.zones) as usize,
        view.zones.revision(),
        plan.clone(),
    );
    if let Some((k, strip)) = CACHE.lock().expect("zone strip cache").as_ref()
        && *k == key
    {
        return strip.clone();
    }
    let strip = compute_strip(view, plan);
    *CACHE.lock().expect("zone strip cache") = Some((key, strip.clone()));
    strip
}

fn compute_strip(view: &SimView, plan: &RoadBuildPlan) -> ZoneStrip {
    let mut out = ZoneStrip::default();
    let Some(content) = view.content.as_deref() else {
        return out;
    };
    let roads = zoning(content);
    if !roads(plan.road).allows_zoning {
        return out;
    }
    let Some(after) = plan_applied(&view.net, content, plan) else {
        return out;
    };
    let mut region: Option<(Vec2, Vec2)> = None;
    for seg in plan
        .segments
        .iter()
        .filter(|s| s.elevation == Elevation::Ground)
    {
        for p in seg.curve.xz() {
            region = Some(region.map_or((p, p), |(lo, hi)| (lo.min(p), hi.max(p))));
        }
    }
    let Some(region) = region else {
        return out;
    };
    for strip in view.zones.preview_layout(&after, &roads, region) {
        for (o, valid) in strip.obbs.iter().zip(&strip.valid) {
            out.cells.push(ZoneGhostCell {
                corners: o.corners(),
                valid: *valid,
            });
        }
        let _ = strip.marks;
    }
    // Ticks: the 8-cell lines of the frontages near the plan (T-265, the layout's own cell
    // lines), each across its band of cells at the centreline of a cross street of the tool's
    // road whose near edge stands on the line.
    let width = content.road(plan.road).width;
    let reach = inflate_bounds(region, 150.0);
    let near: Vec<cp_net::SegmentId> = after
        .segments()
        .filter(|(_, seg)| {
            seg.elevation == Elevation::Ground
                && seg.curve.bounds_distance_xz((region.0 + region.1) / 2.0)
                    <= (region.1 - region.0).length() / 2.0 + 1.0
        })
        .map(|(id, _)| id)
        .collect();
    for line in cp_zone::cell_lines(&after, &roads, near) {
        let m = line.mark;
        if !m.major || m.end || m.axis.cmple(reach.0).any() || m.axis.cmpge(reach.1).any() {
            continue;
        }
        let Some(seg) = after.segment(line.segment) else {
            continue;
        };
        let hw = content.road(seg.road).width / 2.0;
        let at = m.axis + m.onward * (width / 2.0);
        let tick = [at + m.v * hw, at + m.v * (hw + 8.0 * CELL_SIZE)];
        if !out.ticks.iter().any(|o| o[0].distance(tick[0]) < 0.05) {
            out.ticks.push(tick);
        }
    }
    out
}

fn inflate_bounds(b: (Vec2, Vec2), by: f32) -> (Vec2, Vec2) {
    (b.0 - Vec2::splat(by), b.1 + Vec2::splat(by))
}
