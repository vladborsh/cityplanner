//! The one snap resolver (`architecture/system-design.md` §6.8.3 items 1, 5 and 9, T-264).
//!
//! [`resolve`] takes the cursor and what the tool can snap to and answers once: the point, what it
//! is (a node, a point of an edge, or free) and what snapped it. It is a pure function of its
//! inputs; the road tool and the pipe and power-line tool both call it, and the road tool sends
//! the result as the anchors of a `BuildPlan`, which the sim applies as it is.
//!
//! Priority, the first that fires wins:
//! 1. a **node** within its reach, then an **edge** within its reach (a street within its half
//!    width): a direct hit, also when snaps are off;
//! 2. the **intersection of two guides** within [`INTERSECTION_TOL_M`] of the cursor (the
//!    extension of a road's end or its perpendicular through the end, against another such line
//!    or against a ray from the start of the leg);
//! 3. one **guide line** within [`LINE_TOL_M`]: the cursor goes onto it;
//! 4. the **angle** of the leg from its start: the key angles (45/90/135°) from the directions at
//!    the start, the parallels and perpendiculars of the roads near the cursor, the 15° step
//!    (`cp_net::build::angle_snap_guides`, the function the sim's old snaps ran);
//! 5. the **length** along the leg: the perpendicular through a major zone mark within
//!    [`CONTROL_LINE_TOL`], else a multiple of 8 m (roads only).
//!
//! 2 and 3 pin the point, so no angle or length step follows; 4 and 5 are stages of one leg.
//! With `snaps` off only 1 remains.

use bevy_math::Vec2;
use cp_core::curve::CubicBezier3;
use cp_net::build::{GUIDE_HALF_LENGTH, GUIDE_RADIUS, ZONE_CELL, angle_snap_guides};
use cp_net::{SnapReport, SnapReportKind};

use crate::lattice::{CONTROL_LINE_TOL, ControlMark};

/// How far (metres) the cursor may be from the intersection of two guides to snap onto it.
pub const INTERSECTION_TOL_M: f32 = 3.0;
/// How far (metres) the cursor may be from a guide line to snap onto it.
pub const LINE_TOL_M: f32 = 4.0;
/// The guide curves nearest the cursor that give lines (a city has hundreds).
const MAX_GUIDE_CURVES: usize = 24;
/// Key angles from a direction, degrees: what the guide rays through the start are made of.
const KEY_ANGLES_DEG: [f32; 6] = [45.0, -45.0, 90.0, -90.0, 135.0, -135.0];

/// An arm of a junction: its unit direction away from the node and the half width of its road.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapArm {
    pub dir: Vec2,
    pub half_width: f32,
}

/// A node the cursor may land on. Without `arms` any point within `reach` of `pos` is the node
/// (pipes). With them (a junction of roads) the node's footprint is the disc of
/// [`MIN_FOOTPRINT_M`] and the asphalt of every arm up to `reach` along it, so at an acute
/// junction a point on one arm that lies on the other arm's asphalt is inside too.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapNode {
    pub pos: Vec2,
    pub reach: f32,
    pub arms: Vec<SnapArm>,
}

/// The disc around a node that is always its footprint, metres: the smallest piece of road.
pub const MIN_FOOTPRINT_M: f32 = 8.0;

impl SnapNode {
    /// A node taken within a radius.
    pub fn radial(pos: Vec2, reach: f32) -> Self {
        Self {
            pos,
            reach,
            arms: Vec::new(),
        }
    }

    /// Whether `p` is inside the node.
    pub fn contains(&self, p: Vec2) -> bool {
        let rel = p - self.pos;
        if self.arms.is_empty() {
            return rel.length() <= self.reach;
        }
        rel.length() <= MIN_FOOTPRINT_M
            || self.arms.iter().any(|a| {
                let (along, off) = (rel.dot(a.dir), rel.perp_dot(a.dir).abs());
                (0.0..=self.reach).contains(&along) && off < a.half_width
            })
    }
}

/// An edge (a street, a pipe) the cursor may land on: any point within `reach` of its axis is a
/// point of the axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapEdge {
    pub curve: CubicBezier3,
    pub reach: f32,
}

/// What [`resolve`] reads.
#[derive(Clone, Copy, Debug)]
pub struct SnapCtx<'a> {
    pub nodes: &'a [SnapNode],
    pub edges: &'a [SnapEdge],
    /// Curves that give guides: parallels, perpendiculars, and the lines through their ends.
    pub guides: &'a [CubicBezier3],
    /// Column boundaries of the zoning near the cursor ([`ControlMark`]): the length jumps
    /// onto the perpendicular through a major one.
    pub marks: &'a [ControlMark],
    /// The start of the leg being drawn; `None` for the first point of a drawing.
    pub start: Option<Vec2>,
    /// The directions at the start (the roads leaving its node, the leg before it, turned round)
    /// that the angle is measured against.
    pub refs: &'a [Vec2],
    /// Snaps on. Off (`Alt`, the button): a direct hit on a node or edge only.
    pub snaps: bool,
    /// Round the leg's length to the 8 m cell (the road tool; pipes have no step).
    pub length_step: bool,
}

/// What the resolved point is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    /// Open ground.
    Free,
    /// The node at this index of [`SnapCtx::nodes`].
    Node(usize),
    /// A point of the edge at this index of [`SnapCtx::edges`], at curve parameter `t`.
    Edge(usize, f32),
}

/// What snapped the point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnapKind {
    None,
    Node,
    Edge,
    /// Two guide lines meet here.
    GuideIntersection,
    /// On the extension of a road's end.
    Extension,
    /// On the perpendicular through a road's end.
    Perpendicular,
    /// Parallel or perpendicular to a road near the cursor.
    Parallel {
        perpendicular: bool,
    },
    KeyAngle(i32),
    Step(i32),
    /// The length went to the perpendicular through a zone mark that lies this many metres along
    /// its street.
    Mark(i32),
    /// The length was rounded to this many metres.
    Length(i32),
}

/// The answer of [`resolve`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub pos: Vec2,
    pub hit: Hit,
    pub kind: SnapKind,
    /// The guide line to draw (two points on it).
    pub guide: Option<(Vec2, Vec2)>,
}

impl Snap {
    /// The same as the label and guide of the older `SnapReport`.
    pub fn report(&self) -> SnapReport {
        let kind = match self.kind {
            SnapKind::None => SnapReportKind::None,
            SnapKind::Node => SnapReportKind::Node,
            SnapKind::Edge => SnapReportKind::Segment,
            SnapKind::GuideIntersection | SnapKind::Extension | SnapKind::Perpendicular => {
                SnapReportKind::Guide {
                    perpendicular: matches!(self.kind, SnapKind::Perpendicular),
                }
            }
            SnapKind::Parallel { perpendicular } => SnapReportKind::Guide { perpendicular },
            SnapKind::KeyAngle(d) => SnapReportKind::KeyAngle(d),
            SnapKind::Step(d) => SnapReportKind::Step(d),
            SnapKind::Mark(a) => SnapReportKind::ControlLine(a),
            SnapKind::Length(m) => SnapReportKind::CellStep(m),
        };
        SnapReport {
            kind,
            guide: self.guide,
        }
    }
}

/// An infinite line: through `origin` along the unit vector `dir`.
#[derive(Clone, Copy, Debug)]
struct Line {
    origin: Vec2,
    dir: Vec2,
    kind: SnapKind,
    /// A ray from the start of the leg: only the part ahead of the start counts.
    ray: bool,
}

fn perp(v: Vec2) -> Vec2 {
    Vec2::new(-v.y, v.x)
}

fn rotate(v: Vec2, deg: f32) -> Vec2 {
    let (s, c) = deg.to_radians().sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

fn xz(v: bevy_math::Vec3) -> Vec2 {
    Vec2::new(v.x, v.z)
}

fn unit(v: bevy_math::Vec3) -> Vec2 {
    xz(v).normalize_or_zero()
}

/// Where two lines meet; `None` for parallel ones.
fn meet(a: &Line, b: &Line) -> Option<Vec2> {
    let den = a.dir.perp_dot(b.dir);
    if den.abs() < 1e-3 {
        return None;
    }
    let t = (b.origin - a.origin).perp_dot(b.dir) / den;
    Some(a.origin + a.dir * t)
}

fn guide_through(p: Vec2, dir: Vec2) -> Option<(Vec2, Vec2)> {
    Some((p - dir * GUIDE_HALF_LENGTH, p + dir * GUIDE_HALF_LENGTH))
}

fn free(cursor: Vec2) -> Snap {
    Snap {
        pos: cursor,
        hit: Hit::Free,
        kind: SnapKind::None,
        guide: None,
    }
}

/// Resolves `cursor` (§6.8.3 item 1): once, by the priority in the module doc.
pub fn resolve(cursor: Vec2, ctx: &SnapCtx) -> Snap {
    // 1. A direct hit: a node (never the start itself), then an edge.
    let node = ctx
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| ctx.start.is_none_or(|s| s.distance(n.pos) > 1e-3))
        .filter(|(_, n)| n.contains(cursor))
        .min_by(|a, b| {
            a.1.pos
                .distance(cursor)
                .total_cmp(&b.1.pos.distance(cursor))
        });
    if let Some((i, n)) = node {
        return Snap {
            pos: n.pos,
            hit: Hit::Node(i),
            kind: SnapKind::Node,
            guide: None,
        };
    }
    let edge = ctx
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.curve.bounds_distance_xz(cursor) <= e.reach + 1e-3)
        .map(|(i, e)| {
            let (t, d) = e.curve.nearest_xz(cursor);
            (d, i, t, e)
        })
        .filter(|(d, _, _, e)| *d <= e.reach)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, i, t, e)) = edge {
        return Snap {
            pos: xz(e.curve.point(t)),
            hit: Hit::Edge(i, t),
            kind: SnapKind::Edge,
            guide: None,
        };
    }
    if !ctx.snaps {
        return free(cursor);
    }

    // The guide curves near the cursor, nearest first.
    let mut near: Vec<(f32, &CubicBezier3)> = ctx
        .guides
        .iter()
        .filter(|c| c.bounds_distance_xz(cursor) <= GUIDE_RADIUS + 1e-3)
        .map(|c| (c.nearest_xz(cursor).1, c))
        .filter(|(d, _)| *d <= GUIDE_RADIUS)
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    near.truncate(MAX_GUIDE_CURVES);

    // Lines through the ends of the guides: the extension and the perpendicular.
    let mut lines: Vec<Line> = Vec::new();
    for (_, c) in &near {
        for (origin, out) in [
            (xz(c.p0), -unit(c.tangent(0.0))),
            (xz(c.p3), unit(c.tangent(1.0))),
        ] {
            // The start's own end is the ray's business (its key angles), not a second guide.
            if out == Vec2::ZERO || ctx.start.is_some_and(|s| s.distance(origin) < 1e-3) {
                continue;
            }
            lines.push(Line {
                origin,
                dir: out,
                kind: SnapKind::Extension,
                ray: false,
            });
            lines.push(Line {
                origin,
                dir: perp(out),
                kind: SnapKind::Perpendicular,
                ray: false,
            });
        }
    }
    let point_lines = lines.len();
    // Rays through the start: the key angles from its directions, the parallels and
    // perpendiculars of the guides.
    if let Some(start) = ctx.start {
        for &r in ctx.refs {
            for deg in KEY_ANGLES_DEG {
                lines.push(Line {
                    origin: start,
                    dir: rotate(r, deg),
                    kind: SnapKind::KeyAngle(deg.abs() as i32),
                    ray: true,
                });
            }
        }
        for (_, c) in &near {
            let (t, _) = c.nearest_xz(cursor);
            let tan = unit(c.tangent(t));
            if tan == Vec2::ZERO {
                continue;
            }
            for (dir, perpendicular) in [(tan, false), (perp(tan), true)] {
                for dir in [dir, -dir] {
                    lines.push(Line {
                        origin: start,
                        dir,
                        kind: SnapKind::Parallel { perpendicular },
                        ray: true,
                    });
                }
            }
        }
    }

    // 2. The intersection of two guides near the cursor.
    let mut best: Option<(f32, Vec2, Line)> = None;
    for (i, a) in lines[..point_lines].iter().enumerate() {
        for b in lines.iter().skip(i + 1) {
            let Some(x) = meet(a, b) else { continue };
            let miss = x.distance(cursor);
            let ahead =
                |l: &Line| !l.ray || ctx.start.is_none_or(|s| (x - s).dot(l.dir) >= ZONE_CELL);
            if miss <= INTERSECTION_TOL_M
                && ahead(a)
                && ahead(b)
                && best.is_none_or(|(m, ..)| miss < m)
            {
                best = Some((miss, x, *a));
            }
        }
    }
    if let Some((_, x, line)) = best {
        return Snap {
            pos: x,
            hit: Hit::Free,
            kind: SnapKind::GuideIntersection,
            guide: guide_through(x, line.dir),
        };
    }

    // 3. One guide line near the cursor.
    let on_line = lines[..point_lines]
        .iter()
        .map(|l| ((cursor - l.origin).dot(perp(l.dir)).abs(), l))
        .filter(|(d, _)| *d <= LINE_TOL_M)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, l)) = on_line {
        let pos = l.origin + l.dir * (cursor - l.origin).dot(l.dir);
        if ctx.start.is_none_or(|s| s.distance(pos) > 1e-3) {
            return Snap {
                pos,
                hit: Hit::Free,
                kind: l.kind,
                guide: guide_through(pos, l.dir),
            };
        }
    }

    // 4. The angle of the leg from its start.
    let Some(start) = ctx.start else {
        return free(cursor);
    };
    let mut pos = cursor;
    let mut kind = SnapKind::None;
    let mut guide = None;
    if let Some(a) = angle_snap_guides(near.iter().map(|(_, c)| *c), cursor, start, ctx.refs) {
        pos = a.pos;
        guide = a.guide;
        kind = match a.kind {
            cp_net::SnapKind::KeyAngle(d) => SnapKind::KeyAngle(d),
            cp_net::SnapKind::Guide { perpendicular } => SnapKind::Parallel { perpendicular },
            cp_net::SnapKind::Angle(d) => SnapKind::Step(d),
            _ => SnapKind::None,
        };
    }

    // 5. The length along the leg.
    let d = pos - start;
    let len = d.length();
    if !ctx.length_step || len < 1e-3 {
        return Snap {
            pos,
            hit: Hit::Free,
            kind,
            guide,
        };
    }
    let dir = d / len;
    // The perpendicular through a major mark that the leg meets nearest to where it is.
    let mut mark: Option<(f32, f32, &ControlMark)> = None;
    for m in ctx.marks.iter().filter(|m| m.major) {
        let denom = dir.dot(m.dir);
        if m.dir == Vec2::ZERO || denom.abs() < 0.2 {
            continue;
        }
        let u = (m.pos - start).dot(m.dir) / denom;
        let miss = (u - len).abs();
        if u >= ZONE_CELL && miss <= CONTROL_LINE_TOL && mark.is_none_or(|(b, ..)| miss < b) {
            mark = Some((miss, u, m));
        }
    }
    if let Some((_, u, m)) = mark {
        let n = perp(m.dir);
        return Snap {
            pos: start + dir * u,
            hit: Hit::Free,
            kind: SnapKind::Mark(m.along.round() as i32),
            guide: Some((m.pos - n * GUIDE_HALF_LENGTH, m.pos + n * GUIDE_HALF_LENGTH)),
        };
    }
    let stepped = ((len / ZONE_CELL).round() * ZONE_CELL).max(ZONE_CELL);
    Snap {
        pos: start + dir * stepped,
        hit: Hit::Free,
        kind: SnapKind::Length(stepped as i32),
        guide,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::Vec3;

    fn line(a: Vec2, b: Vec2) -> CubicBezier3 {
        CubicBezier3::line(Vec3::new(a.x, 0.0, a.y), Vec3::new(b.x, 0.0, b.y))
    }

    fn ctx<'a>(
        nodes: &'a [SnapNode],
        edges: &'a [SnapEdge],
        guides: &'a [CubicBezier3],
    ) -> SnapCtx<'a> {
        SnapCtx {
            nodes,
            edges,
            guides,
            marks: &[],
            start: None,
            refs: &[],
            snaps: true,
            length_step: true,
        }
    }

    /// A street along x from (-100, 0) to (100, 0), half width 6, with a node at (0, 0) and
    /// the street's own end at (100, 0).
    fn street() -> (Vec<SnapNode>, Vec<SnapEdge>, Vec<CubicBezier3>) {
        let c = line(Vec2::new(-100.0, 0.0), Vec2::new(100.0, 0.0));
        (
            vec![SnapNode::radial(Vec2::new(0.0, 0.0), 8.0)],
            vec![SnapEdge {
                curve: c,
                reach: 6.0,
            }],
            vec![c],
        )
    }

    #[test]
    fn a_node_beats_an_edge() {
        let (n, e, g) = street();
        // 5 m from the node, on the street's asphalt: the node.
        let s = resolve(Vec2::new(5.0, 3.0), &ctx(&n, &e, &g));
        assert_eq!(
            (s.hit, s.kind, s.pos),
            (Hit::Node(0), SnapKind::Node, Vec2::ZERO)
        );
        // 20 m along, 3 m off the axis: the edge, on its axis.
        let s = resolve(Vec2::new(20.0, 3.0), &ctx(&n, &e, &g));
        assert!(matches!(s.hit, Hit::Edge(0, _)) && s.kind == SnapKind::Edge);
        assert!(
            (s.pos - Vec2::new(20.0, 0.0)).length() < 1e-3,
            "{:?}",
            s.pos
        );
    }

    #[test]
    fn an_edge_beats_the_intersection_of_two_guides() {
        let (n, e, g) = street();
        // The perpendicular through the end (100, 0) and the extension meet at the end; a cursor
        // on the street's asphalt near the end is an edge hit, not a guide.
        let s = resolve(Vec2::new(98.0, 2.0), &ctx(&n, &e, &g));
        assert_eq!(s.kind, SnapKind::Edge);
    }

    #[test]
    fn the_intersection_of_two_guides_beats_one_guide() {
        // A street ends at (100, 0) going east, another at (50, 40) going north. The
        // perpendicular through the first's end (x = 100) meets the perpendicular through the
        // second's end (y = 40) at (100, 40).
        let a = line(Vec2::new(30.0, 0.0), Vec2::new(100.0, 0.0));
        let b = line(Vec2::new(50.0, 10.0), Vec2::new(50.0, 40.0));
        let guides = [a, b];
        let s = resolve(Vec2::new(101.5, 38.5), &ctx(&[], &[], &guides));
        assert_eq!(s.kind, SnapKind::GuideIntersection);
        assert!(
            (s.pos - Vec2::new(100.0, 40.0)).length() < 1e-3,
            "{:?}",
            s.pos
        );
        // 3.5 m from that line, 20 m from the crossing: the one line, x = 100.
        let s = resolve(Vec2::new(103.5, 20.0), &ctx(&[], &[], &guides));
        assert_eq!(s.kind, SnapKind::Perpendicular);
        assert!(
            (s.pos - Vec2::new(100.0, 20.0)).length() < 1e-3,
            "{:?}",
            s.pos
        );
    }

    #[test]
    fn a_guide_line_beats_the_angle_and_the_length() {
        // A street ends at (100, 0); a leg from (60, 60) is aimed at the extension (y = 0) beyond
        // it, 4 m off. The line wins: the point lands on it, not on a step of the leg.
        let a = line(Vec2::new(30.0, 0.0), Vec2::new(100.0, 0.0));
        let guides = [a];
        let mut c = ctx(&[], &[], &guides);
        c.start = Some(Vec2::new(150.0, 60.0));
        let refs = [Vec2::new(0.0, -1.0)];
        c.refs = &refs;
        let s = resolve(Vec2::new(131.0, 3.0), &c);
        assert_eq!(s.kind, SnapKind::Extension);
        assert!((s.pos.y).abs() < 1e-3, "{:?}", s.pos);
        assert!((s.pos.x - 131.0).abs() < 1.0);
    }

    #[test]
    fn the_angle_comes_before_the_length() {
        // A leg from the origin with a reference along +x, the cursor 8 degrees off 90: the key
        // angle turns it to 90, then the length goes to a multiple of 8 m.
        let mut c = ctx(&[], &[], &[]);
        c.start = Some(Vec2::ZERO);
        let refs = [Vec2::X];
        c.refs = &refs;
        let cursor = Vec2::from_angle(86.0_f32.to_radians()) * 43.0;
        let s = resolve(cursor, &c);
        assert_eq!(s.kind, SnapKind::Length(40));
        assert!(
            s.pos.x.abs() < 1e-3 && (s.pos.y - 40.0).abs() < 1e-3,
            "{:?}",
            s.pos
        );
        // Without the length step the key angle stays, at the leg's own length.
        c.length_step = false;
        let s = resolve(cursor, &c);
        assert_eq!(s.kind, SnapKind::KeyAngle(90));
        assert!(
            s.pos.x.abs() < 1e-3 && (s.pos.y - 43.0).abs() < 1e-3,
            "{:?}",
            s.pos
        );
    }

    #[test]
    fn a_zone_mark_beats_the_8_m_step() {
        // The leg runs along +x from the origin; a major mark's perpendicular (x = 77) is 2 m
        // from where the cursor is: the length jumps onto it, not to 80.
        let mark = ControlMark {
            pos: Vec2::new(77.0, 30.0),
            dir: Vec2::X,
            along: 64.0,
            major: true,
            onward: Vec2::X,
        };
        let marks = [mark];
        let mut c = ctx(&[], &[], &[]);
        c.marks = &marks;
        c.start = Some(Vec2::ZERO);
        let s = resolve(Vec2::new(79.0, 0.2), &c);
        assert_eq!(s.kind, SnapKind::Mark(64));
        assert!((s.pos.x - 77.0).abs() < 0.1, "{:?}", s.pos);
        let s = resolve(Vec2::new(59.0, 0.1), &c);
        assert_eq!(s.kind, SnapKind::Length(56));
    }

    #[test]
    fn at_an_acute_junction_a_point_on_the_other_arms_asphalt_is_the_node() {
        // Two arms of 6-lane streets (half width 14) at 30 degrees, reach 14. A point 13 m out on
        // one arm, 6.5 m from the other's axis, is inside; a radius of 8 would miss it.
        let node = SnapNode {
            pos: Vec2::ZERO,
            reach: 14.0,
            arms: vec![
                SnapArm {
                    dir: Vec2::X,
                    half_width: 14.0,
                },
                SnapArm {
                    dir: Vec2::from_angle(30f32.to_radians()),
                    half_width: 14.0,
                },
            ],
        };
        let on_second = Vec2::from_angle(30f32.to_radians()) * 13.0;
        assert!(node.contains(on_second));
        assert!(node.contains(Vec2::new(13.0, 0.0)));
        // Beyond the reach along the arms and off the other arm's asphalt: outside.
        assert!(!node.contains(Vec2::new(20.0, 0.0)));
        assert!(!node.contains(Vec2::new(-12.0, 0.0)));
        let nodes = [node];
        let s = resolve(on_second, &ctx(&nodes, &[], &[]));
        assert_eq!((s.hit, s.pos), (Hit::Node(0), Vec2::ZERO));
    }

    #[test]
    fn with_snaps_off_only_a_direct_hit_remains() {
        let (n, e, g) = street();
        let mut c = ctx(&n, &e, &g);
        c.snaps = false;
        c.start = Some(Vec2::new(0.0, 60.0));
        // A free point 40 m off the street stays exactly where it is: no angle, no length.
        let p = Vec2::new(31.3, 47.7);
        assert_eq!(resolve(p, &c), free(p));
        // A hit on the node or on the asphalt still resolves.
        assert_eq!(resolve(Vec2::new(4.0, 1.0), &c).hit, Hit::Node(0));
        assert!(matches!(
            resolve(Vec2::new(40.0, 4.0), &c).hit,
            Hit::Edge(0, _)
        ));
    }

    #[test]
    fn the_start_is_not_a_node_to_snap_to() {
        let n = [SnapNode::radial(Vec2::new(10.0, 10.0), 8.0)];
        let mut c = ctx(&n, &[], &[]);
        c.start = Some(Vec2::new(10.0, 10.0));
        assert_eq!(resolve(Vec2::new(12.0, 11.0), &c).hit, Hit::Free);
        c.start = None;
        assert_eq!(resolve(Vec2::new(12.0, 11.0), &c).hit, Hit::Node(0));
    }
}
