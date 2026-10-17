//! What the road tool shows of the zone cells a road will get (T-185): the strip on both sides
//! of the ghost, a tick every 8 cells along it, and the 8-cell block guides that snap a new
//! road to the distance that closes a square block (8 x 8 cells = 64 m between road edges).
//!
//! The cells come from `cp_zone`'s own layout ([`cp_zone::ZoneGrid::preview_strips`]), so the
//! preview and the cells a built road gets are laid by the same code; the only difference is
//! that the junction setbacks of a road that does not exist yet are not known, so a stretch
//! that ends at a junction is previewed from its full length.

use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::curve::to_xz;
use cp_net::{Elevation, RoadBuildPlan, plan_applied};
use cp_sim::SimView;
use cp_zone::{CELL_SIZE, RoadZoning};
use std::sync::Arc;

/// Cells between two ticks, and between the roads of a closed block (4 rows from each).
pub const BLOCK_CELLS: u32 = 8;
/// Metres a closed block is deep between the edges of its roads.
pub const BLOCK_M: f32 = BLOCK_CELLS as f32 * CELL_SIZE;
/// How far from the closed-block distance the cursor snaps to it, metres.
pub const BLOCK_SNAP_M: f32 = 6.0;
/// A parallel road is one within this angle of the ghost's direction, degrees.
const PARALLEL_DEG: f32 = 8.0;
/// How far from the cursor roads and junctions are looked at, metres.
const NEAR_M: f32 = 140.0;

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
        out.ticks.extend(strip.marks);
    }
    out
}

/// A guide that closes a block: the dashed line and what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockGuide {
    /// The cursor, moved onto the guide when it was within [`BLOCK_SNAP_M`] of it.
    pub cursor: Vec2,
    /// Whether the cursor was moved.
    pub snapped: bool,
    /// Dashed guide lines (from the road or junction to the ghost).
    pub lines: Vec<[Vec2; 2]>,
    /// "block 8 cells" or "N cells — partial block".
    pub note: String,
}

fn cells_text(gap: f32) -> String {
    let cells = (gap / CELL_SIZE).round().max(0.0) as u32;
    if cells == BLOCK_CELLS {
        format!("block {BLOCK_CELLS} cells")
    } else {
        format!("{cells} cells — partial block")
    }
}

/// The 8-cell block guide for a straight ghost from `from` to `cursor` of a road with half width
/// `half_width`: a parallel road (or a junction) whose distance from the ghost is within
/// [`BLOCK_SNAP_M`] of a closed block. `None` when nothing is near enough to say anything.
pub fn block_guide(
    view: &SimView,
    from: Vec2,
    cursor: Vec2,
    half_width: f32,
) -> Option<BlockGuide> {
    let content = view.content.as_deref()?;
    let u = (cursor - from).try_normalize()?;
    if from.distance(cursor) < CELL_SIZE * 2.0 {
        return None;
    }
    let cos_par = PARALLEL_DEG.to_radians().cos();
    let mut best: Option<(f32, BlockGuide)> = None;
    let mut consider = |err: f32, g: BlockGuide| {
        if best.as_ref().is_none_or(|(e, _)| err < *e) {
            best = Some((err, g));
        }
    };
    // Parallel roads.
    for (_, s) in view.net.segments() {
        if s.elevation != Elevation::Ground || s.curve.bounds_distance_xz(cursor) > NEAR_M {
            continue;
        }
        let (t, _) = s.curve.nearest_xz(cursor);
        // The cursor must be beside the road, not past its end.
        if !(0.001..0.999).contains(&t) {
            continue;
        }
        let q = to_xz(s.curve.point(t));
        let tg = to_xz(s.curve.tangent(t)).try_normalize().unwrap_or(Vec2::X);
        if tg.dot(u).abs() < cos_par {
            continue;
        }
        let n = Vec2::new(-tg.y, tg.x);
        let side = (cursor - q).dot(n);
        let hw = content.road(s.road).width * 0.5;
        let gap = side.abs() - hw - half_width;
        if !(2.0 * CELL_SIZE..=BLOCK_M + BLOCK_SNAP_M).contains(&gap) {
            continue;
        }
        let err = (gap - BLOCK_M).abs();
        let snapped = err <= BLOCK_SNAP_M;
        // Moved sideways onto the guide, along the normal of the road.
        let target = if snapped {
            cursor + n * (side.signum() * (BLOCK_M + hw + half_width) - side)
        } else {
            cursor
        };
        consider(
            err,
            BlockGuide {
                cursor: target,
                snapped,
                lines: vec![[target - n * (target - q).dot(n), target]],
                note: if snapped {
                    cells_text(BLOCK_M)
                } else {
                    cells_text(gap)
                },
            },
        );
    }
    // Junctions: the ghost is turned about its start until the junction is a block away.
    for (nid, n) in view.net.nodes() {
        if n.segments.len() < 3 {
            continue;
        }
        let p = to_xz(n.pos);
        if p.distance(cursor) > NEAR_M {
            continue;
        }
        // A cross street parallel to the ghost: the strips between the two start at the
        // junction setbacks, so the cells fill 8 columns when the axes are 64 m + two setbacks
        // apart.
        let setback = view.junctions.get(nid).map_or(0.0, |j| {
            j.arms.iter().map(|a| a.setback).fold(0.0f32, f32::max)
        });
        if setback <= 0.0 {
            continue;
        }
        let want = BLOCK_M + 2.0 * setback;
        let dist = from.distance(cursor);
        let lateral = |dir: Vec2| (p - from).dot(Vec2::new(-dir.y, dir.x));
        let gap = lateral(u).abs() - 2.0 * setback;
        if !(2.0 * CELL_SIZE..=BLOCK_M + BLOCK_SNAP_M).contains(&gap) {
            continue;
        }
        // Along the ghost: the junction must be within the length of the ghost.
        let along = (p - from).dot(u);
        if !(0.0..=dist + NEAR_M).contains(&along) {
            continue;
        }
        let err = (gap - BLOCK_M).abs();
        let mut best_turn: Option<(f32, Vec2)> = None;
        if err <= BLOCK_SNAP_M {
            for k in -60..=60 {
                let a = (k as f32 * 0.1).to_radians();
                let (sn, cs) = a.sin_cos();
                let d = Vec2::new(u.x * cs - u.y * sn, u.x * sn + u.y * cs);
                let e = (lateral(d).abs() - want).abs();
                if best_turn.is_none_or(|(be, _)| e < be) {
                    best_turn = Some((e, d));
                }
            }
        }
        let (snapped, dir) = match best_turn {
            Some((e, d)) if e < 0.25 => (true, d),
            _ => (false, u),
        };
        let target = if snapped { from + dir * dist } else { cursor };
        let foot = from + dir * (p - from).dot(dir);
        consider(
            err + 0.5,
            BlockGuide {
                cursor: target,
                snapped,
                lines: vec![[p, foot]],
                note: if snapped {
                    cells_text(BLOCK_M)
                } else {
                    cells_text(gap)
                },
            },
        );
    }
    best.map(|(_, g)| g)
}

/// The cursor of the road tool after the block guide: where the ghost ends and where a click
/// puts the node (`alt` switches the snap off, like the others), and the guide to draw. Only a
/// zonable road with a point already clicked has a guide; a curve is not guided.
pub fn block_hover(
    view: &SimView,
    tool: &crate::RoadTool,
    hover: Vec2,
    alt: bool,
) -> (Vec2, Option<BlockGuide>) {
    let (Some(content), Some(road), Some(&from)) =
        (view.content.as_deref(), tool.road, tool.points.last())
    else {
        return (hover, None);
    };
    let def = content.road(road);
    if alt
        || !def.allows_zoning
        || tool.mode == crate::RoadMode::Curve
        || tool.tunnel_depth.is_some()
    {
        return (hover, None);
    }
    match block_guide(view, from, hover, def.width * 0.5) {
        Some(g) => (g.cursor, Some(g)),
        None => (hover, None),
    }
}
