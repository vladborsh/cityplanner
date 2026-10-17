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

/// The segment the front goes on into at the exit node, when it does.
fn continuation(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
    fwd: bool,
    side: Side,
) -> Option<(SegmentId, bool)> {
    let (seg, ours) = zoned(net, roads, id)?;
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
        let s = (a - self.a).clamp(0.0, self.len());
        let s = if self.fwd { s } else { self.len() - s };
        let t = self.table.s_to_t(s);
        let tg = to_xz(self.seg.curve.tangent(t))
            .try_normalize()
            .unwrap_or(Vec2::X);
        (
            to_xz(self.seg.curve.point(t)),
            if self.fwd { tg } else { -tg },
        )
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
        let own: BTreeSet<SegmentId> = f.segs.iter().map(|(s, _)| *s).collect();
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
    for j in 0..=n {
        let a = origin + j as f32 * pitch;
        let k = locate(a);
        let (p, u) = parts[k].at(a);
        let s = (a - parts[k].a).clamp(0.0, parts[k].len());
        let arc = if parts[k].fwd { s } else { parts[k].len() - s };
        let mark = Mark {
            axis: p,
            v: side_vec(u, f.side),
            major: j % 8 == 0,
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
