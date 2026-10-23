//! Lane arrows (T-313, §18.2): at the stop line of every car lane of a junction, white arrows
//! for the turns its connections allow (straight, right, left, U-turn, in any combination).
//! They are drawn from the junction's derived links, not from data of their own, so they
//! follow the player's lane rules and the default alike. Near mesh only.

use bevy_math::{Vec2, Vec3};
use cp_content::LaneKind;
use cp_net::{Junction, NetGraph, NodeClass, Turn};

use super::{WHITE, ZEBRA_DEPTH, ZEBRA_GAP, flat_tri};
use crate::chunk::MeshData;

/// Length of an arrow along the lane, metres.
pub const LENGTH: f32 = 3.6;
/// Clear lane between the arrow and the junction's mouth (past the zebra crossing), metres.
const GAP: f32 = ZEBRA_GAP + ZEBRA_DEPTH + 1.0;
/// Half width of the arrow's shaft, metres.
const SHAFT: f32 = 0.14;
/// How far a turning arm reaches across the lane, metres.
const REACH: f32 = 0.95;
/// Lane width the shapes are made for, metres; narrower lanes scale them down.
const LANE: f32 = 3.5;
/// A segment shorter than this gets no arrows, metres.
const MIN_SEGMENT: f32 = GAP + LENGTH + 2.0;
/// The arrows stand this much above the lane surface, metres.
pub const RAISE: f32 = 0.08;

/// A triangle in lane coordinates: `u` along the travel direction, `v` to the driver's right.
type Tri = [(f32, f32); 3];

fn rect(u0: f32, u1: f32, v0: f32, v1: f32, out: &mut Vec<Tri>) {
    out.push([(u0, v0), (u1, v0), (u1, v1)]);
    out.push([(u0, v0), (u1, v1), (u0, v1)]);
}

/// The triangles of the arrow of a lane that may take `turns` (straight, right, left, U-turn),
/// centred on the lane at `u = 0, v = 0`. Empty for no turn.
pub fn shapes(turns: [bool; 4]) -> Vec<Tri> {
    let [straight, right, left, uturn] = turns;
    let mut t = Vec::new();
    if !(straight || right || left || uturn) {
        return t;
    }
    let half = LENGTH * 0.5;
    // The shaft runs to where the arms leave it.
    let top = if straight { 0.7 } else { 0.2 };
    let top = if uturn && !straight { 0.8 } else { top };
    rect(-half, top, -SHAFT, SHAFT, &mut t);
    if straight {
        t.push([(half, 0.0), (top - 0.3, -0.55), (top - 0.3, 0.55)]);
    }
    for (on, side) in [(right, 1.0f32), (left, -1.0)] {
        if on {
            let at = 0.2;
            rect(
                at - SHAFT,
                at + SHAFT,
                0.0f32.min(side * 0.0),
                side * REACH,
                &mut t,
            );
            // The head points sideways.
            t.push([
                (at, side * (REACH + 0.55)),
                (at - 0.5, side * (REACH - 0.05)),
                (at + 0.5, side * (REACH - 0.05)),
            ]);
        }
    }
    if uturn {
        // Over to the left, and back against the travel direction.
        let at = 0.8;
        let v = -REACH;
        rect(at - SHAFT, at + SHAFT, v, 0.0, &mut t);
        rect(0.1, at + SHAFT, v - SHAFT, v + SHAFT, &mut t);
        t.push([(-0.4, v), (0.1, v - 0.45), (0.1, v + 0.45)]);
    }
    t
}

/// Which turns (straight, right, left, U-turn) the connections of a lane make.
fn turn_set(turns: &[Turn]) -> [bool; 4] {
    [
        turns.contains(&Turn::Straight),
        turns.contains(&Turn::Right),
        turns.contains(&Turn::Left),
        turns.contains(&Turn::UTurn),
    ]
}

/// Pushes the arrow of `turns` on a lane whose connection leaves `from` heading `dir` (in the
/// plane), at `y`: the arrow centre is [`GAP`] and half its length before `from`.
pub fn push(m: &mut MeshData, from: Vec3, dir: Vec2, y: f32, width: f32, turns: [bool; 4]) {
    let d = dir.normalize_or_zero();
    if d == Vec2::ZERO {
        return;
    }
    let right = Vec2::new(-d.y, d.x);
    let scale = (width / LANE).min(1.0);
    let c = Vec2::new(from.x, from.z) - d * (GAP + LENGTH * 0.5);
    for tri in shapes(turns) {
        let p = tri.map(|(u, v)| {
            let q = c + d * (u * scale) + right * (v * scale);
            Vec3::new(q.x, y, q.y)
        });
        flat_tri(m, p, WHITE);
    }
}

/// The arrows of junction `j`: one set per inbound car or bus lane with a connection.
pub(super) fn build(graph: &NetGraph, j: &Junction) -> MeshData {
    let mut m = MeshData::default();
    if j.class != NodeClass::Junction {
        return m;
    }
    let mut lanes: std::collections::BTreeMap<cp_net::LaneId, Vec<usize>> = Default::default();
    for (i, c) in j.links.connections.iter().enumerate() {
        lanes.entry(c.from).or_default().push(i);
    }
    for (lane, conns) in lanes {
        let (Some(l), Some(first)) = (graph.lane(lane), conns.first()) else {
            continue;
        };
        if !matches!(l.kind, LaneKind::Car | LaneKind::Bus) {
            continue;
        }
        if graph
            .segment(l.segment)
            .is_none_or(|s| s.length < MIN_SEGMENT)
        {
            continue;
        }
        let turns: Vec<Turn> = conns.iter().map(|&i| j.links.connections[i].turn).collect();
        let curve = &j.links.connections[*first].curve;
        push(
            &mut m,
            curve.p0,
            Vec2::new(curve.p1.x - curve.p0.x, curve.p1.z - curve.p0.z),
            curve.p0.y,
            l.width,
            turn_set(&turns),
        );
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(t: &[Tri]) -> f32 {
        t.iter()
            .map(|[a, b, c]| ((b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1)).abs() * 0.5)
            .sum()
    }

    fn extent(t: &[Tri]) -> (f32, f32, f32, f32) {
        let pts = t.iter().flatten();
        let f = |g: fn(&(f32, f32)) -> f32, init: f32, pick: fn(f32, f32) -> f32| {
            pts.clone().map(g).fold(init, pick)
        };
        (
            f(|p| p.0, f32::MAX, f32::min),
            f(|p| p.0, f32::MIN, f32::max),
            f(|p| p.1, f32::MAX, f32::min),
            f(|p| p.1, f32::MIN, f32::max),
        )
    }

    #[test]
    fn no_turn_no_arrow_and_every_set_has_its_own_shape() {
        assert!(shapes([false; 4]).is_empty());
        let sets = [
            [true, false, false, false],
            [false, true, false, false],
            [false, false, true, false],
            [false, false, false, true],
            [true, true, false, false],
            [true, false, true, false],
            [true, true, true, false],
        ];
        let shapes: Vec<_> = sets.iter().map(|&s| shapes(s)).collect();
        for (i, a) in shapes.iter().enumerate() {
            assert!(area(a) > 0.3, "set {i}");
            for b in &shapes[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn arrows_point_the_way_and_stay_inside_a_lane() {
        let half = LANE * 0.5;
        for (set, side) in [
            ([true, false, false, false], 0.0),
            ([false, true, false, false], 1.0),
            ([false, false, true, false], -1.0),
            ([true, true, true, true], 0.0),
        ] {
            let (u0, u1, v0, v1) = extent(&shapes(set));
            assert!(
                u0 >= -LENGTH * 0.5 - 1e-4 && u1 <= LENGTH * 0.5 + 1e-4,
                "{set:?}"
            );
            assert!(v0 > -half && v1 < half, "{set:?}: {v0} {v1}");
            if side > 0.0 {
                assert!(v1 > 1.0 && v0 > -0.2);
            }
            if side < 0.0 {
                assert!(v0 < -1.0 && v1 < 0.2);
            }
        }
        // Straight reaches the far end of the arrow.
        assert!((extent(&shapes([true, false, false, false])).1 - LENGTH * 0.5).abs() < 1e-4);
    }

    #[test]
    fn an_arrow_lies_flat_before_the_stop_line_on_the_right_side_of_the_travel_direction() {
        let mut m = MeshData::default();
        // Heading +x, the mouth at x = 100; right is +z.
        push(
            &mut m,
            Vec3::new(100.0, 5.0, 0.0),
            Vec2::new(1.0, 0.0),
            5.1,
            3.5,
            [false, true, false, false],
        );
        assert!(!m.positions.is_empty());
        assert!(m.positions.iter().all(|p| (p[1] - 5.1).abs() < 1e-6));
        assert!(m.positions.iter().all(|p| p[0] < 100.0 - GAP + 1e-3));
        assert!(m.positions.iter().any(|p| p[2] > 0.9));
        assert!(m.positions.iter().all(|p| p[2] > -0.2));
    }
}
