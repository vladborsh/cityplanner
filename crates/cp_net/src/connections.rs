//! Lane connections, pedestrian links and conflicts of one junction
//! (`system-design.md` §6.4). Derived data: computed from the graph and the junction's
//! arms by [`compute_links`] (called from `compute_junction`), stored in
//! [`Junction::links`](crate::junction::Junction), never saved and never hashed.
//!
//! **Which lanes.** Vehicle lanes are `Car` and `Bus` lanes. An arm's *inbound* lanes
//! are the ones travelling towards the node, *outbound* away from it (a one-way highway
//! arm has only one kind). Lane positions are measured across the direction of travel;
//! traffic is right-handed, so "right" is `+Z` when heading `+X` (cp_core convention).
//!
//! **Turn classes.** For an inbound arm and a candidate exit arm, the turn angle `δ` is
//! the signed angle from the heading into the node to the heading out (positive = right):
//! `|δ| ≤ 30°` straight, `30° < δ` right, `δ < -30°` left. An arm with no other arm is a
//! dead end: its inbound lanes U-turn onto its outbound lanes (innermost with innermost,
//! so the curves are concentric and do not conflict).
//!
//! **Which turns from which lane** (per inbound arm, lanes ranked right to left): the
//! rightmost lane takes right and straight exits, the leftmost takes left and straight, the
//! middle lanes straight only; a single lane takes every exit. A lane whose set would be
//! empty (e.g. the middle lane of a T stem, which has no straight exit) takes over the set
//! of the nearest lane that has one (ties go to the right), so no direction is left
//! without lanes and no lane is a dead end.
//!
//! **Which outbound lane** (ranked right to left, `p` of them): a right turn lands on the
//! rightmost, a left turn on the leftmost; straight maps inbound rank `r` of `n` to
//! `round(r·(p−1)/(n−1))` (or the rightmost for `n = 1`), so extra lanes merge on the
//! left.
//!
//! **Curves** are cubics from the trimmed inbound lane end to the trimmed outbound lane
//! start, tangent-continuous with both lanes.
//!
//! **Ids.** A connection is identified by `(node, index)` where `index` is its position in
//! [`Links::connections`]; the order is deterministic: inbound arms counter-clockwise,
//! then lanes right to left, then exit arms counter-clockwise from the inbound arm, then
//! outbound lane. Indices change when the junction is recomputed.
//!
//! **Conflicts** ([`Links::conflicts`]) are sorted pairs `(a, b)` with `a < b`, so the
//! relation is symmetric by construction ([`Links::in_conflict`] accepts either order).
//! Two connections conflict if they merge into the same outbound lane from different
//! inbound lanes, or their curves cross in the XZ plane (connections sharing the inbound
//! lane only diverge and never conflict).
//!
//! **Pedestrians.** `Corner` links join the outer sidewalk lane of an arm with the
//! facing sidewalk lane of the next arm counter-clockwise (for a dead end, around the cap).
//! A `Crossing` runs across each arm that has vehicle lanes and sidewalks on both sides.
//! Both are undirected.

use bevy_math::{Vec2, Vec3};
use cp_content::{LaneDir, LaneKind};
use cp_core::curve::CubicBezier3;

use crate::graph::NetGraph;
use crate::junction::Arm;
use crate::model::{LANE_CURVE_PIECES, Lane, LaneId};

/// Turn angle (degrees) separating straight from left/right.
pub const STRAIGHT_BAND_DEG: f32 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    Right,
    Straight,
    Left,
    UTurn,
}

/// A vehicle path through the junction from an inbound to an outbound lane.
#[derive(Clone, Debug, PartialEq)]
pub struct LaneConnection {
    pub from: LaneId,
    pub to: LaneId,
    pub turn: Turn,
    pub curve: CubicBezier3,
    pub length: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PedLinkKind {
    Corner,
    Crossing,
}

/// An undirected pedestrian path between two sidewalk lane ends.
#[derive(Clone, Debug, PartialEq)]
pub struct PedLink {
    pub kind: PedLinkKind,
    pub a: LaneId,
    pub b: LaneId,
    pub curve: CubicBezier3,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Links {
    pub connections: Vec<LaneConnection>,
    /// Sorted pairs of indices into `connections`, `a < b`.
    pub conflicts: Vec<(u16, u16)>,
    pub pedestrian: Vec<PedLink>,
}

impl Links {
    pub fn in_conflict(&self, a: u16, b: u16) -> bool {
        let key = if a < b { (a, b) } else { (b, a) };
        a != b && self.conflicts.binary_search(&key).is_ok()
    }
}

fn is_vehicle(k: LaneKind) -> bool {
    matches!(k, LaneKind::Car | LaneKind::Bus)
}

/// Lane point and travel tangent at segment parameter `t`.
fn lane_at(lane: &Lane, t: f32) -> (Vec3, Vec3) {
    let n = LANE_CURVE_PIECES;
    let s = match lane.dir {
        LaneDir::Forward => t,
        LaneDir::Backward => 1.0 - t,
    }
    .clamp(0.0, 1.0);
    let k = ((s * n as f32) as usize).min(n - 1);
    let u = s * n as f32 - k as f32;
    (lane.curve[k].point(u), lane.curve[k].tangent(u))
}

/// Lateral position of a lane across its travel direction; larger = more to the right.
fn right_pos(lane: &Lane) -> f32 {
    match lane.dir {
        LaneDir::Forward => -lane.offset,
        LaneDir::Backward => lane.offset,
    }
}

/// Position of a sidewalk lane on the counter-clockwise side of an arm; larger = further
/// counter-clockwise.
fn ccw_pos(lane: &Lane, at_start: bool) -> f32 {
    if at_start { -lane.offset } else { lane.offset }
}

struct VLane {
    id: LaneId,
    rank_pos: f32,
    end: Vec3,
    tangent: Vec3,
}

struct ArmLanes {
    inbound: Vec<VLane>,  // right to left
    outbound: Vec<VLane>, // right to left
    dir: Vec2,
}

fn arm_lanes(graph: &NetGraph, arm: &Arm) -> ArmLanes {
    let mut inbound = Vec::new();
    let mut outbound = Vec::new();
    if let Some(seg) = graph.segment(arm.segment) {
        for &lid in &seg.lanes {
            let Some(l) = graph.lane(lid) else { continue };
            if !is_vehicle(l.kind) {
                continue;
            }
            let (end, tangent) = lane_at(l, arm.trim_t);
            let v = VLane {
                id: lid,
                rank_pos: right_pos(l),
                end,
                tangent,
            };
            // Travelling towards the node: forward lanes at a segment end, backward at a start.
            let towards_node = (l.dir == LaneDir::Forward) != arm.at_start;
            if towards_node {
                inbound.push(v);
            } else {
                outbound.push(v);
            }
        }
    }
    let by_right = |a: &VLane, b: &VLane| b.rank_pos.total_cmp(&a.rank_pos).then(a.id.cmp(&b.id));
    inbound.sort_by(by_right);
    outbound.sort_by(by_right);
    ArmLanes {
        inbound,
        outbound,
        dir: Vec2::new(arm.angle.cos(), arm.angle.sin()),
    }
}

fn cross(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

fn classify(dir_in_arm: Vec2, dir_out_arm: Vec2) -> Turn {
    let h = -dir_in_arm;
    let delta = cross(h, dir_out_arm).atan2(h.dot(dir_out_arm)).to_degrees();
    if delta.abs() <= STRAIGHT_BAND_DEG {
        Turn::Straight
    } else if delta > 0.0 {
        Turn::Right
    } else {
        Turn::Left
    }
}

fn connection_curve(from: &VLane, to: &VLane, uturn: bool) -> CubicBezier3 {
    let dist = from.end.distance(to.end);
    let h = if uturn {
        dist * 0.7 + 3.0
    } else {
        (dist / 3.0).max(2.0)
    };
    CubicBezier3::new(
        from.end,
        from.end + from.tangent * h,
        to.end - to.tangent * h,
        to.end,
    )
}

/// Which turns each of `n` inbound lanes (right to left) may take, given the exit classes
/// present.
fn lane_turns(n: usize, has: [bool; 3]) -> Vec<[bool; 3]> {
    // has = [right, straight, left]
    let mut sets: Vec<[bool; 3]> = (0..n)
        .map(|r| {
            let rightmost = r == 0;
            let leftmost = r == n - 1;
            [has[0] && rightmost, has[1], has[2] && leftmost]
        })
        .collect();
    // Middle lanes only go straight; if there is no straight exit they borrow.
    for set in sets.iter_mut().take(n.saturating_sub(1)).skip(1) {
        set[1] = has[1];
    }
    let empty = |s: &[bool; 3]| !s.iter().any(|&b| b);
    for r in 0..n {
        if empty(&sets[r]) {
            // Nearest non-empty lane, ties to the right (lower rank).
            let found = (1..n)
                .flat_map(|d| [r.checked_sub(d), Some(r + d)])
                .flatten()
                .find(|&q| q < n && !empty(&sets[q]));
            if let Some(q) = found {
                sets[r] = sets[q];
            }
        }
    }
    sets
}

/// Computes the junction's connections, conflicts and pedestrian links from its `arms`
/// (counter-clockwise order, as produced by `compute_junction`).
pub fn compute_links(graph: &NetGraph, arms: &[Arm]) -> Links {
    let k = arms.len();
    let lanes: Vec<ArmLanes> = arms.iter().map(|a| arm_lanes(graph, a)).collect();
    let mut connections: Vec<LaneConnection> = Vec::new();

    for i in 0..k {
        let inb = &lanes[i].inbound;
        if inb.is_empty() {
            continue;
        }
        if k == 1 {
            // Dead end: U-turn, innermost with innermost.
            let out = &lanes[i].outbound;
            if out.is_empty() {
                continue;
            }
            // Innermost = leftmost (the end of the right-to-left order).
            for (t, from) in inb.iter().rev().enumerate() {
                let to = &out[out.len() - 1 - t.min(out.len() - 1)];
                connections.push(LaneConnection {
                    from: from.id,
                    to: to.id,
                    turn: Turn::UTurn,
                    curve: connection_curve(from, to, true),
                    length: 0.0,
                });
            }
            continue;
        }
        // Exits in counter-clockwise order starting after arm i.
        let exits: Vec<(usize, Turn)> = (1..k)
            .map(|d| (i + d) % k)
            .filter(|&j| !lanes[j].outbound.is_empty())
            .map(|j| (j, classify(lanes[i].dir, lanes[j].dir)))
            .collect();
        let has = [
            exits.iter().any(|e| e.1 == Turn::Right),
            exits.iter().any(|e| e.1 == Turn::Straight),
            exits.iter().any(|e| e.1 == Turn::Left),
        ];
        let sets = lane_turns(inb.len(), has);
        for (r, from) in inb.iter().enumerate() {
            for &(j, turn) in &exits {
                let slot = match turn {
                    Turn::Right => 0,
                    Turn::Straight => 1,
                    _ => 2,
                };
                if !sets[r][slot] {
                    continue;
                }
                let out = &lanes[j].outbound;
                let p = out.len();
                let idx = match turn {
                    Turn::Right => 0,
                    Turn::Left => p - 1,
                    _ => {
                        let n = inb.len();
                        if n == 1 {
                            0
                        } else {
                            ((r as f32) * (p - 1) as f32 / (n - 1) as f32).round() as usize
                        }
                    }
                };
                let to = &out[idx.min(p - 1)];
                connections.push(LaneConnection {
                    from: from.id,
                    to: to.id,
                    turn,
                    curve: connection_curve(from, to, false),
                    length: 0.0,
                });
            }
        }
    }
    for c in &mut connections {
        c.length = c.curve.arc_table().length();
    }

    // Conflicts.
    let mut conflicts = Vec::new();
    for a in 0..connections.len() {
        for b in a + 1..connections.len() {
            let (ca, cb) = (&connections[a], &connections[b]);
            if ca.from == cb.from {
                continue;
            }
            let merge = ca.to == cb.to;
            if merge || !ca.curve.intersect_xz(&cb.curve).is_empty() {
                conflicts.push((a as u16, b as u16));
            }
        }
    }

    Links {
        pedestrian: pedestrian_links(graph, arms),
        connections,
        conflicts,
    }
}

struct Ped {
    id: LaneId,
    pos: f32,
    end: Vec3,
    tangent: Vec3,
}

fn ped_lanes(graph: &NetGraph, arm: &Arm) -> Vec<Ped> {
    let mut v = Vec::new();
    if let Some(seg) = graph.segment(arm.segment) {
        for &lid in &seg.lanes {
            let Some(l) = graph.lane(lid) else { continue };
            if l.kind != LaneKind::Pedestrian {
                continue;
            }
            // Walk towards the node.
            let (end, tangent) = lane_at(l, arm.trim_t);
            let tangent = if (l.dir == LaneDir::Forward) != arm.at_start {
                tangent
            } else {
                -tangent
            };
            v.push(Ped {
                id: lid,
                pos: ccw_pos(l, arm.at_start),
                end,
                tangent,
            });
        }
    }
    v.sort_by(|a, b| a.pos.total_cmp(&b.pos).then(a.id.cmp(&b.id)));
    v
}

fn ped_curve(a: &Ped, b: &Ped) -> CubicBezier3 {
    let h = (a.end.distance(b.end) / 3.0).max(0.5);
    // Leave `a` heading into the node, arrive at `b` heading out of it.
    CubicBezier3::new(a.end, a.end + a.tangent * h, b.end + b.tangent * h, b.end)
}

fn pedestrian_links(graph: &NetGraph, arms: &[Arm]) -> Vec<PedLink> {
    let k = arms.len();
    let peds: Vec<Vec<Ped>> = arms.iter().map(|a| ped_lanes(graph, a)).collect();
    let mut out = Vec::new();
    // Corners: arm i's most counter-clockwise sidewalk to arm i+1's most clockwise one. A
    // footpath (T-222) beside roads is no arm of the ring: the roads' sidewalks join each other
    // directly, and the footpath's lanes join the sidewalks on either side of it.
    let is_path: Vec<bool> = arms
        .iter()
        .map(|a| crate::junction::is_path_segment(graph, a.segment))
        .collect();
    let ring: Vec<usize> = if is_path.iter().all(|&p| p) {
        (0..k).collect()
    } else {
        (0..k).filter(|&i| !is_path[i]).collect()
    };
    let mut corner = |i: usize, j: usize| {
        let (Some(a), Some(b)) = (peds[i].last(), peds[j].first()) else {
            return;
        };
        if i == j && peds[i].len() < 2 {
            return;
        }
        out.push(PedLink {
            kind: PedLinkKind::Corner,
            a: a.id,
            b: b.id,
            curve: ped_curve(a, b),
        });
    };
    for (n, &i) in ring.iter().enumerate() {
        corner(i, ring[(n + 1) % ring.len()]);
    }
    if ring.len() < k {
        for i in (0..k).filter(|&i| is_path[i]) {
            // The nearest road arm clockwise and counter-clockwise of the footpath.
            let before = (1..k).map(|d| (i + k - d) % k).find(|&j| !is_path[j]);
            let after = (1..k).map(|d| (i + d) % k).find(|&j| !is_path[j]);
            if let Some(j) = before {
                corner(j, i);
            }
            if let Some(j) = after {
                corner(i, j);
            }
        }
    }
    // Crossings: across every arm that carries vehicles and has sidewalks on both sides.
    for i in 0..k {
        let has_vehicles = graph.segment(arms[i].segment).is_some_and(|s| {
            s.lanes
                .iter()
                .filter_map(|&l| graph.lane(l))
                .any(|l| is_vehicle(l.kind))
        });
        if !has_vehicles || peds[i].len() < 2 {
            continue;
        }
        let (a, b) = (&peds[i][0], &peds[i][peds[i].len() - 1]);
        out.push(PedLink {
            kind: PedLinkKind::Crossing,
            a: a.id,
            b: b.id,
            curve: CubicBezier3::line(a.end, b.end),
        });
    }
    out
}

#[cfg(test)]
mod tests;
