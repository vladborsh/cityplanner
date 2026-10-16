//! 2D geometry for cells: oriented squares, overlap and distance tests (XZ plane, metres).

use bevy_math::Vec2;

/// A cell footprint: a rectangle with unit axes `u` (along the road) and `v` (away from it,
/// perpendicular to `u`), `2 * half` long along `u` and `2 * half_v` deep along `v` (a square
/// unless the cell was fitted to a block, T-185).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Obb {
    pub c: Vec2,
    pub u: Vec2,
    pub v: Vec2,
    pub half: f32,
    pub half_v: f32,
}

impl Obb {
    pub fn corners(&self) -> [Vec2; 4] {
        let (a, b) = (self.u * self.half, self.v * self.half_v);
        [
            self.c - a - b,
            self.c + a - b,
            self.c + a + b,
            self.c - a + b,
        ]
    }

    /// Axis-aligned bounds `(min, max)`.
    pub fn aabb(&self) -> (Vec2, Vec2) {
        let r = self.u.abs() * self.half + self.v.abs() * self.half_v;
        (self.c - r, self.c + r)
    }

    /// Point in the box's local frame (`u` → x, `v` → y).
    fn local(&self, p: Vec2) -> Vec2 {
        let d = p - self.c;
        Vec2::new(d.dot(self.u), d.dot(self.v))
    }

    /// Distance from `p` to the box (0 inside).
    pub fn distance_to_point(&self, p: Vec2) -> f32 {
        let l = self.local(p).abs() - Vec2::new(self.half, self.half_v);
        l.max(Vec2::ZERO).length()
    }

    /// Distance from the segment `a`–`b` to the box (0 when they touch or cross).
    pub fn distance_to_segment(&self, a: Vec2, b: Vec2) -> f32 {
        let (la, lb) = (self.local(a), self.local(b));
        if segment_hits_rect(la, lb, Vec2::new(self.half, self.half_v)) {
            return 0.0;
        }
        let mut best = self.distance_to_point(a).min(self.distance_to_point(b));
        for k in self.corners() {
            best = best.min(point_segment_distance(k, a, b));
        }
        best
    }

    /// How deep the two squares interpenetrate (SAT minimum translation distance); `<= 0`
    /// when they are apart or merely touching.
    pub fn penetration(&self, other: &Obb) -> f32 {
        let mut least = f32::INFINITY;
        for axis in [self.u, self.v, other.u, other.v] {
            let proj = |o: &Obb| {
                let c = o.c.dot(axis);
                let r = o.half * o.u.dot(axis).abs() + o.half_v * o.v.dot(axis).abs();
                (c - r, c + r)
            };
            let (a0, a1) = proj(self);
            let (b0, b1) = proj(other);
            let overlap = a1.min(b1) - a0.max(b0);
            if overlap <= 0.0 {
                return overlap;
            }
            least = least.min(overlap);
        }
        least
    }
}

/// Distance from `p` to segment `a`–`b`.
pub fn point_segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 > 1e-12 {
        ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

/// A convex quadrilateral, counter-clockwise (a free-standing building's rectangle).
pub type Quad = [Vec2; 4];

/// Axis-aligned bounds `(min, max)` of a quad.
pub fn quad_aabb(q: &Quad) -> (Vec2, Vec2) {
    let lo = q[0].min(q[1]).min(q[2]).min(q[3]);
    let hi = q[0].max(q[1]).max(q[2]).max(q[3]);
    (lo, hi)
}

/// Whether the interiors of two convex quads overlap (SAT; merely touching is not overlap).
pub fn quads_overlap(a: &Quad, b: &Quad) -> bool {
    for q in [a, b] {
        for i in 0..4 {
            let edge = q[(i + 1) % 4] - q[i];
            let axis = Vec2::new(-edge.y, edge.x);
            let proj = |poly: &Quad| {
                poly.iter()
                    .map(|p| p.dot(axis))
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                        (lo.min(v), hi.max(v))
                    })
            };
            let (a0, a1) = proj(a);
            let (b0, b1) = proj(b);
            if a1.min(b1) - a0.max(b0) <= 1e-4 * axis.length() {
                return false;
            }
        }
    }
    true
}

/// Whether `p` is inside (or on the edge of) the convex counter-clockwise quad.
pub fn quad_contains(q: &Quad, p: Vec2) -> bool {
    (0..4).all(|i| (q[(i + 1) % 4] - q[i]).perp_dot(p - q[i]) >= 0.0)
}

/// Area of the intersection of two convex counter-clockwise quads (Sutherland–Hodgman).
pub fn quad_intersection_area(subject: &Quad, clip: &Quad) -> f32 {
    let (subject, clip) = (&ccw(subject), &ccw(clip));
    let mut poly: Vec<Vec2> = subject.to_vec();
    for i in 0..4 {
        let (a, b) = (clip[i], clip[(i + 1) % 4]);
        let inside = |p: Vec2| (b - a).perp_dot(p - a) >= 0.0;
        let input = std::mem::take(&mut poly);
        for j in 0..input.len() {
            let (p, q) = (input[j], input[(j + 1) % input.len()]);
            match (inside(p), inside(q)) {
                (true, true) => poly.push(q),
                (true, false) => poly.push(cross_at(p, q, a, b)),
                (false, true) => {
                    poly.push(cross_at(p, q, a, b));
                    poly.push(q);
                }
                (false, false) => {}
            }
        }
        if poly.is_empty() {
            return 0.0;
        }
    }
    (0..poly.len())
        .map(|i| poly[i].perp_dot(poly[(i + 1) % poly.len()]))
        .sum::<f32>()
        .abs()
        * 0.5
}

/// The quad with counter-clockwise winding (a mirrored cell comes out clockwise).
pub fn ccw(q: &Quad) -> Quad {
    let area: f32 = (0..4).map(|i| q[i].perp_dot(q[(i + 1) % 4])).sum();
    if area >= 0.0 {
        *q
    } else {
        [q[3], q[2], q[1], q[0]]
    }
}

/// Where the line `p`–`q` crosses the line `a`–`b`.
fn cross_at(p: Vec2, q: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let (d, e) = (q - p, b - a);
    let denom = d.perp_dot(e);
    if denom.abs() < 1e-12 {
        return p;
    }
    p + d * ((a - p).perp_dot(e) / denom)
}

/// Distance from the segment `a`–`b` to the convex quad (0 when they touch or cross).
pub fn quad_segment_distance(q: &Quad, a: Vec2, b: Vec2) -> f32 {
    if quad_contains(q, a) || quad_contains(q, b) {
        return 0.0;
    }
    for i in 0..4 {
        let (c, d) = (q[i], q[(i + 1) % 4]);
        let (r, s) = (b - a, d - c);
        let denom = r.perp_dot(s);
        if denom.abs() > 1e-12 {
            let t = (c - a).perp_dot(s) / denom;
            let u = (c - a).perp_dot(r) / denom;
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                return 0.0;
            }
        }
    }
    let mut best = f32::INFINITY;
    for i in 0..4 {
        best = best.min(point_segment_distance(q[i], a, b));
        best = best.min(point_segment_distance(a, q[i], q[(i + 1) % 4]));
        best = best.min(point_segment_distance(b, q[i], q[(i + 1) % 4]));
    }
    best
}

/// Does the segment `a`–`b` intersect the axis-aligned rectangle `[-h.x, h.x] x [-h.y, h.y]`?
fn segment_hits_rect(a: Vec2, b: Vec2, h: Vec2) -> bool {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q, h) in [(a.x, d.x, h.x), (a.y, d.y, h.y)] {
        if q.abs() < 1e-9 {
            if p.abs() > h {
                return false;
            }
        } else {
            let (mut u0, mut u1) = ((-h - p) / q, (h - p) / q);
            if u0 > u1 {
                std::mem::swap(&mut u0, &mut u1);
            }
            t0 = t0.max(u0);
            t1 = t1.min(u1);
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(c: Vec2, angle: f32) -> Obb {
        let u = Vec2::new(angle.cos(), angle.sin());
        Obb {
            c,
            u,
            v: Vec2::new(-u.y, u.x),
            half: 4.0,
            half_v: 4.0,
        }
    }

    #[test]
    fn touching_squares_do_not_penetrate_and_overlapping_ones_do() {
        let a = square(Vec2::ZERO, 0.0);
        assert!(a.penetration(&square(Vec2::new(8.0, 0.0), 0.0)) <= 1e-5);
        assert!(a.penetration(&square(Vec2::new(9.0, 0.0), 0.0)) < 0.0);
        let p = a.penetration(&square(Vec2::new(6.0, 0.0), 0.0));
        assert!((p - 2.0).abs() < 1e-5);
        // Rotated 45°: a diamond whose tip reaches 4*sqrt(2) from its centre.
        let d = square(Vec2::new(8.0, 0.0), std::f32::consts::FRAC_PI_4);
        assert!(a.penetration(&d) > 0.0);
        assert!(a.penetration(&square(Vec2::new(12.0, 0.0), std::f32::consts::FRAC_PI_4)) < 0.0);
    }

    #[test]
    fn segment_distance_cases() {
        let a = square(Vec2::ZERO, 0.0);
        // Parallel to the right face, 3 m away.
        assert!(
            (a.distance_to_segment(Vec2::new(7.0, -20.0), Vec2::new(7.0, 20.0)) - 3.0).abs() < 1e-5
        );
        // Crossing the box.
        assert_eq!(
            a.distance_to_segment(Vec2::new(-20.0, 1.0), Vec2::new(20.0, 1.0)),
            0.0
        );
        // Passing the corner (4, 4): the segment lies on x + z = 16, 8/sqrt(2) away.
        let d = a.distance_to_segment(Vec2::new(6.0, 10.0), Vec2::new(10.0, 6.0));
        assert!((d - 8.0 / 2f32.sqrt()).abs() < 1e-4, "{d}");
        // Rotated 45°: the diamond's tip is at x = 4*sqrt(2) ~ 5.66, so x = 5 cuts it.
        let r = square(Vec2::ZERO, std::f32::consts::FRAC_PI_4);
        assert_eq!(
            r.distance_to_segment(Vec2::new(5.0, -9.0), Vec2::new(5.0, 9.0)),
            0.0
        );
        let far = r.distance_to_segment(Vec2::new(7.0, -9.0), Vec2::new(7.0, 9.0));
        assert!((far - (7.0 - 4.0 * 2f32.sqrt())).abs() < 1e-4, "{far}");
    }
}
