//! Which segments connect to the map edge (`architecture/system-design.md` §5.1, §9.1).
//!
//! [`EntryReach`] is a derived resource: a flood fill over the segments from every
//! `Outside` node, ignoring lane directions. It is not saved and not hashed; it is recomputed
//! whenever the graph changed ([`update_entry_reach`], `Network` phase), so after a load the
//! same code rebuilds it from the saved graph. Without an `Outside` node it is inactive and
//! the rules that read it stay as they were (old saves, scenes, tests).

use std::collections::BTreeSet;

use bevy_ecs::change_detection::DetectChanges;
use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_math::{Vec2, Vec3};

use crate::graph::NetGraph;
use crate::model::{NodeId, NodeKind, SegmentId};

#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct EntryReach {
    /// Positions of the `Outside` nodes, in node-id order.
    outside: Vec<Vec3>,
    /// Segments connected to an `Outside` node.
    segments: BTreeSet<SegmentId>,
}

impl EntryReach {
    /// Whether the graph has an `Outside` node, i.e. whether the entry rule applies.
    pub fn is_active(&self) -> bool {
        !self.outside.is_empty()
    }

    /// Whether `seg` is connected to an `Outside` node by road.
    pub fn reaches(&self, seg: SegmentId) -> bool {
        self.segments.contains(&seg)
    }

    /// Distance in the XZ plane from `p` to the nearest `Outside` node.
    pub fn distance_to_outside(&self, p: Vec2) -> Option<f32> {
        self.outside
            .iter()
            .map(|o| Vec2::new(o.x, o.z).distance(p))
            .min_by(f32::total_cmp)
    }

    /// Computes the reach of `graph`.
    pub fn compute(graph: &NetGraph) -> Self {
        let mut out = Self::default();
        let mut seen: BTreeSet<NodeId> = BTreeSet::new();
        let mut stack: Vec<NodeId> = Vec::new();
        for (id, n) in graph.nodes() {
            if n.kind == NodeKind::Outside {
                out.outside.push(n.pos);
                seen.insert(id);
                stack.push(id);
            }
        }
        while let Some(id) = stack.pop() {
            let Some(n) = graph.node(id) else { continue };
            for &s in &n.segments {
                if !out.segments.insert(s) {
                    continue;
                }
                if let Some(seg) = graph.segment(s) {
                    for next in [seg.start, seg.end] {
                        if seen.insert(next) {
                            stack.push(next);
                        }
                    }
                }
            }
        }
        out
    }
}

/// `Network`-phase system: recomputes [`EntryReach`] when the graph changed.
pub fn update_entry_reach(graph: Res<NetGraph>, mut reach: ResMut<EntryReach>) {
    if graph.is_changed() || reach.is_added() {
        let next = EntryReach::compute(&graph);
        if *reach != next {
            *reach = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Elevation;
    use cp_content::ContentDb;
    use cp_core::curve::CubicBezier3;

    #[test]
    fn reach_follows_roads_from_outside_and_is_inactive_without_it() {
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        let line =
            |a: f32, b: f32| CubicBezier3::line(Vec3::new(a, 0.0, 100.0), Vec3::new(b, 0.0, 100.0));
        let mut g = NetGraph::new();
        assert!(!EntryReach::compute(&g).is_active());
        g.add_node(
            Vec3::new(0.0, 0.0, 100.0),
            NodeKind::Outside,
            Elevation::Ground,
        );
        let a = g
            .add_segment(&def, road, line(0.0, 100.0), Elevation::Ground)
            .unwrap();
        let b = g
            .add_segment(&def, road, line(100.0, 200.0), Elevation::Ground)
            .unwrap();
        let far = g
            .add_segment(&def, road, line(500.0, 600.0), Elevation::Ground)
            .unwrap();
        let r = EntryReach::compute(&g);
        assert!(r.is_active() && r.reaches(a) && r.reaches(b) && !r.reaches(far));
        assert_eq!(r.distance_to_outside(Vec2::new(30.0, 100.0)), Some(30.0));
        g.remove_segment(a).unwrap();
        assert!(!EntryReach::compute(&g).reaches(b));
    }
}
