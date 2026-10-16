//! Burning buildings (T-143, §16 M2 D "Пожар — рендер"): flames over the roof and a column of
//! smoke, as camera-facing quads in one vertex-coloured mesh with alpha. Pure logic, no Bevy
//! types. Everything depends on render time (seconds) and the building id only, never on the
//! sim tick, so it plays on while the sim is paused.
//!
//! [`burning`] lists the buildings that have the `FIRE` service problem (and stand `Active`);
//! [`build_mesh`] makes the effect of the ones near the camera: [`FLAMES`] tongues of flame
//! at hashed spots inside the footprint, flickering in height and from yellow to red, and
//! [`PUFFS`] smoke puffs that rise, grow and fade.

use bevy_math::Vec3;
use cp_building::{Building, BuildingId, BuildingState};
use cp_service::{ServiceProblems, ServiceState};

use crate::chunk::MeshData;
use crate::problem::Facing;

/// Tongues of flame per burning building.
pub const FLAMES: u32 = 6;
/// Smoke puffs per burning building.
pub const PUFFS: u32 = 5;
/// Flames are drawn nearer than this to the camera, metres; smoke nearer than [`SMOKE_RANGE_M`].
pub const FLAME_RANGE_M: f32 = 500.0;
pub const SMOKE_RANGE_M: f32 = 1200.0;
/// Seconds a puff takes from the roof to the top of the column.
pub const PUFF_LIFE_S: f32 = 4.0;
/// Height of the smoke column above the roof, metres.
pub const COLUMN_M: f32 = 22.0;

/// A burning building and its bounds (the mesh's box).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Burning {
    pub id: BuildingId,
    pub min: Vec3,
    pub max: Vec3,
}

/// The buildings that burn: `Active` with `FIRE` in their service problems. `bounds` gives a
/// building's box (`None` skips it, e.g. its mesh is not built yet).
pub fn burning<'a>(
    buildings: impl Iterator<Item = (BuildingId, &'a Building)>,
    state: &ServiceState,
    bounds: impl Fn(BuildingId) -> Option<(Vec3, Vec3)>,
) -> Vec<Burning> {
    buildings
        .filter(|(id, b)| {
            b.state == BuildingState::Active && state.problems_of(*id).has(ServiceProblems::FIRE)
        })
        .filter_map(|(id, _)| {
            let (min, max) = bounds(id)?;
            Some(Burning { id, min, max })
        })
        .collect()
}

fn hash(id: BuildingId, k: u32) -> u32 {
    let mut h = id.index.wrapping_mul(0x9E37_79B9)
        ^ id.generation.wrapping_mul(0x85EB_CA6B)
        ^ k.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 13;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 16)
}

/// `0..1` from the hash of `(id, k)`.
fn unit(id: BuildingId, k: u32) -> f32 {
    (hash(id, k) & 0xFFFF) as f32 / 65536.0
}

/// One tongue of flame: foot on the roof, height and colour mix at time `t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flame {
    pub foot: Vec3,
    pub height: f32,
    pub width: f32,
    /// Sideways sway of the tip, metres.
    pub sway: f32,
    /// 0 yellow .. 1 red.
    pub heat: f32,
}

/// One smoke puff: centre, radius and opacity at time `t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Puff {
    pub pos: Vec3,
    pub radius: f32,
    pub alpha: f32,
    pub shade: f32,
}

/// The flames of `b` at time `t`: feet at hashed spots of the footprint, on the roof.
pub fn flames(b: &Burning, t: f32) -> Vec<Flame> {
    let size = b.max - b.min;
    (0..FLAMES)
        .map(|k| {
            let (u, v) = (
                0.15 + 0.7 * unit(b.id, 2 * k),
                0.15 + 0.7 * unit(b.id, 2 * k + 1),
            );
            let phase = unit(b.id, 100 + k) * std::f32::consts::TAU;
            let wave = |hz: f32| 0.5 + 0.5 * (std::f32::consts::TAU * hz * t + phase).sin();
            Flame {
                foot: Vec3::new(b.min.x + size.x * u, b.max.y, b.min.z + size.z * v),
                height: 1.6 + 2.4 * wave(1.7) * (0.6 + 0.4 * wave(4.1)),
                width: 1.0 + 0.8 * unit(b.id, 200 + k),
                sway: 0.5 * (std::f32::consts::TAU * 0.9 * t + phase * 2.0).sin(),
                heat: wave(2.3),
            }
        })
        .collect()
}

/// The smoke puffs of `b` at time `t`: each rises through [`COLUMN_M`] in [`PUFF_LIFE_S`] and
/// starts over, the puffs staggered by their index.
pub fn puffs(b: &Burning, t: f32) -> Vec<Puff> {
    let c = (b.min + b.max) * 0.5;
    (0..PUFFS)
        .map(|k| {
            let age = (t / PUFF_LIFE_S + k as f32 / PUFFS as f32 + unit(b.id, 300)).fract();
            let drift = (age * 3.0 + unit(b.id, 310 + k) * 6.0).sin() * 2.0 * age;
            Puff {
                pos: Vec3::new(
                    c.x + drift,
                    b.max.y + 1.0 + age * COLUMN_M,
                    c.z + drift * 0.5,
                ),
                radius: 1.5 + age * 5.0,
                alpha: 0.65 * (1.0 - age) * (age * 8.0).min(1.0),
                shade: 0.12 + 0.25 * age,
            }
        })
        .collect()
}

const YELLOW: [f32; 3] = [1.0, 0.85, 0.2];
const RED: [f32; 3] = [0.9, 0.2, 0.05];

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// One mesh with the effects of the `burning` buildings near `facing.position` at time `t`.
/// Flames are two nested triangles (red outside, hotter inside); smoke puffs are octagons
/// with an alpha. Drawn with alpha blending and no culling.
pub fn build_mesh(burning: &[Burning], facing: &Facing, t: f32) -> MeshData {
    let mut m = MeshData::default();
    let tri = |m: &mut MeshData, p: [Vec3; 3], n: Vec3, c: [f32; 4]| {
        let base = m.positions.len() as u32;
        for v in p {
            m.positions.push(v.to_array());
            m.normals.push(n.to_array());
            m.colors.push(c);
        }
        m.indices.extend([base, base + 1, base + 2]);
    };
    for b in burning {
        let centre = (b.min + b.max) * 0.5;
        let dist = facing.position.distance(centre);
        if dist > SMOKE_RANGE_M {
            continue;
        }
        let toward = (facing.position - centre) / dist.max(1e-3);
        if dist <= FLAME_RANGE_M {
            for f in flames(b, t) {
                let tip = f.foot + Vec3::Y * f.height + facing.right * f.sway;
                let half = facing.right * (f.width * 0.5);
                let col = lerp3(YELLOW, RED, f.heat);
                tri(
                    &mut m,
                    [f.foot - half, f.foot + half, tip],
                    toward,
                    [col[0], col[1], col[2], 0.95],
                );
                let inner = lerp3(YELLOW, [1.0, 1.0, 0.8], 1.0 - f.heat);
                let (h2, w2) = (0.6, 0.5);
                tri(
                    &mut m,
                    [
                        f.foot - half * w2 + toward * 0.05,
                        f.foot + half * w2 + toward * 0.05,
                        f.foot + (tip - f.foot) * h2 + toward * 0.05,
                    ],
                    toward,
                    [inner[0], inner[1], inner[2], 0.95],
                );
            }
        }
        for p in puffs(b, t) {
            if p.alpha <= 0.01 {
                continue;
            }
            let c = [p.shade, p.shade, p.shade * 1.05, p.alpha];
            for i in 0..8 {
                let (a0, a1) = (
                    i as f32 * std::f32::consts::FRAC_PI_4,
                    (i + 1) as f32 * std::f32::consts::FRAC_PI_4,
                );
                let at = |a: f32| p.pos + (facing.right * a.cos() + facing.up * a.sin()) * p.radius;
                tri(&mut m, [p.pos, at(a0), at(a1)], toward, c);
            }
        }
    }
    m
}

#[cfg(test)]
mod tests;
