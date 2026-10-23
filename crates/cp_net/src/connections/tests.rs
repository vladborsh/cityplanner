use cp_content::ContentDb;
use cp_core::curve::to_xz;

use super::*;
use crate::graph::NetGraph;
use crate::junction::{Junction, compute_junction};
use crate::model::{Elevation, NodeId, SegmentId};

fn content() -> ContentDb {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
    ContentDb::load_dir(root).unwrap_or_else(|e| panic!("{e}"))
}

const C: Vec3 = Vec3::new(1000.0, 50.0, 1000.0);

/// Node at `C` with one straight segment per `(angle_deg, road)` leaving it (all start at C).
fn star(db: &ContentDb, arms: &[(f32, &str)]) -> (NetGraph, NodeId, Vec<SegmentId>) {
    let mut g = NetGraph::new();
    let mut segs = Vec::new();
    for &(deg, name) in arms {
        let road = db.road_id(name).unwrap();
        let def = db.road(road).clone();
        let a = deg.to_radians();
        let tip = C + Vec3::new(150.0 * a.cos(), 0.0, 150.0 * a.sin());
        segs.push(
            g.add_segment(&def, road, CubicBezier3::line(C, tip), Elevation::Ground)
                .unwrap(),
        );
    }
    let node = g.find_node(C).unwrap();
    (g, node, segs)
}

fn junction(g: &NetGraph, n: NodeId) -> Junction {
    compute_junction(g, n, 1).unwrap()
}

fn arm_of(g: &NetGraph, j: &Junction, lane: LaneId) -> usize {
    let seg = g.lane(lane).unwrap().segment;
    j.arms.iter().position(|a| a.segment == seg).unwrap()
}

/// Structural checks every junction must satisfy.
fn check(g: &NetGraph, j: &Junction) {
    let l = &j.links;
    for (i, c) in l.connections.iter().enumerate() {
        let (from, to) = (g.lane(c.from).unwrap(), g.lane(c.to).unwrap());
        assert!(from.kind.is_road_vehicle());
        assert!(to.kind.is_road_vehicle());
        // T-302: a connection joins compatible kinds only.
        assert!(
            exit_kinds(from.kind).contains(&to.kind),
            "{:?} -> {:?}",
            from.kind,
            to.kind
        );
        // From runs towards the node, to away from it.
        let fa = &j.arms[arm_of(g, j, c.from)];
        let ta = &j.arms[arm_of(g, j, c.to)];
        assert!(
            (from.dir == LaneDir::Forward) != fa.at_start,
            "from is inbound"
        );
        assert!(
            (to.dir == LaneDir::Forward) == ta.at_start,
            "to is outbound"
        );
        // Endpoints sit on the trimmed lane ends, tangent-continuous.
        let (p0, t0) = lane_at(from, fa.trim_t);
        let (p1, t1) = lane_at(to, ta.trim_t);
        assert!(
            c.curve.p0.distance(p0) < 1e-3 && c.curve.p3.distance(p1) < 1e-3,
            "{i}"
        );
        assert!(c.curve.tangent(0.0).dot(t0) > 0.999, "start tangent {i}");
        assert!(c.curve.tangent(1.0).dot(t1) > 0.999, "end tangent {i}");
        // Lanes running on through a `Middle` node touch (T-263): the connection is a point.
        assert!(c.length > 0.0 || j.class == crate::NodeClass::Middle);
    }
    // Conflicts: sorted, a < b, symmetric through in_conflict, never within one source lane.
    assert!(l.conflicts.windows(2).all(|w| w[0] < w[1]));
    for &(a, b) in &l.conflicts {
        assert!(a < b);
        assert!(l.in_conflict(a, b) && l.in_conflict(b, a));
        assert_ne!(
            l.connections[a as usize].from,
            l.connections[b as usize].from
        );
    }
    assert!(!l.in_conflict(0, 0));
    // Every pair is either in the list or not, regardless of argument order.
    for a in 0..l.connections.len() as u16 {
        for b in 0..l.connections.len() as u16 {
            assert_eq!(l.in_conflict(a, b), l.in_conflict(b, a));
        }
    }
    // Determinism.
    assert_eq!(compute_links(g, &j.arms), j.links);
}

/// Every arm with inbound lanes reaches every other arm that has outbound lanes.
fn check_reachable(g: &NetGraph, j: &Junction) {
    for (i, ai) in j.arms.iter().enumerate() {
        let inbound = arm_lanes(g, ai).inbound.len();
        if inbound == 0 || j.arms.len() < 2 {
            continue;
        }
        for (k, ak) in j.arms.iter().enumerate() {
            if k == i || arm_lanes(g, ak).outbound.is_empty() {
                continue;
            }
            assert!(
                j.links
                    .connections
                    .iter()
                    .any(|c| arm_of(g, j, c.from) == i && arm_of(g, j, c.to) == k),
                "arm {i} cannot reach arm {k}"
            );
        }
    }
    // No dead lane: every inbound lane starts at least one connection.
    for ai in &j.arms {
        for l in arm_lanes(g, ai).inbound {
            if j.arms.len() >= 2 {
                assert!(
                    j.links.connections.iter().any(|c| c.from == l.id),
                    "inbound lane {:?} has no connection",
                    l.id
                );
            }
        }
    }
}

fn turns_from(g: &NetGraph, j: &Junction, lane: LaneId) -> Vec<Turn> {
    let _ = g;
    j.links
        .connections
        .iter()
        .filter(|c| c.from == lane)
        .map(|c| c.turn)
        .collect()
}

#[test]
fn t_junction_single_lane_roads() {
    let db = content();
    // West (180), east (0) and south (90, +z).
    let (g, n, _) = star(
        &db,
        &[(0.0, "basic_2l"), (90.0, "basic_2l"), (180.0, "basic_2l")],
    );
    let j = junction(&g, n);
    check(&g, &j);
    check_reachable(&g, &j);
    assert_eq!(j.links.connections.len(), 6);
    // Arms sorted: east (0), south (90), west (180).
    let by_arm = |i: usize| -> Vec<Turn> {
        let mut t: Vec<Turn> = j
            .links
            .connections
            .iter()
            .filter(|c| arm_of(&g, &j, c.from) == i)
            .map(|c| c.turn)
            .collect();
        t.sort_by_key(|t| *t as u8);
        t
    };
    // From the east heading west: straight to the west arm, left to the south arm (+z is
    // that driver's left).
    assert_eq!(by_arm(0), vec![Turn::Straight, Turn::Left]);
    // From the south stem heading north (-z): right is east, left is west.
    assert_eq!(by_arm(1), vec![Turn::Right, Turn::Left]);
    // From the west heading east: straight and a right turn to the south.
    assert_eq!(by_arm(2), vec![Turn::Right, Turn::Straight]);
}

#[test]
fn four_way_single_lane_roads() {
    let db = content();
    let (g, n, _) = star(
        &db,
        &[
            (0.0, "basic_2l"),
            (90.0, "basic_2l"),
            (180.0, "basic_2l"),
            (270.0, "basic_2l"),
        ],
    );
    let j = junction(&g, n);
    check(&g, &j);
    check_reachable(&g, &j);
    // One inbound lane per arm taking right, straight and left.
    assert_eq!(j.links.connections.len(), 12);
    for c in j.links.connections.chunks(3) {
        let mut t: Vec<u8> = c.iter().map(|c| c.turn as u8).collect();
        t.sort();
        assert_eq!(
            t,
            vec![Turn::Right as u8, Turn::Straight as u8, Turn::Left as u8]
        );
    }
    // Left turn conflicts with the oncoming straight and the cross traffic.
    assert!(!j.links.conflicts.is_empty());
    // Pedestrians: a corner between each pair of neighbouring arms and a crossing per arm.
    let corners = j
        .links
        .pedestrian
        .iter()
        .filter(|p| p.kind == PedLinkKind::Corner)
        .count();
    let crossings = j
        .links
        .pedestrian
        .iter()
        .filter(|p| p.kind == PedLinkKind::Crossing)
        .count();
    assert_eq!((corners, crossings), (4, 4));
}

#[test]
fn six_lane_four_way_follows_the_lane_rules() {
    let db = content();
    let (g, n, _) = star(
        &db,
        &[
            (0.0, "large_6l"),
            (90.0, "large_6l"),
            (180.0, "large_6l"),
            (270.0, "large_6l"),
        ],
    );
    let j = junction(&g, n);
    check(&g, &j);
    check_reachable(&g, &j);
    // Per arm: rightmost lane right+straight, middle straight, leftmost left+straight.
    assert_eq!(j.links.connections.len(), 4 * 5);
    for i in 0..4 {
        let lanes = arm_lanes(&g, &j.arms[i]).inbound; // right to left
        assert_eq!(lanes.len(), 3);
        let mut right = turns_from(&g, &j, lanes[0].id);
        right.sort_by_key(|t| *t as u8);
        assert_eq!(right, vec![Turn::Right, Turn::Straight]);
        assert_eq!(turns_from(&g, &j, lanes[1].id), vec![Turn::Straight]);
        let mut left = turns_from(&g, &j, lanes[2].id);
        left.sort_by_key(|t| *t as u8);
        assert_eq!(left, vec![Turn::Straight, Turn::Left]);
        // Straight keeps the lane rank (3 lanes into 3): rightmost to rightmost.
        let straight_to: Vec<LaneId> = lanes
            .iter()
            .map(|l| {
                j.links
                    .connections
                    .iter()
                    .find(|c| c.from == l.id && c.turn == Turn::Straight)
                    .unwrap()
                    .to
            })
            .collect();
        let out = arm_lanes(&g, &j.arms[(i + 2) % 4]).outbound;
        assert_eq!(straight_to, out.iter().map(|l| l.id).collect::<Vec<_>>());
    }
    // Parallel straights never conflict; a right turn merging nowhere new; left crosses straights.
    let l = &j.links;
    let straights: Vec<u16> = (0..l.connections.len() as u16)
        .filter(|&i| l.connections[i as usize].turn == Turn::Straight)
        .collect();
    assert!(!l.conflicts.is_empty());
    for &a in &straights {
        for &b in &straights {
            let (ca, cb) = (&l.connections[a as usize], &l.connections[b as usize]);
            let same_arm = arm_of(&g, &j, ca.from) == arm_of(&g, &j, cb.from);
            if same_arm && a != b {
                assert!(!l.in_conflict(a, b), "parallel straights conflict");
            }
        }
    }
}

#[test]
fn t_stem_with_three_lanes_has_no_dead_middle_lane() {
    let db = content();
    // A 6-lane stem into a 6-lane through road: stem has no straight exit.
    let (g, n, _) = star(
        &db,
        &[(0.0, "large_6l"), (90.0, "large_6l"), (180.0, "large_6l")],
    );
    let j = junction(&g, n);
    check(&g, &j);
    check_reachable(&g, &j);
    let lanes = arm_lanes(&g, &j.arms[1]).inbound;
    assert_eq!(lanes.len(), 3);
    for l in &lanes {
        assert!(!turns_from(&g, &j, l.id).is_empty());
    }
    // The middle lane borrowed the right lane's turn (ties go right).
    assert_eq!(
        turns_from(&g, &j, lanes[1].id),
        turns_from(&g, &j, lanes[0].id)
    );
}

#[test]
fn merging_into_a_narrower_road_is_a_conflict() {
    let db = content();
    // 6-lane road going straight into a 2-lane road: all three lanes land on its one lane.
    let (g, n, _) = star(&db, &[(0.0, "basic_2l"), (180.0, "large_6l")]);
    let j = junction(&g, n);
    check(&g, &j);
    let west = j
        .arms
        .iter()
        .position(|a| (a.angle.abs() - std::f32::consts::PI).abs() < 1e-3)
        .unwrap();
    let from_west: Vec<usize> = (0..j.links.connections.len())
        .filter(|&i| arm_of(&g, &j, j.links.connections[i].from) == west)
        .collect();
    assert_eq!(from_west.len(), 3);
    let to = j.links.connections[from_west[0]].to;
    assert!(from_west.iter().all(|&i| j.links.connections[i].to == to));
    for (x, &a) in from_west.iter().enumerate() {
        for &b in &from_west[x + 1..] {
            assert!(j.links.in_conflict(a as u16, b as u16));
        }
    }
}

#[test]
fn dead_end_gives_a_uturn() {
    let db = content();
    for (road, lanes) in [("basic_2l", 1), ("large_6l", 3)] {
        let (g, n, _) = star(&db, &[(0.0, road)]);
        let j = junction(&g, n);
        check(&g, &j);
        assert_eq!(j.links.connections.len(), lanes, "{road}");
        assert!(j.links.connections.iter().all(|c| c.turn == Turn::UTurn));
        // Concentric U-turns: no conflicts, one outbound lane each.
        assert!(j.links.conflicts.is_empty(), "{road}");
        let mut to: Vec<_> = j.links.connections.iter().map(|c| c.to).collect();
        to.sort();
        to.dedup();
        assert_eq!(to.len(), lanes);
        // Pedestrians: one corner around the cap and one crossing.
        let kinds: Vec<_> = j.links.pedestrian.iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            vec![PedLinkKind::Corner, PedLinkKind::Crossing],
            "{road}"
        );
    }
}

#[test]
fn one_way_highway_connects_inbound_to_outbound_only() {
    let db = content();
    let road = db.road_id("highway_1w_3l").unwrap();
    let def = db.road(road).clone();
    let mut g = NetGraph::new();
    let west = C - Vec3::new(150.0, 0.0, 0.0);
    let east = C + Vec3::new(150.0, 0.0, 0.0);
    // One segment arrives at C, the next leaves it.
    g.add_segment(&def, road, CubicBezier3::line(west, C), Elevation::Ground)
        .unwrap();
    g.add_segment(&def, road, CubicBezier3::line(C, east), Elevation::Ground)
        .unwrap();
    let n = g.find_node(C).unwrap();
    let j = junction(&g, n);
    check(&g, &j);
    assert_eq!(j.links.connections.len(), 3);
    assert!(j.links.connections.iter().all(|c| c.turn == Turn::Straight));
    // Lane rank is preserved and nothing merges.
    let mut to: Vec<_> = j.links.connections.iter().map(|c| c.to).collect();
    to.sort();
    to.dedup();
    assert_eq!(to.len(), 3);
    assert!(j.links.conflicts.is_empty());
    // No sidewalks on the highway: no pedestrian links.
    assert!(j.links.pedestrian.is_empty());
    // Two arms that are both outbound only have nothing to connect.
    let (g2, n2, _) = star(&db, &[(0.0, "highway_1w_3l"), (180.0, "highway_1w_3l")]);
    assert!(junction(&g2, n2).links.connections.is_empty());
}

#[test]
fn eight_arms_stay_consistent() {
    let db = content();
    let arms: Vec<(f32, &str)> = (0..8).map(|i| (i as f32 * 45.0, "medium_4l")).collect();
    let (g, n, _) = star(&db, &arms);
    let j = junction(&g, n);
    check(&g, &j);
    check_reachable(&g, &j);
}

#[test]
fn pedestrian_corner_ends_are_on_neighbouring_arms() {
    let db = content();
    let (g, n, _) = star(
        &db,
        &[
            (0.0, "basic_2l"),
            (90.0, "basic_2l"),
            (180.0, "basic_2l"),
            (270.0, "basic_2l"),
        ],
    );
    let j = junction(&g, n);
    let center = to_xz(j.pos);
    for p in j
        .links
        .pedestrian
        .iter()
        .filter(|p| p.kind == PedLinkKind::Corner)
    {
        let (ia, ib) = (arm_of(&g, &j, p.a), arm_of(&g, &j, p.b));
        assert_eq!((ia + 1) % 4, ib, "corner joins ccw-adjacent arms");
        // Corner stays near the junction polygon (within the road half-width plus margin).
        for k in 0..=4 {
            let q = to_xz(p.curve.point(k as f32 / 4.0));
            assert!(q.distance(center) < 12.0, "{q}");
        }
    }
}

/// BUG-105: a turn's connection is one smooth bend, the way a car drives it: it leaves and
/// meets its lanes along them, bends one way only, and nowhere much sharper than on average
/// (no corner). On the X junctions of every two-way town road.
#[test]
fn turn_connections_are_smooth_bends() {
    let db = content();
    for road in ["basic_2l", "medium_4l", "large_6l"] {
        let arms: Vec<(f32, &str)> = [0.0, 90.0, 180.0, 270.0].map(|a| (a, road)).into();
        let (g, n, _) = star(&db, &arms);
        let j = junction(&g, n);
        let mut turns = 0;
        for c in &j.links.connections {
            if !matches!(c.turn, Turn::Left | Turn::Right) {
                continue;
            }
            turns += 1;
            const N: usize = 48;
            let pts: Vec<Vec2> = (0..=N)
                .map(|i| to_xz(c.curve.point(i as f32 / N as f32)))
                .collect();
            // Heading change per metre between successive chords.
            let bends: Vec<(f32, f32)> = pts
                .windows(3)
                .map(|w| {
                    let (a, b) = (w[1] - w[0], w[2] - w[1]);
                    (
                        a.perp_dot(b).atan2(a.dot(b)),
                        (a.length() + b.length()) * 0.5,
                    )
                })
                .collect();
            let total: f32 = bends.iter().map(|(d, _)| d).sum();
            assert!(
                (1.3..1.85).contains(&total.abs()),
                "{road}: a quarter turn, got {total}"
            );
            assert!(
                bends
                    .iter()
                    .all(|(d, _)| d.signum() == total.signum() || d.abs() < 1e-4),
                "{road}: bends one way"
            );
            let mean = total.abs() / c.length;
            let sharpest = bends
                .iter()
                .map(|(d, len)| d.abs() / len.max(1e-4))
                .fold(0.0, f32::max);
            assert!(
                sharpest < 2.0 * mean,
                "{road}: a corner: {sharpest} rad/m against {mean} on average"
            );
        }
        assert!(turns >= 8, "{road}: {turns} turns");
    }
}

/// The arm indices of `j` whose segment is road `name` (T-302 tests; arms are sorted by angle,
/// not in the order `star` got them).
fn arms_of_road(db: &ContentDb, g: &NetGraph, j: &Junction, name: &str) -> Vec<usize> {
    let road = db.road_id(name).unwrap();
    j.arms
        .iter()
        .enumerate()
        .filter(|(_, a)| g.segment(a.segment).is_some_and(|s| s.road == road))
        .map(|(i, _)| i)
        .collect()
}

/// T-302: lanes connect by kind. On a T of a tram collector (through) and a local street: the
/// tram lanes run straight on to the other tram lanes only, the cars of the collector turn into
/// the street and go straight, the street's cars never enter a tram lane, and a tram's straight
/// crossing conflicts with a car's left turn.
#[test]
fn tram_lanes_connect_to_tram_lanes_only_and_cross_the_cars() {
    let db = content();
    let (g, n, _) = star(
        &db,
        &[(0.0, "tram_4l"), (180.0, "tram_4l"), (90.0, "basic_2l")],
    );
    let j = junction(&g, n);
    check(&g, &j);
    let kind = |id: LaneId| g.lane(id).unwrap().kind;
    let conns = &j.links.connections;
    let tram: Vec<_> = conns
        .iter()
        .filter(|c| kind(c.from) == LaneKind::Tram)
        .collect();
    assert_eq!(tram.len(), 2, "one straight connection per tram lane");
    assert!(
        tram.iter()
            .all(|c| kind(c.to) == LaneKind::Tram && c.turn == Turn::Straight)
    );
    assert!(
        conns
            .iter()
            .all(|c| (kind(c.to) == LaneKind::Tram) == (kind(c.from) == LaneKind::Tram)),
        "no car enters a tram lane and no tram leaves one"
    );
    // The street's cars reach both collector directions (right and left turns).
    let street = arms_of_road(&db, &g, &j, "basic_2l")[0];
    let street_turns: Vec<Turn> = conns
        .iter()
        .filter(|c| arm_of(&g, &j, c.from) == street)
        .map(|c| c.turn)
        .collect();
    assert!(street_turns.contains(&Turn::Right) && street_turns.contains(&Turn::Left));
    // A tram going straight conflicts with the street's left turn across it.
    let (ti, _) = conns
        .iter()
        .enumerate()
        .find(|(_, c)| kind(c.from) == LaneKind::Tram)
        .unwrap();
    let left = conns
        .iter()
        .enumerate()
        .filter(|(_, c)| arm_of(&g, &j, c.from) == street && c.turn == Turn::Left)
        .map(|(i, _)| i as u16);
    let mut crossed = false;
    for li in left {
        crossed |= j.links.in_conflict(ti as u16, li);
    }
    let (ti2, _) = conns
        .iter()
        .enumerate()
        .filter(|(_, c)| kind(c.from) == LaneKind::Tram)
        .nth(1)
        .unwrap();
    let left2: Vec<u16> = conns
        .iter()
        .enumerate()
        .filter(|(_, c)| arm_of(&g, &j, c.from) == street && c.turn == Turn::Left)
        .map(|(i, _)| i as u16)
        .collect();
    crossed |= left2.iter().any(|&li| j.links.in_conflict(ti2 as u16, li));
    assert!(
        crossed,
        "a tram crossing conflicts with a left turn over it"
    );
}

/// T-302: a bus lane joins the general traffic where it ends. On a T of a bus collector
/// (through) and a local street: bus lanes go on to bus lanes straight ahead and to car lanes
/// of the street; no car lane connects into a bus lane.
#[test]
fn bus_lanes_go_on_to_bus_lanes_or_cars_but_cars_never_enter_a_bus_lane() {
    let db = content();
    let (g, n, _) = star(
        &db,
        &[
            (0.0, "transitway_4l"),
            (180.0, "transitway_4l"),
            (90.0, "basic_2l"),
        ],
    );
    let j = junction(&g, n);
    check(&g, &j);
    let kind = |id: LaneId| g.lane(id).unwrap().kind;
    let conns = &j.links.connections;
    let from_bus: Vec<_> = conns
        .iter()
        .filter(|c| kind(c.from) == LaneKind::Bus)
        .collect();
    assert!(!from_bus.is_empty());
    assert!(
        from_bus
            .iter()
            .any(|c| kind(c.to) == LaneKind::Bus && c.turn == Turn::Straight),
        "bus lane straight on to the next bus lane"
    );
    let street = arms_of_road(&db, &g, &j, "basic_2l")[0];
    assert!(
        from_bus
            .iter()
            .any(|c| kind(c.to) == LaneKind::Car && arm_of(&g, &j, c.to) == street),
        "bus lane into the street's car lane"
    );
    assert!(
        conns
            .iter()
            .all(|c| !(kind(c.from) == LaneKind::Car && kind(c.to) == LaneKind::Bus)),
        "no car lane into a bus lane where the arm has car lanes"
    );
}

/// T-302: into a bus-only street the car lanes do connect (the bus riding the car lane needs
/// the way in; the route search keeps the cars out), and out of it the bus lanes join the
/// street's car lanes.
#[test]
fn a_bus_only_street_is_entered_from_car_lanes() {
    let db = content();
    // A T, not a straight join: a two-arm node is a `Middle` where lanes just run on.
    let (g, n, _) = star(
        &db,
        &[
            (0.0, "basic_2l"),
            (180.0, "transitway_2l"),
            (90.0, "basic_2l"),
        ],
    );
    let j = junction(&g, n);
    check(&g, &j);
    let kind = |id: LaneId| g.lane(id).unwrap().kind;
    let conns = &j.links.connections;
    assert!(
        conns
            .iter()
            .any(|c| kind(c.from) == LaneKind::Car && kind(c.to) == LaneKind::Bus)
    );
    assert!(
        conns
            .iter()
            .any(|c| kind(c.from) == LaneKind::Bus && kind(c.to) == LaneKind::Car)
    );
}

/// T-302: a transit-only street still turns at a crossing of ordinary streets (its lanes are
/// carriageway for the junction), and the ordinary streets' cars never enter it.
#[test]
fn a_tram_street_crossing_a_local_street_keeps_cars_out_of_the_rails() {
    let db = content();
    let (g, n, _) = star(
        &db,
        &[
            (0.0, "tram_2l"),
            (180.0, "tram_2l"),
            (90.0, "basic_2l"),
            (270.0, "basic_2l"),
        ],
    );
    let j = junction(&g, n);
    check(&g, &j);
    let kind = |id: LaneId| g.lane(id).unwrap().kind;
    let conns = &j.links.connections;
    assert!(conns.iter().any(|c| kind(c.from) == LaneKind::Tram));
    assert!(
        conns
            .iter()
            .all(|c| (kind(c.from) == LaneKind::Tram) == (kind(c.to) == LaneKind::Tram))
    );
    // The street's cars go straight across the rails and turn into the other street, never
    // into the tram street.
    let tram_arms = arms_of_road(&db, &g, &j, "tram_2l");
    assert_eq!(tram_arms.len(), 2);
    let car_to_tram_arm = conns
        .iter()
        .filter(|c| kind(c.from) == LaneKind::Car)
        .filter(|c| tram_arms.contains(&arm_of(&g, &j, c.to)))
        .count();
    assert_eq!(car_to_tram_arm, 0, "no car into the tram street's arms");
}

/// BUG-120: on an X of two tram roads every tram lane goes on to every other arm's tram lanes
/// (straight and both turns), and a `tram_4l` runs on into a `tram_2l`.
#[test]
fn tram_lanes_go_every_way_at_an_x_of_tram_roads_and_into_a_narrower_tram_road() {
    let db = content();
    let kind = |g: &NetGraph, id: LaneId| g.lane(id).unwrap().kind;
    let (g, n, _) = star(
        &db,
        &[
            (0.0, "tram_4l"),
            (90.0, "tram_4l"),
            (180.0, "tram_4l"),
            (270.0, "tram_4l"),
        ],
    );
    let j = junction(&g, n);
    check(&g, &j);
    for i in 0..4 {
        for k in (0..4).filter(|&k| k != i) {
            assert!(
                j.links
                    .connections
                    .iter()
                    .any(|c| kind(&g, c.from) == LaneKind::Tram
                        && kind(&g, c.to) == LaneKind::Tram
                        && arm_of(&g, &j, c.from) == i
                        && arm_of(&g, &j, c.to) == k),
                "no rails from arm {i} to arm {k}"
            );
        }
    }
    let (g, n, _) = star(&db, &[(0.0, "tram_4l"), (180.0, "tram_2l")]);
    let j = junction(&g, n);
    check(&g, &j);
    for i in 0..2 {
        assert!(
            j.links
                .connections
                .iter()
                .any(|c| kind(&g, c.from) == LaneKind::Tram
                    && kind(&g, c.to) == LaneKind::Tram
                    && arm_of(&g, &j, c.from) == i),
            "arm {i}: no rails on"
        );
    }
}
