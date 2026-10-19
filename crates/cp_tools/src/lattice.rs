//! The zone-lattice candidate of the road tool's resolver (`architecture/system-design.md`
//! §6.8.3 items 3 and 5, T-266): where a new street lands so that the cells beside it are whole.
//! Everything comes from the cell lines of the zone layout ([`cp_zone::cell_lines`], T-265), the
//! one source of "where the cells are": the ticks the tool draws, the join of a street to another,
//! the length step and the 8-cell block line. Pure: the same net, the same answer.
//!
//! Per vertex of the path (`alt`, snaps off, switches all of it off):
//! 1. **Join**: a point within one cell of a street's axis goes onto the street at the nearest of
//!    its cell lines within [`JOIN_M`] (an 8-cell line wins over a plain one), its near edge on
//!    the line; at the open end of a street, onto the end. A street across a block keeps the
//!    perpendicular of the street it started from.
//! 2. **Direction**: the key angles, the guides and the 15° step (`cp_net::build::angle_snap_guides`).
//! 3. **Length**: up to the perpendicular through an 8-cell line of a street near the end (within
//!    [`CONTROL_LINE_TOL`]), else whole cells from the street it started on (its edge, whole
//!    cells, the edge of the street that will close the block), else a multiple of 8 m. A curve
//!    keeps its corner and slides its end along the line from the corner.
//!
//! The **block line** ([`block_line`]): beside a street the line where the near edge of a street
//! parallel to it leaves 8 cells (a block 4 + 4 deep), highlighted; the cursor goes onto it within
//! [`BLOCK_SNAP_M`].

use std::sync::{Arc, Mutex};

use bevy_math::{Vec2, Vec3};
use cp_content::ContentDb;
use cp_core::curve::{CubicBezier3, to_xz};
use cp_net::build::{GUIDE_HALF_LENGTH, GUIDE_RADIUS, ZONE_CELL, angle_snap_guides};
use cp_net::{
    Elevation, NetGraph, NodeId, RoadPath, RoadPathRequest, SegmentId, SnapKind, SnapReport,
    SnapReportKind,
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
/// A curve may be extended past its end up to this parameter.
const MAX_CURVE_T: f32 = 1.25;

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
}

/// Where a joined vertex landed, with what the next leg takes as direction references.
struct Joined {
    pos: Vec2,
    node: Option<NodeId>,
    /// For a point of a street: the directions along it there.
    along: Option<Vec2>,
    /// How far from the line the vertex was placed (the edge offset), zero otherwise.
    shift: Vec2,
    /// Half width of the street the vertex stands on (zero off a street).
    hw: f32,
    /// The street the vertex stands on (a point of a street).
    seg: Option<SegmentId>,
}

fn dir_xz(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.z).normalize_or_zero()
}

/// Rule 1 for the point `p`.
fn join(cx: &LatticeCtx, p: Vec2) -> Option<Joined> {
    // A point on a node (a chain goes on from the end it just built) stays on it (BUG-073).
    if let Some((id, n)) = cx
        .graph
        .nodes()
        .filter(|(_, n)| n.elevation == Elevation::Ground)
        .find(|(_, n)| to_xz(n.pos).distance(p) < 0.5)
    {
        return Some(Joined {
            pos: to_xz(n.pos),
            node: Some(id),
            along: None,
            shift: Vec2::ZERO,
            hw: 0.0,
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
    let foot = to_xz(s.curve.point(t));
    let hw = cx.content.road(s.road).width * 0.5;
    // The open end of a street, or a junction, within a cell is the node, not a line beside it
    // (BUG-086: a street from beside a junction starts at the junction).
    let end = [s.start, s.end].into_iter().find(|&n| {
        !cp_net::is_plain_cut(cx.graph, n)
            && cx
                .graph
                .node(n)
                .is_some_and(|node| to_xz(node.pos).distance(foot) < ZONE_CELL)
    });
    if let Some(n) = end {
        return Some(Joined {
            pos: to_xz(cx.graph.node(n)?.pos),
            node: Some(n),
            along: None,
            shift: Vec2::ZERO,
            hw: 0.0,
            seg: None,
        });
    }
    // A tick is aimed at where the cross street's centreline will stand (what the ghost shows);
    // a plain cell line, at the line. A tick within reach wins over a plain line.
    let aim = |m: &ControlMark| {
        if m.major {
            m.pos + m.onward * cx.edge_offset
        } else {
            m.pos
        }
        .distance(foot)
    };
    let rank = |m: &ControlMark| aim(m) + if m.major { 0.0 } else { 100.0 };
    if let Some(m) = cx
        .marks
        .iter()
        .filter(|m| aim(m) <= JOIN_M && (m.pos - foot).perp_dot(m.dir).abs() < 1.0)
        .min_by(|a, b| rank(a).total_cmp(&rank(b)))
    {
        let shift = m.onward * cx.edge_offset;
        return Some(Joined {
            pos: m.pos + shift,
            node: None,
            along: Some(m.dir),
            shift,
            hw,
            seg: Some(seg),
        });
    }
    // No line near: the point of the street (the cells there do not care).
    Some(Joined {
        pos: foot,
        node: None,
        along: Some(dir_xz(s.curve.tangent(t))),
        shift: Vec2::ZERO,
        hw,
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
    let v3 = |q: Vec2| Vec3::new(q.x, 0.0, q.y);
    let mut best: Option<(bool, f32, Vec2, f32)> = None;
    for r in refs {
        for k in 0..24 {
            let (sin, cos) = (k as f32 * 15.0).to_radians().sin_cos();
            let dir = Vec2::new(r.x * cos - r.y * sin, r.x * sin + r.y * cos);
            let ray = CubicBezier3::line(v3(from), v3(from + dir * RAY_M));
            for (_, ts) in ray.intersect_xz(&s.curve) {
                let q = to_xz(s.curve.point(ts));
                let d = q.distance(p);
                if q.distance(from) < ZONE_CELL || d > reach {
                    continue;
                }
                let rank = (k % 3 != 0, d);
                if best.is_none_or(|(bk, bd, ..)| rank < (bk, bd)) {
                    best = Some((rank.0, d, q, ts));
                }
            }
        }
    }
    best.map(|(_, _, q, t)| (q, dir_xz(s.curve.tangent(t))))
}

/// How far a snapped direction is followed to meet a street (metres).
const RAY_M: f32 = 2000.0;

/// Directions leaving the node, as the angle snap takes them.
fn node_dirs(graph: &NetGraph, n: NodeId) -> Vec<Vec2> {
    let Some(node) = graph.node(n) else {
        return Vec::new();
    };
    node.segments
        .iter()
        .filter_map(|sid| graph.segment(*sid))
        .map(|s| {
            if s.start == n {
                dir_xz(s.curve.tangent(0.0))
            } else {
                -dir_xz(s.curve.tangent(1.0))
            }
        })
        .collect()
}

/// The perpendicular through an 8-cell line near the leg that the ray `prev + u·dir` meets
/// closest to `u = len`: `(u, the line's point, the unit direction of the perpendicular)`.
fn control_line(cx: &LatticeCtx, prev: Vec2, dir: Vec2, len: f32) -> Option<(f32, Vec2, Vec2)> {
    let mut best: Option<(f32, f32, Vec2, Vec2)> = None;
    for m in cx.marks.iter().filter(|m| m.major) {
        // The perpendicular through the tick's station: the centreline of the street.
        let c = m.pos + m.onward * cx.edge_offset;
        let denom = dir.dot(m.dir);
        if m.dir == Vec2::ZERO || denom.abs() < 0.2 {
            continue;
        }
        let u = (c - prev).dot(m.dir) / denom;
        let miss = (u - len).abs();
        if u >= ZONE_CELL && miss <= CONTROL_LINE_TOL && best.is_none_or(|(bm, ..)| miss < bm) {
            best = Some((miss, u, c, Vec2::new(-m.dir.y, m.dir.x)));
        }
    }
    best.map(|(_, u, c, n)| (u, c, n))
}

/// The point of the quadratic `a → c → b` extended to parameter `t`, and the quadratic cut at `t`.
fn cut_quad(a: Vec2, c: Vec2, b: Vec2, t: f32) -> (Vec2, Vec2) {
    let ctrl = a.lerp(c, t);
    let u = 1.0 - t;
    (ctrl, a * (u * u) + c * (2.0 * u * t) + b * (t * t))
}

fn quad_len(a: Vec2, c: Vec2, b: Vec2) -> f32 {
    let v3 = |p: Vec2| Vec3::new(p.x, 0.0, p.y);
    CubicBezier3::new(
        v3(a),
        v3(a + (c - a) * (2.0 / 3.0)),
        v3(b + (c - b) * (2.0 / 3.0)),
        v3(b),
    )
    .arc_table()
    .length()
}

/// The closest 8 m multiple of the length of the curve (never below 8). The curve keeps its
/// corner `c` and the line `c → b` its end is on: the end slides along that line (BUG-073b), so
/// the street that goes on from it leaves in exactly the direction the player aimed. When the
/// curve is already longer than that, it is cut as a quadratic.
fn step_curve(a: Vec2, c: Vec2, b: Vec2) -> (Vec2, Vec2) {
    let len = quad_len(a, c, b);
    let want = ((len / ZONE_CELL).round() * ZONE_CELL).max(ZONE_CELL);
    if let Some(dir) = (b - c).try_normalize() {
        let at = |k: f32| c + dir * k;
        if quad_len(a, c, at(0.05)) < want {
            let (mut lo, mut hi) = (0.05_f32, 4.0 * len.max(want));
            for _ in 0..50 {
                let mid = 0.5 * (lo + hi);
                if quad_len(a, c, at(mid)) < want {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return (c, at(0.5 * (lo + hi)));
        }
    }
    let len_at = |t: f32| {
        let (ctrl, to) = cut_quad(a, c, b, t);
        quad_len(a, ctrl, to)
    };
    if len_at(MAX_CURVE_T) < want {
        return cut_quad(a, c, b, MAX_CURVE_T);
    }
    let (mut lo, mut hi) = (0.0_f32, MAX_CURVE_T);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if len_at(mid) < want {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    cut_quad(a, c, b, 0.5 * (lo + hi))
}

/// One leg from `prev` to the cursor `p`: rule 2, then rule 3 (unless `alt`).
fn leg(
    cx: &LatticeCtx,
    snap_angle: bool,
    prev: Vec2,
    refs: &[Vec2],
    p: Vec2,
    from_street: Option<(f32, Vec2)>,
) -> (Vec2, SnapReport) {
    let mut report = SnapReport::default();
    let mut end = p;
    if snap_angle {
        let guides: Vec<&CubicBezier3> = cx
            .graph
            .segments()
            .filter(|(_, s)| s.elevation == Elevation::Ground)
            .map(|(_, s)| &s.curve)
            .collect();
        if let Some(a) = angle_snap_guides(guides, p, prev, refs) {
            end = a.pos;
            report = SnapReport {
                kind: match a.kind {
                    SnapKind::KeyAngle(d) => SnapReportKind::KeyAngle(d),
                    SnapKind::Guide { perpendicular } => SnapReportKind::Guide { perpendicular },
                    SnapKind::Angle(d) => SnapReportKind::Step(d),
                    _ => SnapReportKind::None,
                },
                guide: a.guide,
            };
        }
    }
    let d = end - prev;
    let len = d.length();
    if cx.alt || len < 1e-3 {
        return (end, report);
    }
    let dir = d / len;
    if let Some((u, c, n)) = control_line(cx, prev, dir, len) {
        return (
            prev + dir * u,
            SnapReport {
                kind: SnapReportKind::ControlLine(u.round() as i32),
                guide: Some((c - n * GUIDE_HALF_LENGTH, c + n * GUIDE_HALF_LENGTH)),
            },
        );
    }
    // A street across from a street: its end stands where the cells beside it end on the
    // lattice of the street it started at (the edge of that street, whole cells, the edge of the
    // street that will close the block, BUG-073).
    let lattice = from_street
        .filter(|(_, t)| dir.dot(*t).abs() < 0.26)
        .map(|(hw, _)| hw + cx.edge_offset);
    let stepped = match lattice {
        Some(off) => (((len - off) / ZONE_CELL).round() * ZONE_CELL).max(ZONE_CELL) + off,
        None => ((len / ZONE_CELL).round() * ZONE_CELL).max(ZONE_CELL),
    };
    (
        prev + dir * stepped,
        SnapReport {
            kind: SnapReportKind::CellStep(stepped as i32),
            guide: report.guide,
        },
    )
}

/// Applies the rules of the module doc to `req.path`, returning the path and the snap at its
/// last vertex. Pure: the same inputs, the same output; applying it to its own output changes
/// nothing.
pub fn quantize_path(req: &RoadPathRequest, cx: &LatticeCtx) -> (RoadPath, SnapReport) {
    let ground = matches!(req.elevation, cp_net::ElevationMode::Ground);
    let joins = ground && !cx.alt && req.snap.segment;
    let (pts, control): (Vec<Vec2>, Option<Vec2>) = match &req.path {
        RoadPath::Straight { from, to } => (vec![*from, *to], None),
        RoadPath::Curve { from, control, to } => (vec![*from, *to], Some(*control)),
        RoadPath::Free(p) => (p.clone(), None),
    };
    let angle = req.snap.angle && control.is_none();
    let mut out: Vec<Vec2> = Vec::with_capacity(pts.len());
    let mut refs: Vec<Vec2> = Vec::new();
    // Whether `refs` come from a road or a leg (not the fallback for a start in the open).
    let mut real_refs = false;
    let mut report = SnapReport::default();
    let mut stepped_curve = None;
    let mut last_shift = Vec2::ZERO;
    // Half width and direction of the street the last vertex was joined on (BUG-073).
    let mut from_street: Option<(f32, Vec2)> = None;
    for (i, &p) in pts.iter().enumerate() {
        report = SnapReport::default();
        let prev_street = from_street;
        let mut joined = if joins { join(cx, p) } else { None };
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
            j.shift = Vec2::ZERO;
        }
        // A street across a block (BUG-067): when the vertex before it stands on a line of a
        // street and a line of this street lies on the perpendicular from there, join that one,
        // so the new street crosses at a right angle instead of at the line nearest the cursor.
        if i > 0
            && let Some(j) = joined.as_mut()
            && j.node.is_none()
            && !cx.alt
        {
            let from = out[i - 1];
            let mut best: Option<(f32, Vec2, Vec2)> = None;
            for n in refs
                .iter()
                .map(|r| Vec2::new(-r.y, r.x))
                .filter(|n| n.length() > 0.5)
            {
                for m in cx.marks.iter().filter(|m| m.pos.distance(j.pos) <= 12.0) {
                    // The same shift as the first vertex got: the street stays perpendicular.
                    let pos = m.pos + last_shift;
                    let off = (pos - from).perp_dot(n).abs();
                    if off <= 0.3 && best.is_none_or(|(bo, ..)| off < bo) {
                        best = Some((off, pos, m.dir));
                    }
                }
            }
            if let Some((_, pos, dir)) = best {
                j.pos = pos;
                j.along = Some(dir);
                j.shift = last_shift;
            }
        }
        last_shift = joined.as_ref().map_or(Vec2::ZERO, |j| j.shift);
        from_street = joined
            .as_ref()
            .and_then(|j| j.along.filter(|_| j.hw > 0.0).map(|t| (j.hw, t)));
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
        } else if let (Some(c), true) = (control, i == 1) {
            if cx.alt {
                p
            } else {
                let (ctrl, to) = step_curve(out[0], c, p);
                report.kind = SnapReportKind::CellStep(quad_len(out[0], ctrl, to).round() as i32);
                stepped_curve = Some(ctrl);
                to
            }
        } else {
            let (q, r) = leg(cx, angle, out[i - 1], &refs, p, prev_street);
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
        RoadPath::Curve { control, .. } => RoadPath::Curve {
            from: out[0],
            control: stepped_curve.unwrap_or(*control),
            to: out[1],
        },
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
