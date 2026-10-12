//! The "road run" (`architecture/system-design.md` §6.3): what the bulldozer removes with one
//! click. One drawn street is a chain of segments (path vertices, crossings and the 96 m cut
//! leave degree-2 nodes), so the run is the maximal chain through degree-2 nodes whose
//! segments share road type and elevation.

use std::collections::BTreeSet;

use crate::graph::NetGraph;
use crate::model::{NodeId, SegmentId};

/// The run containing `seg`, ordered from one end to the other. Its boundaries are a junction
/// (3 or more segments), a dead end, or a change of road type or elevation; a closed ring with
/// none of those is returned whole. Unknown `seg` gives an empty run.
pub fn road_run(graph: &NetGraph, seg: SegmentId) -> Vec<SegmentId> {
    let Some(first) = graph.segment(seg) else {
        return Vec::new();
    };
    let key = (first.road, first.elevation);
    let mut seen: BTreeSet<SegmentId> = BTreeSet::from([seg]);

    // Walks away from `seg` through `node`, returning the segments met in order.
    let mut walk = |mut node: NodeId, mut from: SegmentId| {
        let mut out = Vec::new();
        while let Some(n) = graph.node(node) {
            if n.segments.len() != 2 {
                break;
            }
            let Some(next) = n.segments.iter().copied().find(|s| *s != from) else {
                break;
            };
            let Some(s) = graph.segment(next) else { break };
            if seen.contains(&next) || (s.road, s.elevation) != key {
                break;
            }
            seen.insert(next);
            out.push(next);
            node = if s.start == node { s.end } else { s.start };
            from = next;
        }
        out
    };
    let mut run: Vec<SegmentId> = walk(first.start, seg);
    run.reverse();
    run.push(seg);
    run.extend(walk(first.end, seg));
    run
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Elevation;
    use bevy_math::Vec3;
    use cp_content::{ContentDb, RoadDef, RoadDefId};
    use cp_core::curve::CubicBezier3;

    const G: Elevation = Elevation::Ground;

    fn content() -> (ContentDb, RoadDefId, RoadDef) {
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap_or_else(|e| panic!("{e}"));
        let id = db.road_id("basic_2l").unwrap();
        let def = db.road(id).clone();
        (db, id, def)
    }

    fn pt(x: f32, z: f32) -> Vec3 {
        Vec3::new(x, 0.0, z)
    }

    /// A chain of `n` equal segments from `a` to `b`, start to end.
    fn chain(
        g: &mut NetGraph,
        (id, def): (RoadDefId, &RoadDef),
        a: Vec3,
        b: Vec3,
        n: usize,
    ) -> Vec<SegmentId> {
        (0..n)
            .map(|i| {
                let p = |k: usize| a.lerp(b, k as f32 / n as f32);
                g.add_segment(def, id, CubicBezier3::line(p(i), p(i + 1)), G)
                    .unwrap()
            })
            .collect()
    }

    /// A 300 m street in 4 pieces between two four-way crossings.
    fn grid(g: &mut NetGraph, road: (RoadDefId, &RoadDef)) -> Vec<SegmentId> {
        let street = chain(g, road, pt(0.0, 0.0), pt(300.0, 0.0), 4);
        for (a, b) in [
            (pt(-80.0, 0.0), pt(0.0, 0.0)),
            (pt(300.0, 0.0), pt(380.0, 0.0)),
            (pt(0.0, -80.0), pt(0.0, 0.0)),
            (pt(0.0, 0.0), pt(0.0, 80.0)),
            (pt(300.0, -80.0), pt(300.0, 0.0)),
            (pt(300.0, 0.0), pt(300.0, 80.0)),
        ] {
            chain(g, road, a, b, 1);
        }
        street
    }

    #[test]
    fn a_street_between_two_crossings_is_one_run_and_the_crossings_survive() {
        let (_db, id, def) = content();
        let mut g = NetGraph::new();
        let street = grid(&mut g, (id, &def));
        for s in &street {
            let run = road_run(&g, *s);
            let mut sorted = run.clone();
            sorted.sort();
            let mut want = street.clone();
            want.sort();
            assert_eq!(sorted, want, "any piece gives the whole street");
        }
        // Ordered end to end.
        let run = road_run(&g, street[2]);
        assert!(run == street || run == street.iter().rev().copied().collect::<Vec<_>>());

        let (a, b) = (
            g.segment(street[0]).unwrap().start,
            g.segment(street[3]).unwrap().end,
        );
        for s in run {
            g.remove_segment(s).unwrap();
        }
        assert_eq!(g.node(a).unwrap().segments.len(), 3);
        assert_eq!(g.node(b).unwrap().segments.len(), 3);
        assert_eq!(g.node_count(), 8, "two crossings and six arm ends");
    }

    #[test]
    fn a_dead_end_branch_is_its_whole_chain() {
        let (_db, id, def) = content();
        let mut g = NetGraph::new();
        grid(&mut g, (id, &def));
        let branch = chain(&mut g, (id, &def), pt(0.0, 0.0), pt(200.0, 200.0), 3);
        assert_eq!(road_run(&g, branch[1]).len(), 3);
        // The crossing it leaves bounds it from the street's pieces.
        assert_eq!(road_run(&g, branch[0]).len(), 3);
        assert_eq!(road_run(&g, branch[2]).len(), 3);
    }

    #[test]
    fn a_change_of_road_type_bounds_the_run() {
        let (db, id, def) = content();
        let mut g = NetGraph::new();
        let street = grid(&mut g, (id, &def));
        let other = db.road_id("medium_4l").unwrap();
        g.upgrade_segment(street[2], db.road(other), other).unwrap();
        assert_eq!(road_run(&g, street[0]), vec![street[0], street[1]]);
        assert_eq!(road_run(&g, street[2]), vec![street[2]]);
        assert_eq!(road_run(&g, street[3]), vec![street[3]]);
    }

    #[test]
    fn a_change_of_elevation_bounds_the_run() {
        let (_db, id, def) = content();
        let mut g = NetGraph::new();
        let a = chain(&mut g, (id, &def), pt(0.0, 0.0), pt(100.0, 0.0), 2);
        let b = g
            .add_segment(
                &def,
                id,
                CubicBezier3::line(pt(100.0, 0.0), pt(200.0, 0.0)),
                Elevation::Bridge,
            )
            .unwrap();
        assert_eq!(road_run(&g, a[0]).len(), 2);
        assert_eq!(road_run(&g, b), vec![b]);
    }

    #[test]
    fn a_closed_ring_without_crossings_is_whole() {
        let (_db, id, def) = content();
        let mut g = NetGraph::new();
        let corners = [
            pt(0.0, 0.0),
            pt(100.0, 0.0),
            pt(100.0, 100.0),
            pt(0.0, 100.0),
        ];
        let ring: Vec<_> = (0..4)
            .map(|i| chain(&mut g, (id, &def), corners[i], corners[(i + 1) % 4], 1)[0])
            .collect();
        let run = road_run(&g, ring[1]);
        assert_eq!(run.len(), 4);
        assert_eq!(run.iter().collect::<BTreeSet<_>>().len(), 4, "no repeats");
    }

    #[test]
    fn an_unknown_segment_has_no_run() {
        let g = NetGraph::new();
        assert!(road_run(&g, SegmentId::new(0, 0)).is_empty());
    }
}
