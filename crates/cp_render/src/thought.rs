//! Thought bubbles over the homes (T-318d, `architecture/system-design.md` §19.5): a cloud with
//! three trailing dots and the glyph of the home's complaint inside, for the complaints that have
//! no problem icon of their own (the icon already hangs over a home without water, power, road,
//! with garbage, crime, the sick or the dead). Not every home at once: once a game hour up to
//! [`THOUGHTS_MAX`] of the unhappy homes in view, picked by a hash of the home and the hour
//! (stable, no RNG, nothing in the sim depends on the camera); each lives [`LIFE_S`] seconds,
//! staggered over [`WAVE_S`]. Pure: the records and the camera in, the bubbles and the mesh out.

use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingId, Complaint, HomeContentment, HomeSlot};

use crate::chunk::MeshData;
use crate::problem::{Facing, icon_size};

/// At most this many bubbles start in a game hour.
pub const THOUGHTS_MAX: usize = 24;
/// Seconds a bubble lives once it shows.
pub const LIFE_S: f32 = 6.0;
/// Seconds over which the hour's bubbles appear, one after another.
pub const WAVE_S: f32 = 1.5;
/// Height of its centre above the roof, metres (the emoji's, `mood::LIFT_M`).
pub const LIFT_M: f32 = 6.0;
/// Its size against a problem icon's at the same distance.
const SIZE_K: f32 = 1.4;
/// Seconds it takes to pop up, and to fade at the end of its life.
const POP_S: f32 = 0.25;
const FADE_S: f32 = 1.0;
/// How far it bobs up and down, metres, and how fast (radians a second).
const BOB_M: f32 = 0.25;
const BOB_W: f32 = 2.0;

/// The glyph inside a bubble: square silhouettes in the style of the problem icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    /// `NoHealth`: a red cross.
    Cross,
    /// `NoPolice`: a shield.
    Shield,
    /// `NoFire`: an extinguisher.
    Extinguisher,
    /// `NoSchool`: an open book.
    Book,
    /// `Pollution`: a puff of smoke.
    Smoke,
    /// `Noise`: sound waves.
    Waves,
    /// `Jobless`: a crossed-out briefcase.
    Briefcase,
    /// `LongCommute`: a clock.
    Clock,
    /// `NoPark`: a tree.
    Tree,
    /// `HighTax`: a coin with a per cent sign.
    Coin,
    /// `Distrust`: a lectern.
    Lectern,
}

/// The glyph of a complaint, or `None` for the ones a problem icon already shows (and `None`).
pub fn glyph_of(c: Complaint) -> Option<Glyph> {
    Some(match c {
        Complaint::NoHealth => Glyph::Cross,
        Complaint::NoPolice => Glyph::Shield,
        Complaint::NoFire => Glyph::Extinguisher,
        Complaint::NoSchool => Glyph::Book,
        Complaint::Pollution => Glyph::Smoke,
        Complaint::Noise => Glyph::Waves,
        Complaint::Jobless => Glyph::Briefcase,
        Complaint::LongCommute => Glyph::Clock,
        Complaint::NoPark => Glyph::Tree,
        Complaint::HighTax => Glyph::Coin,
        Complaint::Distrust => Glyph::Lectern,
        Complaint::None
        | Complaint::NoRoad
        | Complaint::NoWater
        | Complaint::NoPower
        | Complaint::Garbage
        | Complaint::Crime
        | Complaint::BadWater
        | Complaint::Dead
        | Complaint::Sick => return None,
    })
}

/// What a home thinks about: its worst complaint that has a glyph.
pub fn thought_of(slot: &HomeSlot) -> Option<Complaint> {
    slot.complaints
        .iter()
        .copied()
        .find(|&c| glyph_of(c).is_some())
}

/// FNV-1a of a home's index and the game hour: the order the hour's candidates are taken in.
pub fn hour_hash(index: u32, hour: u64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in index.to_le_bytes().into_iter().chain(hour.to_le_bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The bubbles of game hour `hour`: the homes below `complain_below` with a thought, of those
/// `in_view`, the first [`THOUGHTS_MAX`] by [`hour_hash`] (the id on a tie).
pub fn pick_thoughts(
    homes: &HomeContentment,
    complain_below: u8,
    hour: u64,
    in_view: impl Fn(BuildingId) -> bool,
) -> Vec<(BuildingId, Complaint)> {
    let mut all: Vec<(u64, BuildingId, Complaint)> = homes
        .homes()
        .filter(|(_, s)| s.index < complain_below)
        .filter_map(|(id, s)| Some((id, thought_of(s)?)))
        .filter(|&(id, _)| in_view(id))
        .map(|(id, c)| (hour_hash(id.index, hour), id, c))
        .collect();
    all.sort_by_key(|&(h, id, _)| (h, id.index));
    all.truncate(THOUGHTS_MAX);
    all.into_iter().map(|(_, id, c)| (id, c)).collect()
}

/// A shown bubble: over which roof, its glyph, its age in seconds (negative: not yet shown).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bubble {
    pub at: Vec3,
    pub glyph: Glyph,
    pub age: f32,
    /// The selected citizen's bubble lives as long as the selection (no fade).
    pub held: bool,
}

/// Scale and opacity at `age` (0 before it shows and after it lives).
pub fn pop_and_fade(age: f32, held: bool) -> (f32, f32) {
    if age < 0.0 || (!held && age >= LIFE_S) {
        return (0.0, 0.0);
    }
    let scale = (age / POP_S).min(1.0);
    let alpha = if held {
        1.0
    } else {
        ((LIFE_S - age) / FADE_S).clamp(0.0, 1.0)
    };
    (scale, alpha)
}

/// Centre of a bubble at its age, world: over the roof, bobbing.
pub fn centre(b: &Bubble) -> Vec3 {
    b.at + Vec3::Y * (LIFT_M + BOB_M * (b.age * BOB_W).sin())
}

/// Its half width at `dist` metres from the camera, world metres (for a click).
pub fn radius(dist: f32) -> f32 {
    icon_size(dist) * SIZE_K * 0.5
}

const RIM: [f32; 3] = [0.10, 0.10, 0.12];
const CLOUD: [f32; 3] = [0.97, 0.97, 0.95];
const INK: [f32; 3] = [0.15, 0.14, 0.13];
const RED: [f32; 3] = [0.86, 0.22, 0.18];
const BLUE: [f32; 3] = [0.20, 0.38, 0.80];
const BROWN: [f32; 3] = [0.55, 0.36, 0.20];
const GREEN: [f32; 3] = [0.25, 0.62, 0.25];
const GREY: [f32; 3] = [0.50, 0.50, 0.52];
const GOLD: [f32; 3] = [0.92, 0.72, 0.18];

type Shape = (Vec<[f32; 2]>, [f32; 3]);

fn disc(c: [f32; 2], r: f32, n: usize) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect()
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// A bar `w` thick from `a` to `b`.
fn bar(a: [f32; 2], b: [f32; 2], w: f32) -> Vec<[f32; 2]> {
    let d = Vec2::new(b[0] - a[0], b[1] - a[1]);
    let n = Vec2::new(-d.y, d.x).normalize_or_zero() * (w / 2.0);
    vec![
        [a[0] + n.x, a[1] + n.y],
        [a[0] - n.x, a[1] - n.y],
        [b[0] - n.x, b[1] - n.y],
        [b[0] + n.x, b[1] + n.y],
    ]
}

/// An arc of quads centred at `c`, radius `r`, from `a0` to `a1` radians, `w` thick.
fn arc(c: [f32; 2], r: f32, a0: f32, a1: f32, w: f32) -> Vec<Vec<[f32; 2]>> {
    const N: usize = 6;
    let at = |a: f32, rr: f32| [c[0] + rr * a.cos(), c[1] + rr * a.sin()];
    (0..N)
        .map(|i| {
            let s = a0 + (a1 - a0) * i as f32 / N as f32;
            let e = a0 + (a1 - a0) * (i + 1) as f32 / N as f32;
            vec![
                at(s, r - w / 2.0),
                at(e, r - w / 2.0),
                at(e, r + w / 2.0),
                at(s, r + w / 2.0),
            ]
        })
        .collect()
}

/// The cloud: a rim and a white body of three lobes on a flat bottom, and two trailing dots
/// towards the roof (bottom left). In the bubble's unit square, centre at the origin.
fn cloud() -> Vec<Shape> {
    let lobes = [
        ([-0.2, 0.02], 0.26),
        ([0.02, 0.1], 0.31),
        ([0.24, 0.0], 0.25),
    ];
    let dots = [([-0.34, -0.38], 0.07), ([-0.45, -0.5], 0.045)];
    let mut out = Vec::new();
    for (pad, colour) in [(0.035, RIM), (0.0, CLOUD)] {
        for (c, r) in lobes {
            out.push((disc(c, r + pad, 18), colour));
        }
        out.push((rect(-0.3 - pad, -0.24 - pad, 0.34 + pad, 0.04), colour));
        for (c, r) in dots {
            out.push((disc(c, r + pad, 10), colour));
        }
    }
    out
}

/// The glyph's shapes, about 0.36 wide, centred at the cloud's middle.
fn glyph_shapes(g: Glyph) -> Vec<Shape> {
    use std::f32::consts::PI;
    let o = [0.02, 0.0];
    let at = |x: f32, y: f32| [o[0] + x, o[1] + y];
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| rect(o[0] + x0, o[1] + y0, o[0] + x1, o[1] + y1);
    match g {
        Glyph::Cross => vec![
            (r(-0.05, -0.16, 0.05, 0.16), RED),
            (r(-0.16, -0.05, 0.16, 0.05), RED),
        ],
        Glyph::Shield => vec![(
            vec![
                at(-0.14, 0.14),
                at(0.14, 0.14),
                at(0.13, -0.03),
                at(0.0, -0.17),
                at(-0.13, -0.03),
            ],
            BLUE,
        )],
        Glyph::Extinguisher => vec![
            (r(-0.06, -0.17, 0.08, 0.08), RED),
            (r(-0.03, 0.08, 0.04, 0.13), INK),
            (bar(at(0.0, 0.12), at(-0.12, 0.08), 0.035), INK),
        ],
        Glyph::Book => vec![
            (
                vec![
                    at(-0.17, -0.12),
                    at(-0.01, -0.15),
                    at(-0.01, 0.12),
                    at(-0.17, 0.14),
                ],
                BLUE,
            ),
            (
                vec![
                    at(0.01, -0.15),
                    at(0.17, -0.12),
                    at(0.17, 0.14),
                    at(0.01, 0.12),
                ],
                BLUE,
            ),
        ],
        Glyph::Smoke => vec![
            (disc(at(-0.08, -0.06), 0.09, 12), GREY),
            (disc(at(0.06, -0.04), 0.1, 12), GREY),
            (disc(at(-0.01, 0.08), 0.09, 12), GREY),
        ],
        Glyph::Waves => {
            let mut v: Vec<Shape> = vec![(r(-0.17, -0.05, -0.1, 0.05), INK)];
            for k in 0..3 {
                for q in arc(
                    at(-0.12, 0.0),
                    0.08 + 0.06 * k as f32,
                    -PI / 4.0,
                    PI / 4.0,
                    0.03,
                ) {
                    v.push((q, INK));
                }
            }
            v
        }
        Glyph::Briefcase => vec![
            (r(-0.16, -0.12, 0.16, 0.08), BROWN),
            (r(-0.06, 0.08, 0.06, 0.13), BROWN),
            (bar(at(-0.17, -0.15), at(0.17, 0.15), 0.045), RED),
        ],
        Glyph::Clock => {
            let mut v: Vec<Shape> = vec![
                (disc(at(0.0, 0.0), 0.16, 20), INK),
                (disc(at(0.0, 0.0), 0.125, 20), CLOUD),
                (bar(at(0.0, 0.0), at(0.0, 0.1), 0.03), INK),
                (bar(at(0.0, 0.0), at(0.08, 0.0), 0.03), INK),
            ];
            // A little car under it: the way to work.
            v.push((r(-0.1, -0.21, 0.1, -0.15), RED));
            v
        }
        Glyph::Tree => vec![
            (r(-0.03, -0.17, 0.03, -0.02), BROWN),
            (disc(at(0.0, 0.05), 0.13, 16), GREEN),
        ],
        Glyph::Coin => vec![
            (disc(at(0.0, 0.0), 0.16, 20), GOLD),
            (bar(at(-0.07, -0.08), at(0.07, 0.08), 0.03), INK),
            (disc(at(-0.06, 0.06), 0.03, 8), INK),
            (disc(at(0.06, -0.06), 0.03, 8), INK),
        ],
        Glyph::Lectern => vec![
            (
                vec![
                    at(-0.15, 0.06),
                    at(0.15, 0.06),
                    at(0.12, 0.13),
                    at(-0.12, 0.13),
                ],
                BROWN,
            ),
            (r(-0.04, -0.15, 0.04, 0.06), BROWN),
            (r(-0.1, -0.17, 0.1, -0.13), BROWN),
        ],
    }
}

/// One camera-facing mesh of every shown bubble, sized like the problem icons, bobbing over its
/// roof, popping up and fading with its age.
pub fn thought_mesh(bubbles: &[Bubble], facing: &Facing) -> MeshData {
    let mut m = MeshData::default();
    for b in bubbles {
        let (scale, alpha) = pop_and_fade(b.age, b.held);
        if alpha <= 0.0 || scale <= 0.0 {
            continue;
        }
        let pos = centre(b);
        let to_cam = facing.position - pos;
        let dist = to_cam.length();
        let size = icon_size(dist) * SIZE_K * scale;
        let toward = to_cam / dist.max(1e-3);
        let shapes = cloud().into_iter().chain(glyph_shapes(b.glyph));
        for (layer, (pts, c)) in shapes.enumerate() {
            let base = m.positions.len() as u32;
            // Later shapes a hair nearer the camera, so they draw over the cloud.
            let lift = toward * (layer as f32 * 0.002 * size);
            for p in &pts {
                let v = pos + (facing.right * p[0] + facing.up * p[1]) * size + lift;
                m.positions.push(v.to_array());
                m.normals.push(toward.to_array());
                m.colors.push([c[0], c[1], c[2], alpha]);
            }
            for i in 1..pts.len() as u32 - 1 {
                m.indices.extend([base, base + i, base + i + 1]);
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_building::COMPLAINTS_MAX;

    fn homes(n: u32, index: u8, first: Complaint) -> HomeContentment {
        let mut h = HomeContentment::default();
        for i in 0..n {
            let mut c = [Complaint::None; COMPLAINTS_MAX];
            c[0] = first;
            c[1] = Complaint::NoPark;
            h.set(BuildingId::new(i, 0), index, c);
        }
        h
    }

    #[test]
    fn a_problem_with_its_own_icon_gets_no_bubble_and_the_rest_have_a_glyph() {
        let iconed = [
            Complaint::NoRoad,
            Complaint::NoWater,
            Complaint::NoPower,
            Complaint::Garbage,
            Complaint::Crime,
            Complaint::BadWater,
            Complaint::Dead,
            Complaint::Sick,
        ];
        for c in Complaint::ALL {
            assert_eq!(glyph_of(c).is_none(), iconed.contains(&c), "{c:?}");
        }
        assert_eq!(glyph_of(Complaint::None), None);
        // A home that needs water thinks of its park: the water has its icon.
        let h = homes(1, 10, Complaint::NoWater);
        let (_, slot) = h.homes().next().unwrap();
        assert_eq!(thought_of(slot), Some(Complaint::NoPark));
    }

    /// §19.8 T-318d: the hour's set is a pure function of the records and the hour: the same
    /// twice, another the next hour, at most THOUGHTS_MAX, only unhappy homes in view.
    #[test]
    fn the_hours_set_is_deterministic_capped_and_only_unhappy_homes_in_view() {
        let h = homes(100, 30, Complaint::Noise);
        let all = |_| true;
        let a = pick_thoughts(&h, 50, 7, all);
        assert_eq!(a.len(), THOUGHTS_MAX);
        assert_eq!(a, pick_thoughts(&h, 50, 7, all));
        assert_ne!(
            a,
            pick_thoughts(&h, 50, 8, all),
            "another hour, other homes"
        );
        assert!(a.iter().all(|&(_, c)| c == Complaint::Noise));
        // In view: only the even ids, and they are taken in the same order.
        let even = pick_thoughts(&h, 50, 7, |id: BuildingId| id.index.is_multiple_of(2));
        assert!(even.iter().all(|(id, _)| id.index.is_multiple_of(2)));
        let first_even: Vec<_> = a
            .iter()
            .filter(|(id, _)| id.index.is_multiple_of(2))
            .collect();
        assert!(first_even.iter().all(|x| even.contains(x)));
        // Content homes think nothing.
        assert!(pick_thoughts(&homes(100, 60, Complaint::Noise), 50, 7, all).is_empty());
    }

    #[test]
    fn a_bubble_pops_lives_six_seconds_and_a_held_one_stays() {
        assert_eq!(pop_and_fade(-0.1, false), (0.0, 0.0));
        assert_eq!(pop_and_fade(1.0, false), (1.0, 1.0));
        assert_eq!(pop_and_fade(LIFE_S, false).1, 0.0);
        assert_eq!(pop_and_fade(LIFE_S * 10.0, true), (1.0, 1.0));
        let facing = Facing {
            position: Vec3::new(0.0, 100.0, 100.0),
            right: Vec3::X,
            up: Vec3::Y,
        };
        let b = |age| Bubble {
            at: Vec3::ZERO,
            glyph: Glyph::Clock,
            age,
            held: false,
        };
        assert!(
            thought_mesh(&[b(-1.0), b(LIFE_S + 1.0)], &facing)
                .indices
                .is_empty()
        );
        let m = thought_mesh(&[b(1.0)], &facing);
        assert!(!m.indices.is_empty());
        assert_eq!(m.positions.len(), m.colors.len());
    }
}
