//! The ghost of an interchange stamp (T-332, §18.1 "Решения T-332" D1): lines only, the road
//! mesh is rebuilt on the command. Each piece of the stamp is its centre line and, when its
//! road width is known, the two edges; a raised or sunk stretch gets ticks down (or up) to the
//! terrain every [`TICK_SPACING`] metres, so a ramp reads as a ramp from above.

use bevy::math::{Vec2, Vec3};

/// Distance between the height ticks along a piece, metres.
pub const TICK_SPACING: f32 = 16.0;
/// A point less than this above or below the terrain gets no tick, metres.
const TICK_MIN: f32 = 1.0;

/// The lines of a stamp ghost, world coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StampGhostLines {
    /// Centre lines and edges, one polyline each.
    pub strips: Vec<Vec<Vec3>>,
    /// Vertical ticks from a point of a piece to the terrain under it.
    pub ticks: Vec<[Vec3; 2]>,
}

/// The ghost of the stamp pieces `lines` (one polyline each). `widths[i]` is the road width of
/// piece `i` (no edges without it); `ground` is the terrain height at a point.
pub fn stamp_ghost_lines(
    lines: &[Vec<Vec3>],
    widths: &[f32],
    ground: impl Fn(Vec2) -> f32,
) -> StampGhostLines {
    let mut out = StampGhostLines::default();
    for (i, line) in lines.iter().enumerate() {
        if line.len() < 2 {
            continue;
        }
        out.strips.push(line.clone());
        if let Some(&w) = widths.get(i).filter(|w| **w > 0.0) {
            for side in [-0.5, 0.5] {
                out.strips.push(offset(line, w * side));
            }
        }
        for p in samples(line, TICK_SPACING) {
            let g = ground(Vec2::new(p.x, p.z));
            if (p.y - g).abs() > TICK_MIN {
                out.ticks.push([p, Vec3::new(p.x, g, p.z)]);
            }
        }
    }
    out
}

/// `line` moved `by` metres to the left of its direction, in the horizontal plane.
fn offset(line: &[Vec3], by: f32) -> Vec<Vec3> {
    (0..line.len())
        .map(|i| {
            let (a, b) = (line[i.saturating_sub(1)], line[(i + 1).min(line.len() - 1)]);
            let t = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
            let left = Vec3::new(t.z, 0.0, -t.x)
                .try_normalize()
                .unwrap_or(Vec3::ZERO);
            line[i] + left * by
        })
        .collect()
}

/// Points along `line` every `step` metres of its length, from its start.
fn samples(line: &[Vec3], step: f32) -> Vec<Vec3> {
    let mut out = Vec::new();
    let mut next = 0.0;
    let mut walked = 0.0;
    for w in line.windows(2) {
        let len = w[0].distance(w[1]);
        while next <= walked + len {
            let t = if len > 0.0 {
                (next - walked) / len
            } else {
                0.0
            };
            out.push(w[0].lerp(w[1], t));
            next += step;
        }
        walked += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ramp from the ground to +6 m over 64 m, 8 m wide: a centre line and two edges 8 m
    /// apart, and ticks only where it stands more than a metre above the flat terrain.
    #[test]
    fn a_ramp_has_edges_at_its_width_and_ticks_where_it_is_raised() {
        let ramp = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(64.0, 6.0, 0.0)];
        let g = stamp_ghost_lines(std::slice::from_ref(&ramp), &[8.0], |_| 0.0);
        assert_eq!(g.strips.len(), 3);
        let (l, r) = (g.strips[1][0], g.strips[2][0]);
        assert!((l.distance(r) - 8.0).abs() < 1e-4, "{l} {r}");
        assert!((l.z.abs() - 4.0).abs() < 1e-4 && l.y == 0.0);
        // Samples at 0, 16, 32, 48, 64 m: heights 0, 1.5, 3, 4.5, 6; the first has no tick.
        assert_eq!(g.ticks.len(), 4);
        for [top, foot] in &g.ticks {
            assert!(top.y > 1.0 && foot.y == 0.0 && top.x == foot.x && top.z == foot.z);
        }
    }

    /// Without a width a piece is its centre line only; a tunnel's ticks reach up to the ground.
    #[test]
    fn no_width_no_edges_and_a_tunnel_ticks_up() {
        let tunnel = vec![Vec3::new(0.0, -6.0, 0.0), Vec3::new(0.0, -6.0, 40.0)];
        let g = stamp_ghost_lines(&[tunnel], &[], |_| 0.0);
        assert_eq!(g.strips.len(), 1);
        assert_eq!(g.ticks.len(), 3);
        assert!(g.ticks.iter().all(|[p, q]| p.y == -6.0 && q.y == 0.0));
    }
}
