//! Cubic Bézier math for road geometry. See `architecture/system-design.md` §6.1.
//!
//! Curves live in 3D (`Vec3`, Y up); all planar operations (nearest point, offset,
//! intersection) work on the XZ-plane projection, matching how roads lie on the ground.
//! Everything is pure `f32` math with no iteration over unordered collections, so results
//! are bit-identical for identical inputs on the same binary.

use bevy_math::{Vec2, Vec3};

/// Number of cells in an [`ArcTable`]. Chosen for road-scale segments (up to ~1 km).
///
/// Accuracy (see `arc_table_accuracy` test): for curves in a 1 km box with sane road
/// curvature the total length is within 0.1% of a 4096-sample reference, and `s -> t -> s`
/// round trips are within 0.5 m.
pub const ARC_TABLE_CELLS: usize = 64;

/// Pieces used by [`CubicBezier3::offset_xz`].
const OFFSET_PIECES: usize = 32;

/// Maps an XZ-plane point to a `Vec2` (x, z).
#[inline]
pub fn to_xz(p: Vec3) -> Vec2 {
    Vec2::new(p.x, p.z)
}

/// A cubic Bézier curve in 3D.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier3 {
    pub p0: Vec3,
    pub p1: Vec3,
    pub p2: Vec3,
    pub p3: Vec3,
}

impl CubicBezier3 {
    pub const fn new(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3) -> Self {
        Self { p0, p1, p2, p3 }
    }

    /// A straight line from `a` to `b` (handles at thirds, so parameterisation is uniform).
    pub fn line(a: Vec3, b: Vec3) -> Self {
        Self::new(a, a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b)
    }

    /// Point at parameter `t` (not clamped).
    pub fn point(&self, t: f32) -> Vec3 {
        let u = 1.0 - t;
        self.p0 * (u * u * u)
            + self.p1 * (3.0 * u * u * t)
            + self.p2 * (3.0 * u * t * t)
            + self.p3 * (t * t * t)
    }

    /// First derivative dP/dt (not normalised).
    pub fn derivative(&self, t: f32) -> Vec3 {
        let u = 1.0 - t;
        (self.p1 - self.p0) * (3.0 * u * u)
            + (self.p2 - self.p1) * (6.0 * u * t)
            + (self.p3 - self.p2) * (3.0 * t * t)
    }

    /// Unit tangent at `t`; falls back to the chord direction (or `Vec3::X`) when degenerate.
    pub fn tangent(&self, t: f32) -> Vec3 {
        let d = self.derivative(t);
        if d.length_squared() > 1e-12 {
            d.normalize()
        } else {
            (self.p3 - self.p0).try_normalize().unwrap_or(Vec3::X)
        }
    }

    /// Splits at `t` (de Casteljau) into `[0, t]` and `[t, 1]` curves.
    pub fn split(&self, t: f32) -> (Self, Self) {
        let p01 = self.p0.lerp(self.p1, t);
        let p12 = self.p1.lerp(self.p2, t);
        let p23 = self.p2.lerp(self.p3, t);
        let a = p01.lerp(p12, t);
        let b = p12.lerp(p23, t);
        let m = a.lerp(b, t);
        (Self::new(self.p0, p01, a, m), Self::new(m, b, p23, self.p3))
    }

    /// The sub-curve between `t0` and `t1` (`t0 <= t1`).
    pub fn sub_curve(&self, t0: f32, t1: f32) -> Self {
        let (_, right) = self.split(t0);
        if t0 >= 1.0 {
            return right;
        }
        right.split(((t1 - t0) / (1.0 - t0)).clamp(0.0, 1.0)).0
    }

    /// Projection to the XZ-plane (Y dropped).
    pub fn xz(&self) -> [Vec2; 4] {
        [
            to_xz(self.p0),
            to_xz(self.p1),
            to_xz(self.p2),
            to_xz(self.p3),
        ]
    }

    /// Builds the arc-length table (3D length).
    pub fn arc_table(&self) -> ArcTable {
        ArcTable::build(self)
    }

    /// Closest point to `q` on the XZ projection. Returns `(t, distance_xz)`.
    pub fn nearest_xz(&self, q: Vec2) -> (f32, f32) {
        let dist2 = |t: f32| to_xz(self.point(t)).distance_squared(q);
        const COARSE: usize = 128;
        let mut best_t = 0.0;
        let mut best = f32::INFINITY;
        for i in 0..=COARSE {
            let t = i as f32 / COARSE as f32;
            let d = dist2(t);
            if d < best {
                best = d;
                best_t = t;
            }
        }
        // Repeatedly narrow a bracket around the best sample.
        let mut half = 1.0 / COARSE as f32;
        for _ in 0..8 {
            let lo = (best_t - half).max(0.0);
            let hi = (best_t + half).min(1.0);
            const STEPS: usize = 8;
            for i in 0..=STEPS {
                let t = lo + (hi - lo) * i as f32 / STEPS as f32;
                let d = dist2(t);
                if d < best {
                    best = d;
                    best_t = t;
                }
            }
            half = (hi - lo) / STEPS as f32;
        }
        (best_t, best.sqrt())
    }

    /// Curve offset sideways on the XZ-plane by `d` metres, as a chain of cubics.
    ///
    /// Positive `d` is to the left of the travel direction (looking down from +Y, X right,
    /// Z toward the viewer: the offset direction is `(-t.z, t.x)` rotated into XZ, i.e.
    /// `normal = (t.z, 0, -t.x)`). Y is interpolated from the source curve. Each of
    /// [`OFFSET_PIECES`] pieces keeps the exact offset end points and end tangents; handle
    /// lengths are scaled by the offset/original length ratio of the piece.
    ///
    /// Accuracy: distance from the result to the source stays within `|d| ± 1%` while the
    /// minimum radius of curvature is at least `4·|d|` (the proptest domain).
    pub fn offset_xz(&self, d: f32) -> Vec<CubicBezier3> {
        let normal = |t: f32| {
            let tg = self.tangent(t);
            let n = Vec3::new(tg.z, 0.0, -tg.x);
            n.try_normalize().unwrap_or(Vec3::Z)
        };
        let off_point = |t: f32| self.point(t) + normal(t) * d;
        let mut out = Vec::with_capacity(OFFSET_PIECES);
        for i in 0..OFFSET_PIECES {
            let t0 = i as f32 / OFFSET_PIECES as f32;
            let t1 = (i + 1) as f32 / OFFSET_PIECES as f32;
            let piece = self.sub_curve(t0, t1);
            let (q0, q3) = (off_point(t0), off_point(t1));
            let (d0, d3) = (self.tangent(t0), self.tangent(t1));
            let orig = chord_len(&piece);
            let mut shifted = Self::new(q0, q0 + d0, q3 - d3, q3);
            // Scale handles by the offset-polyline / original length ratio.
            let off_len = {
                let mut l = 0.0;
                let mut prev = q0;
                for k in 1..=4 {
                    let p = off_point(t0 + (t1 - t0) * k as f32 / 4.0);
                    l += prev.distance(p);
                    prev = p;
                }
                l
            };
            let ratio = if orig > 1e-6 { off_len / orig } else { 1.0 };
            let h0 = (piece.p1 - piece.p0).length() * ratio;
            let h3 = (piece.p3 - piece.p2).length() * ratio;
            shifted.p1 = q0 + d0 * h0;
            shifted.p2 = q3 - d3 * h3;
            out.push(shifted);
        }
        out
    }

    /// Intersections with `other` on the XZ projection. Returns `(t_self, t_other)` pairs
    /// sorted by `t_self`. Tangential touches may be missed; overlapping curves are not
    /// supported (results capped at 16).
    pub fn intersect_xz(&self, other: &Self) -> Vec<(f32, f32)> {
        let mut hits: Vec<(f32, f32)> = Vec::new();
        intersect_rec(self, (0.0, 1.0), other, (0.0, 1.0), 0, &mut hits);
        // Refine with Newton on f(ta, tb) = A(ta) - B(tb) in XZ.
        for h in &mut hits {
            for _ in 0..3 {
                let f = to_xz(self.point(h.0)) - to_xz(other.point(h.1));
                let da = to_xz(self.derivative(h.0));
                let db = to_xz(other.derivative(h.1));
                let det = da.x * (-db.y) - (-db.x) * da.y;
                if det.abs() < 1e-9 {
                    break;
                }
                let dta = (f.x * (-db.y) - (-db.x) * f.y) / det;
                let dtb = (da.x * f.y - da.y * f.x) / det;
                h.0 = (h.0 - dta).clamp(0.0, 1.0);
                h.1 = (h.1 - dtb).clamp(0.0, 1.0);
            }
        }
        hits.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        hits.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3);
        hits.truncate(16);
        hits
    }
}

fn chord_len(c: &CubicBezier3) -> f32 {
    c.p0.distance(c.p1) + c.p1.distance(c.p2) + c.p2.distance(c.p3)
}

fn bbox(c: &CubicBezier3) -> (Vec2, Vec2) {
    let pts = c.xz();
    let mut lo = pts[0];
    let mut hi = pts[0];
    for p in &pts[1..] {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    (lo, hi)
}

/// Max distance of the inner control points from the chord (XZ).
fn flatness(c: &CubicBezier3) -> f32 {
    let [a, b, cc, d] = c.xz();
    let dir = d - a;
    let len = dir.length();
    if len < 1e-9 {
        return a.distance(b).max(a.distance(cc));
    }
    let n = Vec2::new(-dir.y, dir.x) / len;
    (b - a).dot(n).abs().max((cc - a).dot(n).abs())
}

const FLAT_EPS: f32 = 0.01;
const MAX_DEPTH: u32 = 40;

fn intersect_rec(
    a: &CubicBezier3,
    ra: (f32, f32),
    b: &CubicBezier3,
    rb: (f32, f32),
    depth: u32,
    out: &mut Vec<(f32, f32)>,
) {
    if out.len() >= 64 {
        return;
    }
    let (alo, ahi) = bbox(a);
    let (blo, bhi) = bbox(b);
    const PAD: f32 = 1e-5;
    if ahi.x + PAD < blo.x || bhi.x + PAD < alo.x || ahi.y + PAD < blo.y || bhi.y + PAD < alo.y {
        return;
    }
    let flat = flatness(a) < FLAT_EPS && flatness(b) < FLAT_EPS;
    if flat || depth >= MAX_DEPTH {
        // Treat as segments chord-to-chord.
        let (a0, a1) = (to_xz(a.p0), to_xz(a.p3));
        let (b0, b1) = (to_xz(b.p0), to_xz(b.p3));
        let r = a1 - a0;
        let s = b1 - b0;
        let den = r.x * s.y - r.y * s.x;
        if den.abs() < 1e-12 {
            return;
        }
        let qp = b0 - a0;
        let u = (qp.x * s.y - qp.y * s.x) / den;
        let v = (qp.x * r.y - qp.y * r.x) / den;
        const SLACK: f32 = 1e-3;
        if (-SLACK..=1.0 + SLACK).contains(&u) && (-SLACK..=1.0 + SLACK).contains(&v) {
            let (u, v) = (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
            out.push((ra.0 + (ra.1 - ra.0) * u, rb.0 + (rb.1 - rb.0) * v));
        }
        return;
    }
    let ma = 0.5 * (ra.0 + ra.1);
    let mb = 0.5 * (rb.0 + rb.1);
    let (a1, a2) = a.split(0.5);
    let (b1, b2) = b.split(0.5);
    intersect_rec(&a1, (ra.0, ma), &b1, (rb.0, mb), depth + 1, out);
    intersect_rec(&a1, (ra.0, ma), &b2, (mb, rb.1), depth + 1, out);
    intersect_rec(&a2, (ma, ra.1), &b1, (rb.0, mb), depth + 1, out);
    intersect_rec(&a2, (ma, ra.1), &b2, (mb, rb.1), depth + 1, out);
}

/// Cumulative arc-length table over `t`, for `s <-> t` conversion.
#[derive(Clone, Debug, PartialEq)]
pub struct ArcTable {
    /// `lengths[i]` = arc length from `t = 0` to `t = i / ARC_TABLE_CELLS`.
    lengths: [f32; ARC_TABLE_CELLS + 1],
}

impl ArcTable {
    pub fn build(curve: &CubicBezier3) -> Self {
        let mut lengths = [0.0; ARC_TABLE_CELLS + 1];
        let mut prev = curve.p0;
        for (i, slot) in lengths.iter_mut().enumerate().skip(1) {
            let p = curve.point(i as f32 / ARC_TABLE_CELLS as f32);
            *slot = prev.distance(p);
            prev = p;
        }
        // Convert per-cell chord lengths into a running sum.
        for i in 1..=ARC_TABLE_CELLS {
            lengths[i] += lengths[i - 1];
        }
        Self { lengths }
    }

    /// Total arc length.
    pub fn length(&self) -> f32 {
        self.lengths[ARC_TABLE_CELLS]
    }

    /// Arc length from the start to parameter `t` (clamped to `[0, 1]`).
    pub fn t_to_s(&self, t: f32) -> f32 {
        let x = t.clamp(0.0, 1.0) * ARC_TABLE_CELLS as f32;
        let i = (x as usize).min(ARC_TABLE_CELLS - 1);
        let f = x - i as f32;
        self.lengths[i] + (self.lengths[i + 1] - self.lengths[i]) * f
    }

    /// Parameter at arc length `s` (clamped to `[0, length]`).
    pub fn s_to_t(&self, s: f32) -> f32 {
        let s = s.clamp(0.0, self.length());
        // Last index with lengths[i] <= s (the table is non-decreasing).
        let i = self
            .lengths
            .partition_point(|&l| l <= s)
            .saturating_sub(1)
            .min(ARC_TABLE_CELLS - 1);
        let span = self.lengths[i + 1] - self.lengths[i];
        let f = if span > 1e-9 {
            (s - self.lengths[i]) / span
        } else {
            0.0
        };
        (i as f32 + f) / ARC_TABLE_CELLS as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample_len(c: &CubicBezier3, n: usize) -> f32 {
        let mut prev = c.p0;
        let mut l = 0.0;
        for i in 1..=n {
            let p = c.point(i as f32 / n as f32);
            l += prev.distance(p);
            prev = p;
        }
        l
    }

    #[test]
    fn line_basics() {
        let c = CubicBezier3::line(Vec3::ZERO, Vec3::new(30.0, 0.0, 40.0));
        assert!((c.arc_table().length() - 50.0).abs() < 1e-3);
        assert!((c.tangent(0.3) - Vec3::new(0.6, 0.0, 0.8)).length() < 1e-5);
        let (l, r) = c.split(0.5);
        assert!((l.p3 - Vec3::new(15.0, 0.0, 20.0)).length() < 1e-4);
        assert_eq!(l.p3, r.p0);
    }

    #[test]
    fn nearest_on_line() {
        let c = CubicBezier3::line(Vec3::ZERO, Vec3::new(100.0, 0.0, 0.0));
        let (t, d) = c.nearest_xz(Vec2::new(25.0, 7.0));
        assert!((t - 0.25).abs() < 1e-3 && (d - 7.0).abs() < 1e-3);
    }

    #[test]
    fn crossing_lines() {
        let a = CubicBezier3::line(Vec3::new(-10.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0));
        let b = CubicBezier3::line(Vec3::new(0.0, 0.0, -10.0), Vec3::new(0.0, 0.0, 10.0));
        let h = a.intersect_xz(&b);
        assert_eq!(h.len(), 1);
        assert!((h[0].0 - 0.5).abs() < 1e-3 && (h[0].1 - 0.5).abs() < 1e-3);
    }

    #[test]
    fn arc_table_accuracy() {
        // 1 km S-curve.
        let c = CubicBezier3::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(400.0, 0.0, 200.0),
            Vec3::new(600.0, 0.0, -200.0),
            Vec3::new(1000.0, 0.0, 0.0),
        );
        let reference = sample_len(&c, 4096);
        let tab = c.arc_table();
        assert!((tab.length() - reference).abs() / reference < 1e-3);
        for i in 0..=20 {
            let s = tab.length() * i as f32 / 20.0;
            assert!((tab.t_to_s(tab.s_to_t(s)) - s).abs() < 0.5);
        }
    }

    /// Proptest domain: control points in a 300 m box (road-scale segment), Y within ±10 m.
    fn curve_strategy() -> impl Strategy<Value = CubicBezier3> {
        let p = (-150.0f32..150.0, -10.0f32..10.0, -150.0f32..150.0)
            .prop_map(|(x, y, z)| Vec3::new(x, y, z));
        (p.clone(), p.clone(), p.clone(), p).prop_map(|(a, b, c, d)| CubicBezier3::new(a, b, c, d))
    }

    /// Minimum radius of curvature on XZ, sampled; `inf` for straight curves.
    fn min_radius(c: &CubicBezier3) -> f32 {
        let mut best = f32::INFINITY;
        for i in 0..=200 {
            let t = i as f32 / 200.0;
            let d1 = to_xz(c.derivative(t));
            // Second derivative.
            let u = 1.0 - t;
            let d2 = to_xz(
                (c.p2 - c.p1 * 2.0 + c.p0) * (6.0 * u) + (c.p3 - c.p2 * 2.0 + c.p1) * (6.0 * t),
            );
            let cross = (d1.x * d2.y - d1.y * d2.x).abs();
            if cross < 1e-9 {
                continue;
            }
            best = best.min(d1.length().powi(3) / cross);
        }
        best
    }

    /// True if parts of the curve far apart along its length never come within `2.5·d`
    /// in XZ (no folds/loops that make the offset self-intersect).
    fn no_fold(c: &CubicBezier3, d: f32) -> bool {
        const N: usize = 160;
        let tab = c.arc_table();
        let pts: Vec<(f32, Vec2)> = (0..=N)
            .map(|i| {
                let t = i as f32 / N as f32;
                (tab.t_to_s(t), to_xz(c.point(t)))
            })
            .collect();
        for i in 0..pts.len() {
            for j in i + 1..pts.len() {
                if pts[j].0 - pts[i].0 > 8.0 * d && pts[i].1.distance(pts[j].1) < 2.5 * d {
                    return false;
                }
            }
        }
        true
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn arc_length_monotonic(c in curve_strategy()) {
            let tab = c.arc_table();
            let mut prev = -1.0;
            for i in 0..=100 {
                let s = tab.t_to_s(i as f32 / 100.0);
                prop_assert!(s >= prev);
                prev = s;
            }
            let mut prev_t = -1.0;
            for i in 0..=100 {
                let t = tab.s_to_t(tab.length() * i as f32 / 100.0);
                prop_assert!(t >= prev_t);
                prev_t = t;
            }
            prop_assert!(tab.length() >= c.p0.distance(c.p3) - 1e-2);
        }

        #[test]
        fn split_preserves_shape(c in curve_strategy(), t in 0.05f32..0.95) {
            let (l, r) = c.split(t);
            prop_assert!(l.point(0.5).distance(c.point(t * 0.5)) < 1e-2);
            prop_assert!(r.point(0.5).distance(c.point(t + (1.0 - t) * 0.5)) < 1e-2);
        }

        /// Domain: min XZ radius of curvature >= 4·|d|, no folds (`no_fold`), |d| in 1.5..8 m (lane offsets).
        #[test]
        fn offset_keeps_distance(c in curve_strategy(), d in 1.5f32..8.0, left in any::<bool>()) {
            let d = if left { d } else { -d };
            prop_assume!(min_radius(&c) >= 4.0 * d.abs());
            prop_assume!(c.arc_table().length() > 20.0);
            prop_assume!(no_fold(&c, d.abs()));
            for piece in c.offset_xz(d) {
                for k in 0..=8 {
                    let q = to_xz(piece.point(k as f32 / 8.0));
                    let (_, dist) = c.nearest_xz(q);
                    prop_assert!((dist - d.abs()).abs() <= 0.01 * d.abs(),
                        "dist {} vs d {}", dist, d.abs());
                }
            }
        }

        #[test]
        fn intersections_lie_on_both(a in curve_strategy(), b in curve_strategy()) {
            for (ta, tb) in a.intersect_xz(&b) {
                prop_assert!((0.0..=1.0).contains(&ta) && (0.0..=1.0).contains(&tb));
                let d = to_xz(a.point(ta)).distance(to_xz(b.point(tb)));
                prop_assert!(d < 0.05, "gap {}", d);
            }
        }
    }
}
