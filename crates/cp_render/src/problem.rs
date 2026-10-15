//! Problem icons over buildings (T-125, §13): pure logic, no Bevy types. A building with a
//! problem flag shows a billboard icon above its roof that bobs up and down; with several
//! problems the icons take turns, one per second, most important first. Everything here
//! depends on render time only (seconds since start), never on the sim tick.
//!
//! [`collect`] picks the icon of every building within [`RANGE_M`] of the camera, and
//! [`build_mesh`] turns the picks into one camera-facing mesh (one draw call). The icons are
//! self-made vertex-coloured shapes on a square plate, one silhouette per problem (the user's
//! rule for icons: square, uniform, distinct).

use bevy_math::Vec3;
use cp_building::{Building, BuildingFlags, BuildingId, BuildingState};

use crate::chunk::MeshData;

/// Icons are created only for buildings closer than this to the camera, metres.
pub const RANGE_M: f32 = 1500.0;
/// Seconds each icon of a building with several problems stays up.
pub const CYCLE_S: f32 = 1.0;
/// Bobbing: a sine of this many hertz and this amplitude in metres, phase from the building id.
pub const BOB_HZ: f32 = 1.0;
pub const BOB_AMPLITUDE_M: f32 = 0.5;
/// Height of the icon's resting centre above the roof top, metres.
pub const LIFT_M: f32 = 3.0;

/// A problem the icon shows. Declaration order is the importance order: a building with
/// several shows them in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Problem {
    /// `NO_ROAD` / `NO_PATH`: the building cannot be reached.
    NoRoad,
    NoPower,
    /// `NO_WATER` and `NO_SEWAGE`: for the player both are "the pipe" (T-101), one icon.
    NoWater,
    Abandoned,
}

impl Problem {
    pub const ALL: [Problem; 4] = [
        Problem::NoRoad,
        Problem::NoPower,
        Problem::NoWater,
        Problem::Abandoned,
    ];

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// The set of problems of one building, as a bit set over [`Problem`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Problems(pub u8);

impl Problems {
    pub fn of(b: &Building) -> Self {
        // Under construction and burned-out buildings carry no problem icons.
        let (flags, abandoned) = match b.state {
            BuildingState::Active => (b.flags, false),
            BuildingState::Abandoned => (b.flags, true),
            BuildingState::UnderConstruction | BuildingState::Burned => return Self(0),
        };
        let mut set = 0;
        if flags.has(BuildingFlags::NO_ROAD) || flags.has(BuildingFlags::NO_PATH) {
            set |= Problem::NoRoad.bit();
        }
        if flags.has(BuildingFlags::NO_POWER) {
            set |= Problem::NoPower.bit();
        }
        if flags.has(BuildingFlags::NO_WATER) || flags.has(BuildingFlags::NO_SEWAGE) {
            set |= Problem::NoWater.bit();
        }
        if abandoned {
            set |= Problem::Abandoned.bit();
        }
        Self(set)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    pub fn has(self, p: Problem) -> bool {
        self.0 & p.bit() != 0
    }

    /// The problems in importance order.
    pub fn iter(self) -> impl Iterator<Item = Problem> {
        Problem::ALL.into_iter().filter(move |p| self.has(*p))
    }

    /// The icon on show at render time `t` seconds: the list in importance order, one entry
    /// per [`CYCLE_S`], round and round.
    pub fn shown(self, t: f32) -> Option<Problem> {
        let n = self.len();
        if n == 0 {
            return None;
        }
        let slot = (t.max(0.0) / CYCLE_S) as usize % n;
        self.iter().nth(slot)
    }
}

/// Phase of a building's bobbing, `0..2π`, from its id so neighbours do not move in step.
pub fn bob_phase(id: BuildingId) -> f32 {
    let mut h = id.index.wrapping_mul(0x9E37_79B9) ^ id.generation.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 13;
    (h & 0xFFFF) as f32 / 65536.0 * std::f32::consts::TAU
}

/// Vertical offset of the icon of `id` at time `t`, within `±BOB_AMPLITUDE_M`.
pub fn bob(id: BuildingId, t: f32) -> f32 {
    BOB_AMPLITUDE_M * (std::f32::consts::TAU * BOB_HZ * t + bob_phase(id)).sin()
}

/// A building's problems and where its icon rests, cached between frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marked {
    pub id: BuildingId,
    pub problems: Problems,
    /// Centre of the roof top (the mesh's highest point).
    pub top: Vec3,
}

/// The icon of one building this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconInstance {
    pub id: BuildingId,
    pub problem: Problem,
    /// Centre of the icon, bob included.
    pub pos: Vec3,
}

/// The marked buildings among `buildings`; `top` gives a building's roof (`None` skips it,
/// e.g. its mesh is not built yet).
pub fn mark<'a>(
    buildings: impl Iterator<Item = (BuildingId, &'a Building)>,
    top: impl Fn(BuildingId) -> Option<Vec3>,
) -> Vec<Marked> {
    buildings
        .filter_map(|(id, b)| {
            let problems = Problems::of(b);
            if problems.is_empty() {
                return None;
            }
            Some(Marked {
                id,
                problems,
                top: top(id)?,
            })
        })
        .collect()
}

/// The icons to draw at render time `t` for a camera at `camera`: marked buildings closer
/// than [`RANGE_M`] (horizontally-and-vertically, straight-line).
pub fn collect(marked: &[Marked], camera: Vec3, t: f32) -> Vec<IconInstance> {
    let r2 = RANGE_M * RANGE_M;
    marked
        .iter()
        .filter(|m| m.top.distance_squared(camera) <= r2)
        .filter_map(|m| {
            let problem = m.problems.shown(t)?;
            Some(IconInstance {
                id: m.id,
                problem,
                pos: m.top + Vec3::Y * (LIFT_M + bob(m.id, t)),
            })
        })
        .collect()
}

/// Camera frame used to face the icons towards the viewer.
#[derive(Clone, Copy, Debug)]
pub struct Facing {
    pub position: Vec3,
    pub right: Vec3,
    pub up: Vec3,
}

/// Side length of an icon `distance` metres from the camera: grows with distance so it stays
/// readable from far away.
pub fn icon_size(distance: f32) -> f32 {
    2.5 + distance * 0.01
}

const BORDER: [f32; 4] = [0.07, 0.07, 0.08, 1.0];
const WHITE: [f32; 4] = [0.95, 0.95, 0.93, 1.0];
const DARK: [f32; 4] = [0.10, 0.09, 0.08, 1.0];

/// A convex polygon in the icon's unit square (`-0.5..0.5` both ways, y up) with its colour.
type Shape = (&'static [[f32; 2]], [f32; 4]);

fn plate(p: Problem) -> [f32; 4] {
    match p {
        Problem::NoRoad => [0.80, 0.18, 0.14, 1.0],
        Problem::NoPower => [0.95, 0.74, 0.12, 1.0],
        Problem::NoWater => [0.16, 0.50, 0.85, 1.0],
        Problem::Abandoned => [0.48, 0.50, 0.54, 1.0],
    }
}

/// The glyph on the plate: a road cut in two, a bolt, a drop, boards nailed crosswise.
fn glyph(p: Problem) -> (&'static [Shape], [f32; 4]) {
    let plate_color = plate(p);
    match p {
        Problem::NoRoad => (
            &[
                (
                    &[[-0.30, -0.30], [0.30, -0.30], [0.08, 0.30], [-0.08, 0.30]],
                    WHITE,
                ),
                (
                    &[[-0.34, -0.04], [0.34, -0.04], [0.34, 0.06], [-0.34, 0.06]],
                    [0.80, 0.18, 0.14, 1.0],
                ),
            ],
            plate_color,
        ),
        Problem::NoPower => (
            &[
                (&[[0.12, 0.40], [-0.20, -0.06], [0.06, -0.06]], DARK),
                (&[[0.20, 0.06], [-0.06, 0.06], [-0.12, -0.40]], DARK),
            ],
            plate_color,
        ),
        Problem::NoWater => (
            &[
                (&[[0.0, 0.38], [-0.20, -0.04], [0.20, -0.04]], WHITE),
                (
                    &[
                        [-0.20, -0.04],
                        [-0.14, -0.26],
                        [0.0, -0.34],
                        [0.14, -0.26],
                        [0.20, -0.04],
                        [0.0, 0.0],
                    ],
                    WHITE,
                ),
            ],
            plate_color,
        ),
        Problem::Abandoned => (
            &[
                (
                    &[[-0.34, -0.24], [-0.24, -0.34], [0.34, 0.24], [0.24, 0.34]],
                    DARK,
                ),
                (
                    &[[0.24, -0.34], [0.34, -0.24], [-0.24, 0.34], [-0.34, 0.24]],
                    [0.30, 0.22, 0.14, 1.0],
                ),
            ],
            plate_color,
        ),
    }
}

/// One mesh with a camera-facing quad stack per icon: border, plate, glyph. Layers sit a
/// hair apart towards the camera, so the depth test keeps them in order.
pub fn build_mesh(icons: &[IconInstance], facing: &Facing) -> MeshData {
    let mut m = MeshData::default();
    for icon in icons {
        let to_cam = facing.position - icon.pos;
        let dist = to_cam.length();
        let size = icon_size(dist);
        let toward = to_cam / dist.max(1e-3);
        let normal = toward.to_array();
        let (shapes, plate_color) = glyph(icon.problem);
        let mut layer = 0.0_f32;
        let mut push = |m: &mut MeshData, pts: &[[f32; 2]], color: [f32; 4], scale: f32| {
            let base = m.positions.len() as u32;
            let lift = toward * (layer * 0.02 * size);
            layer += 1.0;
            for p in pts {
                let v = icon.pos + (facing.right * p[0] + facing.up * p[1]) * (size * scale) + lift;
                m.positions.push(v.to_array());
                m.normals.push(normal);
                m.colors.push(color);
            }
            for i in 1..pts.len() as u32 - 1 {
                m.indices.extend([base, base + i, base + i + 1]);
            }
        };
        const SQUARE: [[f32; 2]; 4] = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]];
        push(&mut m, &SQUARE, BORDER, 1.0);
        push(&mut m, &SQUARE, plate_color, 0.86);
        for (pts, color) in shapes {
            push(&mut m, pts, *color, 1.0);
        }
    }
    m
}

#[cfg(test)]
mod tests;
