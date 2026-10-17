//! Street lamps and utility poles along ground roads (T-233).
//!
//! A road def asks for lamps with `lamp_spacing_m` (both sidewalks, staggered, a post with a
//! reaching arm and a head over the carriageway) and for decorative wooden utility poles with
//! `pole_spacing_m` (one side, a crossarm and two sagging wires). Three meshes per segment:
//! `posts` (poles and arms, grey / wood), `heads` (the lamp heads, drawn with a material that
//! glows at night) and `pools` (a soft warm disc of light on the road under each head, drawn
//! additively by night only). Junction areas are skipped: the trimmed curve ends where the
//! junction polygon begins, and a margin is kept from each end.

use super::*;
use crate::chunk::MeshData;
use cp_content::ContentDb;

/// Distance kept from the ends of a segment, metres.
const END_MARGIN: f32 = 5.0;
const POST: [f32; 4] = [0.30, 0.31, 0.33, 1.0];
const WOOD: [f32; 4] = [0.36, 0.25, 0.15, 1.0];
const WIRE: [f32; 4] = [0.08, 0.08, 0.09, 1.0];
const HEAD: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const POOL: [f32; 3] = [1.0, 0.80, 0.42];
/// Lamp height and reach of its arm over the road, metres.
const LAMP_H: f32 = 6.0;
const ARM: f32 = 1.2;
const POLE_H: f32 = 8.0;
/// Radius of a light pool, metres.
pub const POOL_R: f32 = 6.0;

/// The street furniture meshes of one segment.
#[derive(Clone, Debug, Default)]
pub struct Furniture {
    pub posts: MeshData,
    pub heads: MeshData,
    pub pools: MeshData,
    pub min: Vec3,
    pub max: Vec3,
    /// Lamp heads in the piece.
    pub lamps: usize,
    pub poles: usize,
}

impl Furniture {
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
}

/// An oriented box: centre, unit axes (right, up, forward), half sizes.
fn obox(m: &mut MeshData, c: Vec3, ax: [Vec3; 3], half: Vec3, col: [f32; 4]) {
    let corner = |sx: f32, sy: f32, sz: f32| {
        c + ax[0] * (sx * half.x) + ax[1] * (sy * half.y) + ax[2] * (sz * half.z)
    };
    for (axis, sign) in [
        (0, 1.0f32),
        (0, -1.0),
        (1, 1.0),
        (1, -1.0),
        (2, 1.0),
        (2, -1.0),
    ] {
        let n = ax[axis] * sign;
        let (a, b) = match axis {
            0 => (1, 2),
            1 => (2, 0),
            _ => (0, 1),
        };
        let mut p = [Vec3::ZERO; 4];
        for (k, (ua, ub)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .into_iter()
            .enumerate()
        {
            let mut s = [0.0f32; 3];
            s[axis] = sign;
            s[a] = ua;
            s[b] = ub;
            p[k] = corner(s[0], s[1], s[2]);
        }
        flat_quad(m, p, n, col);
    }
}

/// A thin sagging wire between two points as a few thin boxes.
fn wire(m: &mut MeshData, a: Vec3, b: Vec3) {
    const SEGS: usize = 4;
    let sag = |t: f32| -0.5 * 4.0 * t * (1.0 - t);
    let pt = |i: usize| {
        let t = i as f32 / SEGS as f32;
        a.lerp(b, t) + Vec3::Y * sag(t)
    };
    for i in 0..SEGS {
        let (p, q) = (pt(i), pt(i + 1));
        let d = q - p;
        let len = d.length();
        if len < 1e-3 {
            continue;
        }
        let fwd = d / len;
        let right = fwd.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(fwd);
        obox(
            m,
            (p + q) * 0.5,
            [right, up, fwd],
            Vec3::new(0.025, 0.025, len * 0.5),
            WIRE,
        );
    }
}

/// Where along a segment of `length` the items go: every `spacing` metres, centred, `phase`
/// of a spacing shifted (0 or 0.5), inside the end margins.
fn stations(length: f32, spacing: f32, phase: f32, (m0, m1): (f32, f32)) -> Vec<f32> {
    let room = length - m0 - m1;
    if spacing <= 0.5 || room < 0.0 {
        return Vec::new();
    }
    let n = (room / spacing).floor() as usize + 1;
    let first = m0 + (room - (n - 1) as f32 * spacing) * 0.5;
    (0..n)
        .map(|k| first + (k as f32 + phase) * spacing)
        .filter(|&s| s >= m0 - 1e-3 && s <= length - m1 + 1e-3)
        .collect()
}

/// The margin at the end of `seg` at `node`: a real junction (three arms or more) keeps its
/// corner clear, the joint of two segments of one street only a little.
fn margin_at(geo: &JunctionGeometry, node: NodeId) -> f32 {
    if geo.get(node).is_some_and(|j| j.arms.len() >= 3) {
        END_MARGIN
    } else {
        1.5
    }
}

/// Builds the furniture of one ground segment; `None` when the road asks for none.
pub fn build_segment_furniture(
    graph: &NetGraph,
    geo: &JunctionGeometry,
    terrain: &Terrain,
    db: &ContentDb,
    id: SegmentId,
) -> Option<Furniture> {
    let seg = graph.segment(id)?;
    if seg.elevation != Elevation::Ground || seg.road.index() >= db.roads().count() {
        return None;
    }
    let def = db.road(seg.road);
    if def.lamp_spacing_m <= 0.0 && def.pole_spacing_m <= 0.0 {
        return None;
    }
    let curve = geo.trimmed_curve(graph, id)?;
    let table = curve.arc_table();
    let length = table.length();
    // The sidewalks: (outer edge offset, inner lamp offset, sign).
    let mut sides: Vec<(f32, f32, f32)> = seg
        .lanes
        .iter()
        .filter_map(|&l| graph.lane(l))
        .filter(|l| l.kind == LaneKind::Pedestrian)
        .map(|l| {
            let sign = if l.offset >= 0.0 { 1.0 } else { -1.0 };
            (
                l.offset + sign * l.width * 0.5,
                l.offset + sign * (l.width * 0.5 - 0.35),
                sign,
            )
        })
        .collect();
    sides.sort_by(|a, b| a.2.total_cmp(&b.2));
    let margins = (margin_at(geo, seg.start), margin_at(geo, seg.end));
    if sides.is_empty() || length < margins.0 + margins.1 {
        return None;
    }
    // Lamps on every sidewalk (the second one staggered), poles on the right-hand side.
    let mut items: Vec<(f32, usize, bool)> = Vec::new(); // (s, side index, is_pole)
    for (i, _) in sides.iter().enumerate() {
        let phase = if i % 2 == 0 { 0.0 } else { 0.5 };
        for s in stations(length, def.lamp_spacing_m, phase, margins) {
            items.push((s, i, false));
        }
    }
    let pole_side = 0; // sides[0] is the most negative offset: the right
    let pole_s = stations(length, def.pole_spacing_m, 0.25, margins);
    for &s in &pole_s {
        items.push((s, pole_side, true));
    }
    if items.is_empty() {
        return None;
    }
    let frame = Frame {
        curve,
        table,
        length,
        half: half_width(graph, seg),
        start: arm_of(geo, seg.start, id, true),
        end: arm_of(geo, seg.end, id, false),
    };
    let mut base: Vec<f32> = vec![0.0, length];
    base.extend(items.iter().map(|i| i.0));
    base.sort_by(f32::total_cmp);
    base.dedup();
    let (s_list, rings) = ground::rings(&frame, graph, geo, terrain, seg, base);
    let ring_at = |s: f32| -> Option<&Ring> {
        s_list
            .iter()
            .position(|&v| (v - s).abs() < 1e-4)
            .map(|k| &rings[k])
    };

    let mut f = Furniture::default();
    let mut pole_tops: Vec<(f32, [Vec3; 2])> = Vec::new();
    for (s, side, is_pole) in items {
        let Some(r) = ring_at(s) else { continue };
        let (outer, inner, sign) = sides[side];
        let x = if is_pole { outer + 0.35 } else { inner };
        let foot = r.at(x, CURB);
        // Across the road, towards its middle, and along it.
        let toward = (r.xz(x - sign) - r.xz(x))
            .try_normalize()
            .unwrap_or(Vec2::ZERO);
        let toward = Vec3::new(toward.x, 0.0, toward.y);
        let along = toward.cross(Vec3::Y).normalize_or_zero();
        let ax = [along, Vec3::Y, toward];
        if is_pole {
            // Wooden pole with a crossarm and two insulators; wires join consecutive poles.
            obox(
                &mut f.posts,
                foot + Vec3::Y * (POLE_H * 0.5),
                ax,
                Vec3::new(0.10, POLE_H * 0.5, 0.10),
                WOOD,
            );
            let top = foot + Vec3::Y * (POLE_H - 0.5);
            obox(&mut f.posts, top, ax, Vec3::new(0.06, 0.07, 0.8), WOOD);
            let ends = [
                top + toward * 0.7 + Vec3::Y * 0.12,
                top - toward * 0.7 + Vec3::Y * 0.12,
            ];
            for e in ends {
                obox(&mut f.posts, e, ax, Vec3::new(0.05, 0.07, 0.05), POST);
            }
            f.poles += 1;
            pole_tops.push((s, ends));
        } else {
            obox(
                &mut f.posts,
                foot + Vec3::Y * (LAMP_H * 0.5),
                ax,
                Vec3::new(0.09, LAMP_H * 0.5, 0.09),
                POST,
            );
            let top = foot + Vec3::Y * (LAMP_H - 0.06);
            obox(
                &mut f.posts,
                top + toward * (ARM * 0.5),
                ax,
                Vec3::new(0.04, 0.04, ARM * 0.5),
                POST,
            );
            let head = top + toward * ARM - Vec3::Y * 0.1;
            obox(&mut f.heads, head, ax, Vec3::new(0.17, 0.07, 0.4), HEAD);
            // The pool on the carriageway under the head.
            let road_y = r.center.y + 0.03;
            let c = Vec3::new(head.x, road_y, head.z);
            pool(&mut f.pools, c);
            f.lamps += 1;
        }
    }
    for pair in pole_tops.windows(2) {
        for k in 0..2 {
            wire(&mut f.posts, pair[0].1[k], pair[1].1[k]);
        }
    }
    if f.posts.positions.is_empty() && f.heads.positions.is_empty() {
        return None;
    }
    let (lo, hi) = bounds(&f.posts);
    let (mut min, mut max) = (lo, hi);
    for m in [&f.heads, &f.pools] {
        if !m.positions.is_empty() {
            let (a, b) = bounds(m);
            min = min.min(a);
            max = max.max(b);
        }
    }
    f.min = min;
    f.max = max;
    Some(f)
}

/// A soft disc of light: warm in the middle, fading to nothing at the rim (vertex alpha,
/// drawn additively).
fn pool(m: &mut MeshData, c: Vec3) {
    const SIDES: usize = 12;
    let col = |a: f32| [POOL[0], POOL[1], POOL[2], a];
    let centre = push_vertex(m, c, Vec3::Y, col(0.5));
    let mut rings: Vec<Vec<u32>> = Vec::new();
    for (r, a) in [(POOL_R * 0.5, 0.22), (POOL_R, 0.0)] {
        rings.push(
            (0..SIDES)
                .map(|k| {
                    let t = k as f32 / SIDES as f32 * std::f32::consts::TAU;
                    push_vertex(
                        m,
                        c + Vec3::new(t.cos() * r, 0.0, t.sin() * r),
                        Vec3::Y,
                        col(a),
                    )
                })
                .collect(),
        );
    }
    for k in 0..SIDES {
        let k1 = (k + 1) % SIDES;
        tri(m, centre, rings[0][k], rings[0][k1], Vec3::Y);
        for j in 0..1 {
            quad(
                m,
                [rings[j][k], rings[j][k1], rings[j + 1][k1], rings[j + 1][k]],
                Vec3::Y,
            );
        }
    }
}

struct Entry {
    stamp: u64,
    chunk: (i32, i32),
    piece: Furniture,
}

/// All street furniture, batched by 256 m chunk like the roads.
#[derive(Default)]
pub struct FurnitureSet {
    pieces: BTreeMap<SegmentId, Entry>,
    by_chunk: BTreeMap<(i32, i32), BTreeSet<SegmentId>>,
}

impl FurnitureSet {
    /// Lamp heads and poles standing.
    pub fn counts(&self) -> (usize, usize) {
        self.pieces
            .values()
            .fold((0, 0), |a, e| (a.0 + e.piece.lamps, a.1 + e.piece.poles))
    }

    /// Rebuilds the segments whose geometry changed and drops vanished ones.
    pub fn update(
        &mut self,
        graph: &NetGraph,
        geo: &JunctionGeometry,
        terrain: &Terrain,
        db: &ContentDb,
    ) -> Update {
        let mut up = Update::default();
        let gone: Vec<SegmentId> = self
            .pieces
            .keys()
            .filter(|id| graph.segment(**id).is_none())
            .copied()
            .collect();
        for id in gone {
            self.replace(id, 0, None, &mut up);
        }
        let ids: Vec<SegmentId> = graph.segments().map(|(id, _)| id).collect();
        for id in ids {
            let stamp = segment_fingerprint(graph, geo, id);
            let known = self.pieces.get(&id).map(|e| e.stamp);
            if known == Some(stamp) {
                continue;
            }
            // A segment that never had furniture is remembered by an empty entry, so it is
            // not rebuilt until it changes.
            let piece = build_segment_furniture(graph, geo, terrain, db, id);
            self.replace(id, stamp, Some(piece.unwrap_or_default()), &mut up);
        }
        up
    }

    fn replace(&mut self, id: SegmentId, stamp: u64, piece: Option<Furniture>, up: &mut Update) {
        if let Some(old) = self.pieces.remove(&id) {
            if let Some(set) = self.by_chunk.get_mut(&old.chunk) {
                set.remove(&id);
                if set.is_empty() {
                    self.by_chunk.remove(&old.chunk);
                }
            }
            if !old.piece.posts.positions.is_empty() || !old.piece.heads.positions.is_empty() {
                up.dirty_chunks.insert(old.chunk);
            }
            if piece.is_none() {
                up.removed += 1;
            }
        }
        if let Some(piece) = piece {
            let empty = piece.posts.positions.is_empty() && piece.heads.positions.is_empty();
            let chunk = if empty {
                (0, 0)
            } else {
                chunk_of(piece.center())
            };
            if !empty {
                self.by_chunk.entry(chunk).or_default().insert(id);
                up.dirty_chunks.insert(chunk);
                up.rebuilt += 1;
            }
            self.pieces.insert(
                id,
                Entry {
                    stamp,
                    chunk,
                    piece,
                },
            );
        }
    }

    /// Merged mesh of a chunk: kind 0 posts, 1 lamp heads, 2 light pools.
    pub fn chunk_mesh(&self, chunk: (i32, i32), kind: usize) -> Option<MeshData> {
        let ids = self.by_chunk.get(&chunk)?;
        let mut m = MeshData::default();
        for id in ids {
            let p = &self.pieces[id].piece;
            append(&mut m, [&p.posts, &p.heads, &p.pools][kind.min(2)]);
        }
        Some(m)
    }

    pub fn chunk_bounds(&self, chunk: (i32, i32)) -> Option<(Vec3, Vec3)> {
        let ids = self.by_chunk.get(&chunk)?;
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for id in ids {
            let p = &self.pieces[id].piece;
            lo = lo.min(p.min);
            hi = hi.max(p.max);
        }
        Some((lo, hi))
    }

    pub fn chunks(&self) -> impl Iterator<Item = &(i32, i32)> {
        self.by_chunk.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stations_are_centred_spaced_and_inside_the_margins() {
        let m = (END_MARGIN, END_MARGIN);
        let s = stations(100.0, 28.0, 0.0, m);
        assert_eq!(s.len(), 4, "{s:?}");
        assert!((s[0] - (100.0 - s[3])).abs() < 1e-3, "centred: {s:?}");
        assert!(s.windows(2).all(|w| (w[1] - w[0] - 28.0).abs() < 1e-3));
        assert!(s.iter().all(|&v| (END_MARGIN..=95.0).contains(&v)));
        // The staggered side sits half a spacing further and loses one at the end.
        let t = stations(100.0, 28.0, 0.5, m);
        assert!(t.iter().zip(&s).all(|(b, a)| (b - a - 14.0).abs() < 1e-3));
        assert!(t.iter().all(|&v| v <= 95.0));
        assert!(stations(8.0, 28.0, 0.0, m).is_empty());
        assert!(stations(100.0, 0.0, 0.0, m).is_empty());
    }
}
