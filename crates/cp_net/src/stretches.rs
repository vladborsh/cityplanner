//! Stretches, control points and ticks (`architecture/system-design.md` §6.7, v1.4c).
//!
//! A *stretch* is what the player sees as one piece of street: the chain of segments through
//! degree-2 nodes with one road type and elevation and no corner. It is finer than a
//! [`road_run`](crate::run::road_run) (a corner ends a stretch, not a run). Control points
//! (every 64 m from the stretch's anchor) and ticks (every 8 m) are derived guides for the road
//! tool and the bulldozer. Everything here only reads the graph: nothing is stored, saved or
//! scheduled, and the traffic simulation never sees these points.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use bevy_math::{Vec2, Vec3};
use cp_core::curve::to_xz;

use crate::build::{CONTROL_SPACING, ZONE_CELL};
use crate::graph::NetGraph;
use crate::model::{Elevation, NodeId, NodeKind, SegmentId};
use cp_content::RoadDefId;

/// A degree-2 node whose two tangents diverge by more than this is a corner, degrees.
pub const CORNER_DEG: f32 = 10.0;

/// A maximal corner-free chain of segments (see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct Stretch {
    /// Segments in order from the anchor.
    pub segments: Vec<SegmentId>,
    /// The end the control points are counted from.
    pub anchor: NodeId,
    /// The other end. Equal to `anchor` for a ring (or a loop closed at one boundary node).
    pub far: NodeId,
    /// Total arc length, metres.
    pub length: f32,
}

/// A place on a stretch: a control point, a tick or an end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StretchPoint {
    pub seg: SegmentId,
    /// Arc length from the **start** of `seg` (its own direction): what `RemovePart` takes.
    pub s: f32,
    pub pos: Vec3,
    /// Arc length from the stretch's anchor.
    pub along: f32,
}

/// One segment of a chain with the nodes it is walked between.
#[derive(Clone, Copy)]
struct Link {
    seg: SegmentId,
    from: NodeId,
    to: NodeId,
}

fn key(graph: &NetGraph, seg: SegmentId) -> Option<(RoadDefId, Elevation)> {
    graph.segment(seg).map(|s| (s.road, s.elevation))
}

/// Unit direction in XZ in which `seg` leaves `node`.
fn leaving(graph: &NetGraph, seg: SegmentId, node: NodeId) -> Option<Vec2> {
    let s = graph.segment(seg)?;
    let t = if s.start == node {
        s.curve.tangent(0.0)
    } else {
        -s.curve.tangent(1.0)
    };
    to_xz(t).try_normalize()
}

/// True when the two segments meeting at `node` bend by more than [`CORNER_DEG`].
fn is_corner(graph: &NetGraph, node: NodeId, a: SegmentId, b: SegmentId) -> bool {
    match (leaving(graph, a, node), leaving(graph, b, node)) {
        // Straight through means the two leaving directions are opposite.
        (Some(x), Some(y)) => x.dot(y) > -CORNER_DEG.to_radians().cos(),
        _ => false,
    }
}

/// The segment a stretch continues with when it arrives at `node` along `from`, and the node
/// at its far end; `None` at a boundary.
fn next_through(graph: &NetGraph, node: NodeId, from: SegmentId) -> Option<(SegmentId, NodeId)> {
    let n = graph.node(node)?;
    if n.kind == NodeKind::Outside || n.segments.len() != 2 {
        return None;
    }
    let next = n.segments.iter().copied().find(|s| *s != from)?;
    if key(graph, next)? != key(graph, from)? || is_corner(graph, node, from, next) {
        return None;
    }
    let s = graph.segment(next)?;
    Some((next, if s.start == node { s.end } else { s.start }))
}

fn pos(graph: &NetGraph, n: NodeId) -> Vec2 {
    graph.node(n).map_or(Vec2::ZERO, |n| to_xz(n.pos))
}

/// Lexicographic `(x, z)` order by `total_cmp`.
fn pos_cmp(a: Vec2, b: Vec2) -> Ordering {
    a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y))
}

fn reverse(links: &mut [Link]) {
    links.reverse();
    for l in links {
        std::mem::swap(&mut l.from, &mut l.to);
    }
}

/// Orients the chain so that its first node is the anchor (§6.7): the end that is the `start`
/// of its outermost segment while the other end is not; otherwise the smaller `(x, z)`.
fn orient(graph: &NetGraph, links: &mut [Link]) {
    let (first, last) = (links[0], links[links.len() - 1]);
    let starts = |l: Link, n: NodeId| graph.segment(l.seg).is_some_and(|s| s.start == n);
    let keep = match (starts(first, first.from), starts(last, last.to)) {
        (true, false) => true,
        (false, true) => false,
        _ => {
            pos_cmp(pos(graph, first.from), pos(graph, last.to))
                .then_with(|| pos_cmp(pos(graph, first.to), pos(graph, last.from)))
                != Ordering::Greater
        }
    };
    if !keep {
        reverse(links);
    }
}

/// The stretch containing `seg`, or `None` for an unknown segment.
///
/// Boundaries are a junction (3 or more segments), a dead end, an outside connection, a corner,
/// and a change of road type or elevation. A ring without any of those is one whole stretch;
/// its anchor is its node with the smallest `(x, z)`, so the result does not depend on which of
/// its segments was asked for or on arena order.
pub fn stretch(graph: &NetGraph, seg: SegmentId) -> Option<Stretch> {
    let first = graph.segment(seg)?;
    let mut seen: BTreeSet<SegmentId> = BTreeSet::from([seg]);
    let mut walk = |mut node: NodeId, mut from: SegmentId| {
        let mut out = Vec::new();
        while let Some((next, far)) = next_through(graph, node, from) {
            if !seen.insert(next) {
                break;
            }
            out.push(Link {
                seg: next,
                from: node,
                to: far,
            });
            node = far;
            from = next;
        }
        out
    };
    let mut links = walk(first.start, seg);
    reverse(&mut links);
    links.push(Link {
        seg,
        from: first.start,
        to: first.end,
    });
    links.extend(walk(first.end, seg));

    // The walk closed on itself at a node it could pass: a ring. Start at its smallest node.
    let (head, tail) = (links[0], links[links.len() - 1]);
    if head.from == tail.to && next_through(graph, head.from, tail.seg).is_some() {
        let at = (0..links.len())
            .min_by(|&i, &j| {
                pos_cmp(pos(graph, links[i].from), pos(graph, links[j].from))
                    .then(links[i].from.cmp(&links[j].from))
            })
            .unwrap_or(0);
        links.rotate_left(at);
    }
    orient(graph, &mut links);

    let length = links
        .iter()
        .filter_map(|l| graph.segment(l.seg))
        .map(|s| s.length)
        .sum();
    Some(Stretch {
        anchor: links[0].from,
        far: links[links.len() - 1].to,
        segments: links.iter().map(|l| l.seg).collect(),
        length,
    })
}

/// Each segment of the stretch with whether it runs along the walk from the anchor and the
/// arc length at which it begins.
fn layout(graph: &NetGraph, st: &Stretch) -> Vec<(SegmentId, bool, f32)> {
    let mut at = st.anchor;
    let mut offset = 0.0;
    let mut out = Vec::with_capacity(st.segments.len());
    for &id in &st.segments {
        let Some(s) = graph.segment(id) else { break };
        let forward = s.start == at;
        out.push((id, forward, offset));
        at = if forward { s.end } else { s.start };
        offset += s.length;
    }
    out
}

/// The point at arc length `along` from the anchor. A position exactly on a node belongs to the
/// segment that ends there.
fn point_at(graph: &NetGraph, lay: &[(SegmentId, bool, f32)], along: f32) -> Option<StretchPoint> {
    let &(id, forward, offset) = lay
        .iter()
        .find(|(id, _, off)| graph.segment(*id).is_some_and(|s| along <= off + s.length))
        .or(lay.last())?;
    let seg = graph.segment(id)?;
    let walked = (along - offset).clamp(0.0, seg.length);
    let s = if forward { walked } else { seg.length - walked };
    let table = seg.curve.arc_table();
    Some(StretchPoint {
        seg: id,
        s,
        pos: seg.curve.point(table.s_to_t(s)),
        along,
    })
}

/// Points every `step` metres from the anchor, strictly inside the stretch. A point closer than
/// [`ZONE_CELL`] to an end moves onto that end and is dropped there (the end is a stop anyway).
/// Nodes inside the stretch are plain cut points of a straight or curved road and do not move
/// anything.
fn marks(graph: &NetGraph, st: &Stretch, step: f32) -> Vec<StretchPoint> {
    let lay = layout(graph, st);
    let mut out = Vec::new();
    let mut k = 1.0_f32;
    while k * step < st.length {
        let along = k * step;
        k += 1.0;
        if along < ZONE_CELL || st.length - along < ZONE_CELL {
            continue;
        }
        out.extend(point_at(graph, &lay, along));
    }
    out
}

/// The control points of a stretch: 64 m, 128 m, … from the anchor (§6.7), in order.
pub fn control_points(graph: &NetGraph, st: &Stretch) -> Vec<StretchPoint> {
    marks(graph, st, CONTROL_SPACING)
}

/// Where the bulldozer stops: both ends of the stretch and its control points, in order.
/// Nodes inside the stretch are not stops.
pub fn stops(graph: &NetGraph, st: &Stretch) -> Vec<StretchPoint> {
    let lay = layout(graph, st);
    let mut out: Vec<_> = point_at(graph, &lay, 0.0).into_iter().collect();
    out.extend(control_points(graph, st));
    out.extend(point_at(graph, &lay, st.length));
    out
}

/// The pieces of the stretch between `a` and `b` metres from the anchor (`a < b`), one per
/// segment touched: `(segment, s0, s1)` with `s0 < s1` measured from the segment's own start,
/// in order from the anchor. Pieces shorter than a millimetre are left out.
pub fn span(graph: &NetGraph, st: &Stretch, a: f32, b: f32) -> Vec<(SegmentId, f32, f32)> {
    let mut out = Vec::new();
    for (id, forward, offset) in layout(graph, st) {
        let Some(len) = graph.segment(id).map(|s| s.length) else {
            continue;
        };
        let (lo, hi) = (a.max(offset), b.min(offset + len));
        if hi - lo < 1e-3 {
            continue;
        }
        let (l, h) = (lo - offset, hi - offset);
        out.push(if forward {
            (id, l, h)
        } else {
            (id, len - h, len - l)
        });
    }
    out
}

/// The position on the stretch containing `seg` (arc length from its anchor) of the point
/// `s_on_seg` metres from the start of `seg`. `None` for an unknown segment.
pub fn locate(graph: &NetGraph, seg: SegmentId, s_on_seg: f32) -> Option<f32> {
    let st = stretch(graph, seg)?;
    let (_, forward, offset) = layout(graph, &st).into_iter().find(|l| l.0 == seg)?;
    let len = graph.segment(seg)?.length;
    let s = s_on_seg.clamp(0.0, len);
    Some(offset + if forward { s } else { len - s })
}

/// The inverse of [`locate`]: the segment at `along` metres from the anchor and the distance of
/// the point from that segment's start.
pub fn unlocate(graph: &NetGraph, st: &Stretch, along: f32) -> Option<(SegmentId, f32)> {
    point_at(graph, &layout(graph, st), along.clamp(0.0, st.length)).map(|p| (p.seg, p.s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{ElevationMode, RoadPath, RoadPathRequest, SnapOptions, validate_road_path};
    use cp_content::ContentDb;
    use cp_core::curve::CubicBezier3;
    use cp_terrain::{GRID, MAX_HEIGHT, MapMeta, SPACING, Terrain, TileOwnership};

    const C: Vec2 = Vec2::new(5000.0, 5000.0);

    fn at(dx: f32, dz: f32) -> Vec2 {
        C + Vec2::new(dx, dz)
    }

    fn content() -> ContentDb {
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap_or_else(|e| panic!("{e}"))
    }

    fn flat() -> Terrain {
        let h = (100.0 / MAX_HEIGHT * 65535.0).round() as u16;
        let meta = MapMeta {
            name: "t".into(),
            sea_level: 0.0,
            start_tile: (2, 2),
            external_connections: vec![],
            entry_road: None,
            entry_rail: None,
        };
        Terrain::from_samples(meta, &vec![h; GRID * GRID], &vec![0; GRID * GRID]).unwrap()
    }

    /// Plans `path` with the default snaps and adds its segments, as `BuildRoad` does.
    fn draw(g: &mut NetGraph, path: RoadPath) -> Vec<SegmentId> {
        let _ = SPACING;
        let db = content();
        let road = db.road_id("basic_2l").unwrap();
        let req = RoadPathRequest {
            road,
            path,
            elevation: ElevationMode::Ground,
            snap: SnapOptions::default(),
        };
        let tiles = TileOwnership::m1_around((2, 2));
        let plan = validate_road_path(g, &flat(), &tiles, &db, &req).unwrap();
        assert!(plan.splits.is_empty());
        plan.segments
            .iter()
            .map(|s| {
                g.add_segment(db.road(road), road, s.curve, s.elevation)
                    .unwrap()
            })
            .collect()
    }

    fn xz(p: Vec3) -> Vec2 {
        to_xz(p)
    }

    fn node_pos(g: &NetGraph, n: NodeId) -> Vec2 {
        xz(g.node(n).unwrap().pos)
    }

    fn near(a: Vec2, b: Vec2) -> bool {
        a.distance(b) < 0.5
    }

    #[test]
    fn a_straight_200_m_road_has_control_points_at_64_128_192_two_on_cut_nodes_and_one_8_m_from_any()
     {
        let mut g = NetGraph::new();
        let segs = draw(
            &mut g,
            RoadPath::Straight {
                from: at(0.0, 0.0),
                to: at(200.0, 0.0),
            },
        );
        assert_eq!(segs.len(), 3, "auto-cut at 64 and 128 into 3 pieces");
        let st = stretch(&g, segs[1]).unwrap();
        assert_eq!(st.segments, segs);
        assert!((st.length - 200.0).abs() < 0.5);
        assert!(near(node_pos(&g, st.anchor), at(0.0, 0.0)));
        assert!(near(node_pos(&g, st.far), at(200.0, 0.0)));

        let cps = control_points(&g, &st);
        let along: Vec<f32> = cps.iter().map(|p| p.along).collect();
        assert_eq!(along, vec![64.0, 128.0, 192.0]);
        for (p, x) in cps.iter().zip([64.0, 128.0, 192.0]) {
            assert!(near(xz(p.pos), at(x, 0.0)), "{:?}", p.pos);
            // T-127: 64 and 128 are cut nodes; 192 is 8 m from the end and from every node.
            let nearest = g
                .nodes()
                .map(|(_, n)| xz(n.pos).distance(xz(p.pos)))
                .fold(f32::MAX, f32::min);
            assert!(nearest < 0.5 || nearest >= 8.0 - 0.1, "{x}: {nearest}");
            assert_eq!(nearest < 0.5, x < 150.0, "{x}");
        }
        // Stops are the two ends and the control points; the cut nodes are not stops.
        let stops = stops(&g, &st);
        let along: Vec<f32> = stops.iter().map(|p| p.along).collect();
        assert_eq!(along.len(), 5);
        assert!(along[0] == 0.0 && (along[4] - st.length).abs() < 1e-3);
    }

    #[test]
    fn drawn_from_the_other_end_the_anchor_is_on_the_other_end() {
        let mut g = NetGraph::new();
        let segs = draw(
            &mut g,
            RoadPath::Straight {
                from: at(200.0, 0.0),
                to: at(0.0, 0.0),
            },
        );
        let st = stretch(&g, segs[0]).unwrap();
        assert!(near(node_pos(&g, st.anchor), at(200.0, 0.0)));
        let cps = control_points(&g, &st);
        assert!(near(xz(cps[0].pos), at(136.0, 0.0)));
        assert!(near(xz(cps[2].pos), at(8.0, 0.0)));
    }

    #[test]
    fn a_free_path_with_a_right_angle_is_two_stretches_each_anchored_at_its_start() {
        let mut g = NetGraph::new();
        let segs = draw(
            &mut g,
            RoadPath::Free(vec![at(0.0, 0.0), at(100.0, 0.0), at(100.0, 100.0)]),
        );
        let a = stretch(&g, segs[0]).unwrap();
        let b = stretch(&g, *segs.last().unwrap()).unwrap();
        assert_ne!(a.segments, b.segments);
        assert_eq!(a.segments.len() + b.segments.len(), segs.len());
        assert!(near(node_pos(&g, a.anchor), at(0.0, 0.0)));
        assert!(near(node_pos(&g, a.far), at(100.0, 0.0)));
        assert!(near(node_pos(&g, b.anchor), at(100.0, 0.0)));
        assert!(near(node_pos(&g, b.far), at(100.0, 100.0)));
        // Each control point is counted from the stretch's own start.
        assert!(near(xz(control_points(&g, &b)[0].pos), at(100.0, 64.0)));
    }

    #[test]
    fn a_curve_is_one_stretch_its_cut_nodes_are_not_corners() {
        let mut g = NetGraph::new();
        let segs = draw(
            &mut g,
            RoadPath::Curve {
                from: at(0.0, 0.0),
                control: at(150.0, 0.0),
                to: at(150.0, 150.0),
            },
        );
        assert!(segs.len() >= 3, "cut into pieces: {}", segs.len());
        for s in &segs {
            assert_eq!(stretch(&g, *s).unwrap().segments, segs);
        }
    }

    #[test]
    fn a_t_junction_in_the_middle_splits_a_stretch_in_two() {
        let mut g = NetGraph::new();
        let segs = draw(
            &mut g,
            RoadPath::Straight {
                from: at(0.0, 0.0),
                to: at(200.0, 0.0),
            },
        );
        let db = content();
        let road = db.road_id("basic_2l").unwrap();
        let t = g.segment(segs[0]).unwrap().end;
        let tp = g.node(t).unwrap().pos;
        g.add_segment(
            db.road(road),
            road,
            CubicBezier3::line(tp, tp + Vec3::new(0.0, 0.0, 80.0)),
            Elevation::Ground,
        )
        .unwrap();
        let a = stretch(&g, segs[0]).unwrap();
        let b = stretch(&g, segs[2]).unwrap();
        assert_eq!(a.segments, vec![segs[0]]);
        assert_eq!(b.segments, vec![segs[1], segs[2]]);
        assert_eq!(b.anchor, t, "the second part has its own anchor");
        assert!(
            control_points(&g, &a).is_empty(),
            "66.7 m: 64 is on the end"
        );
        let along: Vec<f32> = control_points(&g, &b).iter().map(|p| p.along).collect();
        assert_eq!(
            along,
            vec![64.0, 128.0],
            "counted from the junction; 64 is a cut node, 128 is 8 m from the end"
        );
    }

    #[test]
    fn a_point_within_a_cell_of_an_end_moves_onto_it() {
        let mut g = NetGraph::new();
        let segs = draw(
            &mut g,
            RoadPath::Straight {
                from: at(0.0, 0.0),
                to: at(69.0, 0.0),
            },
        );
        let st = stretch(&g, segs[0]).unwrap();
        // 64 m is 5 m from the far end: it becomes the end, which is a stop already.
        assert!(control_points(&g, &st).is_empty());
        assert_eq!(stops(&g, &st).len(), 2);
    }

    #[test]
    fn locate_and_unlocate_agree_on_a_reversed_segment() {
        let mut g = NetGraph::new();
        let segs = draw(
            &mut g,
            RoadPath::Straight {
                from: at(0.0, 0.0),
                to: at(200.0, 0.0),
            },
        );
        let len = g.segment(segs[1]).unwrap().length;
        let along = locate(&g, segs[1], 10.0).unwrap();
        assert!((along - (g.segment(segs[0]).unwrap().length + 10.0)).abs() < 1e-3);
        let st = stretch(&g, segs[1]).unwrap();
        let (seg, s) = unlocate(&g, &st, along).unwrap();
        assert_eq!(seg, segs[1]);
        assert!((s - 10.0).abs() < 1e-3);
        assert!(locate(&g, segs[1], len + 50.0).unwrap() <= st.length);

        // Same road drawn backwards: the segment runs against the walk from the anchor.
        let mut r = NetGraph::new();
        let back = draw(
            &mut r,
            RoadPath::Straight {
                from: at(200.0, 0.0),
                to: at(0.0, 0.0),
            },
        );
        let st = stretch(&r, back[0]).unwrap();
        assert_eq!(st.segments, back);
        let along = locate(&r, back[0], 10.0).unwrap();
        assert!((along - 10.0).abs() < 1e-3, "forward from the anchor");
        // Walking from the anchor at x = 200 down, a point 10 m from the start of back[0].
        let p = point_at(&r, &layout(&r, &st), 70.0).unwrap();
        assert_eq!(unlocate(&r, &st, 70.0), Some((p.seg, p.s)));
    }

    /// A circle of radius 60 as 8 cubic arcs; `order` rotates which arc is added first and
    /// `ccw` the direction of the segments.
    fn ring(g: &mut NetGraph, order: usize, ccw: bool) -> Vec<SegmentId> {
        let db = content();
        let road = db.road_id("basic_2l").unwrap();
        let (r, n) = (60.0_f32, 8_usize);
        let step = std::f32::consts::TAU / n as f32;
        let h = 4.0 / 3.0 * (step / 4.0).tan() * r;
        let p = |a: f32| Vec3::new(5000.0 + r * a.cos(), 100.0, 5000.0 + r * a.sin());
        let d = |a: f32| Vec3::new(-a.sin(), 0.0, a.cos());
        (0..n)
            .map(|k| {
                let i = (k + order) % n;
                let (a0, a1) = (i as f32 * step, (i + 1) as f32 * step);
                let c = CubicBezier3::new(p(a0), p(a0) + d(a0) * h, p(a1) - d(a1) * h, p(a1));
                let c = if ccw {
                    c
                } else {
                    CubicBezier3::new(c.p3, c.p2, c.p1, c.p0)
                };
                g.add_segment(db.road(road), road, c, Elevation::Ground)
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn a_ring_is_one_deterministic_stretch_independent_of_arena_order() {
        let mut g = NetGraph::new();
        let segs = ring(&mut g, 0, true);
        let st = stretch(&g, segs[3]).unwrap();
        assert_eq!(st.segments.len(), 8);
        assert_eq!(st.anchor, st.far);
        assert_eq!(stretch(&g, segs[3]).unwrap(), st, "two calls are equal");
        for s in &segs {
            assert_eq!(stretch(&g, *s).unwrap(), st, "any segment gives the same");
        }
        // Anchor: the node with the smallest (x, z) is the one at 180 degrees.
        assert!(near(node_pos(&g, st.anchor), Vec2::new(4940.0, 5000.0)));

        for (order, ccw) in [(3, true), (5, false)] {
            let mut h = NetGraph::new();
            let other = ring(&mut h, order, ccw);
            let o = stretch(&h, other[0]).unwrap();
            assert!(near(node_pos(&h, o.anchor), node_pos(&g, st.anchor)));
            assert!((o.length - st.length).abs() < 1e-2);
            let (a, b) = (control_points(&g, &st), control_points(&h, &o));
            assert_eq!(a.len(), b.len());
            if ccw {
                for (p, q) in a.iter().zip(&b) {
                    assert!(near(xz(p.pos), xz(q.pos)), "{:?} {:?}", p.pos, q.pos);
                }
            }
        }
    }

    #[test]
    fn a_road_type_or_elevation_change_bounds_a_stretch() {
        let mut g = NetGraph::new();
        let segs = draw(
            &mut g,
            RoadPath::Straight {
                from: at(0.0, 0.0),
                to: at(200.0, 0.0),
            },
        );
        let db = content();
        let other = db.road_id("medium_4l").unwrap();
        g.upgrade_segment(segs[2], db.road(other), other).unwrap();
        assert_eq!(
            stretch(&g, segs[0]).unwrap().segments,
            vec![segs[0], segs[1]]
        );
        assert_eq!(stretch(&g, segs[2]).unwrap().segments, vec![segs[2]]);
        assert!(stretch(&g, SegmentId::new(99, 0)).is_none());
    }

    /// Arc length of each node of a street drawn from the anchor, in order.
    fn node_alongs(g: &NetGraph, st: &Stretch) -> Vec<f32> {
        let mut at = 0.0;
        let mut out = vec![0.0];
        for s in &st.segments {
            at += g.segment(*s).unwrap().length;
            out.push(at);
        }
        out
    }

    #[test]
    fn no_control_point_is_left_within_8_m_of_a_cut_node_for_any_length_of_8k_m() {
        for k in 1..=60 {
            let len = 8.0 * k as f32;
            let mut g = NetGraph::new();
            let segs = draw(
                &mut g,
                RoadPath::Straight {
                    from: at(0.0, 0.0),
                    to: at(len, 0.0),
                },
            );
            let st = stretch(&g, segs[0]).unwrap();
            assert_eq!(st.segments, segs);
            let nodes = node_alongs(&g, &st);
            let mut m = 64.0;
            while m < st.length - 1e-3 {
                let d = nodes.iter().map(|n| (n - m).abs()).fold(f32::MAX, f32::min);
                assert!(
                    d < 0.01 || d >= 8.0,
                    "{len} m: control point {m} is {d} m from a node (nodes {nodes:?})"
                );
                m += 64.0;
            }
        }
    }

    #[test]
    fn a_street_of_3_times_66_7_m_is_one_stretch_and_the_bulldozer_stops_remove_exactly_64_m() {
        let mut g = NetGraph::new();
        let segs = draw(
            &mut g,
            RoadPath::Straight {
                from: at(0.0, 0.0),
                to: at(200.0, 0.0),
            },
        );
        let st = stretch(&g, segs[0]).unwrap();
        let stops: Vec<f32> = stops(&g, &st).iter().map(|p| p.along).collect();
        assert_eq!(stops.len(), 5);
        // The first stop is the cut node at 64 m: the span from the end to it is one whole
        // segment of 64 m, so a click there removes 64 m, not a 66.7 m piece.
        let pieces = span(&g, &st, 0.0, stops[1]);
        assert_eq!(pieces.len(), 1);
        let (seg, s0, s1) = pieces[0];
        assert_eq!(seg, segs[0]);
        assert!(s0 == 0.0 && (s1 - 64.0).abs() < 0.01, "{s0}..{s1}");
        // The next stop is 64 m further, again exactly one segment.
        let pieces = span(&g, &st, stops[1], stops[2]);
        assert_eq!(pieces.len(), 1);
        assert!((pieces[0].2 - pieces[0].1 - 64.0).abs() < 0.01);
    }
}
