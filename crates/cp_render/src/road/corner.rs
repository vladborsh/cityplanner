//! Sidewalks around a junction polygon (BUG-023 / BUG-024).
//!
//! The junction polygon is flat asphalt that spans the whole width of every arm, sidewalks
//! included, so the sidewalks of the arms end at its edges. This module adds the missing
//! pavement: along every *outer* edge of the polygon (an edge between two arms, or around the
//! outer corner of a bend) a raised band, as wide as the neighbouring arms' sidewalks, that
//! joins the arms' sidewalks into one walk around the corner. The edges that are the end line
//! of an arm stay open: that is where the carriageway enters the junction, so the sidewalk of
//! a road that runs into another one is cut there, and the other road's sidewalk wraps the
//! mouth of the T.

use bevy_math::{Vec2, Vec3};
use cp_net::{Arm, Junction, NetGraph};

use super::{CURB, KERB, SIDEWALK, cross_section, end_corners, flat_quad, push_vertex, tri};
use crate::chunk::MeshData;

/// Longest a mitre may reach, in sidewalk widths (sharp corners fall back to a plain offset).
const MITRE_LIMIT: f32 = 4.0;

/// Radius of the rounding of a sidewalk's curb corner at a junction mouth, metres (T-186). The
/// patch is only as long as the neighbouring sidewalk is wide, so the radius is clamped to it.
pub(super) const CURB_RADIUS: f32 = 3.0;
/// Radius of the inner curb where a bend's outer corner is rounded, metres (T-186).
pub(super) const BEND_RADIUS: f32 = 2.5;
/// Longest turn of one arc segment, radians (a 90 degree corner gets 4 segments).
const ARC_STEP: f32 = std::f32::consts::FRAC_PI_8;

/// The arc of radius up to `radius` that rounds the corner `c` between the lines `c -> a` and
/// `c -> b`, as points from the tangent point on `c -> a` to the one on `c -> b`. The tangent
/// length is clamped to `reach_a` / `reach_b` (so the radius shrinks to fit). `None` when the
/// corner is nearly straight or a needle, or the clamped radius is below `min_radius`.
pub(super) fn fillet(
    a: Vec2,
    c: Vec2,
    b: Vec2,
    radius: f32,
    min_radius: f32,
    reach_a: f32,
    reach_b: f32,
) -> Option<Vec<Vec2>> {
    let (u, v) = ((a - c).try_normalize()?, (b - c).try_normalize()?);
    let phi = u.dot(v).clamp(-1.0, 1.0).acos();
    // Interior angle between 6 and 174 degrees; the rest is a needle or a straight line.
    if !(0.1..=std::f32::consts::PI - 0.1).contains(&phi) {
        return None;
    }
    let half_tan = (phi * 0.5).tan();
    let t = (radius / half_tan).min(reach_a).min(reach_b);
    let r = t * half_tan;
    if t < 0.05 || r < min_radius || !r.is_finite() {
        return None;
    }
    let centre = c + (u + v).try_normalize()? * (r / (phi * 0.5).sin());
    let (p, q) = (c + u * t, c + v * t);
    let (a0, a1) = ((p - centre).to_angle(), (q - centre).to_angle());
    let mut sweep = a1 - a0;
    while sweep > std::f32::consts::PI {
        sweep -= std::f32::consts::TAU;
    }
    while sweep < -std::f32::consts::PI {
        sweep += std::f32::consts::TAU;
    }
    let n = ((sweep.abs() / ARC_STEP).ceil() as usize).clamp(2, 8);
    Some(
        (0..=n)
            .map(|i| match i {
                0 => p,
                _ if i == n => q,
                _ => centre + Vec2::from_angle(a0 + sweep * i as f32 / n as f32) * r,
            })
            .collect(),
    )
}

/// `polygon` with the outer meeting points (corners no arm owns: the outside of a bend) rounded
/// by an arc; `src[i]` is the index in `polygon` that point `i` of the result stands for.
pub(super) fn rounded_polygon(j: &Junction, graph: &NetGraph) -> (Vec<Vec2>, Vec<usize>) {
    let k = j.polygon.len();
    let owned = |p: Vec2| {
        j.arms
            .iter()
            .any(|a| a.right.distance(p) < 1e-4 || a.left.distance(p) < 1e-4)
    };
    let mut out = Vec::with_capacity(k);
    let mut src = Vec::with_capacity(k);
    for i in 0..k {
        let (pv, v, nx) = (
            j.polygon[(i + k - 1) % k],
            j.polygon[i],
            j.polygon[(i + 1) % k],
        );
        if !owned(v) && owned(pv) && owned(nx) {
            // Sidewalk width of the arms whose corners are the neighbours.
            let w = j
                .arms
                .iter()
                .flat_map(|a| [a.right, a.left].map(|c| (a, c)))
                .filter(|(_, c)| c.distance(pv) < 1e-4 || c.distance(nx) < 1e-4)
                .map(|(a, c)| walk_width(graph, a, c))
                .fold(0.0, f32::max);
            let reach = |q: Vec2| 0.9 * v.distance(q);
            if let Some(arc) = fillet(pv, v, nx, w + BEND_RADIUS, w + 0.3, reach(pv), reach(nx)) {
                src.extend(std::iter::repeat_n(i, arc.len()));
                out.extend(arc);
                continue;
            }
        }
        out.push(v);
        src.push(i);
    }
    (out, src)
}

/// One band along polygon edge `from -> from + 1`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Walk {
    pub from: usize,
    pub to: usize,
    /// Inner corners at `from` and `to` (XZ).
    pub inner: [Vec2; 2],
    /// Unit normal of the edge pointing into the polygon (XZ).
    pub inward: Vec2,
}

/// Width of the outermost raised band of `arm`'s road on the side where its corner `p` is
/// (0 when that edge of the road is not raised, e.g. a highway).
fn walk_width(graph: &NetGraph, arm: &Arm, p: Vec2) -> f32 {
    let Some(seg) = graph.segment(arm.segment) else {
        return 0.0;
    };
    let bands = cross_section(graph, seg, false);
    let (right, _) = end_corners(arm);
    let band = if right.distance(p) < 1e-4 {
        bands.first()
    } else {
        bands.last()
    };
    band.filter(|b| b.raise > 1e-3 && b.color == SIDEWALK)
        .map_or(0.0, |b| b.hi - b.lo)
}

fn line_hit(p: Vec2, d: Vec2, q: Vec2, e: Vec2) -> Option<Vec2> {
    let den = d.perp_dot(e);
    if den.abs() < 1e-4 {
        return None;
    }
    let t = (q - p).perp_dot(e) / den;
    Some(p + d * t)
}

/// The arm whose end line is polygon edge `a`-`b`.
fn end_line_arm(j: &Junction, a: Vec2, b: Vec2) -> Option<&Arm> {
    let hit = |p: Vec2, q: Vec2| p.distance(q) < 1e-3;
    j.arms.iter().find(|arm| {
        (hit(arm.right, a) && hit(arm.left, b)) || (hit(arm.left, a) && hit(arm.right, b))
    })
}

/// The sidewalk bands of `j`, one per outer polygon edge that has a raised neighbour.
pub(super) fn walks(graph: &NetGraph, j: &Junction) -> Vec<Walk> {
    let k = j.polygon.len();
    // Width at each polygon vertex: the widest sidewalk among the arms that end in it
    // (two arms share a corner on the inside of a bend or a T), `None` for an outer meeting
    // point that no arm owns.
    let at_vertex: Vec<Option<f32>> = j
        .polygon
        .iter()
        .map(|&p| {
            j.arms
                .iter()
                .filter(|a| a.right.distance(p) < 1e-4 || a.left.distance(p) < 1e-4)
                .map(|a| walk_width(graph, a, p))
                .reduce(f32::max)
        })
        .collect();
    let same_arm = |i: usize| end_line_arm(j, j.polygon[i], j.polygon[(i + 1) % k]).is_some();
    // Per edge: width, or `None` when the edge is an arm's end line or carries no sidewalk.
    let width: Vec<Option<f32>> = (0..k)
        .map(|i| {
            if same_arm(i) {
                return None;
            }
            let (a, b) = (at_vertex[i], at_vertex[(i + 1) % k]);
            let w = match (a, b) {
                (Some(a), Some(b)) => a.max(b),
                (Some(w), None) | (None, Some(w)) => w,
                // Inside a rounded corner: the widest sidewalk at the ends of the run.
                (None, None) => {
                    let back = (1..k).find_map(|n| at_vertex[(i + k - n) % k]);
                    let fwd = (1..k).find_map(|n| at_vertex[(i + 1 + n) % k]);
                    back.into_iter().chain(fwd).fold(0.0, f32::max)
                }
            };
            let len = j.polygon[i].distance(j.polygon[(i + 1) % k]);
            (w > 1e-3 && len > 1e-3).then_some(w.min(len * 0.5))
        })
        .collect();
    let edge = |i: usize| {
        let (a, b) = (j.polygon[i], j.polygon[(i + 1) % k]);
        (a, (b - a).normalize_or_zero())
    };
    // The inner end of the band of edge `i` at its vertex `v` (`v` = `i` or `i + 1`), where
    // `other` is the neighbouring edge there.
    let inner_at = |i: usize, v: usize, other: usize| -> Vec2 {
        let (p, d) = edge(i);
        let w = width[i].unwrap_or(0.0);
        let n = d.perp();
        let base = j.polygon[v];
        let plain = base + n * w;
        let off = (p + n * w, d);
        let hit = match width[other] {
            // Another band: mitre between the two offset lines.
            Some(w2) => {
                let (q, e) = edge(other);
                line_hit(off.0, off.1, q + e.perp() * w2, e)
            }
            // An arm's end line: slide along it.
            None => {
                let (q, e) = edge(other);
                line_hit(off.0, off.1, q, e)
            }
        };
        hit.filter(|h| h.distance(base) <= MITRE_LIMIT * w.max(0.1))
            .unwrap_or(plain)
    };
    (0..k)
        .filter(|&i| width[i].is_some())
        .map(|i| {
            let next = (i + 1) % k;
            let prev = (i + k - 1) % k;
            Walk {
                from: i,
                to: next,
                inner: [inner_at(i, i, prev), inner_at(i, next, next)],
                inward: edge(i).1.perp(),
            }
        })
        .collect()
}

/// Raised band tops (into `top`) and kerb faces (into `kerb`, near mesh only) for `walks`;
/// `ys` are the polygon vertex heights.
pub(super) fn build(
    j: &Junction,
    walks: &[Walk],
    ys: &[f32],
    top: &mut MeshData,
    kerb: &mut MeshData,
) {
    for w in walks {
        let (a, b) = (j.polygon[w.from], j.polygon[w.to]);
        let (ya, yb) = (ys[w.from], ys[w.to]);
        let p = |v: Vec2, y: f32| Vec3::new(v.x, y + CURB, v.y);
        flat_quad(
            top,
            [p(a, ya), p(b, yb), p(w.inner[1], yb), p(w.inner[0], ya)],
            Vec3::Y,
            SIDEWALK,
        );
        // The face towards the road.
        let face = |v: Vec2, y: f32, up: f32| Vec3::new(v.x, y + up, v.y);
        flat_quad(
            kerb,
            [
                face(w.inner[0], ya, CURB),
                face(w.inner[1], yb, CURB),
                face(w.inner[1], yb, 0.0),
                face(w.inner[0], ya, 0.0),
            ],
            Vec3::new(w.inward.x, 0.0, w.inward.y),
            KERB,
        );
    }
}

/// A pavement patch in an inside corner where two arms' end lines meet at a polygon vertex
/// (the T's two mouth corners, the inside of a bend, every corner of a cross): it joins the
/// sidewalks of the two roads, so walkers can turn the corner without crossing asphalt.
#[derive(Clone, Debug)]
pub(super) struct Patch {
    pub vertex: usize,
    /// The vertex, then the outline of the walk-side edge: `pa`, the rounded curb corner (an
    /// arc, T-186; a single point where it cannot be rounded), `pb`.
    pub outline: Vec<Vec2>,
}

pub(super) fn patches(graph: &NetGraph, j: &Junction) -> Vec<Patch> {
    let k = j.polygon.len();
    let mut out = Vec::new();
    for i in 0..k {
        let (pv, v, nx) = (
            j.polygon[(i + k - 1) % k],
            j.polygon[i],
            j.polygon[(i + 1) % k],
        );
        let (Some(a), Some(b)) = (end_line_arm(j, pv, v), end_line_arm(j, v, nx)) else {
            continue;
        };
        if (a.segment, a.at_start) == (b.segment, b.at_start) {
            continue;
        }
        let (wa, wb) = (walk_width(graph, a, v), walk_width(graph, b, v));
        if wa < 1e-3 && wb < 1e-3 {
            continue;
        }
        let (da, db) = ((pv - v).normalize_or_zero(), (nx - v).normalize_or_zero());
        let (pa, pb) = (v + da * wa, v + db * wb);
        // The inner edges of the two sidewalks run along the arms' axes into the polygon.
        let (axis_a, axis_b) = ((-da).perp(), db.perp());
        let x = line_hit(pa, axis_a, pb, axis_b)
            .filter(|x| x.distance(v) <= MITRE_LIMIT * wa.max(wb))
            .unwrap_or(pa + pb - v);
        let mut outline = vec![v];
        match fillet(pa, x, pb, CURB_RADIUS, 0.0, x.distance(pa), x.distance(pb)) {
            Some(arc) => outline.extend(arc),
            None => outline.extend([pa, x, pb]),
        }
        out.push(Patch { vertex: i, outline });
    }
    out
}

/// Top and kerb faces of the corner patches.
pub(super) fn build_patches(
    patches: &[Patch],
    ys: &[f32],
    top: &mut MeshData,
    kerb: &mut MeshData,
) {
    for pt in patches {
        let y = ys[pt.vertex];
        let at = |v: Vec2, up: f32| Vec3::new(v.x, y + up, v.y);
        // A fan from the vertex over the (convex) outline.
        let ids: Vec<u32> = pt
            .outline
            .iter()
            .map(|&v| push_vertex(top, at(v, CURB), Vec3::Y, SIDEWALK))
            .collect();
        for w in ids[1..].windows(2) {
            tri(top, ids[0], w[0], w[1], Vec3::Y);
        }
        // The faces towards the road, along the outline from `pa` to `pb`.
        for w in pt.outline[1..].windows(2) {
            let (p, q) = (w[0], w[1]);
            let n = (q - p).perp().try_normalize().unwrap_or(Vec2::Y);
            // Away from the vertex: the outline's own side is towards it.
            let away = if n.dot(pt.outline[0] - p) > 0.0 {
                -n
            } else {
                n
            };
            flat_quad(
                kerb,
                [at(p, CURB), at(q, CURB), at(q, 0.0), at(p, 0.0)],
                Vec3::new(away.x, 0.0, away.y),
                KERB,
            );
        }
    }
}

#[cfg(test)]
mod fillet_tests {
    use super::*;

    /// Corner at the origin with arms along +x and a direction `deg` degrees counter-clockwise.
    fn corner(deg: f32, radius: f32, reach: f32) -> Option<Vec<Vec2>> {
        let b = Vec2::from_angle(deg.to_radians()) * 20.0;
        fillet(
            Vec2::new(20.0, 0.0),
            Vec2::ZERO,
            b,
            radius,
            0.0,
            reach,
            reach,
        )
    }

    #[test]
    fn a_right_angle_is_a_quarter_circle_of_the_asked_radius() {
        let arc = corner(90.0, 3.0, 20.0).unwrap();
        assert!(arc.len() >= 5);
        let centre = Vec2::new(3.0, 3.0);
        for p in &arc {
            assert!((p.distance(centre) - 3.0).abs() < 1e-3, "{p:?}");
        }
        assert!(arc[0].distance(Vec2::new(3.0, 0.0)) < 1e-3);
        assert!(arc.last().unwrap().distance(Vec2::new(0.0, 3.0)) < 1e-3);
    }

    #[test]
    fn the_radius_shrinks_to_the_reach_and_no_angle_gives_nan() {
        let arc = corner(90.0, 3.0, 1.5).unwrap();
        assert!((arc[0].x - 1.5).abs() < 1e-3);
        for deg in [1.0, 5.0, 30.0, 90.0, 150.0, 175.0, 179.0, 180.0, 359.0] {
            if let Some(arc) = corner(deg, 3.0, 10.0) {
                assert!(arc.len() <= 9);
                assert!(
                    arc.iter().all(|p| p.is_finite() && p.length() <= 20.0),
                    "{deg}: {arc:?}"
                );
            }
        }
        assert!(corner(1.0, 3.0, 10.0).is_none() && corner(179.0, 3.0, 10.0).is_none());
    }
}
