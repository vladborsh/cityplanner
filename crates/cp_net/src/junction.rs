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
//! **Class** (T-263, §6.8.3 item 6): one arm is an `End`; two arms of one road running on in
//! the same direction (or of a road that reads the same both ways) are a `Middle` node when
//! they turn at most [`MIDDLE_DEG`], else a `Bend`; two arms of different roads are a
//! `Transition`; three or more a `Junction` (see [`NodeClass`]).
//!
//! **Setbacks** come from the arms' real edge curves (the segment curve offset by the half
//! width), not their tangents: where arm `i`'s edge meets its neighbour's facing edge
//! (sampled, then refined by Newton steps, `shape::edge_meet`), for neighbours under `150°`
//! apart. A junction arm wants `max(hᵢ, meets)` (for straight arms the meet is
//! `t = (hⱼ + hᵢ·cos θ) / sin θ`, as before); a bend's arms are cut exactly where their inner
//! edges meet, so both ends share that corner; a middle node's at the node (0); at a transition
//! the wider road also keeps [`TRANSITION_LEN`] for its taper; a dead end `hᵢ`. Capped at
//! [`MAX_SETBACK`]. The two ends of a segment share `2 × 45 %` of its length: when their wants
//! fit, both are kept; otherwise a hard end keeps its want and a soft one gives way (see
//! `shared_setback`), so trimmed segments never overlap each other. A node's setbacks therefore
//! also depend on the nodes at the other ends of its segments: recomputing a node recomputes
//! those too.
//!
//! **Corners** lie on the real curve at the cut, square to it (`trim_t` and the corners agree
//! exactly). A middle node's two arms share one end line through the node on the bisector,
//! mitred to keep the road's width.
//!
//! **Polygon** (the node's outline): for each arm in counter-clockwise order, its right corner,
//! its left corner, then the line across the node to the next arm's right corner
//! ([`side_line`]: a curb curve at a junction's convex corner, a mitre on the inside of a bend,
//! a round join around the node on the outside of a bend or along a flat side). Points on a
//! straight run are dropped. Empty for a middle node.
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
pub use shape::{ArmPath, MIDDLE_DEG, NodeClass, SideLine, TRANSITION_LEN, side_line};

/// Longest setback, metres.
pub const MAX_SETBACK: f32 = 60.0;
/// Fraction of a segment's length one end may consume.
pub const MAX_SETBACK_FRACTION: f32 = 0.45;
/// Least setback a soft end (dead end, straight join) keeps when it gives way, metres.
pub const SOFT_SETBACK: f32 = 0.5;
/// Corners of two neighbouring arms closer than this are one point, metres.
const CORNER_SNAP: f32 = 0.01;
/// Width used for a segment without lanes, metres.
pub const DEFAULT_WIDTH: f32 = 8.0;
/// Angular gaps of at least this (degrees) do not form a road corner.
pub const FLAT_GAP_DEG: f32 = 150.0;

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
    /// What the node is to the renderer (T-263).
    pub class: NodeClass,
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

/// A footpath (T-222): every lane of the segment is a `Pedestrian` or (a bike path, T-322) a
/// `Bike` one.
pub fn is_path_segment(graph: &NetGraph, segment: SegmentId) -> bool {
    let Some(seg) = graph.segment(segment) else {
        return false;
    };
    let mut lanes = seg.lanes.iter().filter_map(|&l| graph.lane(l)).peekable();
    lanes.peek().is_some()
        && lanes.all(|l| {
            matches!(
                l.kind,
                cp_content::LaneKind::Pedestrian | cp_content::LaneKind::Bike
            )
        })
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

/// One segment end at a node, before setbacks.
struct Raw {
    segment: SegmentId,
    at_start: bool,
    dir: Vec2,
    angle: f32,
    width: f32,
    length: f32,
    path: bool,
}

/// The node's centre (XZ) and its segment ends sorted counter-clockwise, split into
/// `(footpaths, roads)`. T-222: a footpath that meets a road is no carriageway. It does not
/// shape the junction polygon or the setbacks of the roads; its own end stops at the road's
/// edge.
fn raw_arms(graph: &NetGraph, node: NodeId) -> Option<(Vec2, Vec<Raw>, Vec<Raw>)> {
    let n = graph.node(node)?;
    let c = to_xz(n.pos);
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
                path: is_path_segment(graph, sid),
            });
        }
    }
    raws.sort_by(|a, b| {
        a.angle
            .total_cmp(&b.angle)
            .then(a.segment.cmp(&b.segment))
            .then(b.at_start.cmp(&a.at_start))
    });
    let (footpaths, raws) = if raws.iter().any(|r| !r.path) {
        raws.into_iter().partition(|r| r.path)
    } else {
        (Vec::new(), raws)
    };
    Some((c, footpaths, raws))
}

/// The setback of one end of a segment of `length` that wants `me` while its other end wants
/// `other` (`(setback, hard)` each). The two ends share [`MAX_SETBACK_FRACTION`] of the length
/// each, `2 ×` it together: when both wants fit they are kept; otherwise a hard end keeps its
/// want and a soft one gives way down to [`SOFT_SETBACK`] (BUG-065: a junction a few metres
/// from an auto-cut joint kept only 45 % of the short piece, so its corners did not meet and the
/// sidewalks tore). Both ends compute the same split, so trimmed segments never overlap.
fn shared_setback(me: (f32, bool), other: (f32, bool), length: f32) -> f32 {
    let budget = 2.0 * MAX_SETBACK_FRACTION * length;
    if me.0 + other.0 <= budget {
        return me.0;
    }
    let floor = |(s, hard): (f32, bool)| if hard { s } else { s.min(SOFT_SETBACK) };
    let (fm, fo) = (floor(me), floor(other));
    if fm + fo >= budget {
        return if fm + fo > 0.0 {
            budget * fm / (fm + fo)
        } else {
            budget * 0.5
        };
    }
    let (em, eo) = (me.0 - fm, other.0 - fo);
    let extra = budget - fm - fo;
    if em + eo > 0.0 {
        fm + extra * em / (em + eo)
    } else {
        fm
    }
}

/// What the other end of the segment of road arm `r` wants, `(0, soft)` when that
/// end has no road arm for it.
fn wanted_at_other_end(graph: &NetGraph, r: &Raw) -> (f32, bool) {
    let Some(seg) = graph.segment(r.segment) else {
        return (0.0, false);
    };
    let other = if r.at_start { seg.end } else { seg.start };
    let Some(pos) = graph.node(other).map(|n| n.pos) else {
        return (0.0, false);
    };
    raw_arms(graph, other)
        .and_then(|(_, _, raws)| {
            let (_, wants) = shape::class_and_wants(graph, pos, &raws);
            raws.iter()
                .zip(wants)
                .find(|(o, _)| o.segment == r.segment && o.at_start != r.at_start)
                .map(|(_, w)| w)
        })
        .unwrap_or((0.0, false))
}

/// Computes the junction of `node` from the graph alone. `None` if the node does not exist.
pub fn compute_junction(graph: &NetGraph, node: NodeId, revision: u64) -> Option<Junction> {
    let n = graph.node(node)?;
    let (c, mut footpaths, raws) = raw_arms(graph, node)?;
    let edge = raws.iter().map(|r| r.width * 0.5).fold(0.0f32, f32::max);
    let k = raws.len();
    let (class, wants) = shape::class_and_wants(graph, n.pos, &raws);
    let mut arms = Vec::with_capacity(k);
    // Corners relative to the node centre: the meeting corners of two neighbouring arms are
    // snapped together here, before the (large) map position is added, so they are the very
    // same point instead of a few float steps apart (BUG-065: the near-duplicates stayed apart
    // in the polygon and the corner pavement was not found).
    let mut local: Vec<(Vec2, Vec2)> = Vec::with_capacity(k);
    for (i, r) in raws.iter().enumerate() {
        let h = r.width * 0.5;
        let s = shared_setback(wants[i], wanted_at_other_end(graph, r), r.length);
        let seg = graph.segment(r.segment).expect("exists");
        let trim_t = {
            let tab = seg.curve.arc_table();
            let along = if r.at_start { s } else { tab.length() - s };
            tab.s_to_t(along)
        };
        // The corners lie on the real curve at the cut, square to it (T-263).
        let at = to_xz(seg.curve.point(trim_t)) - c;
        let t = to_xz(seg.curve.tangent(trim_t));
        let out = if r.at_start { t } else { -t }
            .try_normalize()
            .unwrap_or(r.dir);
        let l = rot_ccw(out);
        local.push((at - l * h, at + l * h));
        arms.push(Arm {
            segment: r.segment,
            at_start: r.at_start,
            angle: r.angle,
            width: r.width,
            setback: s,
            trim_t,
            right: Vec2::ZERO,
            left: Vec2::ZERO,
        });
    }
    if class == NodeClass::Middle {
        // One end line through the node, on the bisector, mitred to keep the road's width.
        let (na, nb) = (rot_ccw(raws[0].dir), rot_ccw(raws[1].dir));
        let m = (na - nb).try_normalize().unwrap_or(na);
        let reach = raws[0].width * 0.5 / m.dot(na).max(0.5);
        local[0] = (-m * reach, m * reach);
        local[1] = (m * reach, -m * reach);
    }
    if k >= 2 {
        for i in 0..k {
            let j = (i + 1) % k;
            if local[i].1.distance(local[j].0) < CORNER_SNAP {
                local[j].0 = local[i].1;
            }
        }
    }
    for (a, (right, left)) in arms.iter_mut().zip(&local) {
        a.right = c + *right;
        a.left = c + *left;
    }

    let mut j = Junction {
        node,
        pos: n.pos,
        class,
        arms,
        polygon: Vec::new(),
        links: Links::default(),
        control: ControlPlan::default(),
        revision,
    };
    j.polygon = match (class, k) {
        (_, 0) | (NodeClass::Middle, _) => Vec::new(),
        (_, 1) => {
            let a = &j.arms[0];
            let (d, h) = (raws[0].dir, a.width * 0.5);
            let (s, l) = (a.setback, rot_ccw(d));
            vec![a.right, a.left, c - d * s + l * h, c - d * s - l * h]
        }
        _ => {
            let round = class == NodeClass::Junction;
            let mut pts: Vec<Vec2> = Vec::new();
            for i in 0..k {
                let (a, b) = (&j.arms[i], &j.arms[(i + 1) % k]);
                pts.push(a.right);
                pts.push(a.left);
                let line =
                    shape::side_line(graph, &j, a, b, a.width * 0.5, b.width * 0.5, round).points();
                if line.len() > 2 {
                    pts.extend_from_slice(&line[1..line.len() - 1]);
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
            // Points on a straight run between their neighbours, also across the closing point.
            let mut s = shape::simplify(out);
            let between = |a: Vec2, b: Vec2, c: Vec2| {
                (b - a).perp_dot((c - a).normalize_or_zero()).abs() < 1e-4
                    && (b - a).dot(c - b) > 0.0
            };
            while s.len() > 3 && between(s[s.len() - 2], s[s.len() - 1], s[0]) {
                s.pop();
            }
            while s.len() > 3 && between(s[s.len() - 1], s[0], s[1]) {
                s.remove(0);
            }
            s
        }
    };
    let Junction {
        mut arms, polygon, ..
    } = j;
    // The polygon's arms; the footpaths join `arms` (for their trim and the pedestrian links).
    footpaths.sort_by(|a, b| a.angle.total_cmp(&b.angle).then(a.segment.cmp(&b.segment)));
    for r in &footpaths {
        let h = r.width * 0.5;
        let s = edge.min(r.length * MAX_SETBACK_FRACTION);
        let seg = graph.segment(r.segment).expect("exists");
        let tab = seg.curve.arc_table();
        let along = if r.at_start { s } else { tab.length() - s };
        let l = rot_ccw(r.dir);
        arms.push(Arm {
            segment: r.segment,
            at_start: r.at_start,
            angle: r.angle,
            width: r.width,
            setback: s,
            trim_t: tab.s_to_t(along),
            right: c + r.dir * s - l * h,
            left: c + r.dir * s + l * h,
        });
    }
    arms.sort_by(|a, b| {
        a.angle
            .total_cmp(&b.angle)
            .then(a.segment.cmp(&b.segment))
            .then(b.at_start.cmp(&a.at_start))
    });
    let links = compute_links(graph, &arms, &n.lane_rules);
    Some(Junction {
        node,
        pos: n.pos,
        class,
        arms,
        polygon,
        links,
        control: ControlPlan::default(),
        revision,
    })
}

/// `dirty` plus the nodes at the other ends of their segments (their setbacks share those
/// segments, see `shared_setback`), in id order.
fn with_neighbours(graph: &NetGraph, dirty: impl IntoIterator<Item = NodeId>) -> Vec<NodeId> {
    let mut out = std::collections::BTreeSet::new();
    for id in dirty {
        out.insert(id);
        if let Some(n) = graph.node(id) {
            for seg in n.segments.iter().filter_map(|&s| graph.segment(s)) {
                out.insert(seg.start);
                out.insert(seg.end);
            }
        }
    }
    out.into_iter().collect()
}

/// Brings the geometry of every dirty or missing node up to date **without** a tick, for the
/// renderer: while the game is paused commands still change the graph but the `Network` phase
/// ([`update_junctions`]) does not run, so new roads would be drawn untrimmed and without a
/// junction polygon (BUG-023/024).
///
/// Pure with respect to the sim state: it reads the graph and writes only `geo` (never saved,
/// never hashed). The graph's dirty set is left intact, no light is installed and no control
/// plan is made (a recomputed junction keeps its previous plan), so the next real tick
/// recomputes the same nodes exactly as if this had never run. Returns whether `geo` changed.
pub fn refresh_geometry(graph: &NetGraph, geo: &mut JunctionGeometry) -> bool {
    let stale = with_neighbours(
        graph,
        graph
            .nodes()
            .map(|(id, _)| id)
            .filter(|&id| graph.is_dirty(id) || !geo.junctions.contains_key(&id)),
    );
    let gone = geo.junctions.keys().any(|&id| graph.node(id).is_none());
    if stale.is_empty() && !gone {
        return false;
    }
    let version = geo.version + 1;
    let map = Arc::make_mut(&mut geo.junctions);
    for id in stale {
        match compute_junction(graph, id, version) {
            Some(mut j) => {
                if let Some(old) = map.get(&id) {
                    j.control = old.control.clone();
                }
                map.insert(id, j);
            }
            None => {
                map.remove(&id);
            }
        }
    }
    map.retain(|id, _| graph.node(*id).is_some());
    geo.version = version;
    true
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
    let dirty = with_neighbours(&graph, dirty);
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

mod shape;

#[cfg(test)]
mod tests;
