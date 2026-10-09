use super::*;
use cp_content::{ContentDb, LaneKind};
use cp_core::curve::to_xz;

fn content() -> ContentDb {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
    ContentDb::load_dir(root).unwrap_or_else(|e| panic!("{e}"))
}

fn road(db: &ContentDb, name: &str) -> (RoadDefId, RoadDef) {
    let id = db.road_id(name).unwrap();
    (id, db.road(id).clone())
}

fn bend(a: Vec3, b: Vec3) -> CubicBezier3 {
    // A gentle S-ish curve between a and b, ~90 m long.
    let d = b - a;
    CubicBezier3::new(
        a,
        a + d * 0.3 + Vec3::new(0.0, 0.0, 12.0),
        a + d * 0.7 - Vec3::new(0.0, 0.0, 12.0),
        b,
    )
}

const G: Elevation = Elevation::Ground;

#[test]
fn add_creates_and_reuses_nodes() {
    let db = content();
    let (id, def) = road(&db, "basic_2l");
    let mut g = NetGraph::new();
    let a = g
        .add_segment(
            &def,
            id,
            CubicBezier3::line(Vec3::ZERO, Vec3::new(50.0, 0.0, 0.0)),
            G,
        )
        .unwrap();
    // Second segment starts 0.05 m off the first one's end: node is reused and snapped.
    let b = g
        .add_segment(
            &def,
            id,
            CubicBezier3::line(Vec3::new(50.05, 0.0, 0.0), Vec3::new(100.0, 0.0, 30.0)),
            G,
        )
        .unwrap();
    assert_eq!(g.node_count(), 3);
    let shared = g.segment(a).unwrap().end;
    assert_eq!(g.segment(b).unwrap().start, shared);
    assert_eq!(g.segment(b).unwrap().curve.p0, Vec3::new(50.0, 0.0, 0.0));
    assert_eq!(g.node(shared).unwrap().kind, NodeKind::Middle);
    assert_eq!(g.node(shared).unwrap().segments, vec![a, b]);
    assert_eq!(
        g.node(g.segment(a).unwrap().start).unwrap().kind,
        NodeKind::End
    );
}

#[test]
fn degenerate_segments_are_rejected_without_side_effects() {
    let db = content();
    let (id, def) = road(&db, "basic_2l");
    let mut g = NetGraph::new();
    let before = g.clone();
    // Both ends at the same place.
    let p = Vec3::new(1.0, 0.0, 1.0);
    assert_eq!(
        g.add_segment(&def, id, CubicBezier3::line(p, p), G),
        Err(NetOpError::DegenerateSegment)
    );
    assert_eq!(g, before);
}

#[test]
fn lanes_match_the_road_profile() {
    let db = content();
    for name in ["basic_2l", "medium_4l", "large_6l", "highway_1w_3l"] {
        let (id, def) = road(&db, name);
        let mut g = NetGraph::new();
        let curve = bend(Vec3::ZERO, Vec3::new(80.0, 0.0, 10.0));
        let s = g.add_segment(&def, id, curve, G).unwrap();
        let seg = g.segment(s).unwrap();
        assert_eq!(seg.lanes.len(), def.lanes.len(), "{name}");
        assert_eq!(g.lane_count(), def.lanes.len());
        for (i, (&lid, ld)) in seg.lanes.iter().zip(&def.lanes).enumerate() {
            let l = g.lane(lid).unwrap();
            assert_eq!((l.segment, l.index as usize), (s, i));
            assert_eq!(
                (l.kind, l.dir, l.width, l.offset),
                (ld.kind, ld.dir, ld.width, ld.offset)
            );
            assert!((l.speed_limit - def.speed_kmh / 3.6).abs() < 1e-4);
            // Lateral distance from the segment axis equals |offset| within 1%.
            for piece in l.curve.iter() {
                for j in 0..=4 {
                    let q = to_xz(piece.point(j as f32 / 4.0));
                    let (_, dist) = curve.nearest_xz(q);
                    assert!(
                        (dist - ld.offset.abs()).abs() <= 0.01 * ld.offset.abs() + 0.02,
                        "{name} lane {i}: {dist} vs {}",
                        ld.offset
                    );
                }
            }
            // Travel direction: forward lanes run start->end, backward lanes end->start.
            let (from, to) = (l.curve[0].p0, l.curve[LANE_CURVE_PIECES - 1].p3);
            let d = match l.dir {
                LaneDir::Forward => (to - from).dot(curve.p3 - curve.p0),
                LaneDir::Backward => (from - to).dot(curve.p3 - curve.p0),
            };
            assert!(d > 0.0, "{name} lane {i} direction");
        }
    }
}

#[test]
fn forward_car_lanes_are_on_the_right() {
    let db = content();
    let (id, def) = road(&db, "basic_2l");
    let mut g = NetGraph::new();
    // Heading +X; the right-hand side is +Z in this crate's convention (left is -Z).
    let s = g
        .add_segment(
            &def,
            id,
            CubicBezier3::line(Vec3::ZERO, Vec3::new(60.0, 0.0, 0.0)),
            G,
        )
        .unwrap();
    for &lid in &g.segment(s).unwrap().lanes {
        let l = g.lane(lid).unwrap();
        if l.kind == LaneKind::Car && l.dir == LaneDir::Forward {
            assert!(
                l.curve[0].p0.z > 0.0,
                "forward car lane must be right of travel"
            );
        }
        if l.kind == LaneKind::Car && l.dir == LaneDir::Backward {
            assert!(l.curve[0].p0.z < 0.0);
        }
    }
}

#[test]
fn split_preserves_length_and_topology() {
    let db = content();
    let (id, def) = road(&db, "medium_4l");
    let mut g = NetGraph::new();
    let curve = bend(Vec3::ZERO, Vec3::new(90.0, 2.0, 15.0));
    let s = g.add_segment(&def, id, curve, G).unwrap();
    let whole = g.segment(s).unwrap().length;
    let (n0, n1) = {
        let seg = g.segment(s).unwrap();
        (seg.start, seg.end)
    };
    g.take_dirty();

    let r = g.split_segments(&def, s, &[0.25, 0.6]).unwrap();
    assert!(g.segment(s).is_none());
    assert_eq!((r.nodes.len(), r.segments.len()), (2, 3));
    let sum: f32 = r
        .segments
        .iter()
        .map(|&x| g.segment(x).unwrap().length)
        .sum();
    assert!((sum - whole).abs() <= 0.1, "{sum} vs {whole}");
    // Chain connectivity.
    let ends: Vec<_> = r
        .segments
        .iter()
        .map(|&x| {
            let s = g.segment(x).unwrap();
            (s.start, s.end)
        })
        .collect();
    assert_eq!(ends[0].0, n0);
    assert_eq!(ends[0].1, r.nodes[0]);
    assert_eq!(ends[1], (r.nodes[0], r.nodes[1]));
    assert_eq!(ends[2], (r.nodes[1], n1));
    // Lanes rebuilt; the old ones are gone.
    assert_eq!(g.lane_count(), 3 * def.lanes.len());
    assert_eq!(g.node_count(), 4);
    // Split nodes lie on the original curve.
    let p = g.node(r.nodes[0]).unwrap().pos;
    assert!(p.distance(curve.point(0.25)) < 1e-3);
    // Dirty: both old ends and the new nodes.
    let dirty: Vec<_> = g.dirty_nodes().collect();
    for n in [n0, n1, r.nodes[0], r.nodes[1]] {
        assert!(dirty.contains(&n));
    }
}

#[test]
fn bad_splits_leave_graph_unchanged() {
    let db = content();
    let (id, def) = road(&db, "basic_2l");
    let mut g = NetGraph::new();
    let s = g
        .add_segment(
            &def,
            id,
            CubicBezier3::line(Vec3::ZERO, Vec3::new(50.0, 0.0, 0.0)),
            G,
        )
        .unwrap();
    let before = g.clone();
    for ts in [
        &[0.0][..],
        &[1.0],
        &[0.6, 0.4],
        &[0.5, 0.5],
        &[f32::NAN],
        &[0.005],
    ] {
        assert_eq!(
            g.split_segments(&def, s, ts),
            Err(NetOpError::BadSplit),
            "{ts:?}"
        );
        assert_eq!(g, before);
    }
}

#[test]
fn remove_cleans_dangling_nodes() {
    let db = content();
    let (id, def) = road(&db, "basic_2l");
    let mut g = NetGraph::new();
    let a = g
        .add_segment(
            &def,
            id,
            CubicBezier3::line(Vec3::ZERO, Vec3::new(50.0, 0.0, 0.0)),
            G,
        )
        .unwrap();
    let b = g
        .add_segment(
            &def,
            id,
            CubicBezier3::line(Vec3::new(50.0, 0.0, 0.0), Vec3::new(50.0, 0.0, 50.0)),
            G,
        )
        .unwrap();
    let mid = g.segment(a).unwrap().end;
    g.take_dirty();

    g.remove_segment(a).unwrap();
    // a's far end node is gone; the shared node survives as an End and is dirty.
    assert_eq!(g.node_count(), 2);
    assert_eq!(g.segment_count(), 1);
    assert_eq!(g.lane_count(), def.lanes.len());
    assert_eq!(g.node(mid).unwrap().kind, NodeKind::End);
    assert_eq!(g.node(mid).unwrap().segments, vec![b]);
    assert_eq!(g.dirty_nodes().collect::<Vec<_>>(), vec![mid]);

    g.remove_segment(b).unwrap();
    assert_eq!(
        (g.node_count(), g.segment_count(), g.lane_count()),
        (0, 0, 0)
    );
    assert_eq!(g.dirty_nodes().count(), 0);
    assert_eq!(g.remove_segment(b), Err(NetOpError::UnknownSegment(b)));
}

#[test]
fn outside_nodes_survive_removal() {
    let db = content();
    let (id, def) = road(&db, "basic_2l");
    let mut g = NetGraph::new();
    let edge = g.add_node(Vec3::ZERO, NodeKind::Outside, G);
    let s = g
        .add_segment(
            &def,
            id,
            CubicBezier3::line(Vec3::ZERO, Vec3::new(50.0, 0.0, 0.0)),
            G,
        )
        .unwrap();
    assert_eq!(g.segment(s).unwrap().start, edge);
    g.remove_segment(s).unwrap();
    assert_eq!(g.node_count(), 1);
    assert_eq!(g.node(edge).unwrap().kind, NodeKind::Outside);
}

#[test]
fn node_limit_is_an_error_not_a_panic() {
    let db = content();
    let (id, def) = road(&db, "basic_2l");
    let mut g = NetGraph::new();
    for i in 0..MAX_SEGMENTS_PER_NODE {
        let a = i as f32 * 0.7;
        let end = Vec3::new(40.0 * a.cos(), 0.0, 40.0 * a.sin());
        g.add_segment(&def, id, CubicBezier3::line(Vec3::ZERO, end), G)
            .unwrap();
    }
    let hub = g.find_node(Vec3::ZERO).unwrap();
    assert_eq!(g.node(hub).unwrap().segments.len(), 8);
    assert_eq!(g.node(hub).unwrap().kind, NodeKind::Junction);
    let before = g.clone();
    let r = g.add_segment(
        &def,
        id,
        CubicBezier3::line(Vec3::ZERO, Vec3::new(0.0, 0.0, -40.0)),
        G,
    );
    assert_eq!(r, Err(NetOpError::NodeFull(hub)));
    assert_eq!(g, before);
}

#[test]
fn upgrade_rebuilds_lanes_keeping_geometry() {
    let db = content();
    let (id2, def2) = road(&db, "basic_2l");
    let (id6, def6) = road(&db, "large_6l");
    let mut g = NetGraph::new();
    let curve = bend(Vec3::ZERO, Vec3::new(80.0, 0.0, 10.0));
    let s = g.add_segment(&def2, id2, curve, G).unwrap();
    let nodes = g.node_count();
    g.take_dirty();
    g.upgrade_segment(s, &def6, id6).unwrap();
    let seg = g.segment(s).unwrap();
    assert_eq!(seg.road, id6);
    assert_eq!(seg.curve, curve);
    assert_eq!(seg.lanes.len(), def6.lanes.len());
    assert_eq!(g.lane_count(), def6.lanes.len());
    assert_eq!(g.node_count(), nodes);
    assert_eq!(g.dirty_nodes().count(), 2);
}

#[test]
fn dirty_set_is_sorted_and_drained() {
    let db = content();
    let (id, def) = road(&db, "basic_2l");
    let mut g = NetGraph::new();
    for i in 0..4 {
        let z = i as f32 * 30.0;
        g.add_segment(
            &def,
            id,
            CubicBezier3::line(Vec3::new(0.0, 0.0, z), Vec3::new(50.0, 0.0, z)),
            G,
        )
        .unwrap();
    }
    let d: Vec<_> = g.dirty_nodes().collect();
    assert_eq!(d.len(), 8);
    assert!(d.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(g.take_dirty(), d);
    assert_eq!(g.dirty_nodes().count(), 0);
}

#[test]
fn graph_serde_round_trip_is_byte_stable() {
    let db = content();
    let (id, def) = road(&db, "medium_4l");
    let mut g = NetGraph::new();
    let s = g
        .add_segment(&def, id, bend(Vec3::ZERO, Vec3::new(90.0, 0.0, 5.0)), G)
        .unwrap();
    g.split_segment(&def, s, 0.5).unwrap();
    let bytes = postcard::to_stdvec(&g).unwrap();
    let back: NetGraph = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(back, g);
    assert_eq!(postcard::to_stdvec(&back).unwrap(), bytes);
}

#[test]
fn node_level_is_derived_from_its_segments_not_attachment_order() {
    let db = content();
    let (id, def) = road(&db, "basic_2l");
    let line = |a: f32, b: f32| CubicBezier3::line(Vec3::new(a, 5.0, 0.0), Vec3::new(b, 5.0, 0.0));
    for order in [[G, Elevation::Bridge], [Elevation::Bridge, G]] {
        let mut g = NetGraph::new();
        // Segment 1 ends at x = 50, segment 2 starts there.
        let s = [line(0.0, 50.0), line(50.0, 100.0)];
        for (e, c) in order.into_iter().zip(s) {
            g.add_segment(&def, id, c, e).unwrap();
        }
        let shared = g.find_node(Vec3::new(50.0, 5.0, 0.0)).unwrap();
        assert_eq!(g.node(shared).unwrap().elevation, G, "{order:?}");
    }
    // Only raised segments: elevated beats bridge.
    let mut g = NetGraph::new();
    g.add_segment(&def, id, line(0.0, 50.0), Elevation::Bridge)
        .unwrap();
    g.add_segment(&def, id, line(50.0, 100.0), Elevation::Elevated)
        .unwrap();
    let shared = g.find_node(Vec3::new(50.0, 5.0, 0.0)).unwrap();
    assert_eq!(g.node(shared).unwrap().elevation, Elevation::Elevated);
}
