//! Terrain clearance of ground roads (BUG-025).
//!
//! The road follows the network curve, which is a straight ramp between node heights, while
//! the terrain mesh bumps above it between the rings (10 m grid, three LODs, each a
//! piecewise-linear surface). Every ground ring is therefore lifted to the highest point of
//! *any* LOD's mesh over the strips next to it, so the deck is above the mesh wherever the
//! camera LOD puts it. An end ring is above the terrain under its first strip; node pieces take
//! their heights from the end rings of their arms, so pieces still meet (T-263: no common level
//! lifted to the highest point around the node). The two ends on a middle node share the higher.
//!
//! The highest point of a piecewise-linear surface over a polygon is a mesh node inside it
//! or a crossing of its boundary with a mesh edge (grid lines and the cell diagonals), so
//! the maximum is exact, not sampled.

use bevy_math::Vec2;
use cp_terrain::{GRID, SPACING, Terrain};

use super::*;
use crate::chunk::LOD_COUNT;

/// Ring spacing on straight ground segments where the terrain asks for it, metres.
pub const GROUND_STEP: f32 = 5.0;
/// Deck clearance above the highest terrain mesh point, metres.
pub const CLEARANCE: f32 = 0.1;
/// Rings of a straight segment are dropped while the deck stays within this of the ideal.
const THIN_TOL: f32 = 0.03;

/// Height of LOD `lod`'s terrain mesh at `p` (the triangle split of `build_chunk_mesh`).
pub fn lod_height(terrain: &Terrain, lod: usize, p: Vec2) -> f32 {
    let step = 1usize << lod;
    let cells = (GRID - 1) / step;
    let s = SPACING * step as f32;
    let g = |v: f32| (v / s).clamp(0.0, cells as f32);
    let (gx, gz) = (g(p.x), g(p.y));
    let (ix, iz) = (
        (gx.floor() as usize).min(cells - 1),
        (gz.floor() as usize).min(cells - 1),
    );
    let (fx, fz) = (gx - ix as f32, gz - iz as f32);
    let h = |dx: usize, dz: usize| terrain.node_height((ix + dx) * step, (iz + dz) * step);
    let (a, b, c, d) = (h(0, 0), h(1, 0), h(0, 1), h(1, 1));
    if fx + fz <= 1.0 {
        a + fx * (b - a) + fz * (c - a)
    } else {
        d + (1.0 - fx) * (c - d) + (1.0 - fz) * (b - d)
    }
}

/// Highest mesh point (any LOD) on the segment `p`..`q`.
fn max_on_segment(terrain: &Terrain, p: Vec2, q: Vec2) -> f32 {
    let d = q - p;
    let mut best = f32::MIN;
    for lod in 0..LOD_COUNT {
        let s = SPACING * (1usize << lod) as f32;
        let mut at = |t: f32| best = best.max(lod_height(terrain, lod, p + d * t));
        at(0.0);
        at(1.0);
        // Mesh edges: x = k s, z = k s, and the diagonals x + z = k s.
        for (p0, dd) in [(p.x, d.x), (p.y, d.y), (p.x + p.y, d.x + d.y)] {
            if dd.abs() < 1e-9 {
                continue;
            }
            let (a, b) = (p0 / s, (p0 + dd) / s);
            let (lo, hi) = (a.min(b), a.max(b));
            let mut k = lo.ceil();
            while k <= hi {
                let t = (k * s - p0) / dd;
                if t > 0.0 && t < 1.0 {
                    at(t);
                }
                k += 1.0;
            }
        }
    }
    best
}

/// Whether `p` is inside the polygon (even-odd).
fn inside(poly: &[Vec2], p: Vec2) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            c = !c;
        }
    }
    c
}

/// Highest terrain mesh point (any LOD, and the bilinear ground at the corners) over `poly`.
pub fn max_mesh_height(terrain: &Terrain, poly: &[Vec2]) -> f32 {
    let n = poly.len();
    let mut best = f32::MIN;
    for i in 0..n {
        best = best.max(max_on_segment(terrain, poly[i], poly[(i + 1) % n]));
        best = best.max(terrain.height(poly[i]));
    }
    let (lo, hi) = poly.iter().fold(
        (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
        |(l, h), p| (l.min(*p), h.max(*p)),
    );
    for lod in 0..LOD_COUNT {
        let step = 1usize << lod;
        let s = SPACING * step as f32;
        let cells = (GRID - 1) / step;
        let range = |a: f32, b: f32| {
            let i0 = ((a / s).ceil().max(0.0) as usize).min(cells);
            let i1 = ((b / s).floor().max(0.0) as usize).min(cells);
            i0..=i1
        };
        for iz in range(lo.y, hi.y) {
            for ix in range(lo.x, hi.x) {
                if inside(poly, Vec2::new(ix as f32 * s, iz as f32 * s)) {
                    best = best.max(terrain.node_height(ix * step, iz * step));
                }
            }
        }
    }
    best
}

/// Length of the first and last strip of a ground segment, which junction levels cover.
fn end_strip(length: f32) -> f32 {
    GROUND_STEP.min(length * 0.5)
}

pub(super) fn frame_of<'a>(
    graph: &NetGraph,
    geo: &'a JunctionGeometry,
    id: SegmentId,
) -> Option<Frame<'a>> {
    let seg = graph.segment(id)?;
    let curve = geo.trimmed_curve(graph, id)?;
    let table = curve.arc_table();
    let length = table.length();
    Some(Frame {
        curve,
        table,
        length,
        half: half_width(graph, seg),
        start: arm_of(geo, seg.start, id, true),
        end: arm_of(geo, seg.end, id, false),
    })
}

/// Corners of the strip between rings `a` and `b`.
fn strip(a: &Ring, b: &Ring) -> [Vec2; 4] {
    [a.xz(-a.half), a.xz(a.half), b.xz(b.half), b.xz(-b.half)]
}

/// Height of the end ring of segment `id` at its start or end, on its own: the curve plus
/// [`LIFT`], and on ground above the terrain under the first strip.
fn own_end_level(
    graph: &NetGraph,
    geo: &JunctionGeometry,
    terrain: &Terrain,
    id: SegmentId,
    at_start: bool,
) -> f32 {
    let (Some(seg), Some(f)) = (graph.segment(id), frame_of(graph, geo, id)) else {
        return 0.0;
    };
    let r0 = f.ring(if at_start { 0.0 } else { f.length });
    if seg.elevation != Elevation::Ground || f.length < 0.05 {
        return r0.center.y;
    }
    let e = end_strip(f.length);
    let r1 = f.ring(if at_start { e } else { f.length - e });
    r0.center
        .y
        .max(max_mesh_height(terrain, &strip(&r0, &r1)) + CLEARANCE)
}

/// Height of the end ring of segment `id` at its start or end: its own level, the higher of the
/// two at a middle node (both ends are one line there).
pub(super) fn end_level(
    graph: &NetGraph,
    geo: &JunctionGeometry,
    terrain: &Terrain,
    id: SegmentId,
    at_start: bool,
) -> f32 {
    let own = own_end_level(graph, geo, terrain, id, at_start);
    let node = graph
        .segment(id)
        .map(|s| if at_start { s.start } else { s.end });
    match node.and_then(|n| geo.get(n)) {
        Some(j) if j.class == cp_net::NodeClass::Middle => j
            .arms
            .iter()
            .filter(|a| graph.segment(a.segment).is_some_and(|s| !is_path(graph, s)))
            .map(|a| own_end_level(graph, geo, terrain, a.segment, a.at_start))
            .fold(own, f32::max),
        _ => own,
    }
}

/// Ring positions and rings of a segment. Ground segments get terrain clearance; others the
/// plain curve.
pub(super) fn rings(
    frame: &Frame,
    graph: &NetGraph,
    geo: &JunctionGeometry,
    terrain: &Terrain,
    seg_id: SegmentId,
    seg: &Segment,
    base: Vec<f32>,
) -> (Vec<f32>, Vec<Ring>) {
    let straight = is_straight(&frame.curve);
    if seg.elevation != Elevation::Ground {
        let mut rings: Vec<Ring> = base.iter().map(|&s| frame.ring(s)).collect();
        // The ends as the node pieces have them (the higher of two at a middle node).
        if let Some(r) = rings.first_mut() {
            r.center.y = end_level(graph, geo, terrain, seg_id, true);
        }
        if let Some(r) = rings.last_mut() {
            r.center.y = end_level(graph, geo, terrain, seg_id, false);
        }
        return (base, rings);
    }
    let len = frame.length;
    let e = end_strip(len);

    // Candidates: (s, must keep). Straight segments add optional rings every GROUND_STEP.
    let mut cand: Vec<(f32, bool)> = base.iter().map(|&s| (s, true)).collect();
    let mut extra = vec![(e, true), (len - e, true)];
    if straight {
        let n = (len / GROUND_STEP).ceil().max(1.0) as usize;
        extra.extend((1..n).map(|i| (len * i as f32 / n as f32, false)));
    }
    // Rings from `ring_positions` keep their exact positions (chevron rows match on them);
    // extra ones that land next to one only make it mandatory.
    cand.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged = cand;
    for (s, keep) in extra {
        match merged.iter_mut().find(|c| (c.0 - s).abs() < 0.25) {
            Some(c) => c.1 |= keep,
            None => {
                let at = merged.partition_point(|c| c.0 < s);
                merged.insert(at, (s, keep));
            }
        }
    }

    let all: Vec<Ring> = merged.iter().map(|&(s, _)| frame.ring(s)).collect();
    let level_at = |at_start: bool| Some(end_level(graph, geo, terrain, seg_id, at_start));
    let ends = (level_at(true), level_at(false));
    let required = |rs: &[Ring], i: usize| -> f32 {
        let (a, b) = (&rs[i.saturating_sub(1)], &rs[(i + 1).min(rs.len() - 1)]);
        let y = rs[i].center.y;
        y.max(max_mesh_height(terrain, &strip(a, b)) + CLEARANCE)
    };

    // Thin the optional rings: drop one while the deck stays on the ideal profile.
    let mut kept: Vec<usize> = vec![0];
    if straight {
        let ideal: Vec<f32> = (0..all.len()).map(|i| required(&all, i)).collect();
        for i in 1..all.len() - 1 {
            let l = *kept.last().expect("kept");
            let (s0, s1) = (merged[l].0, merged[i + 1].0);
            let fits = !merged[i].1
                && (l + 1..=i).all(|j| {
                    let t = (merged[j].0 - s0) / (s1 - s0);
                    (ideal[l] + (ideal[i + 1] - ideal[l]) * t - ideal[j]).abs() <= THIN_TOL
                });
            if !fits {
                kept.push(i);
            }
        }
        kept.push(all.len() - 1);
    } else {
        kept.extend(1..all.len());
    }

    let mut out_rings: Vec<Ring> = kept.iter().map(|&i| all[i]).collect();
    let out_s: Vec<f32> = kept.iter().map(|&i| merged[i].0).collect();
    let ys: Vec<f32> = (0..out_rings.len())
        .map(|k| required(&out_rings, k))
        .collect();
    let n = ys.len();
    for (k, r) in out_rings.iter_mut().enumerate() {
        r.center.y = match (k, ends) {
            (0, (Some(l), _)) => l,
            (k, (_, Some(l))) if k == n - 1 => l,
            _ => ys[k],
        };
    }
    (out_s, out_rings)
}
