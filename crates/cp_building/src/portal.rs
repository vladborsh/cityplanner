//! Where people and cars leave and enter a building (`architecture/system-design.md` §25.1):
//! the door and the yard gate on the lot front, the edge nearest the access point. Derived from
//! the building and the network; nothing here is saved.

use bevy_math::Vec2;
use cp_net::NetGraph;

use crate::model::{Building, FOOTPRINT_CELL_M};

/// How far the door sits behind the front edge, metres: a pedestrian steps out of the wall.
pub const DOOR_INSET_M: f32 = 0.5;

/// The building's portals in world XZ.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Portals {
    /// The door: the middle of the front edge, [`DOOR_INSET_M`] inside the lot.
    pub door: Vec2,
    /// The yard gate on the front edge, beside the door.
    pub drive: Vec2,
    /// Unit outward normal of the front edge (towards the road).
    pub out: Vec2,
}

impl Building {
    /// The door and yard gate on the footprint edge nearest the access point; `None` without an
    /// access point (or with one on a segment that is gone).
    pub fn portals(&self, net: &NetGraph) -> Option<Portals> {
        let access = self.access()?;
        let p = net.segment(access.segment)?.curve.point(access.t);
        Some(portals_towards(
            &self.footprint_polygon(),
            self.rot,
            Vec2::new(p.x, p.z),
        ))
    }
}

/// [`Building::portals`] for the counter-clockwise rectangle `quad` turned by `rot`, its front
/// the edge nearest `access` (ties to the lower edge index).
pub fn portals_towards(quad: &[Vec2; 4], rot: f32, access: Vec2) -> Portals {
    let edge_dist = |i: usize| {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        let ab = b - a;
        let t = ((access - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
        access.distance(a + ab * t)
    };
    let front = (1..4).fold(0, |best, i| {
        if edge_dist(i) < edge_dist(best) {
            i
        } else {
            best
        }
    });
    let (a, b) = (quad[front], quad[(front + 1) % 4]);
    let mid = (a + b) * 0.5;
    let along = (b - a).normalize_or_zero();
    // Counter-clockwise: the interior is on the left of each edge, so outward is its right.
    let out = Vec2::new(along.y, -along.x);
    // The gate goes towards the building's local +x, or +y on an edge across it.
    let (s, c) = rot.sin_cos();
    let (x, y) = (Vec2::new(c, s), Vec2::new(-s, c));
    let toward = if along.dot(x).abs() > 1e-3 {
        along * along.dot(x).signum()
    } else {
        along * along.dot(y).signum()
    };
    let half = a.distance(b) * 0.5;
    let off = if half <= FOOTPRINT_CELL_M * 0.5 + 1e-3 {
        1.5
    } else {
        (half - 2.5).min(6.0)
    };
    Portals {
        door: mid - out * DOOR_INSET_M,
        drive: mid + toward * off,
        out,
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use bevy_math::Vec2;

    use super::*;
    use crate::model::footprint_quad;

    /// `p` in the building's local frame (x along `rot`, y across).
    fn local(p: Vec2, pos: Vec2, rot: f32) -> Vec2 {
        let (s, c) = rot.sin_cos();
        let d = p - pos;
        Vec2::new(d.dot(Vec2::new(c, s)), d.dot(Vec2::new(-s, c)))
    }

    fn near(a: Vec2, b: Vec2) -> bool {
        a.distance(b) < 1e-3
    }

    /// A 2×2 lot with the road along its local −y side: the door 0,5 m in from the front
    /// middle, the gate 5,5 m towards +x; turned by 90° the local coordinates stay the same.
    #[test]
    fn lot_portals_sit_on_the_front_nearest_the_access_in_local_coordinates() {
        for rot in [0.0, FRAC_PI_2, 1.0] {
            let pos = Vec2::new(100.0, -40.0);
            let (s, c) = rot.sin_cos();
            let (x, y) = (Vec2::new(c, s), Vec2::new(-s, c));
            // The access is 12 m in front of the −y edge, a little to the side.
            let access = pos - y * 20.0 + x * 3.0;
            let p = portals_towards(&footprint_quad(pos, rot, 2, 2), rot, access);
            assert!(near(local(p.door, pos, rot), Vec2::new(0.0, -7.5)), "{rot}");
            assert!(
                near(local(p.drive, pos, rot), Vec2::new(5.5, -8.0)),
                "{rot}"
            );
            assert!(near(p.out, -y), "{rot}: {:?}", p.out);
        }
    }

    /// A one-cell-wide front: the gate 1,5 m beside the middle; a 4-cell front caps it at 6 m.
    #[test]
    fn gate_offset_follows_the_front_width() {
        let access = Vec2::new(0.0, -30.0);
        let p = portals_towards(&footprint_quad(Vec2::ZERO, 0.0, 1, 3), 0.0, access);
        assert!(near(p.drive, Vec2::new(1.5, -12.0)), "{:?}", p.drive);
        let p = portals_towards(&footprint_quad(Vec2::ZERO, 0.0, 4, 1), 0.0, access);
        assert!(near(p.drive, Vec2::new(6.0, -4.0)), "{:?}", p.drive);
    }

    /// A free-standing 3×2 rectangle with the road off its local +x end: the front is the short
    /// `d` edge, the gate goes towards local +y along it.
    #[test]
    fn free_rectangle_front_can_be_a_short_edge() {
        let rot: f32 = 0.3;
        let pos = Vec2::new(10.0, 10.0);
        let (s, c) = rot.sin_cos();
        let x = Vec2::new(c, s);
        let p = portals_towards(&footprint_quad(pos, rot, 3, 2), rot, pos + x * 25.0);
        assert!(near(local(p.door, pos, rot), Vec2::new(11.5, 0.0)));
        assert!(near(local(p.drive, pos, rot), Vec2::new(12.0, 5.5)));
        assert!(near(p.out, x));
    }
}
