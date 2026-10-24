//! The Trees tool (T-323, §24.2): hovering a segment highlights it and shows the price of
//! planting a row of street trees along it (or "free" to take the row away); a click sends
//! `SetTrees`, Shift-click does the whole stretch (§6.7) from junction to junction. Clicking a
//! planted segment removes the row.

use cp_content::ContentDb;
use cp_core::Cost;
use cp_locale::tr;
use cp_net::{
    NetCommand, NetGraph, SegmentId, curve_outside_owned, segment_cost, stretch, trees_allowed,
};
use cp_sim::SimCommand;
use cp_terrain::TileOwnership;

use crate::upgrade::OUTSIDE_LAND;

/// Why a segment takes no trees.
pub const NOT_ON_BRIDGES: &str = "Not on bridges";
pub const NOT_ON_RAILS: &str = "Not on rails";

/// What a click would do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreesPreview {
    /// The segments to change (highlighted), those that already are as wanted left out.
    pub segs: Vec<SegmentId>,
    /// Everything under the cursor, for the highlight.
    pub shown: Vec<SegmentId>,
    /// `true` plants, `false` removes.
    pub plant: bool,
    /// The price of planting; zero when removing.
    pub cost: Cost,
    pub errors: Vec<String>,
}

/// The preview for the segment `seg`, or its whole stretch with `whole`. The first segment
/// decides: a planted one is cleared, a bare one planted.
pub fn trees_preview(
    net: &NetGraph,
    tiles: &TileOwnership,
    content: &ContentDb,
    seg: SegmentId,
    whole: bool,
) -> TreesPreview {
    let mut out = TreesPreview::default();
    let Some(first) = net.segment(seg) else {
        return out;
    };
    out.plant = !first.trees;
    out.shown = if whole {
        stretch(net, seg).map_or_else(|| vec![seg], |s| s.segments)
    } else {
        vec![seg]
    };
    let per_m = content.balance().trees_cost_per_m;
    for &id in &out.shown {
        let Some(s) = net.segment(id) else { continue };
        if curve_outside_owned(tiles, &s.curve).is_some() {
            out.errors.push(tr(OUTSIDE_LAND));
            break;
        }
        if out.plant {
            if !trees_allowed(content.road(s.road), s.elevation) {
                continue;
            }
            if !s.trees {
                out.segs.push(id);
                out.cost = Cost(out.cost.0 + segment_cost(s.length, per_m).0);
            }
        } else if s.trees {
            out.segs.push(id);
        }
    }
    if out.errors.is_empty() && out.plant && out.segs.is_empty() {
        let s = net.segment(seg).expect("checked above");
        let def = content.road(s.road);
        let rail = def.class == cp_content::RoadClass::Rail;
        out.errors
            .push(tr(if rail || s.elevation == cp_net::Elevation::Ground {
                NOT_ON_RAILS
            } else {
                NOT_ON_BRIDGES
            }));
    }
    out
}

/// One `SetTrees` per segment to change.
pub fn trees_commands(preview: &TreesPreview) -> Vec<SimCommand> {
    if !preview.errors.is_empty() {
        return Vec::new();
    }
    preview
        .segs
        .iter()
        .map(|&seg| {
            SimCommand::Net(NetCommand::SetTrees {
                seg,
                trees: preview.plant,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::Vec3;
    use cp_core::curve::CubicBezier3;
    use cp_net::Elevation;

    fn db() -> ContentDb {
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap()
    }

    fn line(g: &mut NetGraph, db: &ContentDb, road: &str, x: f32, el: Elevation) -> SegmentId {
        let id = db.road_id(road).unwrap();
        g.add_segment(
            db.road(id),
            id,
            CubicBezier3::line(Vec3::new(x, 0.0, 5100.0), Vec3::new(x + 60.0, 0.0, 5100.0)),
            el,
        )
        .unwrap()
    }

    #[test]
    fn a_click_plants_a_segment_a_shift_click_its_stretch_and_a_second_click_clears() {
        let db = db();
        let tiles = TileOwnership::only((2, 2));
        let mut g = NetGraph::new();
        let a = line(&mut g, &db, "basic_2l", 5100.0, Elevation::Ground);
        let b = line(&mut g, &db, "basic_2l", 5160.0, Elevation::Ground);
        let one = trees_preview(&g, &tiles, &db, a, false);
        assert!(one.plant && one.errors.is_empty());
        assert_eq!(one.segs, vec![a]);
        assert!(one.cost.0 > 0);
        let all = trees_preview(&g, &tiles, &db, a, true);
        assert_eq!(all.segs, vec![a, b]);
        let cmds = trees_commands(&all);
        assert_eq!(cmds.len(), 2);
        assert!(matches!(
            cmds[0],
            SimCommand::Net(NetCommand::SetTrees { seg, trees: true }) if seg == a
        ));
        g.set_trees(a, true).unwrap();
        let clear = trees_preview(&g, &tiles, &db, a, false);
        assert!(!clear.plant && clear.cost == Cost::ZERO);
        assert_eq!(clear.segs, vec![a]);
    }

    #[test]
    fn a_bridge_is_refused_with_its_reason() {
        let db = db();
        let tiles = TileOwnership::only((2, 2));
        let mut g = NetGraph::new();
        let s = line(&mut g, &db, "basic_2l", 5100.0, Elevation::Elevated);
        let p = trees_preview(&g, &tiles, &db, s, false);
        assert_eq!(p.errors, vec![tr(NOT_ON_BRIDGES)]);
        assert!(trees_commands(&p).is_empty());
    }
}
