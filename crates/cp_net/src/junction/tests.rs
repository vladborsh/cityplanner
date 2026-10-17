use bevy_ecs::world::World;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use proptest::prelude::*;

use super::*;
use crate::model::Elevation;

const ROADS: [&str; 4] = ["basic_2l", "medium_4l", "large_6l", "highway_1w_3l"];

fn content() -> ContentDb {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
    ContentDb::load_dir(root).unwrap_or_else(|e| panic!("{e}"))
}

const C: Vec3 = Vec3::new(1000.0, 50.0, 1000.0);

/// A node at `C` with one straight segment per `(angle_deg, length, road)` leaving it.
fn star(db: &ContentDb, arms: &[(f32, f32, &str)]) -> (NetGraph, NodeId, Vec<SegmentId>) {
    let mut g = NetGraph::new();
    let mut segs = Vec::new();
    for &(deg, len, name) in arms {
        let road = db.road_id(name).unwrap();
        let def = db.road(road).clone();
        let a = deg.to_radians();
        let tip = C + Vec3::new(len * a.cos(), 0.0, len * a.sin());
        segs.push(
            g.add_segment(&def, road, CubicBezier3::line(C, tip), Elevation::Ground)
                .unwrap(),
        );
    }
    let node = g.find_node(C).unwrap();
    (g, node, segs)
}

fn rel(j: &Junction) -> Vec<Vec2> {
    let c = to_xz(j.pos);
    j.polygon.iter().map(|p| *p - c).collect()
}

fn assert_poly(j: &Junction, expect: &[(f32, f32)]) {
    let got = rel(j);
    assert_eq!(got.len(), expect.len(), "{got:?}");
    for (g, e) in got.iter().zip(expect) {
        assert!(
            g.distance(Vec2::new(e.0, e.1)) < 1e-3,
            "{got:?} vs {expect:?}"
        );
    }
}

// ---------- geometry helpers ----------

fn cross(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

/// Proper intersection of segments (shared endpoints and touching do not count).
fn segments_cross(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let (d1, d2) = (cross(b - a, c - a), cross(b - a, d - a));
    let (d3, d4) = (cross(d - c, a - c), cross(d - c, b - c));
    const EPS: f32 = 1e-3;
    ((d1 > EPS && d2 < -EPS) || (d1 < -EPS && d2 > EPS))
        && ((d3 > EPS && d4 < -EPS) || (d3 < -EPS && d4 > EPS))
}

fn is_simple(poly: &[Vec2]) -> bool {
    let n = poly.len();
    for i in 0..n {
        for j in i + 1..n {
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            if segments_cross(poly[i], poly[(i + 1) % n], poly[j], poly[(j + 1) % n]) {
                return false;
            }
        }
    }
    true
}

/// Point in polygon, boundary counts as inside.
fn contains(poly: &[Vec2], p: Vec2) -> bool {
    let n = poly.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        // On the edge?
        if cross(b - a, p - a).abs() < 1e-2 * (b - a).length().max(1.0)
            && (p - a).dot(p - b) <= 1e-3
        {
            return true;
        }
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

fn area(poly: &[Vec2]) -> f32 {
    let n = poly.len();
    (0..n)
        .map(|i| cross(poly[i], poly[(i + 1) % n]))
        .sum::<f32>()
        * 0.5
}

/// Oriented rectangle: centre line from `from` along unit `dir` for `len`, half width `h`.
fn rect(from: Vec2, dir: Vec2, len: f32, h: f32) -> [Vec2; 4] {
    let l = rot_ccw(dir) * h;
    let e = from + dir * len;
    [from - l, from + l, e + l, e - l]
}

/// Penetration depth of two convex quads by SAT (<= 0 means separated or touching).
fn penetration(a: &[Vec2; 4], b: &[Vec2; 4]) -> f32 {
    let mut least = f32::INFINITY;
    for poly in [a, b] {
        for i in 0..4 {
            let axis = rot_ccw(poly[(i + 1) % 4] - poly[i]).normalize_or_zero();
            let proj = |q: &[Vec2; 4]| {
                let v: Vec<f32> = q.iter().map(|p| p.dot(axis)).collect();
                (
                    v.iter().cloned().fold(f32::INFINITY, f32::min),
                    v.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                )
            };
            let (a0, a1) = proj(a);
            let (b0, b1) = proj(b);
            least = least.min(a1.min(b1) - a0.max(b0));
        }
    }
    least
}

fn arm_rect(j: &Junction, a: &Arm) -> [Vec2; 4] {
    let dir = Vec2::new(a.angle.cos(), a.angle.sin());
    rect(to_xz(j.pos) + dir * a.setback, dir, 40.0, a.width * 0.5)
}

fn check_junction(j: &Junction) {
    assert!(is_simple(&j.polygon), "self-intersecting: {:?}", rel(j));
    assert!(
        contains(&j.polygon, to_xz(j.pos)),
        "node outside {:?}",
        rel(j)
    );
    assert!(area(&j.polygon) > 0.0, "polygon must be counter-clockwise");
    for (i, a) in j.arms.iter().enumerate() {
        for b in &j.arms[i + 1..] {
            let depth = penetration(&arm_rect(j, a), &arm_rect(j, b));
            assert!(depth < 0.1, "arms overlap by {depth}: {a:?} {b:?}");
        }
    }
    for w in j.arms.windows(2) {
        assert!(w[0].angle <= w[1].angle, "arms not sorted by angle");
    }
}

// ---------- snapshots ----------

#[test]
fn t_junction_is_a_square_of_the_road_width() {
    let db = content();
    // basic_2l is 12 m wide: half 6. Arms at 0, 90 and 180 degrees.
    let (g, n, _) = star(
        &db,
        &[
            (0.0, 100.0, "basic_2l"),
            (90.0, 100.0, "basic_2l"),
            (180.0, 100.0, "basic_2l"),
        ],
    );
    let j = compute_junction(&g, n, 1).unwrap();
    assert_eq!(j.arms.len(), 3);
    assert!(j.arms.iter().all(|a| (a.width - 12.0).abs() < 1e-4));
    assert!(
        j.arms.iter().all(|a| (a.setback - 6.0).abs() < 1e-3),
        "{:?}",
        j.arms
    );
    assert_poly(&j, &[(6.0, -6.0), (6.0, 6.0), (-6.0, 6.0), (-6.0, -6.0)]);
    check_junction(&j);
}

#[test]
fn four_way_junction() {
    let db = content();
    let (g, n, _) = star(
        &db,
        &[
            (0.0, 100.0, "basic_2l"),
            (90.0, 100.0, "basic_2l"),
            (180.0, 100.0, "basic_2l"),
            (270.0, 100.0, "basic_2l"),
        ],
    );
    let j = compute_junction(&g, n, 1).unwrap();
    // Arms sort from -90 degrees, so the polygon starts at the bottom-left corner.
    assert_poly(&j, &[(-6.0, -6.0), (6.0, -6.0), (6.0, 6.0), (-6.0, 6.0)]);
    check_junction(&j);
}

#[test]
fn mixed_widths_push_the_narrow_road_back() {
    let db = content();
    // A 12 m street meeting a 28 m avenue at right angles: the street stops at the avenue's
    // half width (14); the avenue's setback is the street's half width (6) but at least its own 14.
    let (g, n, segs) = star(
        &db,
        &[
            (0.0, 100.0, "large_6l"),
            (90.0, 100.0, "basic_2l"),
            (180.0, 100.0, "large_6l"),
        ],
    );
    let j = compute_junction(&g, n, 1).unwrap();
    let street = j.arms.iter().find(|a| a.segment == segs[1]).unwrap();
    assert!((street.setback - 14.0).abs() < 1e-3, "{street:?}");
    for a in j.arms.iter().filter(|a| a.segment != segs[1]) {
        assert!((a.setback - 14.0).abs() < 1e-3);
    }
    check_junction(&j);
}

#[test]
fn y_junction_keeps_arms_at_least_half_width_back() {
    let db = content();
    let (g, n, _) = star(
        &db,
        &[
            (90.0, 100.0, "basic_2l"),
            (210.0, 100.0, "basic_2l"),
            (330.0, 100.0, "basic_2l"),
        ],
    );
    let j = compute_junction(&g, n, 1).unwrap();
    // t = (6 + 6 cos 120) / sin 120 = 3.46 < 6, so the half-width floor applies.
    assert!(j.arms.iter().all(|a| (a.setback - 6.0).abs() < 1e-3));
    assert_eq!(j.polygon.len(), 6);
    check_junction(&j);
    // Arms come out sorted by angle: -150, -30, 90 degrees.
    let angles: Vec<i32> = j
        .arms
        .iter()
        .map(|a| a.angle.to_degrees().round() as i32)
        .collect();
    assert_eq!(angles, vec![-150, -30, 90]);
}

#[test]
fn sharp_y_needs_a_longer_setback() {
    let db = content();
    // 30 degrees between two arms: t = (6 + 6 cos 30) / sin 30 = 22.39.
    let (g, n, _) = star(
        &db,
        &[
            (0.0, 100.0, "basic_2l"),
            (30.0, 100.0, "basic_2l"),
            (200.0, 100.0, "basic_2l"),
        ],
    );
    let j = compute_junction(&g, n, 1).unwrap();
    let s0 = j.arms.iter().find(|a| a.angle.abs() < 1e-3).unwrap();
    assert!((s0.setback - 22.392).abs() < 0.01, "{s0:?}");
    check_junction(&j);
}

#[test]
fn dead_end_is_a_rectangle_around_the_node() {
    let db = content();
    let (g, n, _) = star(&db, &[(0.0, 80.0, "basic_2l")]);
    let j = compute_junction(&g, n, 1).unwrap();
    assert_eq!(j.arms.len(), 1);
    assert!((j.arms[0].setback - 6.0).abs() < 1e-4);
    assert_poly(&j, &[(6.0, -6.0), (6.0, 6.0), (-6.0, 6.0), (-6.0, -6.0)]);
    check_junction(&j);
}

#[test]
fn bend_covers_the_outer_corner() {
    let db = content();
    let (g, n, _) = star(&db, &[(0.0, 80.0, "basic_2l"), (90.0, 80.0, "basic_2l")]);
    assert_eq!(g.node(n).unwrap().kind, crate::model::NodeKind::Middle);
    let j = compute_junction(&g, n, 1).unwrap();
    assert_poly(&j, &[(6.0, -6.0), (6.0, 6.0), (-6.0, 6.0), (-6.0, -6.0)]);
    check_junction(&j);
}

#[test]
fn straight_join_is_a_clean_patch() {
    let db = content();
    let (g, n, _) = star(&db, &[(0.0, 80.0, "medium_4l"), (180.0, 80.0, "medium_4l")]);
    let j = compute_junction(&g, n, 1).unwrap();
    // medium_4l is 19 m wide.
    assert_poly(&j, &[(9.5, -9.5), (9.5, 9.5), (-9.5, 9.5), (-9.5, -9.5)]);
    check_junction(&j);
}

#[test]
fn eight_segment_node() {
    let db = content();
    let arms: Vec<(f32, f32, &str)> = (0..8)
        .map(|i| (i as f32 * 45.0, 150.0, "basic_2l"))
        .collect();
    let (g, n, _) = star(&db, &arms);
    assert_eq!(g.node(n).unwrap().kind, crate::model::NodeKind::Junction);
    let j = compute_junction(&g, n, 1).unwrap();
    assert_eq!(j.arms.len(), 8);
    check_junction(&j);
    // Same input, same output.
    assert_eq!(compute_junction(&g, n, 1).unwrap(), j);
}

#[test]
fn trimmed_curves_meet_the_polygon_and_do_not_overlap() {
    let db = content();
    // A segment between two T-junctions, 60 m long.
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let mut g = NetGraph::new();
    let a = Vec3::new(0.0, 0.0, 0.0);
    let b = Vec3::new(60.0, 0.0, 0.0);
    let mid = g
        .add_segment(&def, road, CubicBezier3::line(a, b), Elevation::Ground)
        .unwrap();
    for end in [a, b] {
        for dz in [-80.0, 80.0] {
            g.add_segment(
                &def,
                road,
                CubicBezier3::line(end, end + Vec3::new(0.0, 0.0, dz)),
                Elevation::Ground,
            )
            .unwrap();
        }
    }
    let mut world = World::new();
    world.insert_resource(g);
    world.init_resource::<JunctionGeometry>();
    world.run_system_cached(update_junctions).unwrap();
    let geo = world.resource::<JunctionGeometry>();
    let g = world.resource::<NetGraph>();
    let tc = geo.trimmed_curve(g, mid).unwrap();
    // 6 m cut off each end of the 60 m segment.
    assert!(
        (tc.p0.x - 6.0).abs() < 0.05 && (tc.p3.x - 54.0).abs() < 0.05,
        "{tc:?}"
    );
    assert!((tc.arc_table().length() - 48.0).abs() < 0.2);
    // Trimmed length plus both setbacks is the whole segment: no overlap, no gap.
    let seg = g.segment(mid).unwrap();
    let total = tc.arc_table().length()
        + geo
            .get(seg.start)
            .unwrap()
            .arms
            .iter()
            .find(|x| x.segment == mid)
            .unwrap()
            .setback
        + geo
            .get(seg.end)
            .unwrap()
            .arms
            .iter()
            .find(|x| x.segment == mid)
            .unwrap()
            .setback;
    assert!((total - seg.length).abs() < 0.2);
}

#[test]
fn short_segments_are_capped_so_ends_never_overlap() {
    let db = content();
    // 8 m long segment between two wide junctions: each end may use at most 45 %.
    let road = db.road_id("large_6l").unwrap();
    let def = db.road(road).clone();
    let mut g = NetGraph::new();
    let a = Vec3::ZERO;
    let b = Vec3::new(8.0, 0.0, 0.0);
    let mid = g
        .add_segment(&def, road, CubicBezier3::line(a, b), Elevation::Ground)
        .unwrap();
    for end in [a, b] {
        for dz in [-80.0, 80.0] {
            g.add_segment(
                &def,
                road,
                CubicBezier3::line(end, end + Vec3::new(0.0, 0.0, dz)),
                Elevation::Ground,
            )
            .unwrap();
        }
    }
    let seg = g.segment(mid).unwrap().clone();
    for node in [seg.start, seg.end] {
        let j = compute_junction(&g, node, 1).unwrap();
        let arm = j.arms.iter().find(|x| x.segment == mid).unwrap();
        assert!(arm.setback <= 8.0 * MAX_SETBACK_FRACTION + 1e-4, "{arm:?}");
    }
}

// ---------- system behaviour ----------

#[test]
fn system_recomputes_dirty_nodes_and_drops_removed_ones() {
    let db = content();
    let (g, n, segs) = star(
        &db,
        &[
            (0.0, 100.0, "basic_2l"),
            (90.0, 100.0, "basic_2l"),
            (180.0, 100.0, "basic_2l"),
        ],
    );
    let mut world = World::new();
    world.insert_resource(g);
    world.init_resource::<JunctionGeometry>();

    world.run_system_cached(update_junctions).unwrap();
    let geo = world.resource::<JunctionGeometry>().clone();
    assert_eq!(geo.len(), 4); // the junction and three far ends
    assert_eq!(geo.version(), 1);
    assert_eq!(geo.get(n).unwrap().arms.len(), 3);
    assert_eq!(world.resource::<NetGraph>().dirty_nodes().count(), 0);

    // Nothing dirty, nothing to do: version stays.
    world.run_system_cached(update_junctions).unwrap();
    assert_eq!(world.resource::<JunctionGeometry>().version(), 1);

    // Removing a segment dirties both its nodes: the junction shrinks to a bend and the
    // far end node disappears.
    world
        .resource_mut::<NetGraph>()
        .remove_segment(segs[1])
        .unwrap();
    world.run_system_cached(update_junctions).unwrap();
    let geo = world.resource::<JunctionGeometry>();
    assert_eq!(geo.version(), 2);
    assert_eq!(geo.len(), 3);
    let j = geo.get(n).unwrap();
    assert_eq!(j.arms.len(), 2);
    assert_eq!(j.revision, 2);
    assert_poly(j, &[(6.0, -6.0), (6.0, 6.0), (-6.0, 6.0), (-6.0, -6.0)]);
    // Untouched junctions keep their old revision.
    assert!(geo.iter().any(|(_, x)| x.revision == 1));
    check_junction(j);
}

#[test]
fn geometry_is_rebuilt_when_it_is_missing_after_a_load() {
    let db = content();
    let (g, _, _) = star(&db, &[(0.0, 100.0, "basic_2l"), (90.0, 100.0, "basic_2l")]);
    let mut g = g;
    g.take_dirty(); // as saved after the Network phase ran
    let mut world = World::new();
    world.insert_resource(g);
    world.init_resource::<JunctionGeometry>();
    world.run_system_cached(update_junctions).unwrap();
    assert_eq!(world.resource::<JunctionGeometry>().len(), 3);
}

// ---------- property test ----------

/// 2–8 arms, every angular gap >= 30 degrees, long segments (no setback cap applies),
/// any mix of the four road types.
fn arms_strategy() -> impl Strategy<Value = Vec<(f32, f32, usize)>> {
    (2usize..=8, 0.0f32..360.0)
        .prop_flat_map(|(k, base)| {
            (
                Just(k),
                Just(base),
                proptest::collection::vec(0.0f32..1.0, k),
                proptest::collection::vec(140.0f32..200.0, k),
                proptest::collection::vec(0usize..4, k),
            )
        })
        .prop_map(|(k, base, weights, lens, roads)| {
            let spare = 360.0 - 30.5 * k as f32;
            let total: f32 = weights.iter().sum::<f32>().max(1e-3);
            let mut angle = base;
            let mut out = Vec::new();
            for i in 0..k {
                out.push((angle, lens[i], roads[i]));
                angle += 30.5 + spare * weights[i] / total;
            }
            out
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn polygon_is_simple_contains_the_node_and_arms_do_not_overlap(arms in arms_strategy()) {
        let db = content();
        let spec: Vec<(f32, f32, &str)> = arms.iter().map(|&(a, l, r)| (a, l, ROADS[r])).collect();
        let (g, n, _) = star(&db, &spec);
        let j = compute_junction(&g, n, 1).unwrap();
        prop_assert_eq!(j.arms.len(), arms.len());
        prop_assert!(is_simple(&j.polygon), "self-intersecting: {:?}", rel(&j));
        prop_assert!(contains(&j.polygon, to_xz(j.pos)), "node outside {:?}", rel(&j));
        prop_assert!(area(&j.polygon) > 0.0);
        for (i, a) in j.arms.iter().enumerate() {
            prop_assert!(a.setback >= a.width * 0.5 - 1e-3 && a.setback <= MAX_SETBACK + 1e-3);
            for b in &j.arms[i + 1..] {
                let depth = penetration(&arm_rect(&j, a), &arm_rect(&j, b));
                prop_assert!(depth < 0.1, "arms overlap by {}: {:?} {:?}", depth, a, b);
            }
        }
    }
}

#[test]
fn refresh_geometry_fills_dirty_nodes_and_leaves_the_dirty_set_alone() {
    let db = content();
    let (g, node, _) = star(&db, &[(0.0, 80.0, "basic_2l"), (90.0, 80.0, "basic_2l")]);
    let dirty: Vec<NodeId> = g.dirty_nodes().collect();
    assert!(!dirty.is_empty());
    let mut geo = JunctionGeometry::default();
    assert!(refresh_geometry(&g, &mut geo));
    assert_eq!(geo.len(), g.node_count());
    assert_eq!(
        geo.get(node).unwrap().polygon,
        compute_junction(&g, node, 1).unwrap().polygon
    );
    // The sim's own state is untouched: the same nodes are still dirty for the next tick.
    assert_eq!(g.dirty_nodes().collect::<Vec<_>>(), dirty);
}

/// BUG-029: a reflex gap of 180-210 degrees next to long setbacks (the other gaps are narrow)
/// made the chord between two arms cut the node out of the polygon. Found by a sweep of 400k
/// random stars, 10 failed; these are two of them.
#[test]
fn a_reflex_gap_under_210_degrees_with_long_setbacks_keeps_the_node_inside() {
    let db = content();
    for spec in [
        vec![
            (290.01697, 183.18942, "highway_1w_3l"),
            (496.66174, 190.68333, "highway_1w_3l"),
            (550.24506, 164.69635, "large_6l"),
            (615.655, 193.17348, "large_6l"),
        ],
        vec![
            (127.46087, 196.46645, "highway_1w_3l"),
            (336.17078, 171.64767, "basic_2l"),
            (404.20416, 140.34453, "large_6l"),
            (456.62292, 197.48096, "large_6l"),
        ],
    ] {
        let (g, n, _) = star(&db, &spec);
        check_junction(&compute_junction(&g, n, 1).unwrap());
    }
}

// ---------- footpaths (T-222) ----------

#[test]
fn a_footpath_beside_a_street_makes_no_junction_polygon_and_stops_at_the_road_edge() {
    let db = content();
    let (g0, n0, _) = star(&db, &[(0.0, 80.0, "basic_2l"), (180.0, 80.0, "basic_2l")]);
    let plain = compute_junction(&g0, n0, 0).unwrap();
    let (g, n, segs) = star(
        &db,
        &[
            (0.0, 80.0, "basic_2l"),
            (180.0, 80.0, "basic_2l"),
            (90.0, 40.0, "park_path"),
        ],
    );
    let j = compute_junction(&g, n, 0).unwrap();
    // The polygon is the street's alone: no corner points for the path.
    assert_eq!(rel(&j), rel(&plain));
    assert_eq!(j.arms.len(), 3);
    let path = j.arms.iter().find(|a| a.segment == segs[2]).unwrap();
    let half = db.road(db.road_id("basic_2l").unwrap()).width * 0.5;
    assert!((path.setback - half).abs() < 1e-3, "{}", path.setback);
    // The street's arms keep their setbacks.
    for a in j.arms.iter().filter(|a| a.segment != segs[2]) {
        let b = plain.arms.iter().find(|b| b.segment == a.segment).unwrap();
        assert_eq!(a.setback, b.setback);
    }
    // Its sidewalks still join (directly, and through the path's lanes).
    assert!(j.links.pedestrian.len() > plain.links.pedestrian.len());
    // A footpath on its own keeps the ordinary junction.
    let (g, n, _) = star(&db, &[(0.0, 40.0, "park_path"), (90.0, 40.0, "park_path")]);
    assert!(compute_junction(&g, n, 0).unwrap().polygon.len() >= 4);
}
