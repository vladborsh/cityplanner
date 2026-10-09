//! Junction geometry (`system-design.md` §6.4): derived data recomputed in the `Network`
//! phase for every dirty node.
//!
//! **Per node**, [`compute_junction`] does:
//! 1. collects the *arms* (segment ends at the node) with their outward direction in the
//!    XZ plane and road width, and sorts them counter-clockwise by angle (ties by
//!    `SegmentId`, then start-before-end), so the order is deterministic;
//! 2. gives each arm a **setback** `s` (metres from the node centre along the segment);
//! 3. cuts each segment at its setback (`trim_t`) and builds the junction **polygon** from
//!    the corner points of the arms.
//!
//! **Width** of an arm is the road's full width, read off its lanes
//! (`2 · max(|offset| + width/2)`), so geometry is a pure function of the [`NetGraph`] —
//! no content lookup, nothing extra to save.
//!
//! **Setback formula.** For arm `i` with half-width `hᵢ` and a neighbour `j` whose
//! direction is `θ` away (the counter-clockwise gap for the left side, the clockwise gap
//! for the right side), the side edges of the two roads meet at distance
//! `t = (hⱼ + hᵢ·cos θ) / sin θ` along arm `i` (for `θ = 90°` this is the neighbour's
//! half-width). For gaps of `150°` and more the edges no longer form a corner and `t = hᵢ`.
//! The setback is `max(hᵢ, t_left, t_right)`, capped at [`MAX_SETBACK`] and at `45 %` of the
//! segment length — the two ends of a segment are capped independently, so trimmed
//! segments never overlap each other. Every arm keeps at least `hᵢ`, which also gives
//! straight joins (two collinear arms) a clean `w × w` patch. A dead end (one arm) uses
//! `s = h` and a `w × w` rectangle around the node.
//!
//! **Polygon**: for each arm in counter-clockwise order, its right corner then its left
//! corner (`node + d·s ∓ l·h`, `l` = `d` rotated 90° counter-clockwise). Where the
//! counter-clockwise gap to the next arm exceeds `210°` (a bend, or the outer side of a
//! Y/T with a wide empty side) the polygon is closed with the point where the two outer
//! road edges meet, so the pavement covers the outside of the corner. Repeated points are
//! dropped. The polygon is simple (no self-intersection) and contains the node whenever
//! no cap was applied; capped short segments can distort it, which is a visual issue
//! only.
//!
//! **Storage**: [`JunctionGeometry`] is a separate resource, *not* part of [`NetGraph`]: it
//! is never saved and never hashed (it is a function of hashed state). After a load it is
//! empty and [`update_junctions`] rebuilds every node it is missing; `SaveData::restore`
//! also resets it, so a restore into a running sim cannot leave stale geometry.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_math::{Vec2, Vec3};
use cp_content::{RoadDefId, SimContent};
use cp_core::curve::{CubicBezier3, to_xz};

use crate::connections::{Links, compute_links};
use crate::control::{ControlPlan, auto_control, plan_control};
use crate::graph::NetGraph;
use crate::model::{NodeId, SegmentId};

/// Longest setback, metres.
pub const MAX_SETBACK: f32 = 60.0;
/// Fraction of a segment's length one end may consume.
pub const MAX_SETBACK_FRACTION: f32 = 0.45;
/// Width used for a segment without lanes, metres.
pub const DEFAULT_WIDTH: f32 = 8.0;
/// Angular gaps of at least this (degrees) do not form a road corner.
const FLAT_GAP_DEG: f32 = 150.0;
/// Gaps wider than this get an extra outer corner point in the polygon.
const OUTER_GAP_DEG: f32 = 210.0;

/// One segment end at a junction.
#[derive(Clone, Debug, PartialEq)]
pub struct Arm {
    pub segment: SegmentId,
    /// `true` if the segment *starts* at this node, `false` if it ends there.
    pub at_start: bool,
    /// Outward direction angle in radians, `atan2(z, x)`, in `(-π, π]`.
    pub angle: f32,
    /// Full road width, metres.
    pub width: f32,
    /// Distance from the node centre to where the segment is cut, metres.
    pub setback: f32,
    /// Segment-curve parameter of the cut (small near the start for `at_start` arms, near
    /// 1 for the others).
    pub trim_t: f32,
    /// Corner points on the polygon (XZ), looking outward from the node.
    pub right: Vec2,
    pub left: Vec2,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Junction {
    pub node: NodeId,
    pub pos: Vec3,
    /// Arms sorted counter-clockwise by angle.
    pub arms: Vec<Arm>,
    /// Counter-clockwise polygon (XZ); empty for a node without segments.
    pub polygon: Vec<Vec2>,
    /// Lane connections, conflicts and pedestrian links (see [`crate::connections`]).
    pub links: Links,
    /// Priority and traffic-light plan (see [`crate::control`]). Filled in by
    /// [`update_junctions`]; empty when the junction comes straight from [`compute_junction`].
    pub control: ControlPlan,
    /// Value of [`JunctionGeometry::version`] when this junction was last recomputed.
    pub revision: u64,
}

/// All junctions of the network. Cheap to clone (`Arc`), so `SimView` carries a copy.
#[derive(Resource, Clone, Debug, Default)]
pub struct JunctionGeometry {
    junctions: Arc<BTreeMap<NodeId, Junction>>,
    version: u64,
}

impl JunctionGeometry {
    pub fn get(&self, node: NodeId) -> Option<&Junction> {
        self.junctions.get(&node)
    }
    /// Junctions in node-id order.
    pub fn iter(&self) -> impl Iterator<Item = (&NodeId, &Junction)> {
        self.junctions.iter()
    }
    pub fn len(&self) -> usize {
        self.junctions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.junctions.is_empty()
    }
    /// Increases whenever any junction changed. A renderer remembers the last version it
    /// built and rebuilds the junctions whose `revision` is greater; nodes missing from
    /// the map were removed.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The segment's curve cut at both junctions: the part a renderer draws as road.
    /// Falls back to the whole curve for an end without junction data.
    pub fn trimmed_curve(&self, graph: &NetGraph, segment: SegmentId) -> Option<CubicBezier3> {
        let seg = graph.segment(segment)?;
        let t_of = |node: NodeId, at_start: bool, default: f32| {
            self.get(node)
                .and_then(|j| {
                    j.arms
                        .iter()
                        .find(|a| a.segment == segment && a.at_start == at_start)
                })
                .map_or(default, |a| a.trim_t)
        };
        let t0 = t_of(seg.start, true, 0.0);
        let t1 = t_of(seg.end, false, 1.0);
        Some(if t1 > t0 {
            seg.curve.sub_curve(t0, t1)
        } else {
            seg.curve
        })
    }
}

fn rot_ccw(v: Vec2) -> Vec2 {
    Vec2::new(-v.y, v.x)
}

/// Full road width of a segment, from its lanes.
fn segment_width(graph: &NetGraph, segment: SegmentId) -> f32 {
    let Some(seg) = graph.segment(segment) else {
        return DEFAULT_WIDTH;
    };
    let half = seg
        .lanes
        .iter()
        .filter_map(|&l| graph.lane(l))
        .map(|l| l.offset.abs() + l.width * 0.5)
        .fold(0.0f32, f32::max);
    if half > 0.0 {
        2.0 * half
    } else {
        DEFAULT_WIDTH
    }
}

/// Distance along arm `i` (half-width `hi`) at which its side edge meets the edge of a
/// neighbour (half-width `hj`) `gap` radians away.
fn corner_distance(hi: f32, hj: f32, gap: f32) -> f32 {
    if gap >= FLAT_GAP_DEG.to_radians() {
        return hi;
    }
    // Near-parallel arms are only possible if the graph violates the 30° rule; keep it finite.
    let sin = gap.sin().max(0.2);
    ((hj + hi * gap.cos()) / sin).max(hi)
}

/// Computes the junction of `node` from the graph alone. `None` if the node does not exist.
pub fn compute_junction(graph: &NetGraph, node: NodeId, revision: u64) -> Option<Junction> {
    let n = graph.node(node)?;
    let c = to_xz(n.pos);

    struct Raw {
        segment: SegmentId,
        at_start: bool,
        dir: Vec2,
        angle: f32,
        width: f32,
        length: f32,
    }
    let mut raws: Vec<Raw> = Vec::new();
    for &sid in &n.segments {
        let seg = graph.segment(sid).expect("node links existing segments");
        for (at_start, hit) in [(true, seg.start == node), (false, seg.end == node)] {
            if !hit {
                continue;
            }
            let t = if at_start {
                seg.curve.tangent(0.0)
            } else {
                -seg.curve.tangent(1.0)
            };
            let dir = to_xz(t).try_normalize().unwrap_or(Vec2::X);
            raws.push(Raw {
                segment: sid,
                at_start,
                dir,
                angle: dir.y.atan2(dir.x),
                width: segment_width(graph, sid),
                length: seg.length,
            });
        }
    }
    raws.sort_by(|a, b| {
        a.angle
            .total_cmp(&b.angle)
            .then(a.segment.cmp(&b.segment))
            .then(b.at_start.cmp(&a.at_start))
    });

    let k = raws.len();
    let mut arms = Vec::with_capacity(k);
    for (i, r) in raws.iter().enumerate() {
        let h = r.width * 0.5;
        let mut s = h;
        if k >= 2 {
            let next = &raws[(i + 1) % k];
            let prev = &raws[(i + k - 1) % k];
            let ccw_gap = (next.angle - r.angle).rem_euclid(std::f32::consts::TAU);
            let cw_gap = (r.angle - prev.angle).rem_euclid(std::f32::consts::TAU);
            // With two arms both gaps are measured to the same neighbour.
            let gap = |g: f32| if g == 0.0 { std::f32::consts::TAU } else { g };
            s = s
                .max(corner_distance(h, next.width * 0.5, gap(ccw_gap)))
                .max(corner_distance(h, prev.width * 0.5, gap(cw_gap)));
        }
        s = s.min(MAX_SETBACK).min(r.length * MAX_SETBACK_FRACTION);
        let seg = graph.segment(r.segment).expect("exists");
        let trim_t = {
            let tab = seg.curve.arc_table();
            let along = if r.at_start { s } else { tab.length() - s };
            tab.s_to_t(along)
        };
        let l = rot_ccw(r.dir);
        arms.push(Arm {
            segment: r.segment,
            at_start: r.at_start,
            angle: r.angle,
            width: r.width,
            setback: s,
            trim_t,
            right: c + r.dir * s - l * h,
            left: c + r.dir * s + l * h,
        });
    }

    let polygon = match k {
        0 => Vec::new(),
        1 => {
            let a = &arms[0];
            let (d, h) = (raws[0].dir, a.width * 0.5);
            let (s, l) = (a.setback, rot_ccw(d));
            vec![
                c + d * s - l * h,
                c + d * s + l * h,
                c - d * s + l * h,
                c - d * s - l * h,
            ]
        }
        _ => {
            let mut pts: Vec<Vec2> = Vec::new();
            for i in 0..k {
                let (a, b) = (&arms[i], &arms[(i + 1) % k]);
                pts.push(a.right);
                pts.push(a.left);
                let gap = (b.angle - a.angle).rem_euclid(std::f32::consts::TAU);
                let gap = if gap == 0.0 {
                    std::f32::consts::TAU
                } else {
                    gap
                };
                if gap > OUTER_GAP_DEG.to_radians() {
                    // Meet of a's left edge line and b's right edge line.
                    let (da, db) = (raws[i].dir, raws[(i + 1) % k].dir);
                    let cross = da.x * db.y - da.y * db.x;
                    if cross.abs() > 1e-3 {
                        let diff = b.right - a.left;
                        let t = (diff.x * db.y - diff.y * db.x) / cross;
                        pts.push(a.left + da * t);
                    }
                }
            }
            let mut out: Vec<Vec2> = Vec::with_capacity(pts.len());
            for p in pts {
                if out.last().is_none_or(|q| q.distance(p) > 1e-3) {
                    out.push(p);
                }
            }
            while out.len() > 1 && out[0].distance(out[out.len() - 1]) <= 1e-3 {
                out.pop();
            }
            out
        }
    };
    let links = compute_links(graph, &arms);
    Some(Junction {
        node,
        pos: n.pos,
        arms,
        polygon,
        links,
        control: ControlPlan::default(),
        revision,
    })
}

/// The `Network`-phase system: recomputes dirty nodes (draining the graph's dirty set),
/// nodes that have no geometry yet (after a load), and drops junctions of removed nodes.
/// Does nothing — and bumps no version — when everything is up to date.
pub fn update_junctions(
    mut graph: ResMut<NetGraph>,
    mut geo: ResMut<JunctionGeometry>,
    content: Option<Res<SimContent>>,
) {
    let complete = geo.junctions.len() == graph.node_count();
    if graph.dirty_nodes().next().is_none() && complete {
        return;
    }
    let rank_of = |road: RoadDefId| {
        content.as_ref().map_or(0, |c| {
            if road.index() < c.0.roads().count() {
                c.0.road(road).rank
            } else {
                0
            }
        })
    };
    // Geometry, auto-installed lights, then the control plan, for one node.
    let build = |graph: &mut NetGraph, id: NodeId, version: u64| {
        let mut j = compute_junction(graph, id, version)?;
        if let Some(c) = auto_control(graph, &j) {
            graph.set_auto_control(id, c);
        }
        j.control = plan_control(graph, &j, &rank_of);
        Some(j)
    };
    let dirty = graph.take_dirty();
    let version = geo.version + 1;
    let map = Arc::make_mut(&mut geo.junctions);
    for id in dirty {
        match build(&mut graph, id, version) {
            Some(j) => {
                map.insert(id, j);
            }
            None => {
                map.remove(&id);
            }
        }
    }
    let missing: Vec<NodeId> = graph
        .nodes()
        .map(|(id, _)| id)
        .filter(|id| !map.contains_key(id))
        .collect();
    for id in missing {
        let j = build(&mut graph, id, version).expect("node exists");
        map.insert(id, j);
    }
    map.retain(|id, _| graph.node(*id).is_some());
    geo.version = version;
}

#[cfg(test)]
mod tests;
