//! Problem icons over buildings (T-125, §13): pure logic, no Bevy types. A building with a
//! problem flag shows a billboard icon above its roof that bobs up and down; with several
//! problems the icons take turns, one per second, most important first. Everything here
//! depends on render time only (seconds since start), never on the sim tick.
//!
//! [`collect`] picks the icons: buildings nearer than [`NEAR_M`] get one each, farther ones are
//! grouped into a fixed world grid whose cell grows with distance ([`CELL_M`]) and each cell
//! gets one cluster icon with a count badge. [`build_mesh`] turns the picks into one
//! camera-facing mesh (one draw call). The icons are
//! self-made vertex-coloured shapes on a square plate, one silhouette per problem (the user's
//! rule for icons: square, uniform, distinct).

use bevy_math::Vec3;
use cp_building::{Building, BuildingFlags, BuildingId, BuildingState};
use cp_service::{ServiceProblems, ServiceState};

use crate::chunk::MeshData;

/// Buildings closer than this to the camera get an icon each, metres.
pub const NEAR_M: f32 = 400.0;
/// Distance bands: band `b` starts at `BAND_EDGES[b - 1]` (band 0 is nearer than `NEAR_M`).
const BAND_EDGES: [f32; 4] = [NEAR_M, 800.0, 1600.0, 3200.0];
/// Cluster cell side per band, metres; band 0 draws single icons.
pub const CELL_M: [f32; 5] = [0.0, 64.0, 128.0, 256.0, 512.0];
/// A band changes only once the distance is this far past its edge (fraction of the edge).
const HYSTERESIS: f32 = 0.1;
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
    /// `ServiceProblems::FIRE` (T-126): the building burns.
    Fire,
    /// `NO_ROAD` / `NO_PATH`: the building cannot be reached.
    NoRoad,
    NoPower,
    /// `NO_WATER` and `NO_SEWAGE`: for the player both are "the pipe" (T-101), one icon.
    NoWater,
    /// `ServiceProblems::DEAD`: bodies in the building.
    Dead,
    /// `ServiceProblems::SICK`: sick residents.
    Sick,
    /// `ServiceProblems::GARBAGE`: the garbage is not collected.
    Garbage,
    /// `ServiceProblems::CRIME`: a crime was committed.
    Crime,
    /// `BuildingFlags::NO_WORKERS`: too few workers.
    NoWorkers,
    Abandoned,
}

impl Problem {
    pub const ALL: [Problem; 10] = [
        Problem::Fire,
        Problem::NoRoad,
        Problem::NoPower,
        Problem::NoWater,
        Problem::Dead,
        Problem::Sick,
        Problem::Garbage,
        Problem::Crime,
        Problem::NoWorkers,
        Problem::Abandoned,
    ];

    fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// The set of problems of one building, as a bit set over [`Problem`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Problems(pub u16);

impl Problems {
    /// The problems of `b`; `service` are its `ServiceProblems` bits (shown while it is Active).
    pub fn of(b: &Building, service: ServiceProblems) -> Self {
        // Under construction and burned-out buildings carry no problem icons.
        let (flags, abandoned) = match b.state {
            BuildingState::Active => (b.flags, false),
            BuildingState::Abandoned => (b.flags, true),
            BuildingState::UnderConstruction | BuildingState::Burned => return Self(0),
        };
        let mut set = 0;
        let svc = |bit: u16, p: Problem| {
            if !abandoned && service.has(bit) {
                p.bit()
            } else {
                0
            }
        };
        set |= svc(ServiceProblems::FIRE, Problem::Fire)
            | svc(ServiceProblems::DEAD, Problem::Dead)
            | svc(ServiceProblems::SICK, Problem::Sick)
            | svc(ServiceProblems::GARBAGE, Problem::Garbage)
            | svc(ServiceProblems::CRIME, Problem::Crime);
        if flags.has(BuildingFlags::NO_WORKERS) {
            set |= Problem::NoWorkers.bit();
        }
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
    /// Buildings with `problem` this icon stands for: 1 for a single icon, else the badge.
    pub count: u32,
}

/// The marked buildings among `buildings`; `top` gives a building's roof (`None` skips it,
/// e.g. its mesh is not built yet).
pub fn mark<'a>(
    buildings: impl Iterator<Item = (BuildingId, &'a Building)>,
    state: &ServiceState,
    top: impl Fn(BuildingId) -> Option<Vec3>,
) -> Vec<Marked> {
    buildings
        .filter_map(|(id, b)| {
            let problems = Problems::of(b, state.problems_of(id));
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

/// Distance band of a building, with hysteresis around the edges relative to `prev`
/// (`u8::MAX` for none yet).
fn band(distance: f32, prev: u8) -> u8 {
    let fresh = BAND_EDGES.iter().filter(|&&e| distance >= e).count() as u8;
    if prev as usize >= CELL_M.len() || prev == fresh {
        return fresh;
    }
    if fresh > prev && distance < BAND_EDGES[prev as usize] * (1.0 + HYSTERESIS) {
        return prev;
    }
    if fresh < prev && distance > BAND_EDGES[prev as usize - 1] * (1.0 - HYSTERESIS) {
        return prev;
    }
    fresh
}

/// Per-building distance bands, kept between frames for the hysteresis. Clear it when the
/// marked list is rebuilt.
#[derive(Debug, Default)]
pub struct Lod {
    bands: Vec<u8>,
}

impl Lod {
    pub fn clear(&mut self) {
        self.bands.clear();
    }
}

#[derive(Default)]
struct Cell {
    buildings: u32,
    counts: [u32; Problem::ALL.len()],
    problems: u16,
    sum: Vec3,
    first: u32,
    first_id: Option<BuildingId>,
}

/// The icons to draw at render time `t` for a camera at `camera`: one per marked building
/// nearer than [`NEAR_M`]; beyond it one per non-empty cell of the world grid of its band
/// ([`CELL_M`]), at the centroid of its buildings, showing the cell's problems in turn (most
/// important first) with the number of buildings that have the one on show.
pub fn collect(marked: &[Marked], lod: &mut Lod, camera: Vec3, t: f32) -> Vec<IconInstance> {
    lod.bands.resize(marked.len(), u8::MAX);
    let mut out = Vec::new();
    let mut cells: std::collections::HashMap<(u8, i32, i32), Cell> = Default::default();
    for (m, b) in marked.iter().zip(lod.bands.iter_mut()) {
        *b = band(m.top.distance(camera), *b);
        if *b == 0 {
            if let Some(problem) = m.problems.shown(t) {
                out.push(IconInstance {
                    id: m.id,
                    problem,
                    pos: m.top + Vec3::Y * (LIFT_M + bob(m.id, t)),
                    count: 1,
                });
            }
            continue;
        }
        let size = CELL_M[*b as usize];
        let key = (
            *b,
            (m.top.x / size).floor() as i32,
            (m.top.z / size).floor() as i32,
        );
        let c = cells.entry(key).or_default();
        c.buildings += 1;
        c.problems |= m.problems.0;
        for p in m.problems.iter() {
            c.counts[p as usize] += 1;
        }
        c.sum += m.top;
        if c.first_id.is_none_or(|f| m.id < f) {
            c.first_id = Some(m.id);
            c.first = m.id.index;
        }
    }
    let mut keys: Vec<_> = cells.keys().copied().collect();
    keys.sort_unstable();
    for key in keys {
        let c = &cells[&key];
        let (Some(id), Some(problem)) = (c.first_id, Problems(c.problems).shown(t)) else {
            continue;
        };
        let centre = c.sum / c.buildings as f32;
        out.push(IconInstance {
            id,
            problem,
            pos: centre + Vec3::Y * (LIFT_M + bob(id, t)),
            count: c.counts[problem as usize],
        });
    }
    out
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
        Problem::Fire => [0.86, 0.34, 0.07, 1.0],
        Problem::Dead => [0.52, 0.48, 0.64, 1.0],
        Problem::Sick => [0.18, 0.60, 0.34, 1.0],
        Problem::Garbage => [0.46, 0.38, 0.20, 1.0],
        Problem::Crime => [0.20, 0.26, 0.58, 1.0],
        Problem::NoWorkers => [0.58, 0.36, 0.70, 1.0],
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
        // A flame of two tongues, hot core in the bigger one.
        Problem::Fire => (
            &[
                (&[[-0.30, -0.32], [-0.04, -0.32], [-0.18, 0.12]], WHITE),
                (&[[-0.12, -0.36], [0.30, -0.36], [0.10, 0.40]], WHITE),
                (
                    &[[-0.02, -0.34], [0.20, -0.34], [0.10, -0.02]],
                    [0.95, 0.74, 0.12, 1.0],
                ),
            ],
            plate_color,
        ),
        // A gravestone with a cross.
        Problem::Dead => (
            &[
                (
                    &[
                        [-0.22, -0.34],
                        [0.22, -0.34],
                        [0.22, 0.10],
                        [0.12, 0.32],
                        [-0.12, 0.32],
                        [-0.22, 0.10],
                    ],
                    WHITE,
                ),
                (
                    &[[-0.03, -0.08], [0.03, -0.08], [0.03, 0.22], [-0.03, 0.22]],
                    DARK,
                ),
                (
                    &[[-0.12, 0.08], [0.12, 0.08], [0.12, 0.14], [-0.12, 0.14]],
                    DARK,
                ),
            ],
            plate_color,
        ),
        // A medical cross.
        Problem::Sick => (
            &[
                (
                    &[[-0.10, -0.34], [0.10, -0.34], [0.10, 0.34], [-0.10, 0.34]],
                    WHITE,
                ),
                (
                    &[[-0.34, -0.10], [0.34, -0.10], [0.34, 0.10], [-0.34, 0.10]],
                    WHITE,
                ),
            ],
            plate_color,
        ),
        // A bin with a lid and ribs.
        Problem::Garbage => (
            &[
                (
                    &[[-0.18, -0.34], [0.18, -0.34], [0.24, 0.16], [-0.24, 0.16]],
                    WHITE,
                ),
                (
                    &[[-0.32, 0.16], [0.32, 0.16], [0.32, 0.26], [-0.32, 0.26]],
                    WHITE,
                ),
                (
                    &[[-0.04, 0.26], [0.04, 0.26], [0.04, 0.34], [-0.04, 0.34]],
                    WHITE,
                ),
                (
                    &[[-0.12, -0.26], [-0.07, -0.26], [-0.07, 0.08], [-0.12, 0.08]],
                    DARK,
                ),
                (
                    &[[0.07, -0.26], [0.12, -0.26], [0.12, 0.08], [0.07, 0.08]],
                    DARK,
                ),
            ],
            plate_color,
        ),
        // A burglar's mask with eye holes.
        Problem::Crime => (
            &[
                (
                    &[
                        [-0.22, -0.22],
                        [0.22, -0.22],
                        [0.36, 0.08],
                        [0.22, 0.24],
                        [-0.22, 0.24],
                        [-0.36, 0.08],
                    ],
                    WHITE,
                ),
                (
                    &[[-0.24, -0.06], [-0.12, -0.06], [-0.08, 0.04], [-0.26, 0.04]],
                    DARK,
                ),
                (
                    &[[0.12, -0.06], [0.24, -0.06], [0.26, 0.04], [0.08, 0.04]],
                    DARK,
                ),
            ],
            plate_color,
        ),
        // A hard hat over a question mark.
        Problem::NoWorkers => (
            &[
                (
                    &[
                        [0.3, 0.0],
                        [0.22, 0.26],
                        [0.0, 0.36],
                        [-0.22, 0.26],
                        [-0.3, 0.0],
                    ],
                    WHITE,
                ),
                (
                    &[[-0.36, -0.08], [0.36, -0.08], [0.36, 0.0], [-0.36, 0.0]],
                    WHITE,
                ),
                (
                    &[[-0.1, -0.22], [0.1, -0.22], [0.1, -0.16], [-0.1, -0.16]],
                    WHITE,
                ),
                (
                    &[[0.04, -0.3], [0.1, -0.3], [0.1, -0.22], [0.04, -0.22]],
                    WHITE,
                ),
                (
                    &[[-0.03, -0.4], [0.03, -0.4], [0.03, -0.34], [-0.03, -0.34]],
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

/// Seven-segment digits for the count badge: segments top, upper-left, upper-right, middle,
/// lower-left, lower-right, bottom.
const SEGMENTS: [u8; 10] = [
    0b1110111, 0b0010010, 0b1011101, 0b1011011, 0b0111010, 0b1101011, 0b1101111, 0b1010010,
    0b1111111, 0b1111011,
];

/// Rectangles `[x0, y0, x1, y1]` of digit `d` in a cell of width 1 and height 1 (y up).
fn digit_rects(d: u32) -> Vec<[f32; 4]> {
    const T: f32 = 0.2;
    let all = [
        [0.0, 1.0 - T, 1.0, 1.0],
        [0.0, 0.5, T, 1.0],
        [1.0 - T, 0.5, 1.0, 1.0],
        [0.0, 0.5 - T / 2.0, 1.0, 0.5 + T / 2.0],
        [0.0, 0.0, T, 0.5],
        [1.0 - T, 0.0, 1.0, 0.5],
        [0.0, 0.0, 1.0, T],
    ];
    let bits = SEGMENTS[d as usize % 10];
    (0..7)
        .filter(|i| bits & (1 << (6 - i)) != 0)
        .map(|i| all[i])
        .collect()
}

/// The badge shapes ("×" and the digits of `count`, white on a dark box hanging off the
/// plate's lower right corner) as `(polygon, colour)` in the icon's unit square.
fn badge(count: u32) -> Vec<(Vec<[f32; 2]>, [f32; 4])> {
    let digits: Vec<u32> = count
        .min(9999)
        .to_string()
        .bytes()
        .map(|b| u32::from(b - b'0'))
        .collect();
    let n = digits.len() as f32;
    let adv = (0.95 / (n + 1.0)).min(0.22);
    let (h, pad) = (0.28, 0.04);
    let width = (n + 1.0) * adv + 2.0 * pad;
    let x0 = 0.5 - width;
    let y0 = -0.5 - (h + 2.0 * pad) / 2.0;
    let rect = |a: f32, b: f32, c: f32, d: f32| vec![[a, b], [c, b], [c, d], [a, d]];
    let mut out = vec![(rect(x0, y0, 0.5, y0 + h + 2.0 * pad), BORDER)];
    let (gx, gy) = (x0 + pad, y0 + pad);
    // The multiplication sign: two thin diagonals.
    let (cx, cy, r, w) = (gx + adv * 0.4, gy + h / 2.0, adv * 0.32, 0.018);
    out.push((
        vec![
            [cx - r - w, cy - r],
            [cx - r + w, cy - r - w],
            [cx + r + w, cy + r],
            [cx + r - w, cy + r + w],
        ],
        WHITE,
    ));
    out.push((
        vec![
            [cx + r - w, cy - r - w],
            [cx + r + w, cy - r],
            [cx - r + w, cy + r + w],
            [cx - r - w, cy + r],
        ],
        WHITE,
    ));
    for (i, d) in digits.iter().enumerate() {
        let ox = gx + adv * (i as f32 + 1.0);
        for r in digit_rects(*d) {
            let (dw, dh) = (adv * 0.75, h);
            out.push((
                rect(
                    ox + r[0] * dw,
                    gy + r[1] * dh,
                    ox + r[2] * dw,
                    gy + r[3] * dh,
                ),
                WHITE,
            ));
        }
    }
    out
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
        if icon.count > 1 {
            for (pts, color) in badge(icon.count) {
                push(&mut m, &pts, color, 1.0);
            }
        }
    }
    m
}

#[cfg(test)]
mod tests;
