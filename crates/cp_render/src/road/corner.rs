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

use super::{CURB, KERB, SIDEWALK, cross_section, end_corners, flat_quad};
use crate::chunk::MeshData;

/// Longest a mitre may reach, in sidewalk widths (sharp corners fall back to a plain offset).
const MITRE_LIMIT: f32 = 4.0;

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
                (None, None) => 0.0,
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
#[derive(Clone, Copy, Debug)]
pub(super) struct Patch {
    pub vertex: usize,
    pub quad: [Vec2; 4],
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
        out.push(Patch {
            vertex: i,
            quad: [v, pa, x, pb],
        });
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
        flat_quad(top, pt.quad.map(|v| at(v, CURB)), Vec3::Y, SIDEWALK);
        // The two faces towards the road: pa -> x and x -> pb.
        for (p, q) in [(pt.quad[1], pt.quad[2]), (pt.quad[2], pt.quad[3])] {
            let n = (q - p).perp().try_normalize().unwrap_or(Vec2::Y);
            // Away from the vertex: the quad's own side is towards it.
            let away = if n.dot(pt.quad[0] - p) > 0.0 { -n } else { n };
            flat_quad(
                kerb,
                [at(p, CURB), at(q, CURB), at(q, 0.0), at(p, 0.0)],
                Vec3::new(away.x, 0.0, away.y),
                KERB,
            );
        }
    }
}
