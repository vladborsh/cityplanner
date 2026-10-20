//! The cell candidate of the road tool's resolver (`architecture/system-design.md` §6.8,
//! T-275 spec v2, T-278): the player builds by whole cells, roads are whole cells wide, and a
//! street's end lands a whole number of cells from the centre it starts at, so every block is
//! whole cells by construction. The sections (10 cells, [`SECTION_M`]) are only guides now: the
//! ticks along the ghost ([`cell_ticks`]) and weak pulls. Pure: the same net, the same answer.
//!
//! Per vertex of the path (`alt`, snaps off, switches all of it off):
//! 1. **Join**: a cursor within a junction's pull ([`node_pull`]: 1 cell, 2 when a 4-cell road
//!    meets there) goes to its centre; a point on a street goes to the street's whole cell nearest
//!    it ([`street_cell`]: whole cells from the nearer end of its stretch), or to its section mark
//!    within [`WEAK_M`]. The end of a leg on a street goes where the leg's square direction meets
//!    it (BUG-088).
//! 2. **Direction**: within 7° of a reference direction (the street it leaves, the leg before)
//!    or its square, exactly that.
//! 3. **Length**: whole cells from the start (the 10/20/30 marks are whole cells of the same
//!    frame), or a line of a street (its extension, or the square through one of its nodes) within
//!    [`WEAK_M`].
//!
//! A curve (T-268, [`quantize_curve`]) starts like a street and leaves in the direction of its
//! bend click, snapped like a leg (rule 2), at any distance. Its end goes to a junction or a
//! street's whole cell (rule 1), else to the guides within [`WEAK_M`] of the cursor (a whole
//! section from the start along or across its direction, the extension of a street, the square
//! through a node; two of them meet in a point), else to the nearest whole cell from the start.
//! Its end direction snaps like a leg (the start's direction, the street or junction it ends on,
//! or their squares), and the bend then moves to where the two directions meet: both ends leave
//! exactly so.

use std::sync::{Arc, Mutex};

use bevy_math::{Vec2, Vec3};
use cp_content::ContentDb;
use cp_core::curve::{CubicBezier3, to_xz};
use cp_net::build::{GUIDE_HALF_LENGTH, GUIDE_RADIUS, ZONE_CELL};
use cp_net::{
    Elevation, NetGraph, NodeId, RoadPath, RoadPathRequest, SegmentId, SnapReport, SnapReportKind,
};
use cp_sim::SimView;
use cp_zone::{CELL_SIZE, CellLine};

/// How far (metres, along the ray) the cursor may be from the perpendicular through an 8-cell
/// line for the length to jump onto it.
pub const CONTROL_LINE_TOL: f32 = 4.0;
/// How far along a street (metres) a joined point may move to a cell line: the soft snap.
pub const JOIN_M: f32 = 6.0;
/// How far (metres) the cursor may be from the 8-cell block line to snap onto it.
pub const BLOCK_SNAP_M: f32 = 3.0;
/// Cells between the edges of two parallel streets in a block (4 rows from each).
pub const BLOCK_CELLS: u32 = 8;

/// A cell line of the zoning along a street, as the tool uses it: the perpendicular to `dir`
/// through `pos` is a candidate for the length step and the joins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlMark {
    /// On the street axis.
    pub pos: Vec2,
    /// Unit direction of the street there.
    pub dir: Vec2,
    /// Metres from the start of the cells, for the label.
    pub along: f32,
    /// Every 8th column boundary: the length step jumps only to these; a join goes to the
    /// nearest boundary of any kind.
    pub major: bool,
    /// Unit vector along the street in the direction the cells are counted.
    pub onward: Vec2,
}

/// A cell line with the street it stands on.
#[derive(Clone, Copy, Debug)]
pub struct StreetLine {
    pub line: CellLine,
    /// Half width of the street.
    pub hw: f32,
    /// Unit direction of the street at the line (its own direction).
    pub dir: Vec2,
}

/// The cache holds the net snapshot itself (not its address): a dropped net's address can be
/// reused by the next one.
type LineCache = Option<(Arc<NetGraph>, Option<Arc<ContentDb>>, Arc<Vec<StreetLine>>)>;
static LINES: Mutex<LineCache> = Mutex::new(None);

/// Every cell line of the net of `view` (the layout is a pure function of the net: cached per
/// net snapshot, not per zone revision, so buildings growing do not lay it again).
pub fn all_lines(view: &SimView) -> Arc<Vec<StreetLine>> {
    if let Some((net, content, lines)) = LINES.lock().expect("cell line cache").as_ref()
        && Arc::ptr_eq(net, &view.net)
        && match (content, &view.content) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    {
        return Arc::clone(lines);
    }
    let lines = Arc::new(match view.content.as_deref() {
        Some(content) => {
            let roads = cp_zone::road_zoning(content);
            let ids = view.net.segments().map(|(id, _)| id);
            cp_zone::cell_lines(&view.net, &roads, ids)
                .into_iter()
                .filter_map(|line| {
                    let s = view.net.segment(line.segment)?;
                    let t = s.curve.arc_table().s_to_t(line.mark.arc);
                    let dir = to_xz(s.curve.tangent(t)).try_normalize()?;
                    Some(StreetLine {
                        line,
                        hw: content.road(s.road).width * 0.5,
                        dir,
                    })
                })
                .collect()
        }
        None => Vec::new(),
    });
    *LINES.lock().expect("cell line cache") = Some((
        Arc::clone(&view.net),
        view.content.clone(),
        Arc::clone(&lines),
    ));
    lines
}

/// The cell lines within `radius` of any of `points`, as marks; `major` only for an 8-cell line
/// that is not the end of its frontage (a tick: where a cross street's near edge leaves 8 cells).
pub fn marks_near(view: &SimView, points: &[Vec2], radius: f32) -> Vec<ControlMark> {
    all_lines(view)
        .iter()
        .filter(|l| {
            points
                .iter()
                .any(|p| p.distance(l.line.mark.axis) <= radius)
        })
        .map(|l| {
            let m = l.line.mark;
            ControlMark {
                pos: m.axis,
                dir: l.dir,
                along: 0.0,
                major: m.major && !m.end,
                onward: m.onward,
            }
        })
        .collect()
}

/// What [`quantize_path`] reads.
#[derive(Clone, Copy)]
pub struct LatticeCtx<'a> {
    pub graph: &'a NetGraph,
    pub content: &'a ContentDb,
    /// Snaps off: no join and no length step (the direction snaps stay).
    pub alt: bool,
    /// The cell lines of the streets near the path.
    pub marks: &'a [ControlMark],
    /// Half the width of the road being drawn: a street joined to a line is placed so that its
    /// near edge stands on it, its axis this much further on (BUG-067).
    pub edge_offset: f32,
    /// The bend of a curve is being placed (T-268): the last leg keeps its direction snap but
    /// not its length step or joins, the bend may stand anywhere along it.
    pub bend: bool,
}

/// Where a joined vertex landed, with what the next leg takes as direction references.
struct Joined {
    pos: Vec2,
    node: Option<NodeId>,
    /// For a point of a street: the directions along it there.
    along: Option<Vec2>,
    /// The street the vertex stands on (a point of a street).
    seg: Option<SegmentId>,
}

fn dir_xz(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.z).normalize_or_zero()
}

/// The direction of the street `c` at `t`: the chord of a straight one (exact to its ends, where
/// the tangent of the cubic is off by its rounding: 9 mm over a 480 m street), else the tangent.
fn street_dir(c: &CubicBezier3, t: f32) -> Vec2 {
    if is_straight(c) {
        (to_xz(c.p3) - to_xz(c.p0)).normalize_or_zero()
    } else {
        dir_xz(c.tangent(t))
    }
}

/// The junction pull of node `n` (T-275 spec v2, item 8; T-278): a cursor within 1 cell of its
/// centre goes to it, within 2 cells when a 4-cell road meets there.
pub fn node_pull(graph: &NetGraph, content: &ContentDb, n: NodeId) -> f32 {
    let wide = graph.node(n).is_some_and(|node| {
        node.segments.iter().any(|&s| {
            graph
                .segment(s)
                .is_some_and(|s| content.road(s.road).width >= 4.0 * ZONE_CELL - 1e-3)
        })
    });
    if wide { 2.0 * ZONE_CELL } else { ZONE_CELL }
}

/// The section marks of the stretch of `seg` (T-275 spec v2, item 5): every [`SECTION_M`] from
/// either end of the stretch (a junction, a dead end or a corner), where a cross street's centre
/// stands; each as `(arc length from the stretch's anchor, point, direction of the street)`.
pub fn section_marks(graph: &NetGraph, seg: SegmentId) -> Vec<(f32, Vec2, Vec2)> {
    let Some(st) = cp_net::stretch(graph, seg) else {
        return Vec::new();
    };
    let mut at: Vec<f32> = Vec::new();
    let mut k = SECTION_M;
    while k < st.length - 1.0 {
        for a in [k, st.length - k] {
            if !at.iter().any(|b| (b - a).abs() < 0.5) {
                at.push(a);
            }
        }
        k += SECTION_M;
    }
    at.sort_by(f32::total_cmp);
    at.into_iter()
        .filter_map(|a| {
            let (sid, s_on) = cp_net::unlocate(graph, &st, a)?;
            let sg = graph.segment(sid)?;
            let t = sg.curve.arc_table().s_to_t(s_on);
            Some((a, to_xz(sg.curve.point(t)), street_dir(&sg.curve, t)))
        })
        .collect()
}

/// The whole cell of the street `seg` nearest the point `s` metres along it (from its start)
/// (T-278): whole cells from the nearer end of its stretch, kept a cell clear of both ends, unless
/// a section mark ([`section_marks`]) is within [`WEAK_M`]. The point and the street's direction
/// there; `None` on a stretch shorter than two cells.
pub fn street_cell(graph: &NetGraph, seg: SegmentId, s: f32) -> Option<(Vec2, Vec2)> {
    let st = cp_net::stretch(graph, seg)?;
    let along = cp_net::locate(graph, seg, s)?;
    if st.length < 2.0 * ZONE_CELL {
        return None;
    }
    let mark = section_marks(graph, seg)
        .into_iter()
        .filter(|m| (m.0 - along).abs() <= WEAK_M)
        .min_by(|a, b| (a.0 - along).abs().total_cmp(&(b.0 - along).abs()));
    if let Some((_, p, d)) = mark {
        return Some((p, d));
    }
    let cells = |k: f32| (k / ZONE_CELL).round() * ZONE_CELL;
    let at = if along <= st.length / 2.0 {
        cells(along)
    } else {
        st.length - cells(st.length - along)
    }
    .clamp(ZONE_CELL, st.length - ZONE_CELL);
    let (sid, s_on) = cp_net::unlocate(graph, &st, at)?;
    let sg = graph.segment(sid)?;
    let t = sg.curve.arc_table().s_to_t(s_on);
    Some((to_xz(sg.curve.point(t)), street_dir(&sg.curve, t)))
}

/// Rule 1 for the point `p`: the centre of a junction within its pull, else the whole cell of
/// the street under it.
fn join(cx: &LatticeCtx, p: Vec2) -> Option<Joined> {
    if let Some((id, n)) = cx
        .graph
        .nodes()
        .filter(|(id, n)| n.elevation == Elevation::Ground && !cp_net::is_plain_cut(cx.graph, *id))
        .filter(|(id, n)| to_xz(n.pos).distance(p) <= node_pull(cx.graph, cx.content, *id))
        .min_by(|a, b| {
            to_xz(a.1.pos)
                .distance(p)
                .total_cmp(&to_xz(b.1.pos).distance(p))
        })
    {
        return Some(Joined {
            pos: to_xz(n.pos),
            node: Some(id),
            along: None,
            seg: None,
        });
    }
    let mut best: Option<(f32, SegmentId, f32)> = None;
    for (id, s) in cx.graph.segments() {
        if s.elevation != Elevation::Ground {
            continue;
        }
        let reach = cx.content.road(s.road).width * 0.5 + ZONE_CELL;
        if s.curve.bounds_distance_xz(p) > reach + 1e-3 {
            continue;
        }
        let (t, d) = s.curve.nearest_xz(p);
        if d <= reach && best.is_none_or(|(bd, ..)| d < bd) {
            best = Some((d, id, t));
        }
    }
    let (_, seg, t) = best?;
    let s = cx.graph.segment(seg)?;
    let at = s.curve.arc_table().t_to_s(t);
    let (pos, along) = street_cell(cx.graph, seg, at)
        .unwrap_or((to_xz(s.curve.point(t)), street_dir(&s.curve, t)));
    Some(Joined {
        pos,
        node: None,
        along: Some(along),
        seg: Some(seg),
    })
}

/// Rule 1 with rule 2 for the end of a leg from `from` that joins a street (BUG-088): the point
/// where a snapped direction from `from` (a key angle or the 15° step from the references) meets
/// the street, nearest the cursor `p` within the street's reach, key angles first. `None` when no
/// such direction meets the street there.
fn square_on_street(
    cx: &LatticeCtx,
    seg: SegmentId,
    from: Vec2,
    refs: &[Vec2],
    p: Vec2,
) -> Option<(Vec2, Vec2)> {
    let s = cx.graph.segment(seg)?;
    let reach = cx.content.road(s.road).width * 0.5 + ZONE_CELL;
    // The whole stretch: a direction may meet the street at a plain cut, the end of `seg`.
    let segs = cp_net::stretch(cx.graph, seg).map_or(vec![seg], |st| st.segments);
    let v3 = |q: Vec2| Vec3::new(q.x, 0.0, q.y);
    let mut best: Option<(f32, Vec2, Vec2)> = None;
    for r in refs {
        for k in 0..4 {
            let (sin, cos) = (k as f32 * 90.0).to_radians().sin_cos();
            let dir = Vec2::new(r.x * cos - r.y * sin, r.x * sin + r.y * cos);
            let ray = CubicBezier3::line(v3(from), v3(from + dir * RAY_M));
            for sg in segs.iter().filter_map(|id| cx.graph.segment(*id)) {
                for (_, ts) in ray.intersect_xz(&sg.curve) {
                    let q = to_xz(sg.curve.point(ts));
                    let d = q.distance(p);
                    if q.distance(from) < ZONE_CELL || d > reach {
                        continue;
                    }
                    if best.is_none_or(|(bd, ..)| d < bd) {
                        best = Some((d, q, street_dir(&sg.curve, ts)));
                    }
                }
            }
        }
    }
    best.map(|(_, q, t)| (q, t))
}

/// How far a snapped direction is followed to meet a street (metres).
const RAY_M: f32 = 2000.0;

/// Directions leaving the node, as the angle snap takes them: those of straight streets first
/// (exact, where a curve's tangent is only as exact as its control points).
fn node_dirs(graph: &NetGraph, n: NodeId) -> Vec<Vec2> {
    let Some(node) = graph.node(n) else {
        return Vec::new();
    };
    let mut dirs: Vec<(bool, Vec2)> = node
        .segments
        .iter()
        .filter_map(|sid| graph.segment(*sid))
        .map(|s| {
            let d = if s.start == n {
                street_dir(&s.curve, 0.0)
            } else {
                -street_dir(&s.curve, 1.0)
            };
            (!is_straight(&s.curve), d)
        })
        .collect();
    dirs.sort_by_key(|d| d.0);
    dirs.into_iter().map(|d| d.1).collect()
}

/// Whether the curve is a straight line (its controls on its chord).
fn is_straight(c: &CubicBezier3) -> bool {
    let (s0, s3) = (to_xz(c.p0), to_xz(c.p3));
    let u = (s3 - s0).normalize_or_zero();
    [c.p1, c.p2]
        .iter()
        .all(|p| (to_xz(*p) - s0).perp_dot(u).abs() < 0.01)
}

/// One leg from `prev` to the cursor `p`: rule 2, then rule 3 (unless `alt`).
fn leg(
    cx: &LatticeCtx,
    snap_angle: bool,
    prev: Vec2,
    refs: &[Vec2],
    p: Vec2,
) -> (Vec2, SnapReport) {
    let mut report = SnapReport::default();
    let d = p - prev;
    let len = d.length();
    if len < 1e-3 {
        return (p, report);
    }
    let mut dir = d / len;
    // Rule 2: within SQUARE_DEG of a reference or its square, exactly that (spec v2, item 7).
    if snap_angle && let Some((c, square)) = square_dir(refs, dir) {
        dir = exact_dir(cx, prev, c);
        report.kind = SnapReportKind::KeyAngle(if square { 90 } else { 0 });
        report.guide = Some((prev, prev + dir * GUIDE_HALF_LENGTH * 2.0));
    }
    let along = d.dot(dir);
    if cx.alt || cx.bend {
        return (prev + dir * along, report);
    }
    // Rule 3: whole cells from the start (T-278; a 10/20/30 mark is one of them), or a line of a
    // street (its extension or the square through one of its nodes, item 6) within WEAK_M.
    let mut at = ((along / ZONE_CELL).round() * ZONE_CELL).max(ZONE_CELL);
    let mut kind = SnapReportKind::CellStep(at as i32);
    let cursor = prev + dir * along;
    let mut best_line: Option<(f32, f32)> = None;
    let mut consider = |u: f32| {
        let miss = (u - along).abs();
        if u >= ZONE_CELL && miss <= WEAK_M && best_line.is_none_or(|(m, _)| miss < m) {
            best_line = Some((miss, u));
        }
    };
    for (id, n) in cx.graph.nodes() {
        let q = to_xz(n.pos);
        if n.elevation != Elevation::Ground
            || cp_net::is_plain_cut(cx.graph, id)
            || q.distance(cursor) > SECTION_M
            || q.distance(prev) < 1e-3
        {
            continue;
        }
        // The square through the node (it stands beside the leg, not on it).
        if (q - prev).perp_dot(dir).abs() >= ZONE_CELL {
            consider((q - prev).dot(dir));
        }
    }
    for (_, sg) in cx.graph.segments() {
        if sg.elevation != Elevation::Ground || sg.curve.bounds_distance_xz(cursor) > SECTION_M {
            continue;
        }
        // The street's axis line (straight streets), where the leg crosses it.
        let (a, b) = (to_xz(sg.curve.p0), to_xz(sg.curve.p3));
        let u = (b - a).normalize_or_zero();
        let den = dir.perp_dot(u);
        if u == Vec2::ZERO || den.abs() < 0.2 {
            continue;
        }
        consider((a - prev).perp_dot(u) / den);
    }
    if let Some((_, u)) = best_line {
        at = u;
        kind = SnapReportKind::Guide {
            perpendicular: true,
        };
    }
    (
        prev + dir * at,
        SnapReport {
            kind,
            guide: report.guide,
        },
    )
}

/// Rule 2 for the unit direction `dir`: the reference or square of one within [`SQUARE_DEG`]
/// nearest it, and whether it is a square of a reference.
fn square_dir(refs: &[Vec2], dir: Vec2) -> Option<(Vec2, bool)> {
    let cos = SQUARE_DEG.to_radians().cos();
    let cands: Vec<Vec2> = refs
        .iter()
        .flat_map(|r| [*r, Vec2::new(-r.y, r.x), -*r, Vec2::new(r.y, -r.x)])
        .filter(|c| c.length() > 0.5)
        .map(|c| c.normalize())
        .collect();
    let nearest = cands
        .iter()
        .copied()
        .max_by(|a, b| a.dot(dir).total_cmp(&b.dot(dir)))
        .filter(|c| c.dot(dir) >= cos)?;
    // Of references that are the same direction but for rounding, the first.
    let best = cands
        .into_iter()
        .find(|c| same_dir(*c, nearest))
        .unwrap_or(nearest);
    Some((best, refs.iter().any(|r| r.dot(best).abs() < 0.5)))
}

/// A guide line for the end of a curve: through `at` along the unit `dir`.
#[derive(Clone, Copy, Debug)]
struct Line {
    at: Vec2,
    dir: Vec2,
}

impl Line {
    /// How far `p` is from the line.
    fn miss(&self, p: Vec2) -> f32 {
        (p - self.at).perp_dot(self.dir).abs()
    }

    /// Where two lines meet; `None` when they are near parallel.
    fn meet(&self, o: &Line) -> Option<Vec2> {
        let den = self.dir.perp_dot(o.dir);
        if den.abs() < 0.2 {
            return None;
        }
        Some(self.at + self.dir * ((o.at - self.at).perp_dot(o.dir) / den))
    }

    fn foot(&self, p: Vec2) -> Vec2 {
        self.at + self.dir * (p - self.at).dot(self.dir)
    }
}

/// The guides near the end `p` of a curve from `a` leaving along `u` (T-268, user decision
/// 2026-10-20): the lines a whole section from `a` along and across `u`, the extension of every
/// straight ground street within a section of `p`, and the squares and extensions through its
/// junctions and dead ends there.
fn curve_guides(cx: &LatticeCtx, a: Vec2, u: Vec2, p: Vec2) -> Vec<Line> {
    let w = Vec2::new(-u.y, u.x);
    let mut out = Vec::new();
    let (pu, pw) = ((p - a).dot(u), (p - a).dot(w));
    for (dir, along, k) in [(w, u, pu), (u, w, pw)] {
        let n = (k / SECTION_M).round();
        for m in [n - 1.0, n, n + 1.0] {
            if m != 0.0 {
                out.push(Line {
                    at: a + along * (m * SECTION_M),
                    dir,
                });
            }
        }
    }
    for (_, sg) in cx.graph.segments() {
        if sg.elevation != Elevation::Ground || sg.curve.bounds_distance_xz(p) > SECTION_M {
            continue;
        }
        let (s0, s3) = (to_xz(sg.curve.p0), to_xz(sg.curve.p3));
        if is_straight(&sg.curve)
            && let Some(dir) = (s3 - s0).try_normalize()
        {
            out.push(Line { at: s0, dir });
        }
    }
    for (id, n) in cx.graph.nodes() {
        let q = to_xz(n.pos);
        if n.elevation != Elevation::Ground
            || cp_net::is_plain_cut(cx.graph, id)
            || q.distance(p) > SECTION_M
            || q.distance(a) < 1e-3
        {
            continue;
        }
        for d in node_dirs(cx.graph, id) {
            out.push(Line { at: q, dir: d });
            out.push(Line {
                at: q,
                dir: Vec2::new(-d.y, d.x),
            });
        }
    }
    out
}

/// Where the end `p` of a curve from `a` leaving along `u` lands when it joins nothing (T-268):
/// where the two guides nearest it within [`WEAK_M`] meet, else on the nearest guide at the
/// nearest whole cell from `a` along it, else at the nearest whole cell from `a` (along and across
/// `u`). With the directions of the guides it landed on.
fn curve_end(cx: &LatticeCtx, a: Vec2, u: Vec2, p: Vec2) -> (Vec2, Vec<Vec2>, SnapReportKind) {
    let w = Vec2::new(-u.y, u.x);
    let cell = |p: Vec2| {
        let (k, l) = ((p - a).dot(u), (p - a).dot(w));
        a + u * ((k / ZONE_CELL).round() * ZONE_CELL) + w * ((l / ZONE_CELL).round() * ZONE_CELL)
    };
    let mut near: Vec<(f32, Line)> = curve_guides(cx, a, u, p)
        .into_iter()
        .map(|l| (l.miss(p), l))
        .filter(|(m, _)| *m <= WEAK_M)
        .collect();
    near.sort_by(|x, y| x.0.total_cmp(&y.0));
    let guide = SnapReportKind::Guide {
        perpendicular: true,
    };
    let Some(&(_, first)) = near.first() else {
        return (cell(p), Vec::new(), SnapReportKind::CellStep(0));
    };
    let second = near
        .iter()
        .filter_map(|(_, l)| Some((first.meet(l)?, l.dir)))
        .find(|(q, _)| q.distance(p) <= WEAK_M * 1.5);
    if let Some((q, d)) = second {
        return (q, vec![first.dir, d], guide);
    }
    // On the one guide, at the whole cell from `a` along it nearest the cursor.
    let foot = first.foot(p);
    let k = (foot - a).dot(first.dir);
    let base = foot - first.dir * k;
    let q = base + first.dir * ((k / ZONE_CELL).round() * ZONE_CELL);
    (q, vec![first.dir], guide)
}

/// Applies the curve rules of the module doc to the curve `from → control → to` (T-268).
fn quantize_curve(
    cx: &LatticeCtx,
    joins: bool,
    angle: bool,
    from: Vec2,
    control: Vec2,
    to: Vec2,
) -> (RoadPath, SnapReport) {
    let mut report = SnapReport::default();
    let start = if joins { join(cx, from) } else { None };
    let a = start.as_ref().map_or(from, |j| j.pos);
    let mut refs: Vec<Vec2> = match &start {
        Some(Joined { node: Some(n), .. }) => node_dirs(cx.graph, *n),
        Some(Joined { along: Some(t), .. }) => vec![*t],
        _ => Vec::new(),
    };
    if refs.is_empty() {
        refs.push(Vec2::X);
    }
    let Some(mut u) = (control - a).try_normalize() else {
        return (
            RoadPath::Curve {
                from: a,
                control,
                to,
            },
            report,
        );
    };
    if angle && let Some((c, _)) = square_dir(&refs, u) {
        u = exact_dir(cx, a, c);
    }
    let mut c = a + u * (control - a).dot(u).max(0.0);
    let mut end_refs = vec![u];
    let b = if cx.alt {
        to
    } else if let Some(j) = join(cx, to).filter(|_| joins) {
        report.kind = if j.node.is_some() {
            SnapReportKind::Node
        } else {
            SnapReportKind::Segment
        };
        match (j.node, j.along) {
            (Some(n), _) => end_refs.extend(node_dirs(cx.graph, n)),
            (_, Some(t)) => end_refs.push(t),
            _ => {}
        }
        j.pos
    } else {
        let (q, dirs, kind) = curve_end(cx, a, u, to);
        report.kind = kind;
        end_refs.extend(dirs);
        q
    };
    // The end direction: snapped, the bend moves to where it meets the start's direction.
    if angle
        && let Some(e) = (b - c).try_normalize()
        && let Some((e, _)) = square_dir(&end_refs, e)
    {
        let e = exact_dir(cx, b, e);
        let start_ray = Line { at: a, dir: u };
        let end_ray = Line { at: b, dir: e };
        if let Some(q) = start_ray.meet(&end_ray)
            && (q - a).dot(u) > ZONE_CELL
            && (b - q).dot(e) > ZONE_CELL
        {
            c = q;
            report.guide = Some((b, b - e * GUIDE_HALF_LENGTH * 2.0));
        }
    }
    (
        RoadPath::Curve {
            from: a,
            control: c,
            to: b,
        },
        report,
    )
}

/// One section (T-275 spec v2, item 3): 10 cells between road centres; a guide since T-278.
pub const SECTION_M: f32 = 10.0 * ZONE_CELL;
/// Within this many degrees of a reference direction or its square the leg goes exactly so.
const SQUARE_DEG: f32 = 7.0;
/// Two directions closer than this (radians, about 0.006 degrees) are one direction.
const SAME_DIR_RAD: f32 = 1e-4;
/// How far (metres) from a vertex the straight streets give [`exact_dir`] their directions.
const EXACT_REACH_M: f32 = 3.0 * SECTION_M;

fn same_dir(a: Vec2, b: Vec2) -> bool {
    a.dot(b) > 0.0 && a.perp_dot(b).abs() < SAME_DIR_RAD
}

/// The snapped direction `dir` from `at`, made exact: the chord of a straight ground street near
/// `at` (or its square) that is the same direction but for rounding. A curve's tangent is as
/// exact as its control points (2e-5 rad at the map's coordinates): a street squared off it would
/// pass that on (1 cm over 480 m) to every street squared off that one.
fn exact_dir(cx: &LatticeCtx, at: Vec2, dir: Vec2) -> Vec2 {
    cx.graph
        .segments()
        .filter(|(_, s)| {
            s.elevation == Elevation::Ground
                && s.curve.bounds_distance_xz(at) <= EXACT_REACH_M
                && is_straight(&s.curve)
        })
        .filter_map(|(_, s)| (to_xz(s.curve.p3) - to_xz(s.curve.p0)).try_normalize())
        .flat_map(|r| [r, Vec2::new(-r.y, r.x), -r, Vec2::new(r.y, -r.x)])
        .find(|c| same_dir(*c, dir))
        .unwrap_or(dir)
}
/// How near (metres) the cursor must be to a guide (a section mark, a line of a street) for the
/// end to take it instead of the nearest whole cell (T-278: half a cell, so a sloppy hand still
/// reaches the cell beside a guide).
pub const WEAK_M: f32 = 0.5 * ZONE_CELL;

/// Applies the rules of the module doc to `req.path`, returning the path and the snap at its
/// last vertex. Pure: the same inputs, the same output; applying it to its own output changes
/// nothing.
pub fn quantize_path(req: &RoadPathRequest, cx: &LatticeCtx) -> (RoadPath, SnapReport) {
    let ground = matches!(req.elevation, cp_net::ElevationMode::Ground);
    let joins = ground && !cx.alt && req.snap.segment;
    let pts: Vec<Vec2> = match &req.path {
        RoadPath::Straight { from, to } => vec![*from, *to],
        RoadPath::Curve { from, to, .. } => vec![*from, *to],
        RoadPath::Free(p) => p.clone(),
    };
    if let RoadPath::Curve { from, control, to } = req.path {
        return quantize_curve(cx, joins, req.snap.angle, from, control, to);
    }
    let angle = req.snap.angle;
    let mut out: Vec<Vec2> = Vec::with_capacity(pts.len());
    let mut refs: Vec<Vec2> = Vec::new();
    // Whether `refs` come from a road or a leg (not the fallback for a start in the open).
    let mut real_refs = false;
    let mut report = SnapReport::default();
    for (i, &p) in pts.iter().enumerate() {
        report = SnapReport::default();
        let last = i + 1 == pts.len();
        let mut joined = if joins && !(cx.bend && last) {
            join(cx, p)
        } else {
            None
        };
        // The end of a leg on a street stands where the leg's snapped direction meets it, not
        // at the cursor's foot (BUG-088: a block closed along the guides is square). Only from
        // a start with real directions (a road, or the leg before); a start in the open has none.
        if i > 0
            && real_refs
            && angle
            && !cx.alt
            && let Some(j) = joined.as_mut()
            && j.node.is_none()
            && let Some(seg) = j.seg
            && let Some((q, along)) = square_on_street(cx, seg, out[i - 1], &refs, p)
        {
            j.pos = q;
            j.along = Some(along);
        }
        let mut next_refs = Vec::new();
        let pos = if let Some(j) = joined {
            report.kind = if j.node.is_some() {
                SnapReportKind::Node
            } else {
                SnapReportKind::Segment
            };
            if let Some(n) = j.node {
                next_refs = node_dirs(cx.graph, n);
            }
            if let Some(t) = j.along {
                next_refs = vec![t, -t];
            }
            j.pos
        } else if i == 0 {
            p
        } else {
            let (q, r) = leg(cx, angle, out[i - 1], &refs, p);
            report = r;
            q
        };
        if i > 0 {
            let back = out[i - 1] - pos;
            if back.length() > 1e-3 {
                next_refs.push(-back.normalize());
            }
        }
        real_refs = !next_refs.is_empty();
        if next_refs.is_empty() {
            next_refs.push(Vec2::X);
        }
        refs = next_refs;
        out.push(pos);
    }
    let path = match &req.path {
        RoadPath::Straight { .. } => RoadPath::Straight {
            from: out[0],
            to: out[1],
        },
        RoadPath::Curve { .. } => unreachable!("a curve is quantized above"),
        RoadPath::Free(_) => RoadPath::Free(out),
    };
    (path, report)
}

/// The 8-cell block line beside a street: the dashed line and what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockGuide {
    /// The cursor, moved onto the line when it was within [`BLOCK_SNAP_M`] of it.
    pub cursor: Vec2,
    /// Whether the cursor was moved.
    pub snapped: bool,
    /// Dashed guide lines (from the street to the ghost).
    pub lines: Vec<[Vec2; 2]>,
    /// "block 8 cells" or "N cells — partial block".
    pub note: String,
}

fn cells_text(cells: u32) -> String {
    if cells == BLOCK_CELLS {
        format!("block {BLOCK_CELLS} cells")
    } else {
        format!("{cells} cells — partial block")
    }
}

/// The block line for a ghost of a road of half width `half_width` from `from` to `cursor`: of
/// the streets near the cursor, the one whose cells the cursor stands beside, at whole cells from
/// its edge. Within [`BLOCK_SNAP_M`] of the 8-cell line (the near edge of a parallel street
/// leaves a block 4 + 4 deep) the cursor goes onto it; otherwise the count of whole cells between
/// the edges is shown. `None` when no street's cells are beside the cursor.
pub fn block_line(view: &SimView, from: Vec2, cursor: Vec2, half_width: f32) -> Option<BlockGuide> {
    if from.distance(cursor) < CELL_SIZE * 2.0 {
        return None;
    }
    let lines = all_lines(view);
    let reach = BLOCK_CELLS as f32 * CELL_SIZE + 2.0 * CELL_SIZE;
    let mut best: Option<(f32, BlockGuide)> = None;
    for l in lines.iter() {
        let m = l.line.mark;
        // Beside the line's street, on the side of its cells, within a block and a bit.
        let rel = cursor - m.axis;
        let side = rel.dot(m.v);
        let along = rel.dot(m.onward).abs();
        if side <= 0.0 || along > CELL_SIZE / 2.0 {
            continue;
        }
        let gap = side - l.hw - half_width;
        if !(2.0 * CELL_SIZE - BLOCK_SNAP_M..=reach).contains(&gap) {
            continue;
        }
        let want = BLOCK_CELLS as f32 * CELL_SIZE;
        let err = (gap - want).abs();
        let snapped = err <= BLOCK_SNAP_M;
        let target = if snapped {
            cursor + m.v * (want - gap)
        } else {
            cursor
        };
        let cells = if snapped {
            BLOCK_CELLS
        } else {
            (gap / CELL_SIZE).floor().max(0.0) as u32
        };
        if best.as_ref().is_none_or(|(e, _)| err < *e) {
            best = Some((
                err,
                BlockGuide {
                    cursor: target,
                    snapped,
                    lines: vec![[m.axis + m.v * l.hw, target]],
                    note: cells_text(cells),
                },
            ));
        }
    }
    best.map(|(_, g)| g)
}

/// The cursor of the road tool after the block line: where the ghost ends and where a click puts
/// the point (`alt` switches it off, like the other snaps), and the line to draw. Only a zonable
/// road with a point already clicked has one; a curve does not.
pub fn block_hover(
    view: &SimView,
    tool: &crate::RoadTool,
    hover: Vec2,
    alt: bool,
) -> (Vec2, Option<BlockGuide>) {
    let (Some(content), Some(road), Some(&from)) =
        (view.content.as_deref(), tool.road, tool.points.last())
    else {
        return (hover, None);
    };
    let def = content.road(road);
    if alt
        || !def.allows_zoning
        || tool.mode == crate::RoadMode::Curve
        || tool.tunnel_depth.is_some()
    {
        return (hover, None);
    }
    match block_line(view, from, hover, def.width * 0.5) {
        Some(g) => (g.cursor, Some(g)),
        None => (hover, None),
    }
}

/// How far from the path the cell lines of a street still count: a block (24 cells) and more.
const MARK_REACH: f32 = 220.0;

/// The cell lines near the vertices of `path`, as the length step and the joins take them.
pub fn path_marks(view: &SimView, path: &RoadPath) -> Vec<ControlMark> {
    let points: Vec<Vec2> = match path {
        RoadPath::Straight { from, to } => vec![*from, *to],
        RoadPath::Curve { from, control, to } => vec![*from, *control, *to],
        RoadPath::Free(v) => v.clone(),
    };
    marks_near(view, &points, MARK_REACH)
}

/// The radius within which the road tool draws the ticks around the cursor.
pub const TICK_RADIUS: f32 = GUIDE_RADIUS;

/// A tick of the road being drawn (T-278): across it at a whole cell from the start of its leg.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellTick {
    pub a: Vec2,
    pub b: Vec2,
    /// Every 10th cell (a section, the 10/20/30 silhouette): longer and drawn bold.
    pub major: bool,
}

/// The ticks along `path` for a road of half width `half_width`: one every cell from the start of
/// each leg (a curve: along its arc), every 10th a major one reaching a cell past the road's edge.
pub fn cell_ticks(path: &RoadPath, half_width: f32) -> Vec<CellTick> {
    let polylines: Vec<Vec<Vec2>> = match path {
        RoadPath::Straight { from, to } => vec![vec![*from, *to]],
        RoadPath::Free(v) => v.windows(2).map(|w| w.to_vec()).collect(),
        RoadPath::Curve { from, control, to } => {
            const N: usize = 128;
            vec![
                (0..=N)
                    .map(|i| {
                        let t = i as f32 / N as f32;
                        let s = 1.0 - t;
                        *from * (s * s) + *control * (2.0 * s * t) + *to * (t * t)
                    })
                    .collect(),
            ]
        }
    };
    let mut out = Vec::new();
    for line in polylines {
        let (mut walked, mut k) = (0.0, 1u32);
        for w in line.windows(2) {
            let (p, q) = (w[0], w[1]);
            let len = p.distance(q);
            let Some(d) = (q - p).try_normalize() else {
                continue;
            };
            let n = Vec2::new(-d.y, d.x);
            while k as f32 * ZONE_CELL <= walked + len + 1e-3 {
                let at = p + d * (k as f32 * ZONE_CELL - walked);
                let major = k % 10 == 0;
                let r = if major {
                    half_width + ZONE_CELL
                } else {
                    half_width
                };
                out.push(CellTick {
                    a: at - n * r,
                    b: at + n * r,
                    major,
                });
                k += 1;
            }
            walked += len;
        }
    }
    out
}
