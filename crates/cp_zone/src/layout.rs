//! Zone layout v3 (`architecture/system-design.md` §6.8.3 item 4, T-265): the cells are a pure
//! function of the network, laid like Cities: Skylines lays them, in fixed 8 m cells. A
//! *frontage* is one side of a street between the cross streets of that side: a chain of
//! segments of one width through nodes where the street runs on (within `CORNER_DEG`) and, at a
//! T, along its through side. Its cells are 8 x 8 m, never stretched, [`crate::BLOCK_DEPTH`] rows
//! deep, counted from the edge of a cross street on this side (the frontage's start, else its
//! end: §6.8.3 item 4 (в)), else from its start node; a rest under 8 m at the other end gets no
//! cell. A frontage runs in the direction of
//! its oldest segment (smallest `Segment::order`), so a demolished and rebuilt street gets the
//! same cells as a fresh one.

use std::collections::BTreeSet;

use bevy_math::Vec2;
use cp_content::RoadDefId;
use cp_core::curve::{ArcTable, CubicBezier3, to_xz};
use cp_net::{CORNER_DEG, Elevation, NetGraph, Segment, SegmentId};

use crate::grid::{CELL_SIZE, RoadZoning, Side};

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

/// A column boundary of a frontage (a *cell line*): where a cell ends and the next begins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    /// On the street axis.
    pub axis: Vec2,
    /// Unit vector from the axis to the cells.
    pub v: Vec2,
    /// Unit vector along the street in the direction the frontage counts its cells.
    pub onward: Vec2,
    /// Every 8th boundary from the start of the frontage: where the near edge of a cross street
    /// leaves a block of 8 cells (the road tool's tick).
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

/// A rounded corner of a block (§6.8.3 item 4, "Уточнение", T-265): a curved segment that turns
/// the street by at least this much end to end...
pub const ROUNDED_CORNER_DEG: f32 = 30.0;
/// ...is no longer than this (metres)...
pub const ROUNDED_CORNER_MAX_M: f32 = 80.0;

/// The turn of a segment end to end, radians.
fn turn(seg: &Segment) -> f32 {
    let a = to_xz(seg.curve.tangent(0.0)).try_normalize();
    let b = to_xz(seg.curve.tangent(1.0)).try_normalize();
    match (a, b) {
        (Some(a), Some(b)) => a.dot(b).clamp(-1.0, 1.0).acos(),
        _ => 0.0,
    }
}

/// The segment that runs straight on from `id` at its exit node (within `CORNER_DEG`).
fn straight_on(net: &NetGraph, id: SegmentId, fwd: bool) -> Option<&Segment> {
    straight_on_id(net, id, fwd).and_then(|(s, _)| net.segment(s))
}

/// [`straight_on`] with the segment's id and whether it is walked along its own direction.
fn straight_on_id(net: &NetGraph, id: SegmentId, fwd: bool) -> Option<(SegmentId, bool)> {
    let seg = net.segment(id)?;
    let (heading, node) = exit_of(seg, fwd);
    let n = net.node(node)?;
    let cos = CORNER_DEG.to_radians().cos();
    n.segments
        .iter()
        .filter(|&&s| s != id)
        .filter_map(|&s| {
            net.segment(s)
                .map(|g| (s, g.start == node, leave_dir(g, g.start == node)))
        })
        .filter(|(_, _, d)| d.dot(heading) >= cos)
        .max_by(|a, b| a.2.dot(heading).total_cmp(&b.2.dot(heading)))
        .map(|(s, at_start, _)| (s, at_start))
}

/// How far from a rounded corner its virtual corner may lie, from either end of the bend, for
/// the legs to count from it (§6.8.3 item 4, ruling on T-266), metres.
pub const VIRTUAL_CORNER_MAX_M: f32 = 40.0;

/// The end of a straight leg at a rounded corner (§6.8.3 item 4, ruling on T-266): the cells
/// count from the edge of the other leg's line at the virtual corner (where the two legs' lines
/// meet), as at a sharp corner, when it lies within [`VIRTUAL_CORNER_MAX_M`] of both ends of the
/// bend; else from the leg's own node. Negative: the edge stands beyond the node, over the bend
/// (the cells the bend's road covers are dropped by the general validity rules). `None` when the
/// street does not run on into a rounded corner there.
fn rounded_corner_end(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
    fwd: bool,
) -> Option<FrontEnd> {
    let (seg, ours) = zoned(net, roads, id)?;
    let (u1, node) = exit_of(seg, fwd);
    let (bend, bend_fwd) = straight_on_id(net, id, fwd)?;
    if !rounded_corner(net, bend) {
        return None;
    }
    let b_seg = net.segment(bend)?;
    // The other leg's direction is the bend's own at its far end (the leg runs straight on from
    // it): a cut of that leg, which nudges its tangent by a float step, moves nothing here.
    let (u2, far) = exit_of(b_seg, bend_fwd);
    let (next, _) = straight_on_id(net, bend, bend_fwd)?;
    let (_, theirs) = zoned(net, roads, next)?;
    let (p1, p2) = (to_xz(net.node(node)?.pos), to_xz(net.node(far)?.pos));
    let c = u1.perp_dot(u2);
    if c.abs() < 1e-3 {
        return Some(FrontEnd::Open);
    }
    let dd = p2 - p1;
    // The virtual corner: `p1 + u1·s = p2 - u2·r`.
    let s = dd.perp_dot(u2) / c;
    let r = -dd.perp_dot(u1) / c;
    if s <= 0.0 || r <= 0.0 || s > VIRTUAL_CORNER_MAX_M || r > VIRTUAL_CORNER_MAX_M {
        return Some(FrontEnd::Open);
    }
    let (cos_t, sin_t) = ((-u1).dot(u2), c.abs());
    let d = (theirs.half_width + ours.half_width * cos_t) / sin_t;
    Some(FrontEnd::Edge(d - s))
}

/// Whether segment `id` is a rounded corner: it turns by [`ROUNDED_CORNER_DEG`] or more, is
/// no longer than [`ROUNDED_CORNER_MAX_M`] and runs on into a straight segment at both ends (a
/// piece of a long arc cut automatically is no corner: its neighbours are curved). A frontage
/// ends at its nodes; the curve is a frontage of its own, the legs count from its virtual corner
/// (see `rounded_corner_end`).
fn rounded_corner(net: &NetGraph, id: SegmentId) -> bool {
    let Some(seg) = net.segment(id) else {
        return false;
    };
    let straight = |g: &Segment| turn(g) < CORNER_DEG.to_radians();
    turn(seg) >= ROUNDED_CORNER_DEG.to_radians()
        && seg.length <= ROUNDED_CORNER_MAX_M
        && [true, false]
            .into_iter()
            .all(|fwd| straight_on(net, id, fwd).is_some_and(straight))
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
    if (theirs.half_width - ours.half_width).abs() > 1e-3
        || rounded_corner(net, id)
        || rounded_corner(net, straight.seg)
    {
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
    if let Some(e) = rounded_corner_end(net, roads, id, fwd) {
        return e;
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

/// The cross street a front ends at, at the exit node of `id` (the one [`end_of`] takes its
/// edge from; at a rounded corner the leg beyond the bend). `None` at a dead end or the open
/// side of a bend.
fn cross_seg(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
    fwd: bool,
    side: Side,
) -> Option<SegmentId> {
    let (seg, _) = zoned(net, roads, id)?;
    if let Some((bend, bend_fwd)) = straight_on_id(net, id, fwd)
        && rounded_corner(net, bend)
    {
        return match rounded_corner_end(net, roads, id, fwd)? {
            FrontEnd::Edge(_) => straight_on_id(net, bend, bend_fwd).map(|(s, _)| s),
            FrontEnd::Open => None,
        };
    }
    let (heading, node) = exit_of(seg, fwd);
    let (back, v) = (-heading, side_vec(heading, side));
    let mut best: Option<(f32, SegmentId)> = None;
    for a in arms_at(net, roads, node, id) {
        let theta = a.dir.dot(v).atan2(a.dir.dot(back));
        if theta > 1e-3 && theta <= EDGE_MAX_DEG.to_radians() && best.is_none_or(|(t, _)| theta < t)
        {
            best = Some((theta, a.seg));
        }
    }
    best.map(|(_, s)| s)
}

/// The key that orders the two ends of a block's frontages (§6.8.3 item 4 (г), BUG-089): the
/// build order of the cross street there, then the corner node's position in millimetres. The
/// two frontages facing each other across a block end at the same two cross streets, so both
/// pick the same end to count from.
fn end_key(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    id: SegmentId,
    fwd: bool,
    sides: [Side; 2],
) -> Option<(u32, i64, i64)> {
    let cross = sides
        .into_iter()
        .find_map(|side| cross_seg(net, roads, id, fwd, side))?;
    let (_, node) = exit_of(net.segment(id)?, fwd);
    let p = to_xz(net.node(node)?.pos);
    Some((
        net.segment(cross)?.order,
        (p.x * 1000.0).round() as i64,
        (p.y * 1000.0).round() as i64,
    ))
}

/// Whether the front of `id` ends at a dead end at its exit node (a node of one segment; an
/// outside connection is the edge of the map, not a dead end).
fn dead_end(net: &NetGraph, id: SegmentId, fwd: bool) -> bool {
    net.segment(id)
        .and_then(|s| net.node(exit_of(s, fwd).1))
        .is_some_and(|n| n.segments.len() == 1 && n.kind != cp_net::NodeKind::Outside)
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
    // Oriented by the oldest segment's own direction: the smallest build order (the pieces of
    // one drawn road share it and its direction), then the segment's start point, never the
    // arena slot a rebuilt segment may reuse.
    let key = |s: SegmentId| {
        net.segment(s).map(|g| {
            let p = to_xz(g.curve.p0);
            (
                g.order,
                (p.x * 1000.0).round() as i64,
                (p.y * 1000.0).round() as i64,
            )
        })
    };
    let oldest = f.segs.iter().min_by_key(|(s, _)| key(*s)).copied();
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
        // Past either end the front goes on along the straight line (a straddling cell, BUG-043).
        let beyond = raw - raw.clamp(0.0, self.len());
        (to_xz(self.seg.curve.point(t)) + heading * beyond, heading)
    }
}

/// How the two ends of `f` count: an open end (a dead end, the convex side of a corner) at a
/// node where the street's other side stops at a cross street's edge starts at that same
/// distance, so the cells and ticks of the two sides of a street stand opposite each other
/// (BUG-067; T-265 keeps this one rule: it moves an end, it fits nothing).
fn shared_ends(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    f: &Frontage,
) -> (FrontEnd, FrontEnd) {
    let (Some(&(first, first_fwd)), Some(&(last, last_fwd))) = (f.segs.first(), f.segs.last())
    else {
        return (f.start, f.end);
    };
    // Walking back from the first segment our side is `side.flip()`, the other side `side`.
    let start = match f.start {
        FrontEnd::Open => match end_of(net, roads, first, !first_fwd, f.side) {
            FrontEnd::Edge(d) => FrontEnd::Edge(d),
            FrontEnd::Open => FrontEnd::Open,
        },
        e => e,
    };
    let end = match f.end {
        FrontEnd::Open => match end_of(net, roads, last, last_fwd, f.side.flip()) {
            FrontEnd::Edge(d) => FrontEnd::Edge(d),
            FrontEnd::Open => FrontEnd::Open,
        },
        e => e,
    };
    (start, end)
}

/// The blocks of one frontage: 8 m columns from its start, as many as fit before its end.
pub(crate) fn layout_frontage(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
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
    let (mut start, mut end) = shared_ends(net, roads, f);
    let (Some(&(s_id, s_fwd)), Some(&(e_id, e_fwd))) = (f.segs.first(), f.segs.last()) else {
        return Vec::new();
    };
    // (д) An open end at a dead end keeps the room of a cross street of its own road: no cell in
    // its last half width, so a street of that road built there later moves nothing (BUG-089).
    let own = |id: SegmentId| net.segment(id).map_or(0.0, |s| roads(s.road).half_width);
    if start == FrontEnd::Open && dead_end(net, s_id, !s_fwd) {
        start = FrontEnd::Edge(own(s_id));
    }
    if end == FrontEnd::Open && dead_end(net, e_id, e_fwd) {
        end = FrontEnd::Edge(own(e_id));
    }
    let edge = |e: FrontEnd| match e {
        FrontEnd::Edge(d) => d,
        FrontEnd::Open => 0.0,
    };
    let (a0, a1) = (edge(start), run - edge(end));
    // A centimetre of slack: a block drawn to whole cells measures a hair under them.
    let n = (((a1 - a0) + 0.01) / CELL_SIZE).floor().max(0.0) as u32;
    if n == 0 {
        return Vec::new();
    }
    let n = n.min(u32::from(u16::MAX));
    // The cells count from the edge of a cross street (the corner of the block), else from the
    // start: from the end when only the end stands at a cross street, the rest under 8 m then
    // falls at the start. Between two cross streets (г): from the older one's (by build order,
    // then the corner's position), so the frontages facing each other across a block share one
    // lattice and the rest falls at the same end of both (BUG-089).
    let ks = end_key(net, roads, s_id, !s_fwd, [f.side.flip(), f.side]);
    let ke = end_key(net, roads, e_id, e_fwd, [f.side, f.side.flip()]);
    let from_end = match (ks, ke) {
        (Some(a), Some(b)) => b < a,
        (None, Some(_)) => matches!(end, FrontEnd::Edge(_)),
        _ => false,
    };
    let origin = if from_end {
        a1 - n as f32 * CELL_SIZE
    } else {
        a0
    };
    let locate = |a: f32| -> usize {
        parts
            .iter()
            .rposition(|p| p.a <= a + 1e-4)
            .unwrap_or(0)
            .min(parts.len() - 1)
    };
    let centre = |j: u32| origin + (j as f32 + 0.5) * CELL_SIZE;

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
            s0: local(first) - CELL_SIZE / 2.0,
            cols: cols.len() as u16,
            inner,
            cell_len: CELL_SIZE,
            row_depth: CELL_SIZE,
            marks: Vec::new(),
        });
    }
    for j in 0..=n {
        let a = origin + j as f32 * CELL_SIZE;
        let k = locate(a);
        let (p, u) = parts[k].at(a);
        let s = (a - parts[k].a).clamp(0.0, parts[k].len());
        let arc = if parts[k].fwd { s } else { parts[k].len() - s };
        let mark = Mark {
            axis: p,
            v: side_vec(u, f.side),
            onward: if from_end { -u } else { u },
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

/// A cell line of a frontage: the segment side it stands on and the column boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellLine {
    pub segment: SegmentId,
    pub side: Side,
    pub mark: Mark,
}

/// The columns each frontage through `segments` lays on each of those segments, as
/// `(segment, side, columns)` (side in the segment's own direction): the cells a road gets along
/// it, what the road tool's label counts (§6.8.3, the top rule, T-275).
pub fn columns(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    segments: &[SegmentId],
) -> Vec<(SegmentId, Side, u16)> {
    let mut seen: BTreeSet<(SegmentId, Side)> = BTreeSet::new();
    let mut out = Vec::new();
    for &id in segments {
        for side in [Side::Left, Side::Right] {
            if seen.contains(&(id, side)) {
                continue;
            }
            let Some(f) = frontage_of(net, roads, id, side) else {
                seen.insert((id, side));
                continue;
            };
            for i in 0..f.segs.len() {
                seen.insert((f.segs[i].0, f.block_side(i)));
            }
            out.extend(
                layout_frontage(net, roads, &f)
                    .into_iter()
                    .filter(|d| segments.contains(&d.segment))
                    .map(|d| (d.segment, d.side, d.cols)),
            );
        }
    }
    out
}

/// The cell lines of every frontage through `segments` in `net` (each frontage once, all of its
/// segments): every column boundary, its 8-cell lines `major` (§6.8.3 item 3, T-265). The one
/// source of "where the cells are" for the road tool's ticks, its ghost and its snaps (T-266).
pub fn cell_lines(
    net: &NetGraph,
    roads: &dyn Fn(RoadDefId) -> RoadZoning,
    segments: impl IntoIterator<Item = SegmentId>,
) -> Vec<CellLine> {
    let mut seen: BTreeSet<(SegmentId, Side)> = BTreeSet::new();
    let mut out = Vec::new();
    for id in segments {
        for side in [Side::Left, Side::Right] {
            if seen.contains(&(id, side)) {
                continue;
            }
            let Some(f) = frontage_of(net, roads, id, side) else {
                seen.insert((id, side));
                continue;
            };
            for i in 0..f.segs.len() {
                seen.insert((f.segs[i].0, f.block_side(i)));
            }
            for d in layout_frontage(net, roads, &f) {
                out.extend(d.marks.iter().map(|&mark| CellLine {
                    segment: d.segment,
                    side: d.side,
                    mark,
                }));
            }
        }
    }
    out
}
