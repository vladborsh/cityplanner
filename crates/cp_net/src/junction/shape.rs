//! Node geometry v2 (`system-design.md` §6.8.3 item 6): the arms' real edge curves, where
//! neighbouring edges meet, the class of a node, and the lines across a node that its outline,
//! curbs and the renderer's pieces follow.

use std::f32::consts::{PI, TAU};

use bevy_math::{Vec2, Vec3};
use cp_core::curve::{ArcTable, CubicBezier3, to_xz};

use super::{Arm, FLAT_GAP_DEG, Junction, MAX_SETBACK, Raw, rot_ccw};
use crate::graph::NetGraph;
use crate::model::SegmentId;

/// Two arms of one road through a node that turn at most this much are a `Middle` node:
/// no node piece, one shared end line (degrees, as `CORNER_DEG` of the stretches).
pub const MIDDLE_DEG: f32 = 10.0;
/// Length over which the wider road narrows to the other at a `Transition` node, metres.
pub const TRANSITION_LEN: f32 = 8.0;
/// Edges are searched for a meeting point this far out along each arm, metres.
const MEET_REACH: f32 = 160.0;
/// Samples per arm of the meeting search; the crossing is then refined by Newton steps.
const MEET_SAMPLES: usize = 64;
/// Spacing of the points of a line along an arm and around a node, metres and radians.
const LINE_STEP: f32 = 2.0;
const ARC_STEP: f32 = PI / 16.0;

/// What a node is to the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NodeClass {
    /// One arm: the road ends in a cap around the node (or no arm at all).
    #[default]
    End,
    /// Two arms of one road running on in the same direction, turning at most [`MIDDLE_DEG`]:
    /// no node piece, the two ends meet on one mitred line through the node.
    Middle,
    /// Two arms of one road turning more: the road's profile runs on through a joint piece
    /// between the two cuts (lanes, markings and sidewalks continuous).
    Bend,
    /// Two arms of different roads (or of one road meeting itself head-on): a taper between
    /// the two profiles.
    Transition,
    /// Three or more arms: carriageway between the arms' ends and curb curves, sidewalk corners.
    Junction,
}

/// A segment's curve walked from a node outward, relative to the node (XZ), with its arc
/// length table. `at(s, xi)` is the point `s` metres from the node, `xi` to the left looking out.
pub struct ArmPath {
    curve: CubicBezier3,
    /// Arc length table, `None` for a straight arm (arc length is then linear in `t`).
    table: Option<ArcTable>,
    pub length: f32,
}

impl ArmPath {
    pub fn new(graph: &NetGraph, segment: SegmentId, at_start: bool, origin: Vec3) -> Option<Self> {
        let c = graph.segment(segment)?.curve;
        let (p0, p1, p2, p3) = if at_start {
            (c.p0, c.p1, c.p2, c.p3)
        } else {
            (c.p3, c.p2, c.p1, c.p0)
        };
        let curve = CubicBezier3::new(p0 - origin, p1 - origin, p2 - origin, p3 - origin);
        Some(Self::of(curve))
    }

    fn of(curve: CubicBezier3) -> Self {
        let mut path = Self {
            curve,
            table: None,
            length: curve.p0.distance(curve.p3),
        };
        if path.line().is_none() {
            let table = curve.arc_table();
            path.length = table.length();
            path.table = Some(table);
        }
        path
    }

    fn s_to_t(&self, s: f32) -> f32 {
        match &self.table {
            Some(t) => t.s_to_t(s),
            None => (s / self.length.max(1e-6)).clamp(0.0, 1.0),
        }
    }

    fn t_to_s(&self, t: f32) -> f32 {
        match &self.table {
            Some(table) => table.t_to_s(t),
            None => t * self.length,
        }
    }

    /// The start (the node) and unit direction of the arm when its curve is a straight line.
    fn line(&self) -> Option<(Vec2, Vec2)> {
        let (p0, p3) = (to_xz(self.curve.p0), to_xz(self.curve.p3));
        let d = (p3 - p0).try_normalize()?;
        let off = |p: Vec3| (to_xz(p) - p0).perp_dot(d).abs();
        (off(self.curve.p1) < 1e-4 && off(self.curve.p2) < 1e-4).then_some((p0, d))
    }

    fn dir_t(&self, u: f32) -> Vec2 {
        to_xz(self.curve.derivative(u))
            .try_normalize()
            .or_else(|| to_xz(self.curve.p3 - self.curve.p0).try_normalize())
            .unwrap_or(Vec2::X)
    }

    fn edge_t(&self, u: f32, xi: f32) -> Vec2 {
        to_xz(self.curve.point(u)) + rot_ccw(self.dir_t(u)) * xi
    }

    /// Unit direction away from the node `s` metres out.
    pub fn dir(&self, s: f32) -> Vec2 {
        self.dir_t(self.s_to_t(s.clamp(0.0, self.length)))
    }

    /// The point `s` metres out, `xi` to the left looking out (node-relative XZ).
    pub fn at(&self, s: f32, xi: f32) -> Vec2 {
        self.edge_t(self.s_to_t(s.clamp(0.0, self.length)), xi)
    }
}

/// Parameters `(x, y)` in `[0, 1]` where segments `a0 a1` and `b0 b1` cross.
fn seg_hit(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> Option<(f32, f32)> {
    let (d, e) = (a1 - a0, b1 - b0);
    let den = d.perp_dot(e);
    if den.abs() < 1e-9 {
        return None;
    }
    let w = b0 - a0;
    let (x, y) = (w.perp_dot(e) / den, w.perp_dot(d) / den);
    let eps = 1e-5;
    ((-eps..=1.0 + eps).contains(&x) && (-eps..=1.0 + eps).contains(&y))
        .then_some((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
}

/// Where `a`'s left edge (`ha` to its left, looking out) meets `b`'s right edge (`hb` to its
/// right): arc lengths from the node along each, at the crossing farthest out along `a`. The
/// edges are the real offset curves: sampled, then refined by Newton steps.
pub(super) fn edge_meet(pa: &ArmPath, ha: f32, pb: &ArmPath, hb: f32) -> Option<(f32, f32)> {
    // Two straight arms (most of a city): the edges are lines, their meeting point is exact.
    if let (Some((a0, da)), Some((b0, db))) = (pa.line(), pb.line()) {
        let (ea, eb) = (a0 + rot_ccw(da) * ha, b0 - rot_ccw(db) * hb);
        let den = da.perp_dot(db);
        if den.abs() < 1e-9 {
            return None;
        }
        let w = eb - ea;
        let (s, t) = (w.perp_dot(db) / den, w.perp_dot(da) / den);
        let reach = |p: &ArmPath| p.length.min(MEET_REACH);
        let inside = |x: f32, p: &ArmPath| (-1e-4..=reach(p) + 1e-4).contains(&x);
        return (inside(s, pa) && inside(t, pb)).then_some((s.max(0.0), t.max(0.0)));
    }
    sampled_meet(pa, ha, pb, hb)
}

/// [`edge_meet`] for any curves: the edges sampled, the farthest crossing along `a` refined by
/// Newton steps while they bring the edges closer.
fn sampled_meet(pa: &ArmPath, ha: f32, pb: &ArmPath, hb: f32) -> Option<(f32, f32)> {
    let n = MEET_SAMPLES;
    let ua = pa.s_to_t(pa.length.min(MEET_REACH));
    let ub = pb.s_to_t(pb.length.min(MEET_REACH));
    let sa: Vec<Vec2> = (0..=n)
        .map(|i| pa.edge_t(ua * i as f32 / n as f32, ha))
        .collect();
    let sb: Vec<Vec2> = (0..=n)
        .map(|i| pb.edge_t(ub * i as f32 / n as f32, -hb))
        .collect();
    let mut best: Option<(f32, f32)> = None;
    for i in 0..n {
        for k in 0..n {
            if let Some((x, y)) = seg_hit(sa[i], sa[i + 1], sb[k], sb[k + 1]) {
                let (u, v) = (
                    ua * (i as f32 + x) / n as f32,
                    ub * (k as f32 + y) / n as f32,
                );
                if best.is_none_or(|(bu, _)| u > bu) {
                    best = Some((u, v));
                }
            }
        }
    }
    let (mut u, mut v) = best?;
    let h = 1e-3;
    let residual = |u: f32, v: f32| (pa.edge_t(u, ha) - pb.edge_t(v, -hb)).length();
    let mut r = residual(u, v);
    for _ in 0..8 {
        if r < 1e-6 {
            break;
        }
        let f = pa.edge_t(u, ha) - pb.edge_t(v, -hb);
        let du = (pa.edge_t(u + h, ha) - pa.edge_t(u - h, ha)) / (2.0 * h);
        let dv = -(pb.edge_t(v + h, -hb) - pb.edge_t(v - h, -hb)) / (2.0 * h);
        let det = du.perp_dot(dv);
        if det.abs() < 1e-9 {
            break;
        }
        let nu = (u + (-f).perp_dot(dv) / det).clamp(0.0, 1.0);
        let nv = (v + du.perp_dot(-f) / det).clamp(0.0, 1.0);
        // Only a step that brings the edges closer: never wander off to another crossing.
        let nr = residual(nu, nv);
        if nr >= r {
            break;
        }
        (u, v, r) = (nu, nv, nr);
    }
    Some((pa.t_to_s(u), pb.t_to_s(v)))
}

/// Whether the segment's lanes read the same from either end: each lane has a mirror image of
/// the same kind and width on the other side, running the other way (sidewalks either way).
fn symmetric(graph: &NetGraph, segment: SegmentId) -> bool {
    let Some(seg) = graph.segment(segment) else {
        return false;
    };
    let lanes: Vec<_> = seg.lanes.iter().filter_map(|&l| graph.lane(l)).collect();
    lanes.iter().all(|l| {
        lanes.iter().any(|m| {
            m.kind == l.kind
                && (m.offset + l.offset).abs() < 1e-3
                && (m.width - l.width).abs() < 1e-3
                && (l.kind == cp_content::LaneKind::Pedestrian || m.dir != l.dir)
        })
    })
}

/// Drops the points of `pts` that lie on the line through their neighbours.
pub(super) fn simplify(pts: Vec<Vec2>) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(pts.len());
    for p in pts {
        if out.last().is_some_and(|q| q.distance(p) <= 1e-4) {
            continue;
        }
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let d = (p - a).try_normalize().unwrap_or(Vec2::ZERO);
            if (b - a).perp_dot(d).abs() < 1e-4 && (b - a).dot(d) > 0.0 && (p - b).dot(d) > 0.0 {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// Counter-clockwise gap from angle `a` to angle `b`, in `(0, 2π]`.
pub(super) fn ccw_gap(a: f32, b: f32) -> f32 {
    let g = (b - a).rem_euclid(TAU);
    if g == 0.0 { TAU } else { g }
}

/// The class of a node with the two road arms `a` and `b`.
fn two_arm_class(graph: &NetGraph, a: &Raw, b: &Raw) -> NodeClass {
    let turn = PI - a.dir.dot(b.dir).clamp(-1.0, 1.0).acos();
    let road = |r: &Raw| graph.segment(r.segment).map(|s| s.road);
    // Head-on (both start or both end here) is the same profile only for a road that reads the
    // same both ways.
    let same = road(a) == road(b) && (a.at_start != b.at_start || symmetric(graph, a.segment));
    match (same, turn <= MIDDLE_DEG.to_radians()) {
        (true, true) => NodeClass::Middle,
        (true, false) => NodeClass::Bend,
        (false, _) => NodeClass::Transition,
    }
}

/// The class of a node with road arms `raws` (sorted counter-clockwise) and the setback each
/// arm wants, with whether it is *hard* (see `shared_setback`).
pub(super) fn class_and_wants(
    graph: &NetGraph,
    origin: Vec3,
    raws: &[Raw],
) -> (NodeClass, Vec<(f32, bool)>) {
    let k = raws.len();
    let half = |r: &Raw| r.width * 0.5;
    if k < 2 {
        return (
            NodeClass::End,
            raws.iter()
                .map(|r| (half(r).min(MAX_SETBACK), false))
                .collect(),
        );
    }
    // Most nodes of a city are middle nodes: classed before any curve work.
    if k == 2 && two_arm_class(graph, &raws[0], &raws[1]) == NodeClass::Middle {
        return (NodeClass::Middle, vec![(0.0, false); 2]);
    }
    let paths: Vec<Option<ArmPath>> = raws
        .iter()
        .map(|r| ArmPath::new(graph, r.segment, r.at_start, origin))
        .collect();
    // Where arm `i`'s left edge meets arm `j`'s right edge: `(s_i, s_j)`.
    let meet = |i: usize, j: usize| match (&paths[i], &paths[j]) {
        (Some(a), Some(b)) => edge_meet(a, half(&raws[i]), b, half(&raws[j])),
        _ => None,
    };
    let flat = FLAT_GAP_DEG.to_radians();
    if k == 2 {
        let (a, b) = (&raws[0], &raws[1]);
        let turn = PI - a.dir.dot(b.dir).clamp(-1.0, 1.0).acos();
        let class = two_arm_class(graph, a, b);
        // The inner side of the turn is the gap under 180 degrees.
        let inner = if ccw_gap(a.angle, b.angle) < PI {
            meet(0, 1)
        } else {
            meet(1, 0).map(|(sb, sa)| (sa, sb))
        };
        let (mut sa, mut sb) = inner.unwrap_or_else(|| {
            let s = half(a).max(half(b)) * (turn * 0.5).tan();
            (s, s)
        });
        if class == NodeClass::Transition && (a.width - b.width).abs() > 0.01 {
            if a.width > b.width {
                sa = sa.max(TRANSITION_LEN);
            } else {
                sb = sb.max(TRANSITION_LEN);
            }
        }
        return (
            class,
            vec![(sa.min(MAX_SETBACK), true), (sb.min(MAX_SETBACK), true)],
        );
    }
    let mut wants: Vec<f32> = raws.iter().map(half).collect();
    for i in 0..k {
        let j = (i + 1) % k;
        if ccw_gap(raws[i].angle, raws[j].angle) >= flat {
            continue;
        }
        let (si, sj) = meet(i, j).unwrap_or_else(|| {
            let gap = ccw_gap(raws[i].angle, raws[j].angle);
            (
                super::corner_distance(half(&raws[i]), half(&raws[j]), gap),
                super::corner_distance(half(&raws[j]), half(&raws[i]), gap),
            )
        });
        wants[i] = wants[i].max(si);
        wants[j] = wants[j].max(sj);
    }
    (
        NodeClass::Junction,
        wants
            .into_iter()
            .map(|s| (s.min(MAX_SETBACK), true))
            .collect(),
    )
}

/// Points along `path` from `s_cut` back to the node, the lateral `xi` going from `x_cut` at the
/// cut to `x_node` at the node (`sign` +1 for the left side looking out, -1 for the right).
fn part(path: &ArmPath, s_cut: f32, x_cut: f32, x_node: f32, sign: f32) -> Vec<Vec2> {
    let n = (s_cut / LINE_STEP).ceil().max(1.0) as usize;
    (0..=n)
        .map(|i| {
            let f = 1.0 - i as f32 / n as f32;
            let s = s_cut * f;
            path.at(s, sign * (x_node + (x_cut - x_node) * f))
        })
        .collect()
}

/// The first crossing of polyline `a` (walked from its start) with polyline `b`:
/// `(index in a, index in b, point)`.
fn polyline_hit(a: &[Vec2], b: &[Vec2]) -> Option<(usize, usize, Vec2)> {
    for i in 0..a.len().saturating_sub(1) {
        let mut best: Option<(f32, usize)> = None;
        for k in 0..b.len().saturating_sub(1) {
            if let Some((x, _)) = seg_hit(a[i], a[i + 1], b[k], b[k + 1])
                && best.is_none_or(|(bx, _)| x < bx)
            {
                best = Some((x, k));
            }
        }
        if let Some((x, k)) = best {
            return Some((i, k, a[i] + (a[i + 1] - a[i]) * x));
        }
    }
    None
}

/// A line across a node in three parts, so the lines beside it can be sampled alike: `a` runs
/// from the first arm's cut towards the node, `joint` turns (a mitre point, an arc around the
/// node or a corner curve), `b` runs out to the second arm's cut. Each part starts where the
/// previous one ends. World XZ.
#[derive(Clone, Debug, PartialEq)]
pub struct SideLine {
    pub a: Vec<Vec2>,
    pub joint: Vec<Vec2>,
    pub b: Vec<Vec2>,
}

impl SideLine {
    fn point(p: Vec2) -> Self {
        Self {
            a: vec![p],
            joint: vec![p],
            b: vec![p],
        }
    }

    /// The whole line, from the first cut to the second.
    pub fn points(&self) -> Vec<Vec2> {
        let mut out = self.a.clone();
        for part in [&self.joint, &self.b] {
            out.extend(part.iter().skip(1));
        }
        out
    }

    fn offset(mut self, c: Vec2) -> Self {
        for part in [&mut self.a, &mut self.joint, &mut self.b] {
            for p in part.iter_mut() {
                *p += c;
            }
        }
        self
    }
}

/// The line across node `j` from arm `a`'s left side to arm `b`'s right side (`b` follows `a`
/// counter-clockwise, or is the other arm of a node of degree 2), `ra` and `rb` metres from their
/// centre lines, from `a`'s cut to `b`'s cut. Topology decides the shape, not coordinates:
/// * a convex corner (gap under [`FLAT_GAP_DEG`]) of a junction, with `round`: the curb curve,
///   a quadratic Bezier between the two cut points whose tangents run along the arms;
/// * otherwise each arm contributes its own edge from its cut back to the node (a curved arm's
///   real offset curve; the lateral tapers to the value of the arm with the shorter setback),
///   and where the gap is under 180 degrees the two meet in a mitre, else they are joined by an
///   arc around the node (a round join: the outside of a bend, the flat side of a T).
pub fn side_line(
    graph: &NetGraph,
    j: &Junction,
    a: &Arm,
    b: &Arm,
    ra: f32,
    rb: f32,
    round: bool,
) -> SideLine {
    let c = to_xz(j.pos);
    let (Some(pa), Some(pb)) = (
        ArmPath::new(graph, a.segment, a.at_start, j.pos),
        ArmPath::new(graph, b.segment, b.at_start, j.pos),
    ) else {
        return SideLine::point(c);
    };
    let (p0, p1) = (pa.at(a.setback, ra), pb.at(b.setback, -rb));
    if p0.distance(p1) < 1e-3 {
        return SideLine::point(p0).offset(c);
    }
    let gap = ccw_gap(a.angle, b.angle);
    let straight = SideLine {
        a: vec![p0],
        joint: vec![p0, p1],
        b: vec![p1],
    };
    if round && gap < FLAT_GAP_DEG.to_radians() {
        // Tangent lines into the node from both cuts.
        let (da, db) = (-pa.dir(a.setback), -pb.dir(b.setback));
        let den = da.perp_dot(db);
        if den.abs() > 1e-6 {
            let w = p1 - p0;
            let (t, u) = (w.perp_dot(db) / den, w.perp_dot(da) / den);
            if t > 0.01 && u > 0.01 {
                let x = p0 + da * t;
                let n = (p0.distance(x) + x.distance(p1)).ceil().clamp(2.0, 24.0) as usize;
                let joint = (0..=n)
                    .map(|i| {
                        let f = i as f32 / n as f32;
                        p0 * (1.0 - f) * (1.0 - f) + x * 2.0 * f * (1.0 - f) + p1 * f * f
                    })
                    .collect();
                return SideLine {
                    a: vec![p0],
                    joint,
                    b: vec![p1],
                }
                .offset(c);
            }
        }
        return straight.offset(c);
    }
    let r_node = if (a.setback - b.setback).abs() < 1e-4 {
        ra.min(rb)
    } else if a.setback < b.setback {
        ra
    } else {
        rb
    };
    let mut pa_pts = part(&pa, a.setback, ra, r_node, 1.0);
    let mut pb_pts = part(&pb, b.setback, rb, r_node, -1.0);
    pb_pts.reverse();
    // Exact ends.
    pa_pts[0] = p0;
    *pb_pts.last_mut().expect("points") = p1;
    if gap >= PI {
        let (qa, qb) = (*pa_pts.last().expect("points"), pb_pts[0]);
        let mut joint = vec![qa];
        if r_node > 1e-4 {
            let (t0, t1) = (qa.y.atan2(qa.x), qb.y.atan2(qb.x));
            let mut sweep = (t1 - t0).rem_euclid(TAU);
            if sweep > TAU - 1e-3 {
                sweep = 0.0;
            }
            let m = (sweep / ARC_STEP).ceil() as usize;
            joint.extend(
                (1..m).map(|i| Vec2::from_angle(t0 + sweep * i as f32 / m as f32) * r_node),
            );
        }
        joint.push(qb);
        return SideLine {
            a: simplify(pa_pts),
            joint: simplify(joint),
            b: simplify(pb_pts),
        }
        .offset(c);
    }
    match polyline_hit(&pa_pts, &pb_pts) {
        Some((i, k, x)) => {
            let mut a_part: Vec<Vec2> = pa_pts[..=i].to_vec();
            a_part.push(x);
            let mut b_part = vec![x];
            b_part.extend_from_slice(&pb_pts[k + 1..]);
            SideLine {
                a: simplify(a_part),
                joint: vec![x],
                b: simplify(b_part),
            }
            .offset(c)
        }
        None => straight.offset(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm(deg: f32, len: f32) -> ArmPath {
        let d = Vec2::from_angle(deg.to_radians()) * len;
        ArmPath::of(CubicBezier3::line(Vec3::ZERO, Vec3::new(d.x, 0.0, d.y)))
    }

    /// The exact meeting point of two straight arms is the one the sampled search converges to.
    #[test]
    fn straight_arms_meet_where_the_sampled_search_finds_them() {
        for (a, b, ha, hb) in [
            (0.0, 90.0, 6.0, 6.0),
            (0.0, 45.0, 14.0, 6.0),
            (-20.0, 20.0, 6.0, 9.5),
            (10.0, 150.0, 3.75, 6.0),
            (0.0, 30.0, 14.0, 14.0),
        ] {
            let (pa, pb) = (arm(a, 90.0), arm(b, 70.0));
            let (exact, sampled) = (edge_meet(&pa, ha, &pb, hb), sampled_meet(&pa, ha, &pb, hb));
            match (exact, sampled) {
                (Some(e), Some(s)) => assert!(
                    (e.0 - s.0).abs() < 1e-3 && (e.1 - s.1).abs() < 1e-3,
                    "{a} {b}: {e:?} vs {s:?}"
                ),
                // Both find no crossing (here `b`'s edge meets `a`'s behind the node).
                (None, None) => assert_eq!((a, b), (10.0, 150.0)),
                other => panic!("{a} {b}: {other:?}"),
            }
        }
        // Edges that only meet behind the node do not meet.
        assert!(edge_meet(&arm(0.0, 90.0), 6.0, &arm(270.0, 90.0), 6.0).is_none());
    }
}
