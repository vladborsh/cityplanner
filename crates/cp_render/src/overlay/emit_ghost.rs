//! The reach of a polluting or noisy building while its ghost is under the cursor (T-300 step 3,
//! `architecture/system-design.md` §11.1): a ring and a soft fill of the source's radius, brown
//! for pollution and violet for noise. The tool writes [`EmitGhost`] from
//! `cp_economy::emission_of`, the rule the sim uses, so the circle is what the building does.

use bevy::prelude::*;
use cp_sim::SimView;

use super::{OverlayLayers, flat, push};
use crate::chunk::MeshData;

/// What the placement ghost emits, written by `cp_tools` (default: nothing).
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct EmitGhost {
    pub site: Option<EmitSite>,
}

/// The ghost's centre (XZ, metres) and the radius in metres of each reach it has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EmitSite {
    pub centre: Vec2,
    pub pollution_m: Option<f32>,
    pub noise_m: Option<f32>,
}

/// Brown-yellow pollution, violet noise.
pub const POLLUTION_COLOR: [f32; 3] = [0.60, 0.48, 0.18];
pub const NOISE_COLOR: [f32; 3] = [0.54, 0.36, 0.84];
/// Alpha of the fill at the centre; it falls to 0 at the rim.
const FILL_ALPHA: f32 = 0.35;
const RING_ALPHA: f32 = 0.9;
/// Width of the ring, metres.
const RING_W: f32 = 2.0;
/// A hair above the lot marker of the service ghost (0.8 m).
const LIFT: f32 = 0.8;
const SEGMENTS: usize = 64;

/// One circle: a ring `RING_W` wide just inside `radius` and a fan fading outwards.
fn circle(
    m: &mut MeshData,
    centre: Vec2,
    radius: f32,
    rgb: [f32; 3],
    height: &impl Fn(Vec2) -> f32,
) {
    let at = |p: Vec2| [p.x, height(p) + LIFT, p.y];
    let dir = |k: usize| {
        let t = k as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        Vec2::new(t.cos(), t.sin())
    };
    let c0 = push(m, at(centre), [rgb[0], rgb[1], rgb[2], FILL_ALPHA]);
    let rim: Vec<u32> = (0..SEGMENTS)
        .map(|k| {
            push(
                m,
                at(centre + dir(k) * radius),
                [rgb[0], rgb[1], rgb[2], 0.0],
            )
        })
        .collect();
    for k in 0..SEGMENTS {
        // Counter-clockwise seen from above (+y): x towards z is clockwise, so wind it reversed.
        m.indices
            .extend_from_slice(&[c0, rim[(k + 1) % SEGMENTS], rim[k]]);
    }
    let (r0, r1) = ((radius - RING_W).max(0.0), radius);
    for k in 0..SEGMENTS {
        let (u, v) = (dir(k), dir(k + 1));
        flat(
            m,
            [
                at(centre + u * r0),
                at(centre + u * r1),
                at(centre + v * r1),
                at(centre + v * r0),
            ],
            [rgb[0], rgb[1], rgb[2], RING_ALPHA],
        );
    }
}

/// The circles of `site`, at `height(p)` + a lift.
pub fn emit_ghost_mesh(site: &EmitSite, height: impl Fn(Vec2) -> f32) -> MeshData {
    let mut m = MeshData::default();
    if let Some(r) = site.pollution_m.filter(|r| *r > 0.0) {
        circle(&mut m, site.centre, r, POLLUTION_COLOR, &height);
    }
    if let Some(r) = site.noise_m.filter(|r| *r > 0.0) {
        circle(&mut m, site.centre, r, NOISE_COLOR, &height);
    }
    m
}

#[derive(Resource, Default)]
pub(super) struct EmitGhostLayer {
    key: Option<(EmitGhost, usize)>,
    entity: Option<Entity>,
}

/// Marks the ghost's circles.
#[derive(Component)]
pub struct EmitGhostCircles;

pub(super) fn sync_emit_ghost(
    view: Option<Res<SimView>>,
    overlay: Res<OverlayLayers>,
    ghost: Res<EmitGhost>,
    mut layer: ResMut<EmitGhostLayer>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let (Some(view), Some(material)) = (view, overlay.material.as_ref()) else {
        return;
    };
    let key = (
        ghost.clone(),
        view.terrain
            .as_ref()
            .map_or(0, |t| std::sync::Arc::as_ptr(t) as usize),
    );
    if layer.key.as_ref() == Some(&key) {
        return;
    }
    layer.key = Some(key);
    if let Some(e) = layer.entity.take() {
        commands.entity(e).despawn();
    }
    let (Some(site), Some(terrain)) = (ghost.site.as_ref(), view.terrain.as_deref()) else {
        return;
    };
    let mesh = emit_ghost_mesh(site, |p| crate::chunk::terrain_mesh_height(terrain, p));
    layer.entity = super::transit::spawn(
        &mut commands,
        &mut meshes,
        material,
        mesh,
        "emit ghost",
        EmitGhostCircles,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_reach_is_a_circle_of_its_radius_in_its_colour() {
        let site = EmitSite {
            centre: Vec2::new(100.0, 200.0),
            pollution_m: Some(64.0),
            noise_m: Some(96.0),
        };
        let m = emit_ghost_mesh(&site, |_| 5.0);
        assert!(m.triangle_count() > 0);
        let of = |rgb: [f32; 3]| -> Vec<usize> {
            (0..m.positions.len())
                .filter(|&i| m.colors[i][..3] == rgb)
                .collect()
        };
        for (rgb, r) in [(POLLUTION_COLOR, 64.0), (NOISE_COLOR, 96.0)] {
            let idx = of(rgb);
            assert!(!idx.is_empty());
            let far = idx
                .iter()
                .map(|&i| Vec2::new(m.positions[i][0] - 100.0, m.positions[i][2] - 200.0).length())
                .fold(0.0f32, f32::max);
            assert!((far - r).abs() < 0.01, "{far} vs {r}");
            // The fill fades to nothing at the rim; the ring is strong.
            assert!(idx.iter().any(|&i| m.colors[i][3] == 0.0));
            assert!(idx.iter().any(|&i| m.colors[i][3] == RING_ALPHA));
            assert!(idx.iter().all(|&i| (m.positions[i][1] - 5.8).abs() < 1e-4));
        }
    }

    #[test]
    fn a_building_with_neither_reach_draws_nothing_and_the_fill_faces_up() {
        let none = EmitSite {
            centre: Vec2::ZERO,
            pollution_m: None,
            noise_m: Some(0.0),
        };
        assert!(emit_ghost_mesh(&none, |_| 0.0).indices.is_empty());
        let one = EmitSite {
            noise_m: Some(16.0),
            ..none
        };
        let m = emit_ghost_mesh(&one, |_| 0.0);
        for t in m.indices.chunks(3) {
            let p = |i: u32| Vec2::new(m.positions[i as usize][0], m.positions[i as usize][2]);
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            // Facing up (+y) means counter-clockwise seen from above: x right, z towards the
            // viewer, i.e. a negative cross product in (x, z).
            assert!((b - a).perp_dot(c - a) <= 1e-4, "{t:?}");
        }
    }
}
