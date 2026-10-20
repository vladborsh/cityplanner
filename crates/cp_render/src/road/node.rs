//! Node pieces v2 (`architecture/system-design.md` §6.8.3 item 6, T-263). The class `cp_net`
//! gives a node decides its piece, and every point comes from topology (arm, side, band), never
//! from matching coordinates:
//! * `End`: a cap around the dead end: carriageway, and the sidewalks running round it.
//! * `Middle`: no piece; the two segment ends meet on one mitred end line.
//! * `Bend` and `Transition`: a joint between the two cuts, built from lines across the node
//!   ([`cp_net::side_line`]). A bend has a line at every band boundary of its road, so lanes,
//!   markings and sidewalks run on through it (mitred on the inside, round on the outside). A
//!   transition has lines at the edges and kerbs only: the wider road narrows to the other, and
//!   a sidewalk next to a road without one narrows to nothing.
//! * `Junction`: the carriageway between the arms' kerbs at their cuts and the curb curves; each
//!   corner's sidewalk between its curb curve and its outer curve, narrowing to nothing towards
//!   an arm without a sidewalk.
//!
//! Heights come from the arms' end rings (the same rings their segments end with) and run
//! linearly across the node; the piece is not lifted to a common level.

use bevy_math::{Vec2, Vec3};
use cp_net::{Arm, Elevation, Junction, NetGraph, NodeClass, SideLine, side_line};
use cp_terrain::Terrain;

use super::*;

/// How far a junction's corner may reach beyond the arms' half widths, metres (the curb curves
/// of `cp_net` round the corner within it; the junction sweep allows it).
#[cfg(test)]
pub(super) const CURB_RADIUS: f32 = 3.0;
/// Spacing of the points of the lines across a node, metres.
const SAMPLE: f32 = 1.0;

/// One road arm at its cut, as its segment ends there.
struct ArmEnd<'a> {
    arm: &'a Arm,
    /// The segment's end ring (with the corners and height its segment uses).
    ring: Ring,
    /// `xi = sign * x`: the arm's lateral (to the left looking out of the node) from the
    /// segment's own (to the left of its travel direction).
    sign: f32,
    bands: Vec<Band>,
    /// Segment `x` of the outer edge, of the kerb (where the carriageway ends) and the sidewalk
    /// width, on the arm's right (`[0]`, `xi < 0`) and left (`[1]`) sides.
    edge: [f32; 2],
    kerb: [f32; 2],
    walk: [f32; 2],
    path: bool,
}

impl ArmEnd<'_> {
    fn xz(&self, x: f32) -> Vec2 {
        self.ring.xz(x)
    }
    fn y(&self) -> f32 {
        self.ring.center.y
    }
}

fn arm_end<'a>(
    graph: &NetGraph,
    geo: &JunctionGeometry,
    terrain: &Terrain,
    arm: &'a Arm,
    markings: bool,
) -> Option<ArmEnd<'a>> {
    let seg = graph.segment(arm.segment)?;
    let f = ground::frame_of(graph, geo, arm.segment)?;
    let mut ring = f.ring(if arm.at_start { 0.0 } else { f.length });
    ring.center.y = ground::end_level(graph, geo, terrain, arm.segment, arm.at_start);
    let bands = cross_section(graph, seg, markings);
    let sign = if arm.at_start { -1.0 } else { 1.0 };
    let side = |b: &Band, outer: f32, inner: f32| {
        if b.raise > 1e-3 && b.color == SIDEWALK {
            (outer, inner, b.hi - b.lo)
        } else {
            (outer, outer, 0.0)
        }
    };
    let (first, last) = (*bands.first()?, *bands.last()?);
    let (neg_x, pos_x) = (
        side(&first, first.lo, first.hi),
        side(&last, last.hi, last.lo),
    );
    let (right, left) = if sign > 0.0 {
        (neg_x, pos_x)
    } else {
        (pos_x, neg_x)
    };
    Some(ArmEnd {
        arm,
        ring,
        sign,
        bands,
        edge: [right.0, left.0],
        kerb: [right.1, left.1],
        walk: [right.2, left.2],
        path: is_path(graph, seg),
    })
}

fn poly_len(p: &[Vec2]) -> f32 {
    p.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// `n >= 1` points evenly spaced by arc length along `p` (its ends included).
fn resample(p: &[Vec2], n: usize) -> Vec<Vec2> {
    let total = poly_len(p);
    if n < 2 || total < 1e-6 {
        return vec![p[0]; n.max(1)];
    }
    let mut out = Vec::with_capacity(n);
    let (mut seg, mut acc) = (0usize, 0.0f32);
    for i in 0..n {
        let target = total * i as f32 / (n - 1) as f32;
        while seg + 2 < p.len() && acc + p[seg].distance(p[seg + 1]) < target {
            acc += p[seg].distance(p[seg + 1]);
            seg += 1;
        }
        let l = p[seg].distance(p[seg + 1]).max(1e-9);
        let t = ((target - acc) / l).clamp(0.0, 1.0);
        out.push(p[seg].lerp(p[seg + 1], t));
    }
    *out.last_mut().expect("points") = *p.last().expect("points");
    out
}

/// The lines resampled alike part by part: point `i` of one line faces point `i` of the next.
fn align(lines: &[SideLine]) -> Vec<Vec<Vec2>> {
    let count = |part: fn(&SideLine) -> &[Vec2]| {
        lines
            .iter()
            .map(|l| {
                let p = part(l);
                if p.len() < 2 {
                    1
                } else {
                    // Straight two-point parts stay two points, so corners are kept exactly.
                    ((poly_len(p) / SAMPLE).ceil() as usize + 1).min(p.len().max(2) * 8)
                }
            })
            .max()
            .unwrap_or(1)
            .max(if lines.iter().any(|l| part(l).len() >= 2) {
                2
            } else {
                1
            })
    };
    let (na, nj, nb) = (count(|l| &l.a), count(|l| &l.joint), count(|l| &l.b));
    lines
        .iter()
        .map(|l| {
            let mut out = resample(&l.a, na);
            out.extend(resample(&l.joint, nj).into_iter().skip(1));
            out.extend(resample(&l.b, nb).into_iter().skip(1));
            out
        })
        .collect()
}

fn reversed(l: SideLine) -> SideLine {
    let rev = |mut v: Vec<Vec2>| {
        v.reverse();
        v
    };
    SideLine {
        a: rev(l.b),
        joint: rev(l.joint),
        b: rev(l.a),
    }
}

/// A line across a node of degree 2 from `a`'s segment lateral `xa` to `b`'s `xb` (walking
/// from `a` through the node to `b`); its ends are exactly the points the end rings have there.
fn joint_line(
    graph: &NetGraph,
    j: &Junction,
    a: &ArmEnd,
    b: &ArmEnd,
    xa: f32,
    xb: f32,
) -> SideLine {
    let (ea, eb) = (a.sign * xa, b.sign * xb);
    // Walking right is `a`'s left and `b`'s right: the counter-clockwise gap from `a` to `b`.
    let mut l = if ea > 0.0 || (ea == 0.0 && eb <= 0.0) {
        side_line(graph, j, a.arm, b.arm, ea.max(0.0), (-eb).max(0.0), false)
    } else {
        reversed(side_line(
            graph,
            j,
            b.arm,
            a.arm,
            eb.max(0.0),
            (-ea).max(0.0),
            false,
        ))
    };
    l.a[0] = a.xz(xa);
    *l.b.last_mut().expect("points") = b.xz(xb);
    if l.joint.len() == 1 && l.a.len() == 1 && l.b.len() == 1 {
        // One point: both ends are the very same corner.
        l.b[0] = l.a[0];
        l.joint[0] = l.a[0];
    }
    l
}

fn lerp_y(ya: f32, yb: f32, i: usize, n: usize) -> f32 {
    if n < 2 {
        ya
    } else {
        ya + (yb - ya) * i as f32 / (n - 1) as f32
    }
}

/// Points of line `p` at heights from `ya` to `yb`, raised by `raise`.
fn lift(p: &[Vec2], ya: f32, yb: f32, raise: f32) -> Vec<Vec3> {
    p.iter()
        .enumerate()
        .map(|(i, q)| Vec3::new(q.x, lerp_y(ya, yb, i, p.len()) + raise, q.y))
        .collect()
}

/// The strip between lines `lo` and `hi` (same length), coloured `color(i)` for quad `i`.
fn strip(m: &mut MeshData, lo: &[Vec3], hi: &[Vec3], color: impl Fn(usize) -> [f32; 4]) {
    for i in 0..lo.len().saturating_sub(1) {
        let q = [lo[i], lo[i + 1], hi[i + 1], hi[i]];
        if (q[1] - q[0]).length() + (q[2] - q[3]).length() < 1e-6 {
            continue;
        }
        flat_quad(m, q, Vec3::Y, color(i));
    }
}

/// A vertical face along `p` from `p.y + low` to `p.y + high`, facing towards `towards`.
fn wall(m: &mut MeshData, p: &[Vec3], low: f32, high: f32, towards: &[Vec3], color: [f32; 4]) {
    for i in 0..p.len().saturating_sub(1) {
        let (a, b) = (p[i], p[i + 1]);
        if a.distance(b) < 1e-6 {
            continue;
        }
        let mid = (towards[i] + towards[i + 1]) * 0.5 - (a + b) * 0.5;
        let facing = Vec3::new(mid.x, 0.0, mid.z)
            .try_normalize()
            .unwrap_or(Vec3::X);
        flat_quad(
            m,
            [
                a + Vec3::Y * high,
                b + Vec3::Y * high,
                b + Vec3::Y * low,
                a + Vec3::Y * low,
            ],
            facing,
            color,
        );
    }
}

/// Skirt (ground) or fascia (raised) under the outer line `p`, whose inside is towards `inside`.
fn edge(m: &mut MeshData, terrain: &Terrain, raised: bool, p: &[Vec3], inside: &[Vec3]) {
    let away: Vec<Vec3> = p.iter().zip(inside).map(|(a, b)| *a * 2.0 - *b).collect();
    for i in 0..p.len().saturating_sub(1) {
        let (a, b) = (p[i], p[i + 1]);
        if a.distance(b) < 1e-6 {
            continue;
        }
        let (da, db) = if raised {
            (DECK, DECK)
        } else {
            (
                skirt_depth(terrain, Vec2::new(a.x, a.z), a.y),
                skirt_depth(terrain, Vec2::new(b.x, b.z), b.y),
            )
        };
        let mid = (away[i] + away[i + 1]) * 0.5 - (a + b) * 0.5;
        let facing = Vec3::new(mid.x, 0.0, mid.z)
            .try_normalize()
            .unwrap_or(Vec3::X);
        flat_quad(
            m,
            [a, b, b - Vec3::Y * db, a - Vec3::Y * da],
            facing,
            if raised { CONCRETE } else { EMBANKMENT },
        );
    }
}

/// Underside of a raised piece: the outline at `DECK` below, facing down.
fn underside(m: &mut MeshData, outline: &[Vec3]) {
    let poly: Vec<Vec2> = outline.iter().map(|p| Vec2::new(p.x, p.z)).collect();
    let ids: Vec<u32> = outline
        .iter()
        .map(|p| push_vertex(m, *p - Vec3::Y * DECK, -Vec3::Y, CONCRETE))
        .collect();
    for [a, b, c] in triangulate(&poly) {
        tri(m, ids[a], ids[b], ids[c], -Vec3::Y);
    }
}

fn node_pier(m: &mut MeshData, graph: &NetGraph, terrain: &Terrain, j: &Junction) {
    let ground = terrain.height(to_xz(j.pos));
    let top = j.pos.y + LIFT - DECK;
    if top - ground > 0.5 && graph.node(j.node).is_some() {
        pier(m, Vec3::new(j.pos.x, ground, j.pos.z), top);
    }
}

/// Length of the stretch of the middle line of a joint, for its dashes.
fn dash_len(lines: &[Vec<Vec2>]) -> f32 {
    lines.get(lines.len() / 2).map_or(0.0, |l| poly_len(l))
}

/// The piece of a bend or a transition.
fn joint(
    graph: &NetGraph,
    terrain: &Terrain,
    j: &Junction,
    a: &ArmEnd,
    b: &ArmEnd,
    markings: bool,
    raised: bool,
) -> MeshData {
    // Lines `(x on a, x on b)` from the walking right to the walking left, and the bands
    // between neighbouring lines.
    let mut xs: Vec<(f32, f32)> = Vec::new();
    let mut bands: Vec<Band> = Vec::new();
    if j.class == NodeClass::Bend {
        // `x` on `b` of a lateral `x` on `a`: the same walking lateral.
        let on_b = |x: f32| b.sign * -(a.sign * x);
        // Walking right first: the walking lateral is `-a.sign * x`.
        let mut list = a.bands.clone();
        if a.sign > 0.0 {
            list.reverse();
        }
        for band in &list {
            let (lo, hi) = if a.sign > 0.0 {
                (band.hi, band.lo)
            } else {
                (band.lo, band.hi)
            };
            if xs.is_empty() {
                xs.push((lo, on_b(lo)));
            }
            xs.push((hi, on_b(hi)));
            bands.push(*band);
        }
    } else {
        let floor = if a.path && b.path { GRAVEL } else { ASPHALT };
        let walk = |w: bool| Band {
            lo: 0.0,
            hi: 0.0,
            color: if w { SIDEWALK } else { floor },
            raise: if w { CURB } else { 0.0 },
            dashed: false,
        };
        xs = vec![
            (a.edge[1], b.edge[0]),
            (a.kerb[1], b.kerb[0]),
            (a.kerb[0], b.kerb[1]),
            (a.edge[0], b.edge[1]),
        ];
        bands = vec![
            walk(a.walk[1] > 0.0 || b.walk[0] > 0.0),
            walk(false),
            walk(a.walk[0] > 0.0 || b.walk[1] > 0.0),
        ];
    }
    let lines: Vec<SideLine> = xs
        .iter()
        .map(|&(xa, xb)| joint_line(graph, j, a, b, xa, xb))
        .collect();
    let pts = align(&lines);
    let (ya, yb) = (a.y(), b.y());
    let length = dash_len(&pts);
    let period = dash_period(length);
    let mut m = MeshData::default();
    for (k, band) in bands.iter().enumerate() {
        let (lo, hi) = (
            lift(&pts[k], ya, yb, band.raise),
            lift(&pts[k + 1], ya, yb, band.raise),
        );
        let n = lo.len();
        strip(&mut m, &lo, &hi, |i| {
            if band.dashed {
                let s = |i: usize| length * i as f32 / (n - 1).max(1) as f32;
                if dash_on(s(i), s(i + 1), period) {
                    band.color
                } else {
                    ASPHALT
                }
            } else {
                band.color
            }
        });
    }
    if markings {
        // Kerb faces where neighbouring bands differ in height, towards the lower one.
        for k in 1..bands.len() {
            let (l, h) = (bands[k - 1], bands[k]);
            if (l.raise - h.raise).abs() < 1e-3 {
                continue;
            }
            let (low, high, towards) = if l.raise < h.raise {
                (l.raise, h.raise, k - 1)
            } else {
                (h.raise, l.raise, k + 1)
            };
            let line = lift(&pts[k], ya, yb, 0.0);
            let other = lift(&pts[towards], ya, yb, 0.0);
            wall(&mut m, &line, low, high, &other, KERB);
        }
    }
    let last = pts.len() - 1;
    for (k, inner, band) in [(0, 1, bands[0]), (last, last - 1, bands[bands.len() - 1])] {
        let p = lift(&pts[k], ya, yb, band.raise);
        let q = lift(&pts[inner], ya, yb, band.raise);
        edge(&mut m, terrain, raised, &p, &q);
    }
    if raised {
        let mut outline = lift(&pts[0], ya, yb, 0.0);
        let mut back = lift(&pts[last], ya, yb, 0.0);
        back.reverse();
        outline.extend(back);
        underside(&mut m, &outline);
        node_pier(&mut m, graph, terrain, j);
    }
    m
}

/// The piece of a junction of three or more arms.
fn junction(
    graph: &NetGraph,
    terrain: &Terrain,
    j: &Junction,
    arms: &[ArmEnd],
    markings: bool,
    raised: bool,
) -> MeshData {
    let k = arms.len();
    let floor = if arms.iter().all(|a| a.path) {
        GRAVEL
    } else {
        ASPHALT
    };
    let mut m = MeshData::default();
    let mut asphalt: Vec<Vec3> = Vec::new();
    let mut outline: Vec<Vec3> = Vec::new();
    for i in 0..k {
        let (a, b) = (&arms[i], &arms[(i + 1) % k]);
        let (ya, yb) = (a.y(), b.y());
        let at = |e: &ArmEnd, x: f32| {
            let p = e.xz(x);
            Vec3::new(p.x, e.y(), p.y)
        };
        asphalt.push(at(a, a.kerb[0]));
        asphalt.push(at(a, a.kerb[1]));
        outline.push(at(a, a.edge[0]));
        outline.push(at(a, a.edge[1]));
        let r = |x: f32| x.abs();
        let mut curb = side_line(graph, j, a.arm, b.arm, r(a.kerb[1]), r(b.kerb[0]), true);
        let mut outer = side_line(graph, j, a.arm, b.arm, r(a.edge[1]), r(b.edge[0]), true);
        for (l, xa, xb) in [
            (&mut curb, a.kerb[1], b.kerb[0]),
            (&mut outer, a.edge[1], b.edge[0]),
        ] {
            l.a[0] = a.xz(xa);
            *l.b.last_mut().expect("points") = b.xz(xb);
        }
        let pts = align(&[curb, outer]);
        let (c, o) = (&pts[0], &pts[1]);
        let n = c.len();
        let inner = |p: &[Vec2]| -> Vec<Vec3> { lift(p, ya, yb, 0.0) };
        // The curb's inner points belong to the carriageway outline (the ends are the kerbs).
        asphalt.extend(inner(c).into_iter().skip(1).take(n.saturating_sub(2)));
        outline.extend(inner(o).into_iter().skip(1).take(n.saturating_sub(2)));
        let walk = a.walk[1] > 0.0 || b.walk[0] > 0.0;
        let raise = if walk { CURB } else { 0.0 };
        if walk {
            let (top_c, top_o) = (lift(c, ya, yb, CURB), lift(o, ya, yb, CURB));
            strip(&mut m, &top_o, &top_c, |_| SIDEWALK);
            if markings {
                let base = lift(c, ya, yb, 0.0);
                let towards: Vec<Vec3> = base
                    .iter()
                    .zip(lift(o, ya, yb, 0.0))
                    .map(|(p, q)| *p * 2.0 - q)
                    .collect();
                wall(&mut m, &base, 0.0, CURB, &towards, KERB);
            }
        }
        edge(
            &mut m,
            terrain,
            raised,
            &lift(o, ya, yb, raise),
            &lift(c, ya, yb, raise),
        );
    }
    let poly: Vec<Vec2> = asphalt.iter().map(|p| Vec2::new(p.x, p.z)).collect();
    let ids: Vec<u32> = asphalt
        .iter()
        .map(|p| push_vertex(&mut m, *p, Vec3::Y, floor))
        .collect();
    for [x, y, z] in triangulate(&poly) {
        tri(&mut m, ids[x], ids[y], ids[z], Vec3::Y);
    }
    if raised {
        underside(&mut m, &outline);
        node_pier(&mut m, graph, terrain, j);
    }
    m
}

/// Keeps every upward face of a ground node piece [`LIFT`] above the terrain: its heights come
/// from the arms' ends, and a wide node (T-275: 4-cell roads are 32 m) spans enough
/// uneven ground for its middle to dip under it (BUG-025).
fn above_ground(m: &mut MeshData, terrain: &Terrain) {
    for (p, n) in m.positions.iter_mut().zip(&m.normals) {
        if n[1] > 0.9 {
            let at = Vec2::new(p[0], p[2]);
            let ground = (0..crate::chunk::LOD_COUNT)
                .map(|l| super::ground::lod_height(terrain, l, at))
                .fold(terrain.height(at), f32::max)
                + LIFT;
            p[1] = p[1].max(ground);
        }
    }
}

/// The cap of a dead end: from the cut back past the node by the arm's setback plus its half
/// width (the polygon of `cp_net`).
fn cap(
    graph: &NetGraph,
    terrain: &Terrain,
    j: &Junction,
    a: &ArmEnd,
    markings: bool,
    raised: bool,
) -> MeshData {
    let mut m = MeshData::default();
    let (r, l) = (a.xz(a.edge[0]), a.xz(a.edge[1]));
    // Away from the arm, back over the node.
    let back = Vec2::new(-(l - r).y, (l - r).x).normalize_or_zero();
    let back = if back.dot(to_xz(j.pos) - (r + l) * 0.5) < 0.0 {
        -back
    } else {
        back
    };
    let depth = a.arm.setback + a.arm.width * 0.5;
    let y = a.y();
    let wb = a.walk[0].max(a.walk[1]);
    let p = |x: f32, d: f32, raise: f32| {
        let q = a.xz(x) + back * d;
        Vec3::new(q.x, y + raise, q.y)
    };
    let floor = if a.path { GRAVEL } else { ASPHALT };
    let (kr, kl, er, el) = (a.kerb[0], a.kerb[1], a.edge[0], a.edge[1]);
    let inner = depth - wb;
    flat_quad(
        &mut m,
        [
            p(kr, 0.0, 0.0),
            p(kl, 0.0, 0.0),
            p(kl, inner, 0.0),
            p(kr, inner, 0.0),
        ],
        Vec3::Y,
        floor,
    );
    if wb > 0.0 {
        for q in [
            [
                p(er, 0.0, CURB),
                p(kr, 0.0, CURB),
                p(kr, inner, CURB),
                p(er, depth, CURB),
            ],
            [
                p(er, depth, CURB),
                p(kr, inner, CURB),
                p(kl, inner, CURB),
                p(el, depth, CURB),
            ],
            [
                p(kl, 0.0, CURB),
                p(el, 0.0, CURB),
                p(el, depth, CURB),
                p(kl, inner, CURB),
            ],
        ] {
            flat_quad(&mut m, q, Vec3::Y, SIDEWALK);
        }
        if markings {
            let kerb = [
                p(kr, 0.0, 0.0),
                p(kr, inner, 0.0),
                p(kl, inner, 0.0),
                p(kl, 0.0, 0.0),
            ];
            let centre = (kerb[0] + kerb[1] + kerb[2] + kerb[3]) * 0.25;
            wall(&mut m, &kerb, 0.0, CURB, &[centre; 4], KERB);
        }
    }
    let raise = if wb > 0.0 { CURB } else { 0.0 };
    let rim = [
        p(er, 0.0, raise),
        p(er, depth, raise),
        p(el, depth, raise),
        p(el, 0.0, raise),
    ];
    let centre = (rim[0] + rim[1] + rim[2] + rim[3]) * 0.25;
    edge(&mut m, terrain, raised, &rim, &[centre; 4]);
    if raised {
        underside(
            &mut m,
            &[
                p(er, 0.0, 0.0),
                p(el, 0.0, 0.0),
                p(el, depth, 0.0),
                p(er, depth, 0.0),
            ],
        );
        node_pier(&mut m, graph, terrain, j);
    }
    m
}

/// The mesh of node `j` (near with markings and kerbs, or far), `None` for a middle node or one
/// without road arms.
pub(super) fn build(
    graph: &NetGraph,
    geo: &JunctionGeometry,
    terrain: &Terrain,
    j: &Junction,
    markings: bool,
) -> Option<MeshData> {
    let node = graph.node(j.node)?;
    if j.class == NodeClass::Middle || node.elevation == Elevation::Tunnel {
        return None;
    }
    let raised = node.elevation != Elevation::Ground;
    let all: Vec<ArmEnd> = j
        .arms
        .iter()
        .filter_map(|a| arm_end(graph, geo, terrain, a, markings))
        .collect();
    // Footpaths that meet a road stop at its edge and shape nothing (T-222).
    let arms: Vec<ArmEnd> = if all.iter().all(|a| a.path) {
        all
    } else {
        all.into_iter().filter(|a| !a.path).collect()
    };
    let mut m = match (j.class, arms.len()) {
        (_, 0) => return None,
        (NodeClass::End, _) | (_, 1) => cap(graph, terrain, j, &arms[0], markings, raised),
        (NodeClass::Bend | NodeClass::Transition, 2) => {
            joint(graph, terrain, j, &arms[0], &arms[1], markings, raised)
        }
        _ => junction(graph, terrain, j, &arms, markings, raised),
    };
    if !raised {
        above_ground(&mut m, terrain);
    }
    Some(m)
}
