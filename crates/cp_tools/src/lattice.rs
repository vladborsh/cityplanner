//! The section candidate of the road tool's resolver (`architecture/system-design.md` §6.8,
//! T-275 spec v2): the player builds by whole cells, roads are whole cells wide, and a street's
//! end lands a whole number of sections (10 cells, [`SECTION_M`]) from the centre it starts at,
//! so every block is whole cells by construction. Pure: the same net, the same answer.
//!
//! Per vertex of the path (`alt`, snaps off, switches all of it off):
//! 1. **Join**: a cursor within a junction's pull ([`node_pull`]: 1.5 cells, 2.5 when a 4-cell
//!    road meets there) goes to its centre; a point on a street goes to the street's nearest
//!    section mark ([`section_mark`]: every 10 cells from either end of its stretch). The end of a
//!    leg on a street goes where the leg's square direction meets it (BUG-088).
//! 2. **Direction**: within 7° of a reference direction (the street it leaves, the leg before)
//!    or its square, exactly that.
//! 3. **Length**: whole sections from the start, or a line of a street within a section (its
//!    extension, or the square through one of its nodes) within 1.5 cells. A curve keeps its
//!    corner and slides its end along the line from the corner.

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
    /// The street the vertex stands on (a point of a street).
    seg: Option<SegmentId>,
}

fn dir_xz(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.z).normalize_or_zero()
}

/// The junction pull of node `n` (T-275 spec v2, item 8): a cursor within 1.5 cells of its
/// centre goes to it, within 2.5 cells when a 4-cell road meets there.
pub fn node_pull(graph: &NetGraph, content: &ContentDb, n: NodeId) -> f32 {
    let wide = graph.node(n).is_some_and(|node| {
        node.segments.iter().any(|&s| {
            graph
                .segment(s)
                .is_some_and(|s| content.road(s.road).width >= 4.0 * ZONE_CELL - 1e-3)
        })
    });
    if wide {
        2.5 * ZONE_CELL
    } else {
        1.5 * ZONE_CELL
    }
}

/// The section mark of the street `seg` nearest the point `s` metres along it (from its start):
/// every [`SECTION_M`] from either end of its stretch (a junction, a dead end or a corner), the
/// centre where a cross street stands (T-275 spec v2, item 5). The point and the street's
/// direction there; `None` on a stretch shorter than a section.
pub fn section_mark(graph: &NetGraph, seg: SegmentId, s: f32) -> Option<(Vec2, Vec2)> {
    let st = cp_net::stretch(graph, seg)?;
    let along = cp_net::locate(graph, seg, s)?;
    let mut best: Option<(f32, f32)> = None;
    let mut k = SECTION_M;
    while k < st.length - 1.0 {
        for at in [k, st.length - k] {
            let miss = (at - along).abs();
            if best.is_none_or(|(m, _)| miss < m) {
                best = Some((miss, at));
            }
        }
        k += SECTION_M;
    }
    let (_, at) = best?;
    let (sid, s_on) = cp_net::unlocate(graph, &st, at)?;
    let sg = graph.segment(sid)?;
    let t = sg.curve.arc_table().s_to_t(s_on);
    Some((to_xz(sg.curve.point(t)), dir_xz(sg.curve.tangent(t))))
}

/// Rule 1 for the point `p`: the centre of a junction within its pull, else the section mark of
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
    let (pos, along) = section_mark(cx.graph, seg, at)
        .unwrap_or((to_xz(s.curve.point(t)), dir_xz(s.curve.tangent(t))));
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
    let v3 = |q: Vec2| Vec3::new(q.x, 0.0, q.y);
    let mut best: Option<(bool, f32, Vec2, f32)> = None;
    for r in refs {
        for k in 0..4 {
            let (sin, cos) = (k as f32 * 90.0).to_radians().sin_cos();
            let dir = Vec2::new(r.x * cos - r.y * sin, r.x * sin + r.y * cos);
            let ray = CubicBezier3::line(v3(from), v3(from + dir * RAY_M));
            for (_, ts) in ray.intersect_xz(&s.curve) {
                let q = to_xz(s.curve.point(ts));
                let d = q.distance(p);
                if q.distance(from) < ZONE_CELL || d > reach {
                    continue;
                }
                let rank = (false, d);
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
) -> (Vec2, SnapReport) {
    let mut report = SnapReport::default();
    let d = p - prev;
    let len = d.length();
    if len < 1e-3 {
        return (p, report);
    }
    let mut dir = d / len;
    // Rule 2: within SQUARE_DEG of a reference or its square, exactly that (spec v2, item 7).
    if snap_angle {
        let cos = SQUARE_DEG.to_radians().cos();
        let best = refs
            .iter()
            .flat_map(|r| [*r, Vec2::new(-r.y, r.x), -*r, Vec2::new(r.y, -r.x)])
            .filter(|c| c.length() > 0.5)
            .map(|c| c.normalize())
            .max_by(|a, b| a.dot(dir).total_cmp(&b.dot(dir)))
            .filter(|c| c.dot(dir) >= cos);
        if let Some(c) = best {
            let square = refs.iter().any(|r| r.dot(c).abs() < 0.5);
            dir = c;
            report.kind = SnapReportKind::KeyAngle(if square { 90 } else { 0 });
            report.guide = Some((prev, prev + dir * GUIDE_HALF_LENGTH * 2.0));
        }
    }
    let along = d.dot(dir);
    if cx.alt {
        return (prev + dir * along, report);
    }
    // Rule 3: whole sections from the start (spec v2, item 5), or a line of a street within a
    // section (its extension or the square through one of its nodes, item 6) within LINE_TOL_M.
    let mut at = ((along / SECTION_M).round() * SECTION_M).max(SECTION_M);
    let mut kind = SnapReportKind::CellStep(at as i32);
    let cursor = prev + dir * along;
    let mut best_line: Option<(f32, f32)> = None;
    let mut consider = |u: f32| {
        let miss = (u - along).abs();
        if u >= ZONE_CELL && miss <= LINE_TOL_M && best_line.is_none_or(|(m, _)| miss < m) {
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
    if let Some((miss, u)) = best_line
        && miss < (at - along).abs()
    {
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

/// One section (T-275 spec v2, item 3): 10 cells between road centres.
pub const SECTION_M: f32 = 10.0 * ZONE_CELL;
/// Within this many degrees of a reference direction or its square the leg goes exactly so.
const SQUARE_DEG: f32 = 7.0;
/// How near (metres along the leg) the cursor must be to a line of a street for the end to take
/// it instead of the section step.
const LINE_TOL_M: f32 = 1.5 * ZONE_CELL;

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
    for (i, &p) in pts.iter().enumerate() {
        report = SnapReport::default();
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
