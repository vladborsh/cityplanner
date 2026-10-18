//! Zone layout v2 (`architecture/system-design.md` §7.1): the cells are a pure function of the
//! network. A *frontage* is one side of a street between the cross streets of that side; its
//! cells form one lattice that runs from edge to edge of the cross streets, so a rectangular
//! block gets one grid whatever order its streets were built in.

use std::collections::BTreeSet;

use bevy_math::Vec2;
use cp_content::RoadDefId;
use cp_core::curve::{ArcTable, CubicBezier3, to_xz};
use cp_net::{CORNER_DEG, Elevation, NetGraph, Segment, SegmentId};

use crate::grid::{BLOCK_DEPTH, CELL_SIZE, RoadZoning, Side};
use crate::roads::{RoadIndex, inflate};

/// A cross road counts as the far side of a block only when it runs within this of parallel.
const PARALLEL_DEG: f32 = 15.0;
/// A chain is one straight line when no kink between segments exceeds this (degrees)...
const STRAIGHT_KINK_DEG: f32 = 0.25;
/// ...and no control point is farther than this from the chord (metres).
const STRAIGHT_CHORD_M: f32 = 0.5;
/// Sector angles above this are not a corner of a cross street but a way on (degrees).
const EDGE_MAX_DEG: f32 = 170.0;

/// How a frontage ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FrontEnd {
    /// Beside a cross street: the cells stop `d` metres from the node, at its edge.
    Edge(f32),
    /// A dead end or the convex side of a bend: the cells run to the node.
    Open,
}

/// One side of a street between its cross streets (§7.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Frontage {
    /// The segments in front order; `true`: walked along the segment's own direction.
    pub segs: Vec<(SegmentId, bool)>,
    /// The side of the front's direction (not of the segments').
    pub side: Side,
    pub start: FrontEnd,
    pub end: FrontEnd,
}

impl Frontage {
    /// The side of segment `i` in its own direction.
    pub fn block_side(&self, i: usize) -> Side {
        if self.segs[i].1 {
            self.side
        } else {
            self.side.flip()
        }
    }
}

/// The cells of one side of one segment, before they are blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockDraft {
    pub segment: SegmentId,
    pub side: Side,
    pub curve: CubicBezier3,
    pub length: f32,
    pub s0: f32,
    pub cols: u16,
    pub inner: f32,
    pub cell_len: f32,
    pub row_depth: f32,
    /// Column boundaries of the frontage that stand on this segment's stretch.
    pub marks: Vec<Mark>,
}

/// A column boundary of a frontage: where a cell ends and the next begins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    /// On the street axis.
    pub axis: Vec2,
    /// Unit vector from the axis to the cells.
    pub v: Vec2,
    /// Every 8th boundary from the start of the frontage.
    pub major: bool,
    /// The first or the last boundary of the frontage (a cross street's edge or an open end).
    pub end: bool,
    /// Arc position along the segment's own direction.
    pub arc: f32,
}

impl BlockDraft {
    /// Whether `b` has exactly these parameters (bit for bit).
    pub fn same_as(&self, b: &crate::grid::ZoneBlock) -> bool {
        let bits =
            |c: &CubicBezier3| [c.p0, c.p1, c.p2, c.p3].map(|p| [p.x, p.y, p.z].map(f32::to_bits));
        self.side == b.side
            && self.cols == b.cols
            && bits(&self.curve) == bits(&b.curve)
            && self.length.to_bits() == b.length.to_bits()
            && self.s0.to_bits() == b.s0.to_bits()
            && self.inner.to_bits() == b.inner.to_bits()
            && self.cell_len.to_bits() == b.cell_len.to_bits()
            && self.row_depth.to_bits() == b.row_depth.to_bits()
    }
}

/// `n` cells filling `x` metres: 8 when `x` is within `fit_m` of a closed block (64 m), else the
/// count that keeps the cell within a quarter of 8 m; failing that, standard cells and a rest.
pub fn fit_cells(x: f32, fit_m: f32) -> (u32, f32) {
    // The count is decided on `x` to the centimetre: the same distance measured along a street
    // and across a block differs in the last bits, and on a tie (100 m is 12.5 cells) the two
    // must not round apart, or the perpendicular frontages of a block get different lattices.
    let q = (x * 100.0).round() / 100.0;
    let n = if fit_m > 0.0 && (q - 8.0 * CELL_SIZE).abs() <= fit_m {
        8
    } else {
        (q / CELL_SIZE).round().max(0.0) as u32
    };
    if n >= 1 {
        let pitch = x / n as f32;
        if (0.75 * CELL_SIZE..=1.25 * CELL_SIZE).contains(&pitch) {
            return (n, pitch);
        }
    }
    ((q / CELL_SIZE).floor().max(0.0) as u32, CELL_SIZE)
}

/// How many cells away a facing road still fits the row depth (and the column pitch of the
/// perpendicular frontages): 24 cells, 192 m.
pub(crate) const FACING_CELLS: f32 = 24.0;

fn side_vec(heading: Vec2, side: Side) -> Vec2 {
    Vec2::new(heading.y, -heading.x) * side.sign()
}

fn leave_dir(seg: &Segment, at_start: bool) -> Vec2 {
    to_xz(if at_start {
        seg.curve.tangent(0.0)
    } else {
        -seg.curve.tangent(1.0)
    })
    .try_normalize()
    .unwrap_or(Vec2::X)
}

struct Arm {
    seg: SegmentId,
    /// The arm leaves the node at its segment's start.
    at_start: bool,
    dir: Vec2,
    half_width: f32,
}

fn arms_at(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    node: cp_net::NodeId,
    except: SegmentId,
) -> Vec<Arm> {
    let mut out = Vec::new();
    let Some(n) = net.node(node) else {
        return out;
    };
    for &sid in &n.segments {
        if sid == except {
            continue;
        }
        let Some(s) = net.segment(sid) else { continue };
        if s.elevation != Elevation::Ground || roads(s.road).footpath {
            continue;
        }
        for at_start in [true, false] {
            if (at_start && s.start == node) || (!at_start && s.end == node) {
                out.push(Arm {
                    seg: sid,
                    at_start,
                    dir: leave_dir(s, at_start),
                    half_width: roads(s.road).half_width,
                });
            }
        }
    }
    out
}

fn zoned<'a>(
    net: &'a NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
) -> Option<(&'a Segment, RoadZoning)> {
    let s = net.segment(id)?;
    let r = roads(s.road);
    (s.elevation == Elevation::Ground && r.allows_zoning && s.start != s.end && s.length > 0.0)
        .then_some((s, r))
}

/// Walking `seg` (`fwd`: along its direction) with `side` of the walk: the heading at the exit
/// node, the node and our half width.
fn exit_of(seg: &Segment, fwd: bool) -> (Vec2, cp_net::NodeId) {
    if fwd {
        (
            to_xz(seg.curve.tangent(1.0))
                .try_normalize()
                .unwrap_or(Vec2::X),
            seg.end,
        )
    } else {
        (
            to_xz(-seg.curve.tangent(0.0))
                .try_normalize()
                .unwrap_or(Vec2::X),
            seg.start,
        )
    }
}

/// The arm a street goes on along at the exit node of `id` whatever stands beside it (a T or
/// a crossing does not end a street), when there is one of our width.
fn through_arm(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
    fwd: bool,
) -> Option<(SegmentId, bool)> {
    let (seg, ours) = zoned(net, roads, id)?;
    let (heading, node) = exit_of(seg, fwd);
    let cos = CORNER_DEG.to_radians().cos();
    let arms = arms_at(net, roads, node, id);
    let straight = arms
        .iter()
        .filter(|a| a.dir.dot(heading) >= cos)
        .max_by(|a, b| a.dir.dot(heading).total_cmp(&b.dir.dot(heading)))?;
    let (_, theirs) = zoned(net, roads, straight.seg)?;
    ((theirs.half_width - ours.half_width).abs() <= 1e-3)
        .then_some((straight.seg, straight.at_start))
}

/// The segment the front goes on into at the exit node, when it does.
fn continuation(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
    fwd: bool,
    side: Side,
) -> Option<(SegmentId, bool)> {
    let (seg, ours) = zoned(net, roads, id)?;
    if tight_corner_edge(net, roads, id, fwd, side).is_some() {
        return None;
    }
    let (heading, node) = exit_of(seg, fwd);
    let arms = arms_at(net, roads, node, id);
    let cos = CORNER_DEG.to_radians().cos();
    let straight = arms
        .iter()
        .filter(|a| a.dir.dot(heading) >= cos)
        .max_by(|a, b| a.dir.dot(heading).total_cmp(&b.dir.dot(heading)))?;
    let (_, theirs) = zoned(net, roads, straight.seg)?;
    if (theirs.half_width - ours.half_width).abs() > 1e-3 {
        return None;
    }
    let v = side_vec(heading, side);
    let clear = arms
        .iter()
        .filter(|a| !(a.seg == straight.seg && a.at_start == straight.at_start))
        .all(|a| a.dir.dot(v) < 1e-3);
    clear.then_some((straight.seg, straight.at_start))
}

/// How the front ends at the exit node of `id` when it does not go on.
fn end_of(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
    fwd: bool,
    side: Side,
) -> FrontEnd {
    let Some((seg, ours)) = zoned(net, roads, id) else {
        return FrontEnd::Open;
    };
    if let Some(d) = tight_corner_edge(net, roads, id, fwd, side) {
        return FrontEnd::Edge(d);
    }
    let (heading, node) = exit_of(seg, fwd);
    let back = -heading;
    let v = side_vec(heading, side);
    let mut best: Option<(f32, f32)> = None;
    for a in arms_at(net, roads, node, id) {
        let theta = a.dir.dot(v).atan2(a.dir.dot(back));
        if theta > 1e-3 && theta <= EDGE_MAX_DEG.to_radians() && best.is_none_or(|(t, _)| theta < t)
        {
            best = Some((theta, a.half_width));
        }
    }
    match best {
        Some((theta, h2)) => {
            let d = (h2 + ours.half_width * theta.cos()) / theta.sin();
            FrontEnd::Edge(d.max(0.0))
        }
        None => FrontEnd::Open,
    }
}

/// Walks on from `id`: the segments passed (starting with `id`) and how the front ends.
fn extend(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
    fwd: bool,
    side: Side,
    seen: &mut BTreeSet<SegmentId>,
) -> (Vec<(SegmentId, bool)>, FrontEnd) {
    let mut out = vec![(id, fwd)];
    let (mut cur, mut cur_fwd) = (id, fwd);
    loop {
        match continuation(net, roads, cur, cur_fwd, side) {
            Some((next, nfwd)) if !seen.contains(&next) && next != id => {
                seen.insert(next);
                out.push((next, nfwd));
                (cur, cur_fwd) = (next, nfwd);
            }
            Some(_) => return (out, FrontEnd::Open),
            None => return (out, end_of(net, roads, cur, cur_fwd, side)),
        }
    }
}

/// The frontage of `side` (in the segment's own direction) of segment `id`; `None` when the
/// segment is not zoned.
pub fn frontage_of(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
    side: Side,
) -> Option<Frontage> {
    zoned(net, roads, id)?;
    if tight_inner(net, roads, id, side) {
        return None;
    }
    let mut seen = BTreeSet::from([id]);
    let (fwd_list, end) = extend(net, roads, id, true, side, &mut seen);
    let (back_list, start) = extend(net, roads, id, false, side.flip(), &mut seen);
    let mut segs: Vec<(SegmentId, bool)> = back_list
        .iter()
        .skip(1)
        .rev()
        .map(|&(s, f)| (s, !f))
        .collect();
    segs.extend(fwd_list);
    let mut f = Frontage {
        segs,
        side,
        start,
        end,
    };
    // Oriented by the oldest segment's own direction.
    let oldest = f.segs.iter().min_by_key(|(s, _)| *s).copied();
    if oldest.is_some_and(|(_, fwd)| !fwd) {
        f.segs = f.segs.iter().rev().map(|&(s, fwd)| (s, !fwd)).collect();
        f.side = f.side.flip();
        std::mem::swap(&mut f.start, &mut f.end);
    }
    Some(f)
}

/// All frontages of the net, in the order of their first segment.
pub fn frontages(net: &NetGraph, roads: &dyn Fn(RoadDefId) -> RoadZoning) -> Vec<Frontage> {
    let mut seen: BTreeSet<(SegmentId, Side)> = BTreeSet::new();
    let mut out = Vec::new();
    for (id, _) in net.segments() {
        for side in [Side::Left, Side::Right] {
            if seen.contains(&(id, side)) {
                continue;
            }
            let Some(f) = frontage_of(net, roads, id, side) else {
                continue;
            };
            for i in 0..f.segs.len() {
                seen.insert((f.segs[i].0, f.block_side(i)));
            }
            out.push(f);
        }
    }
    out
}

struct Part<'a> {
    seg: &'a Segment,
    table: ArcTable,
    fwd: bool,
    /// Arc position of the part's start on the front.
    a: f32,
}

impl Part<'_> {
    fn len(&self) -> f32 {
        self.table.length()
    }

    /// Point and heading (along the front) at front position `a`.
    fn at(&self, a: f32) -> (Vec2, Vec2) {
        let raw = a - self.a;
        let s = raw.clamp(0.0, self.len());
        let s = if self.fwd { s } else { self.len() - s };
        let t = self.table.s_to_t(s);
        let tg = to_xz(self.seg.curve.tangent(t))
            .try_normalize()
            .unwrap_or(Vec2::X);
        let heading = if self.fwd { tg } else { -tg };
        // Past either end the front goes on along the straight line (the cells that reach over
        // a rounded corner's bend, BUG-073b; a straddling cell, BUG-043).
        let beyond = raw - raw.clamp(0.0, self.len());
        (to_xz(self.seg.curve.point(t)) + heading * beyond, heading)
    }
}

fn is_straight(parts: &[Part]) -> bool {
    let kink = STRAIGHT_KINK_DEG.to_radians().cos();
    for p in parts {
        let c = &p.seg.curve;
        let (a, b) = (to_xz(c.p0), to_xz(c.p3));
        let chord = b - a;
        let l = chord.length();
        if l < 1e-3 {
            return false;
        }
        let n = Vec2::new(-chord.y, chord.x) / l;
        if [c.p1, c.p2]
            .iter()
            .any(|q| (to_xz(*q) - a).dot(n).abs() > STRAIGHT_CHORD_M)
        {
            return false;
        }
    }
    parts.windows(2).all(|w| {
        let (_, h0) = w[0].at(w[1].a);
        let (_, h1) = w[1].at(w[1].a);
        h0.dot(h1) >= kink
    })
}

/// How far from our axis, along `v` from `p`, the nearest near-parallel road edge is, or `None`.
fn facing_gap(
    idx: &RoadIndex,
    own: &BTreeSet<SegmentId>,
    p: Vec2,
    u: Vec2,
    v: Vec2,
    inner: f32,
    reach: f32,
) -> Option<f32> {
    let end = p + v * reach;
    let bounds = inflate((p.min(end), p.max(end)), 16.0);
    let cos_par = PARALLEL_DEG.to_radians().cos();
    let mut best: Option<f32> = None;
    for (sid, piece) in idx.pieces_in(bounds) {
        if own.contains(sid) || piece.footpath {
            continue;
        }
        let e = piece.b - piece.a;
        let el = e.length();
        if el < 1e-4 || (e / el).dot(u).abs() < cos_par {
            continue;
        }
        let denom = v.perp_dot(e);
        if denom.abs() < 1e-6 {
            continue;
        }
        let t = (piece.a - p).perp_dot(e) / denom;
        let s = (piece.a - p).perp_dot(v) / denom;
        if !(0.0..=1.0).contains(&s) || t <= 0.0 || t > reach {
            continue;
        }
        let cos_phi = denom.abs() / el;
        let gap = t - piece.half_width / cos_phi.max(0.2) - inner;
        if gap >= CELL_SIZE / 2.0 && best.is_none_or(|b| gap < b) {
            best = Some(gap);
        }
    }
    best
}

/// How the two ends of `f` count (BUG-067): a street without a cross street along it has one
/// start for both of its sides. When the other side of the same stretch of street ends at a
/// cross street (`Edge(d)`) where `f` runs to the node (`Open`, the convex side of a corner),
/// `f` starts at the same distance `d`, so the columns of the two sides stand opposite each
/// other and the ticks match.
fn shared_ends(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    f: &Frontage,
) -> (FrontEnd, FrontEnd) {
    let (mut start, mut end) = (f.start, f.end);
    if !(matches!(start, FrontEnd::Open) || matches!(end, FrontEnd::Open)) {
        return (start, end);
    }
    let Some(&(first, _)) = f.segs.first() else {
        return (start, end);
    };
    let Some(g) = frontage_of(net, roads, first, f.block_side(0).flip()) else {
        return (start, end);
    };
    let ids = |x: &Frontage| x.segs.iter().map(|(s, _)| *s).collect::<BTreeSet<_>>();
    if g.segs.len() != f.segs.len() || ids(&g) != ids(f) {
        return (start, end);
    }
    // Same stretch: the other side's ends in this frontage's direction.
    let (g_start, g_end) = if g.segs.first().map(|s| s.0) == f.segs.first().map(|s| s.0) {
        (g.start, g.end)
    } else {
        (g.end, g.start)
    };
    if let (FrontEnd::Open, FrontEnd::Edge(d)) = (start, g_start) {
        start = FrontEnd::Edge(d);
    }
    if let (FrontEnd::Open, FrontEnd::Edge(d)) = (end, g_end) {
        end = FrontEnd::Edge(d);
    }
    (start, end)
}

/// The blocks of one frontage: where its columns stand and how deep its rows are.
pub(crate) fn layout_frontage(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    idx: &RoadIndex,
    fit_m: f32,
    f: &Frontage,
) -> Vec<BlockDraft> {
    let mut parts: Vec<Part> = Vec::new();
    let mut run = 0.0;
    for &(id, fwd) in &f.segs {
        let Some(seg) = net.segment(id) else {
            return Vec::new();
        };
        let table = ArcTable::build(&seg.curve);
        let l = table.length();
        parts.push(Part {
            seg,
            table,
            fwd,
            a: run,
        });
        run += l;
    }
    let Some(first) = f.segs.first().and_then(|(s, _)| net.segment(*s)) else {
        return Vec::new();
    };
    let inner = roads(first.road).half_width;
    let total = run;
    let (f_start, f_end) = shared_ends(net, roads, f);
    let a0 = match f_start {
        FrontEnd::Edge(d) => d,
        FrontEnd::Open => 0.0,
    };
    let a1 = total
        - match f_end {
            FrontEnd::Edge(d) => d,
            FrontEnd::Open => 0.0,
        };
    let l = a1 - a0;
    if l < 1.0 {
        return Vec::new();
    }
    let straight = is_straight(&parts);
    let both_edges = matches!(f_start, FrontEnd::Edge(_)) && matches!(f_end, FrontEnd::Edge(_));
    // The row depth of the frontages facing a block is fitted to the gap only within `reach`
    // (see below); a block longer than that keeps the standard 8 m cell on both axes, or the
    // perpendicular frontages would not share one lattice (BUG-056).
    let reach = FACING_CELLS * CELL_SIZE + fit_m;
    let fits = l + 2.0 * inner <= reach;
    let (n, pitch, origin) = if straight && both_edges && fits {
        let (n, pitch) = fit_cells(l, fit_m);
        (n, pitch, a0)
    } else {
        let n = ((l + 0.01) / CELL_SIZE).floor() as u32;
        let origin = if matches!(f_start, FrontEnd::Open) && matches!(f_end, FrontEnd::Edge(_)) {
            a1 - n as f32 * CELL_SIZE
        } else {
            a0
        };
        (n, CELL_SIZE, origin)
    };
    if n == 0 {
        return Vec::new();
    }
    let n = n.min(u32::from(u16::MAX));
    let locate = |a: f32| -> usize {
        parts
            .iter()
            .rposition(|p| p.a <= a + 1e-4)
            .unwrap_or(0)
            .min(parts.len() - 1)
    };
    let centre = |j: u32| origin + (j as f32 + 0.5) * pitch;

    // Row depth: straight fronts fit it to the road facing them.
    let mut row_depth = CELL_SIZE;
    if straight {
        // Not facing roads: the street itself and the bends of rounded corners (BUG-073b: their
        // cells are the legs' lattice, a bite out of it where the arc passes).
        let mut own: BTreeSet<SegmentId> = f.segs.iter().map(|(s, _)| *s).collect();
        let (lo, hi) = parts.iter().fold(
            (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
            |(lo, hi), p| {
                let (a, b) = (to_xz(p.seg.curve.p0), to_xz(p.seg.curve.p3));
                (lo.min(a.min(b)), hi.max(a.max(b)))
            },
        );
        own.extend(
            idx.segments_near(inflate((lo, hi), reach + 16.0))
                .into_iter()
                .filter(|id| tight_bend(net, roads, *id)),
        );
        let mut gap: Option<f32> = None;
        for j in 0..n {
            let a = centre(j);
            let (p, u) = parts[locate(a)].at(a);
            let v = side_vec(u, f.side);
            if let Some(g) = facing_gap(idx, &own, p, u, v, inner, reach) {
                gap = Some(gap.map_or(g, |b| b.min(g)));
            }
        }
        if let Some(g) = gap {
            let free = g + 0.0;
            let (_, rd) = fit_cells(free, fit_m);
            row_depth = rd;
        }
    }
    let _ = BLOCK_DEPTH;

    let mut out = Vec::new();
    for (k, part) in parts.iter().enumerate() {
        let cols: Vec<u32> = (0..n).filter(|&j| locate(centre(j)) == k).collect();
        let (Some(&lo), Some(&hi)) = (cols.first(), cols.last()) else {
            continue;
        };
        // Local arc position of the column centre that comes first along the segment.
        let local = |j: u32| {
            let s = centre(j) - part.a;
            if part.fwd { s } else { part.len() - s }
        };
        let first = if part.fwd { lo } else { hi };
        out.push(BlockDraft {
            segment: f.segs[k].0,
            side: f.block_side(k),
            curve: part.seg.curve,
            length: part.len(),
            s0: local(first) - pitch / 2.0,
            cols: cols.len() as u16,
            inner,
            cell_len: pitch,
            row_depth,
            marks: Vec::new(),
        });
    }
    // Ticks count from the street EDGE of the block: from the far end only when that is the one
    // that stands at a cross street (BUG-073: the far open end of a street leaving a corner
    // must not set the phase).
    let from_end = matches!(f_start, FrontEnd::Open) && matches!(f_end, FrontEnd::Edge(_));
    for j in 0..=n {
        let a = origin + j as f32 * pitch;
        let k = locate(a);
        let (p, u) = parts[k].at(a);
        let s = (a - parts[k].a).clamp(0.0, parts[k].len());
        let arc = if parts[k].fwd { s } else { parts[k].len() - s };
        let mark = Mark {
            axis: p,
            v: side_vec(u, f.side),
            major: (if from_end { n - j } else { j }) % 8 == 0,
            end: j == 0 || j == n,
            arc,
        };
        if let Some(d) = out
            .iter_mut()
            .find(|d: &&mut BlockDraft| d.segment == f.segs[k].0)
        {
            d.marks.push(mark);
        }
    }
    out
}

/// A segment that turns the street: its end tangents differ by more than this (degrees).
const BEND_DEG: f32 = 10.0;

fn is_bend(net: &NetGraph, id: SegmentId) -> bool {
    net.segment(id).is_some_and(|s| {
        let a = to_xz(s.curve.tangent(0.0)).try_normalize();
        let b = to_xz(s.curve.tangent(1.0)).try_normalize();
        matches!((a, b), (Some(a), Some(b)) if a.dot(b) < BEND_DEG.to_radians().cos())
    })
}

/// A street that runs into a rounded bend and out of it into another street: where the two
/// straight pieces would meet is a corner (BUG-073), the cross street's edge `d` from it. The
/// result is the distance from the exit node of `(sid, fwd)` to that edge along the street
/// (negative: the edge lies on the bend, beyond the node).
fn bend_corner(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    sid: SegmentId,
    fwd: bool,
    tight: bool,
) -> Option<f32> {
    let (seg, ours) = zoned(net, roads, sid)?;
    let (u1, node) = exit_of(seg, fwd);
    let (bend, bend_fwd) = through_arm(net, roads, sid, fwd)?;
    if !is_bend(net, bend) {
        return None;
    }
    let (b_seg, _) = zoned(net, roads, bend)?;
    let (_, far) = exit_of(b_seg, bend_fwd);
    let (next, next_at_start) = through_arm(net, roads, bend, bend_fwd)?;
    if is_bend(net, next) {
        return None;
    }
    let u2 = leave_dir(net.segment(next)?, next_at_start);
    let p1 = to_xz(net.node(node)?.pos);
    let p2 = to_xz(net.node(far)?.pos);
    let c = u1.perp_dot(u2);
    if c.abs() < 1e-3 {
        return None;
    }
    let dd = p2 - p1;
    let s = dd.perp_dot(u2) / c;
    let r = -dd.perp_dot(u1) / c;
    if s <= 0.0 || r <= 0.0 {
        return None;
    }
    // A rounded corner of a block (BUG-073b): a real turn, tight and short; a long gentle
    // curve is a bend of the street, its cells follow it.
    if tight
        && (u1.dot(u2) > TIGHT_TURN_DEG.to_radians().cos()
            || s > TIGHT_REACH_M
            || r > TIGHT_REACH_M
            || b_seg.length > TIGHT_LENGTH_M)
    {
        return None;
    }
    let (cos_t, sin_t) = ((-u1).dot(u2), c.abs());
    let d = (ours.half_width * (1.0 + cos_t)) / sin_t;
    Some(d - s)
}

/// A bend counts as a rounded corner of a block when it turns by at least this (degrees)...
const TIGHT_TURN_DEG: f32 = 30.0;
/// ...its virtual corner is no farther than this from either end (metres)...
const TIGHT_REACH_M: f32 = 40.0;
/// ...and the bend itself is no longer than this.
const TIGHT_LENGTH_M: f32 = 80.0;

/// The inner side of a rounded corner (BUG-073b): the cells of the straight street into it are
/// the same lattice up to the virtual corner, whose edge stands `d` from the node (negative:
/// beyond it, over the bend); the cells the bend's road covers are dropped as invalid. The
/// distance from the exit node of `(sid, fwd)` when `side` is the side the street turns to.
fn tight_corner_edge(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    sid: SegmentId,
    fwd: bool,
    side: Side,
) -> Option<f32> {
    if is_bend(net, sid) {
        return None;
    }
    let (seg, _) = zoned(net, roads, sid)?;
    let (heading, _) = exit_of(seg, fwd);
    let (bend, bend_fwd) = through_arm(net, roads, sid, fwd)?;
    if !is_bend(net, bend) {
        return None;
    }
    let d = bend_corner(net, roads, sid, fwd, true)?;
    let (out, _) = exit_of(net.segment(bend)?, bend_fwd);
    (out.dot(side_vec(heading, side)) > 0.0).then_some(d)
}

/// Whether the bend `id` is the rounded corner of a block between two straight legs.
fn tight_bend(net: &NetGraph, roads: &dyn Fn(RoadDefId) -> RoadZoning, id: SegmentId) -> bool {
    is_bend(net, id)
        && through_arm(net, roads, id, false)
            .is_some_and(|a| bend_corner(net, roads, a.0, !a.1, true).is_some())
}

/// Whether `side` of the bend `id` (in its own direction) is the inner side of a rounded corner
/// whose two legs lay the cells instead.
fn tight_inner(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
    side: Side,
) -> bool {
    if !is_bend(net, id) {
        return false;
    }
    let (Some(a), Some(b)) = (
        through_arm(net, roads, id, false),
        through_arm(net, roads, id, true),
    ) else {
        return false;
    };
    // The leg at the start walks into the bend along it; the one at the end walks into it
    // against it, and sees the same side the other way round.
    tight_corner_edge(net, roads, a.0, !a.1, side).is_some()
        && tight_corner_edge(net, roads, b.0, !b.1, side.flip()).is_some()
}

/// A stretch of a street between two junctions (or its ends), where its ticks are counted.
#[derive(Clone, Copy, Debug)]
struct Interval {
    /// Chain arc positions of its two ends (the nodes).
    lo: f32,
    hi: f32,
    /// Distance from the end node to the cross street's edge, where there is one.
    d_lo: Option<f32>,
    d_hi: Option<f32>,
}

impl Interval {
    /// The ticks are counted from the far end only when that is the one at a cross street.
    fn counts_from_end(&self) -> bool {
        self.d_lo.is_none() && self.d_hi.is_some()
    }
}

/// The street a segment belongs to, oriented, cut into the stretches between its junctions.
struct Street {
    chain: Vec<(SegmentId, bool)>,
    /// The arc position of the start of each chain element, and the total length.
    starts: Vec<f32>,
    total: f32,
    intervals: Vec<Interval>,
}

fn street_of(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
) -> Option<Street> {
    zoned(net, roads, id)?;
    let mut seen = BTreeSet::from([id]);
    let mut walk = |first: (SegmentId, bool)| {
        let mut out = vec![first];
        let mut cur = first;
        while let Some(next) = through_arm(net, roads, cur.0, cur.1) {
            // A bend ends the street: the straight pieces on either side are streets of their
            // own that meet at the corner the bend rounds off (BUG-073).
            if is_bend(net, next.0) || is_bend(net, cur.0) || !seen.insert(next.0) {
                break;
            }
            out.push(next);
            cur = next;
        }
        out
    };
    let fwd_list = walk((id, true));
    let back_list = walk((id, false));
    let mut chain: Vec<(SegmentId, bool)> = back_list
        .iter()
        .skip(1)
        .rev()
        .map(|&(s, f)| (s, !f))
        .collect();
    chain.extend(fwd_list);
    // Oriented by the oldest segment's own direction, like the frontages.
    if chain
        .iter()
        .min_by_key(|(s, _)| *s)
        .is_some_and(|(_, fwd)| !fwd)
    {
        chain = chain.iter().rev().map(|&(s, f)| (s, !f)).collect();
    }
    let mut starts = Vec::new();
    let mut total = 0.0;
    for &(sid, _) in &chain {
        starts.push(total);
        total += ArcTable::build(&net.segment(sid)?.curve).length();
    }
    // The edge of the cross streets at the exit node of a walk: on either side.
    let corner = |sid: SegmentId, fwd: bool| -> Option<f32> {
        [Side::Left, Side::Right]
            .into_iter()
            .filter_map(|side| match end_of(net, roads, sid, fwd, side) {
                FrontEnd::Edge(d) => Some(d),
                FrontEnd::Open => None,
            })
            .min_by(f32::total_cmp)
            .or_else(|| bend_corner(net, roads, sid, fwd, false))
    };
    let mut cuts: Vec<(f32, Option<f32>, Option<f32>)> = Vec::new();
    // (arc position of the node, offset before it, offset after it)
    cuts.push((0.0, None, corner(chain[0].0, !chain[0].1)));
    for i in 0..chain.len() - 1 {
        let before = corner(chain[i].0, chain[i].1);
        let after = corner(chain[i + 1].0, !chain[i + 1].1);
        if before.is_some() || after.is_some() {
            cuts.push((starts[i + 1], before, after));
        }
    }
    let last = *chain.last()?;
    cuts.push((total, corner(last.0, last.1), None));
    let intervals = cuts
        .windows(2)
        .map(|w| Interval {
            lo: w[0].0,
            hi: w[1].0,
            d_lo: w[0].2,
            d_hi: w[1].1,
        })
        .collect();
    Some(Street {
        chain,
        starts,
        total,
        intervals,
    })
}

/// Whether the ticks of the street of `id` are counted along that segment's own direction.
pub fn ticks_count_forward(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
) -> bool {
    let Some(st) = street_of(net, roads, id) else {
        return true;
    };
    let Some(i) = st.chain.iter().position(|(s, _)| *s == id) else {
        return true;
    };
    let mid = st.starts[i] + 0.01;
    let from_end = st
        .intervals
        .iter()
        .find(|iv| iv.lo <= mid && mid <= iv.hi)
        .is_some_and(Interval::counts_from_end);
    st.chain[i].1 != from_end
}

/// A tick of a street (BUG-073): the centreline of the next cross street, one series per street
/// shown on both of its sides at the same station.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreetTick {
    /// The segment the tick stands on and its arc position along that segment's own direction.
    pub seg: SegmentId,
    pub arc: f32,
    /// On the street axis (where a cross street's centreline would cross it).
    pub axis: Vec2,
    /// Unit direction of the street there, along the segment's own direction.
    pub dir: Vec2,
    /// Unit direction along the street away from its origin (the way the ticks are counted).
    pub onward: Vec2,
    /// 1, 2, ...: the cross street this tick is the centreline of.
    pub k: u32,
}

/// The ticks of the street `id` belongs to (the segments that run on through its junctions),
/// for a cross street of width `width`. Station of the k-th tick from the origin (the street's
/// start corner, i.e. the near edge of its cross street; the street's start node when it has none):
/// `k*64 + (k-1)*width + width/2`, so a street drawn from it leaves exactly 8 cells between its
/// edge and the previous street's edge and lands on the opposite street's tick.
pub fn street_ticks(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
    width: f32,
) -> Vec<StreetTick> {
    let Some(st) = street_of(net, roads, id) else {
        return Vec::new();
    };
    let mut tables: Vec<(&Segment, ArcTable)> = Vec::new();
    for &(sid, _) in &st.chain {
        let Some(seg) = net.segment(sid) else {
            return Vec::new();
        };
        tables.push((seg, ArcTable::build(&seg.curve)));
    }
    let mut out = Vec::new();
    for iv in &st.intervals {
        let (a_lo, a_hi) = (
            iv.lo + iv.d_lo.unwrap_or(0.0),
            iv.hi - iv.d_hi.unwrap_or(0.0),
        );
        let from_end = iv.counts_from_end();
        for k in 1..=4096u32 {
            let station = k as f32 * 64.0 + (k - 1) as f32 * width + width / 2.0;
            if station + width / 2.0 > a_hi - a_lo + 0.01 {
                break;
            }
            let a = if from_end {
                a_hi - station
            } else {
                a_lo + station
            };
            let Some(i) = st.starts.iter().rposition(|&b| b <= a + 1e-4) else {
                continue;
            };
            let (seg, table) = &tables[i];
            let fwd = st.chain[i].1;
            let s = (a - st.starts[i]).clamp(0.0, table.length());
            let local = if fwd { s } else { table.length() - s };
            let t = table.s_to_t(local);
            let tangent = to_xz(seg.curve.tangent(t))
                .try_normalize()
                .unwrap_or(Vec2::X);
            let along = if fwd { tangent } else { -tangent };
            out.push(StreetTick {
                seg: st.chain[i].0,
                arc: local,
                axis: to_xz(seg.curve.point(t)),
                dir: tangent,
                onward: if from_end { -along } else { along },
                k,
            });
        }
    }
    let _ = st.total;
    out
}
