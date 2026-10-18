//! The silhouettes of the zoo and the amusement park areas (T-228): their gates and four
//! attractions each. The ferris wheel and the carousel turn: their wheel and their roundabout
//! are [`Rotor`]s (own meshes turned by the frame clock, a view effect only), the rest stands in
//! the building mesh.

use bevy_math::{Quat, Vec2, Vec3};
use cp_content::Massing;

use super::park_area::{BLOOMS, IRON, PAVING, ROCK, STONE, WATER, WATER_TOP, gable, lamp};
use super::{
    GRASS, GRAVEL, LOG, PLANK, Rotor, RotorKind, STEEL, ServiceCx, block, cone, cyl, tree,
};
use crate::building::{Frame, Lod};
use crate::chunk::MeshData;
use crate::road::{flat_quad, push_vertex, tri};

const SAND: [f32; 4] = [0.80, 0.70, 0.48, 1.0];
const DIRT: [f32; 4] = [0.62, 0.50, 0.32, 1.0];
const HAY: [f32; 4] = [0.88, 0.76, 0.36, 1.0];
const WOOL: [f32; 4] = [0.93, 0.92, 0.88, 1.0];
const HIDE: [f32; 4] = [0.55, 0.55, 0.58, 1.0];
const MANE: [f32; 4] = [0.48, 0.30, 0.14, 1.0];
const RINK: [f32; 4] = [0.25, 0.26, 0.30, 1.0];

/// Ferris wheel: radius of the rim and height of the hub above the lot ground, metres.
const WHEEL_R: f32 = 11.0;
const WHEEL_HUB: f32 = 13.2;
/// Carousel: radius of the turning platform, metres.
const CAROUSEL_R: f32 = 5.6;

/// Whether `m` is one of the zoo or amusement park silhouettes of this module.
pub fn is_theme_park(m: Massing) -> bool {
    (m as u8) >= Massing::ZooGate as u8 && (m as u8) <= Massing::FerrisWheel as u8
}

/// A square rod of width `w` from `a` to `b` (world or rotor space): its four sides.
pub(super) fn rod(m: &mut MeshData, a: Vec3, b: Vec3, w: f32, col: [f32; 4]) {
    let d = (b - a).normalize_or_zero();
    let side = if d.y.abs() > 0.9 { Vec3::X } else { Vec3::Y };
    let p = d.cross(side).normalize_or_zero() * (w * 0.5);
    let q = d.cross(p).normalize_or_zero() * (w * 0.5);
    let ring = [p + q, p - q, -p - q, -p + q];
    for k in 0..4 {
        let (r0, r1) = (ring[k], ring[(k + 1) % 4]);
        flat_quad(
            m,
            [a + r0, a + r1, b + r1, b + r0],
            (r0 + r1).normalize_or_zero(),
            col,
        );
    }
}

/// A stylised animal standing at `at` facing `+u` (`dir` = -1 turns it): a body, a head on a
/// neck of `neck` metres, four legs; `s` scales it.
#[allow(clippy::too_many_arguments)]
fn animal(
    m: &mut MeshData,
    f: &Frame,
    at: (f32, f32),
    y: f32,
    s: f32,
    dir: f32,
    neck: f32,
    (body, head): ([f32; 4], [f32; 4]),
) {
    let (u, v) = at;
    let leg = 0.5 * s;
    block(
        m,
        f,
        (u, v),
        (0.6 * s, 0.25 * s),
        y + leg,
        y + leg + 0.5 * s,
        body,
        body,
    );
    for (du, dv) in [(-0.45, -0.17), (-0.45, 0.17), (0.45, -0.17), (0.45, 0.17)] {
        block(
            m,
            f,
            (u + du * s, v + dv * s),
            (0.07 * s, 0.07 * s),
            y,
            y + leg,
            body,
            body,
        );
    }
    let hu = u + dir * 0.62 * s;
    let top = y + leg + 0.5 * s + neck;
    if neck > 0.0 {
        block(
            m,
            f,
            (hu, v),
            (0.1 * s, 0.1 * s),
            y + leg + 0.3 * s,
            top,
            body,
            body,
        );
    }
    block(
        m,
        f,
        (hu + dir * 0.12 * s, v),
        (0.22 * s, 0.15 * s),
        top - 0.1 * s,
        top + 0.22 * s,
        head,
        head,
    );
}

/// An elephant at `at` facing `+u` (`dir` = -1 turns it), `s` scales it.
fn elephant(m: &mut MeshData, f: &Frame, at: (f32, f32), y: f32, s: f32, dir: f32, near: bool) {
    let (u, v) = at;
    let leg = 1.1 * s;
    block(
        m,
        f,
        (u, v),
        (1.3 * s, 0.75 * s),
        y + leg,
        y + leg + 1.5 * s,
        HIDE,
        HIDE,
    );
    for (du, dv) in [(-0.9, -0.45), (-0.9, 0.45), (0.9, -0.45), (0.9, 0.45)] {
        block(
            m,
            f,
            (u + du * s, v + dv * s),
            (0.28 * s, 0.28 * s),
            y,
            y + leg,
            HIDE,
            HIDE,
        );
    }
    let hu = u + dir * 1.55 * s;
    block(
        m,
        f,
        (hu, v),
        (0.5 * s, 0.55 * s),
        y + leg + 0.6 * s,
        y + leg + 1.9 * s,
        HIDE,
        HIDE,
    );
    // The trunk.
    block(
        m,
        f,
        (hu + dir * 0.55 * s, v),
        (0.14 * s, 0.14 * s),
        y + 0.4 * s,
        y + leg + 1.0 * s,
        HIDE,
        HIDE,
    );
    if near {
        // Ears and tusks.
        for sv in [-1.0f32, 1.0] {
            block(
                m,
                f,
                (hu - dir * 0.3 * s, v + sv * 0.62 * s),
                (0.35 * s, 0.06 * s),
                y + leg + 0.7 * s,
                y + leg + 1.8 * s,
                mul(HIDE, 0.9),
                HIDE,
            );
            block(
                m,
                f,
                (hu + dir * 0.55 * s, v + sv * 0.25 * s),
                (0.06 * s, 0.06 * s),
                y + leg + 0.3 * s,
                y + leg + 0.7 * s,
                WOOL,
                WOOL,
            );
        }
    }
}

pub(super) fn mul(c: [f32; 4], k: f32) -> [f32; 4] {
    [c[0] * k, c[1] * k, c[2] * k, c[3]]
}

/// A post-and-rail fence round the rectangle `(cu, cv) ± (hu, hv)`, leaving a gap of `gap`
/// metres in the middle of the front side.
#[allow(clippy::too_many_arguments)]
fn fence(
    m: &mut MeshData,
    f: &Frame,
    (cu, cv): (f32, f32),
    (hu, hv): (f32, f32),
    y: f32,
    h: f32,
    gap: f32,
    col: [f32; 4],
) {
    let rail = |m: &mut MeshData, (u, v): (f32, f32), (eu, ev): (f32, f32)| {
        for k in [0.45f32, 0.9] {
            block(
                m,
                f,
                (u, v),
                (eu, ev),
                y + h * k - 0.06,
                y + h * k + 0.06,
                col,
                col,
            );
        }
    };
    rail(m, (cu, cv + hv), (hu, 0.05));
    rail(m, (cu - hu, cv), (0.05, hv));
    rail(m, (cu + hu, cv), (0.05, hv));
    let side = (hu - gap * 0.5) * 0.5;
    for s in [-1.0f32, 1.0] {
        rail(m, (cu + s * (hu - side), cv - hv), (side, 0.05));
    }
    let posts_u = (hu / 2.0).ceil() as i32;
    let posts_v = (hv / 2.0).ceil() as i32;
    for k in -posts_u..=posts_u {
        let u = cu + hu * k as f32 / posts_u as f32;
        for (pv, front) in [(cv - hv, true), (cv + hv, false)] {
            if front && (u - cu).abs() < gap * 0.5 {
                continue;
            }
            block(m, f, (u, pv), (0.08, 0.08), y, y + h, col, col);
        }
    }
    for k in -posts_v + 1..posts_v {
        let v = cv + hv * k as f32 / posts_v as f32;
        for pu in [cu - hu, cu + hu] {
            block(m, f, (pu, v), (0.08, 0.08), y, y + h, col, col);
        }
    }
}

/// A ticket booth: a small box with a gabled roof and a counter window.
fn booth(m: &mut MeshData, f: &Frame, at: (f32, f32), y: f32, wall: [f32; 4], roof: [f32; 4]) {
    block(m, f, at, (1.0, 0.8), y, y + 2.3, wall, wall);
    gable(m, f, at, (1.15, 0.95), (y + 2.3, y + 3.1), roof);
    block(
        m,
        f,
        (at.0, at.1 - 0.81),
        (0.6, 0.02),
        y + 1.0,
        y + 1.8,
        [0.18, 0.22, 0.28, 1.0],
        [0.18, 0.22, 0.28, 1.0],
    );
}

/// A striped tower of `bands` alternating rings up to `y + h`, a cone on top and a flag.
#[allow(clippy::too_many_arguments)]
fn striped_tower(
    m: &mut MeshData,
    f: &Frame,
    at: (f32, f32),
    y: f32,
    (r, h): (f32, f32),
    bands: usize,
    n: usize,
    (a, b, top): ([f32; 4], [f32; 4], [f32; 4]),
    flag: bool,
) {
    for k in 0..bands {
        let (y0, y1) = (
            y + h * k as f32 / bands as f32,
            y + h * (k + 1) as f32 / bands as f32,
        );
        cyl(
            m,
            f,
            at,
            (r, r),
            (y0, y1),
            n,
            if k % 2 == 0 { a } else { b },
            None,
        );
    }
    cone(m, f, at, r * 1.25, (y + h, y + h + 2.0 * r), n, top);
    if flag {
        let t = y + h + 2.0 * r;
        block(m, f, at, (0.04, 0.04), t, t + 1.4, IRON, IRON);
        block(
            m,
            f,
            (at.0 + 0.45, at.1),
            (0.42, 0.03),
            t + 0.9,
            t + 1.35,
            a,
            a,
        );
    }
}

pub fn theme_park(m: &mut MeshData, massing: Massing, sc: &ServiceCx) {
    let (f, b) = (sc.f, sc.base);
    let (hu, hv, cv) = (sc.hu, sc.hv, sc.cv);
    let near = sc.lod == Lod::Near;
    let far = sc.lod == Lod::Far;
    let n = if near {
        10
    } else if far {
        5
    } else {
        7
    };
    let r = hu.min(hv);
    let fnd = [0.30, 0.30, 0.30, 1.0];
    let ground = |m: &mut MeshData, top: f32, col: [f32; 4]| {
        block(
            m,
            f,
            (0.0, cv),
            (0.96 * hu, 0.96 * hv),
            sc.lo - 0.3,
            b + top,
            fnd,
            col,
        );
    };
    // Lot-relative point: `(u, v)` in -1..1 of the half extents.
    let at = |u: f32, v: f32| (u * hu, cv + v * hv);
    let y = b + 0.12;
    match massing {
        Massing::ZooGate => {
            // Two rock pillars under a log beam, a giraffe standing on it as the sign.
            ground(m, 0.12, GRASS);
            block(
                m,
                f,
                at(0.0, -0.1),
                (0.22 * hu, 0.85 * hv),
                y,
                y + 0.06,
                GRAVEL,
                GRAVEL,
            );
            let front = -0.6;
            for s in [-1.0f32, 1.0] {
                cyl(
                    m,
                    f,
                    at(s * 0.36, front),
                    (0.85, 0.6),
                    (y, y + 4.2),
                    if far { 5 } else { 7 },
                    ROCK,
                    Some(ROCK),
                );
            }
            block(
                m,
                f,
                at(0.0, front),
                (0.44 * hu, 0.3),
                y + 3.5,
                y + 4.1,
                LOG,
                LOG,
            );
            // The giraffe on the beam, facing the street side's left.
            let g = at(-0.05, front);
            animal(
                m,
                f,
                g,
                y + 4.1,
                2.0,
                1.0,
                if far { 1.6 } else { 2.0 },
                (sc.trim, sc.trim),
            );
            if !far {
                // A sign board under the beam and the fence either side.
                block(
                    m,
                    f,
                    (g.0, g.1 - 0.05),
                    (0.2 * hu, 0.06),
                    y + 2.7,
                    y + 3.4,
                    sc.wall,
                    sc.wall,
                );
                for s in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        at(s * 0.7, front),
                        (0.26 * hu, 0.06),
                        y + 0.9,
                        y + 1.05,
                        LOG,
                        LOG,
                    );
                    block(
                        m,
                        f,
                        at(s * 0.7, front),
                        (0.26 * hu, 0.06),
                        y + 1.6,
                        y + 1.75,
                        LOG,
                        LOG,
                    );
                    for pu in [0.44, 0.7, 0.96] {
                        block(m, f, at(s * pu, front), (0.1, 0.1), y, y + 2.0, LOG, LOG);
                    }
                }
                booth(m, f, at(0.62, 0.05), y, sc.wall, sc.roof);
            }
            if near {
                // The giraffe's spots: dark patches on its flanks.
                let spot = [0.45, 0.28, 0.12, 1.0];
                for (du, dy) in [(-0.6, 1.5), (0.15, 1.65), (0.7, 1.35), (-0.15, 1.2)] {
                    block(
                        m,
                        f,
                        (g.0 + du, g.1),
                        (0.16, 0.52),
                        y + 4.1 + dy,
                        y + 4.1 + dy + 0.22,
                        spot,
                        spot,
                    );
                }
            }
            for (u, v) in [(-0.7, 0.55), (0.0, 0.75), (0.7, 0.6)] {
                tree(m, f, at(u, v), y, 7.0, n, near);
            }
        }
        Massing::PettingZoo => {
            // A dirt paddock with a fence, a small red barn, hay bales, sheep and goats.
            ground(m, 0.12, GRASS);
            let (pc, ph) = (at(0.0, -0.15), (0.82 * hu, 0.68 * hv));
            block(m, f, pc, ph, y, y + 0.04, DIRT, DIRT);
            let barn = at(0.4, 0.45);
            block(
                m,
                f,
                barn,
                (0.32 * hu, 0.24 * hv),
                y,
                y + 2.6,
                sc.wall,
                sc.wall,
            );
            gable(
                m,
                f,
                barn,
                (0.36 * hu, 0.28 * hv),
                (y + 2.6, y + 4.0),
                sc.roof,
            );
            block(
                m,
                f,
                (barn.0, barn.1 - 0.24 * hv - 0.02),
                (0.12 * hu, 0.02),
                y,
                y + 1.8,
                sc.trim,
                sc.trim,
            );
            if !far {
                fence(m, f, pc, ph, y, 1.1, 1.6, PLANK);
                for (u, v) in [(-0.6, 0.45), (-0.35, 0.5)] {
                    block(m, f, at(u, v), (0.6, 0.45), y, y + 0.9, HAY, HAY);
                }
                let critters: [((f32, f32), f32, [f32; 4]); 5] = [
                    ((-0.45, -0.45), 1.0, WOOL),
                    ((0.05, -0.55), -1.0, WOOL),
                    ((-0.15, -0.05), 1.0, [0.55, 0.42, 0.30, 1.0]),
                    ((0.45, -0.2), -1.0, WOOL),
                    ((-0.6, 0.05), -1.0, [0.85, 0.80, 0.72, 1.0]),
                ];
                for (k, (p, d, col)) in critters.into_iter().enumerate() {
                    if !near && k >= 3 {
                        break;
                    }
                    animal(m, f, at(p.0, p.1), y, 1.0, d, 0.0, (col, IRON));
                }
            }
            if near {
                tree(m, f, at(-0.75, 0.75), y, 6.0, n, near);
                block(
                    m,
                    f,
                    at(0.6, -0.1),
                    (0.9, 0.3),
                    y,
                    y + 0.5,
                    PLANK,
                    WATER_TOP,
                );
            }
        }
        Massing::Aviary => {
            // A net dome of ribs over trees, a pond and perches, birds inside.
            ground(m, 0.12, GRASS);
            let c = at(0.0, 0.05);
            let (rad, h) = (0.82 * r, 10.0);
            cyl(m, f, c, (rad, rad), (y, y + 0.5), n * 2, STONE, Some(GRASS));
            let yb = y + 0.5;
            let ribs = if far { 4 } else { 8 };
            let segs = if far { 4 } else { 8 };
            let pt = |phi: f32, th: f32| {
                f.at(
                    c.0 + rad * th.sin() * phi.cos(),
                    c.1 + rad * th.sin() * phi.sin(),
                    yb + h * th.cos(),
                )
            };
            let half = std::f32::consts::FRAC_PI_2;
            for k in 0..ribs {
                let phi = k as f32 / ribs as f32 * std::f32::consts::TAU;
                for s in 0..segs {
                    let (t0, t1) = (
                        half * s as f32 / segs as f32,
                        half * (s + 1) as f32 / segs as f32,
                    );
                    rod(m, pt(phi, t0), pt(phi, t1), 0.18, sc.wall);
                }
            }
            if !far {
                // Two rings of the net.
                let ring_n = ribs * 2;
                for th in [0.55f32, 1.05] {
                    for k in 0..ring_n {
                        let (a0, a1) = (
                            k as f32 / ring_n as f32 * std::f32::consts::TAU,
                            (k + 1) as f32 / ring_n as f32 * std::f32::consts::TAU,
                        );
                        rod(m, pt(a0, th), pt(a1, th), 0.12, sc.wall);
                    }
                }
                cyl(
                    m,
                    f,
                    at(0.3, -0.25),
                    (0.22 * r, 0.22 * r),
                    (yb - 0.1, yb + 0.03),
                    n,
                    WATER,
                    Some(WATER_TOP),
                );
            }
            tree(m, f, at(-0.3, 0.2), yb, 6.5, n, near);
            tree(m, f, at(0.25, 0.4), yb, 5.0, n, near);
            if near {
                // Perches and bright birds.
                for (k, (u, v, hh)) in [
                    (-0.45, -0.3, 2.2),
                    (0.0, -0.45, 3.0),
                    (-0.1, 0.0, 6.2),
                    (0.4, 0.15, 4.0),
                    (0.15, -0.1, 7.5),
                    (-0.35, 0.45, 3.4),
                ]
                .into_iter()
                .enumerate()
                {
                    let p = at(u, v);
                    if hh < 4.0 {
                        block(m, f, p, (0.05, 0.05), yb, yb + hh, LOG, LOG);
                    }
                    let col = if k % 2 == 0 {
                        sc.trim
                    } else {
                        BLOOMS[k % BLOOMS.len()]
                    };
                    block(m, f, p, (0.3, 0.15), yb + hh, yb + hh + 0.3, col, col);
                }
            }
            if !far {
                booth(m, f, at(-0.72, -0.78), y, PLANK, sc.roof);
            }
        }
        Massing::BigCats => {
            // A rock outcrop on sand behind a moat, lions on it, a viewing wall in front.
            ground(m, 0.12, SAND);
            let rock = at(0.05, 0.25);
            cone(m, f, rock, 0.42 * r, (y, y + 4.5), n, ROCK);
            cone(m, f, at(-0.4, 0.4), 0.3 * r, (y, y + 3.0), n, ROCK);
            block(m, f, rock, (0.22 * r, 0.16 * r), y, y + 3.2, ROCK, ROCK);
            // The lion on the rock and a lioness lying below it.
            animal(
                m,
                f,
                (rock.0, rock.1 - 0.05 * r),
                y + 3.2,
                1.5,
                -1.0,
                0.0,
                (sc.trim, MANE),
            );
            if !far {
                block(
                    m,
                    f,
                    (rock.0 - 0.95, rock.1 - 0.05 * r),
                    (0.34, 0.34),
                    y + 3.25,
                    y + 4.4,
                    MANE,
                    MANE,
                );
                animal(m, f, at(0.5, -0.05), y, 1.3, 1.0, 0.0, (sc.trim, sc.trim));
                // The moat: a water band across the front.
                block(
                    m,
                    f,
                    at(0.0, -0.45),
                    (0.9 * hu, 0.07 * hv),
                    y - 0.05,
                    y + 0.02,
                    WATER,
                    WATER_TOP,
                );
            }
            // The viewing wall with its glass.
            block(
                m,
                f,
                at(0.0, -0.62),
                (0.9 * hu, 0.15),
                y,
                y + 1.0,
                STONE,
                STONE,
            );
            if !far {
                block(
                    m,
                    f,
                    at(0.0, -0.62),
                    (0.88 * hu, 0.05),
                    y + 1.0,
                    y + 2.0,
                    [0.55, 0.70, 0.80, 1.0],
                    [0.55, 0.70, 0.80, 1.0],
                );
                let shelter = at(-0.55, -0.82);
                block(
                    m,
                    f,
                    shelter,
                    (0.25 * hu, 0.08 * hv),
                    y,
                    y + 0.05,
                    PAVING,
                    PAVING,
                );
                for s in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        (shelter.0 + s * 0.22 * hu, shelter.1),
                        (0.1, 0.1),
                        y,
                        y + 2.6,
                        LOG,
                        LOG,
                    );
                }
                block(
                    m,
                    f,
                    shelter,
                    (0.27 * hu, 0.1 * hv),
                    y + 2.6,
                    y + 2.85,
                    sc.roof,
                    sc.roof,
                );
            }
            for (u, v) in [(0.75, 0.7), (-0.8, 0.0)] {
                tree(m, f, at(u, v), y, 6.5, n, near);
            }
            if near {
                for (u, v) in [(0.5, 0.45), (-0.2, -0.2), (0.75, 0.1)] {
                    cone(m, f, at(u, v), 0.6, (y, y + 0.7), 5, ROCK);
                }
            }
        }
        Massing::ElephantHouse => {
            // A round house with a dome at the back, a sand paddock with a pool and elephants.
            ground(m, 0.12, GRASS);
            let paddock = (at(0.0, -0.25), (0.86 * hu, 0.6 * hv));
            block(m, f, paddock.0, paddock.1, y, y + 0.04, SAND, SAND);
            let house = at(0.0, 0.55);
            let hr = 0.34 * r;
            cyl(m, f, house, (hr, hr), (y, y + 5.0), n.max(8), sc.wall, None);
            let rings = if far { 2 } else { 4 };
            for k in 0..rings {
                let (t0, t1) = (k as f32 / rings as f32, (k + 1) as f32 / rings as f32);
                let rr = |t: f32| hr * 1.05 * (1.0 - t * t).sqrt().max(0.12);
                cyl(
                    m,
                    f,
                    house,
                    (rr(t0), rr(t1)),
                    (y + 5.0 + 4.0 * t0, y + 5.0 + 4.0 * t1),
                    n.max(8),
                    sc.roof,
                    (k + 1 == rings).then_some(sc.roof),
                );
            }
            // The big door towards the paddock.
            block(
                m,
                f,
                (house.0, house.1 - hr + 0.2),
                (1.6, 0.3),
                y,
                y + 4.2,
                [0.35, 0.25, 0.16, 1.0],
                sc.trim,
            );
            elephant(m, f, at(-0.35, -0.3), y, 1.35, 1.0, near);
            if !far {
                elephant(m, f, at(0.3, -0.05), y, 0.85, -1.0, near);
                cyl(
                    m,
                    f,
                    at(0.45, -0.55),
                    (0.2 * r, 0.2 * r),
                    (y - 0.1, y + 0.06),
                    n,
                    WATER,
                    Some(WATER_TOP),
                );
                // Stone bollards and a rail round the paddock.
                fence(m, f, paddock.0, paddock.1, y, 1.3, 3.0, STONE);
            }
            if near {
                cyl(
                    m,
                    f,
                    house,
                    (0.12 * hr, 0.1 * hr),
                    (y + 8.8, y + 9.8),
                    6,
                    sc.trim,
                    Some(sc.roof),
                );
            }
            for (u, v) in [(-0.8, 0.75), (0.8, 0.8), (-0.85, 0.3)] {
                tree(m, f, at(u, v), y, 8.0, n, near);
            }
        }
        Massing::AmusementGate => {
            // A bright arch between two striped towers, pennants strung across, ticket booths.
            ground(m, 0.12, PAVING);
            let front = -0.5;
            let th = 6.0;
            for s in [-1.0f32, 1.0] {
                striped_tower(
                    m,
                    f,
                    at(s * 0.62, front),
                    y,
                    (1.0, th),
                    if far { 2 } else { 6 },
                    if far { 6 } else { n },
                    (sc.wall, sc.roof, sc.trim),
                    !far,
                );
            }
            // The arch: a half ring of short pieces from tower to tower.
            let (span, rise) = (0.62 * hu, 2.6);
            let pieces = if far { 5 } else { 11 };
            for k in 0..pieces {
                let a = (k as f32 + 0.5) / pieces as f32 * std::f32::consts::PI;
                let (u, h) = (-span * a.cos(), rise * a.sin());
                let col = if k % 2 == 0 { sc.trim } else { sc.wall };
                block(
                    m,
                    f,
                    (u, cv + front * hv),
                    (span / pieces as f32 + 0.1, 0.25),
                    y + th - 1.4 + h,
                    y + th - 0.8 + h,
                    col,
                    col,
                );
            }
            // The name board under the crown of the arch.
            block(
                m,
                f,
                at(0.0, front - 0.02),
                (0.3 * hu, 0.08),
                y + th + 0.2,
                y + th + 1.3,
                sc.roof,
                sc.roof,
            );
            if !far {
                for s in [-1.0f32, 1.0] {
                    booth(m, f, at(s * 0.55, 0.35), y, sc.wall, sc.trim);
                }
                lamp(m, f, at(0.0, 0.7), y, 3.2);
            }
            if near {
                // Pennants on a sagging line from tower top to tower top, behind the arch.
                let k_n = 12;
                for k in 0..k_n {
                    let t = (k as f32 + 0.5) / k_n as f32;
                    let u = (t * 2.0 - 1.0) * 0.62 * hu;
                    let sag = 1.2 * (1.0 - (t * 2.0 - 1.0).powi(2));
                    let yy = y + th + 2.2 - sag;
                    let col = BLOOMS[k % BLOOMS.len()];
                    let p0 =
                        push_vertex(m, f.at(u - 0.3, cv + (front + 0.12) * hv, yy), Vec3::Y, col);
                    let p1 =
                        push_vertex(m, f.at(u + 0.3, cv + (front + 0.12) * hv, yy), Vec3::Y, col);
                    let p2 =
                        push_vertex(m, f.at(u, cv + (front + 0.12) * hv, yy - 0.6), Vec3::Y, col);
                    let nrm = f.dir(0.0, -1.0);
                    tri(m, p0, p1, p2, nrm);
                    tri(m, p0, p1, p2, -nrm);
                }
            }
        }
        Massing::Carousel => {
            // The plinth, a low fence and the booth; the roundabout itself is the rotor.
            ground(m, 0.12, PAVING);
            let c = at(0.0, 0.0);
            cyl(
                m,
                f,
                c,
                (CAROUSEL_R + 0.6, CAROUSEL_R + 0.6),
                (y, y + 0.3),
                n * 2,
                STONE,
                Some(STONE),
            );
            if !far {
                booth(m, f, at(0.78, -0.8), y, sc.wall, sc.roof);
                let posts = if near { 20 } else { 10 };
                let fr = CAROUSEL_R + 1.2;
                for k in 0..posts {
                    let a = k as f32 / posts as f32 * std::f32::consts::TAU;
                    if (a - 1.5 * std::f32::consts::PI).abs() < 0.3 {
                        continue;
                    }
                    block(
                        m,
                        f,
                        (c.0 + fr * a.cos(), c.1 + fr * a.sin()),
                        (0.06, 0.06),
                        y,
                        y + 0.9,
                        sc.trim,
                        sc.trim,
                    );
                }
            }
            if far {
                // Far away the rotor is all there is; give the lot a hint of the canopy too.
                block(m, f, c, (0.3, 0.3), y + 0.3, y + 0.6, sc.roof, sc.roof);
            }
            if near {
                lamp(m, f, at(-0.85, -0.85), y, 3.0);
            }
        }
        Massing::BumperCars => {
            // An open pavilion: a dark rink inside a padded rim, posts, a striped tent roof.
            ground(m, 0.12, PAVING);
            let (c, h) = (at(0.0, 0.0), (0.82 * hu, 0.72 * hv));
            block(m, f, c, h, y, y + 0.25, sc.trim, RINK);
            let (ry0, ry1) = (y + 5.0, y + 5.6);
            for (pu, pv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                block(
                    m,
                    f,
                    (c.0 + pu * h.0, c.1 + pv * h.1),
                    (0.18, 0.18),
                    y,
                    ry0,
                    STEEL,
                    STEEL,
                );
            }
            // The eaves band and a striped tent roof over it (stripes across the ridge).
            let eaves = (h.0 + 0.4, h.1 + 0.4);
            block(m, f, c, eaves, ry0, ry1, sc.roof, sc.roof);
            let stripes = if far { 1 } else { 8 };
            for k in 0..stripes {
                let w = eaves.0 / stripes as f32;
                let u = c.0 - eaves.0 + w * (2 * k + 1) as f32;
                let col = if k % 2 == 0 { sc.roof } else { sc.trim };
                gable(m, f, (u, c.1), (w, eaves.1), (ry1, ry1 + 2.4), col);
            }
            if !far {
                for s in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        (c.0, c.1 + s * h.1),
                        (0.18, 0.18),
                        y,
                        ry0,
                        STEEL,
                        STEEL,
                    );
                    block(
                        m,
                        f,
                        (c.0 + s * h.0, c.1),
                        (0.18, 0.18),
                        y,
                        ry0,
                        STEEL,
                        STEEL,
                    );
                }
                // Stripes on the eaves band and a sign over the front.
                let k_n = 8;
                for k in 0..k_n {
                    if k % 2 == 1 {
                        continue;
                    }
                    let u = c.0 + (h.0 + 0.4) * ((k as f32 + 0.5) / k_n as f32 * 2.0 - 1.0);
                    block(
                        m,
                        f,
                        (u, c.1 - h.1 - 0.42),
                        ((h.0 + 0.4) / k_n as f32, 0.03),
                        ry0,
                        ry1,
                        sc.trim,
                        sc.trim,
                    );
                }
                block(
                    m,
                    f,
                    (c.0, c.1 - h.1 - 0.5),
                    (0.35 * h.0, 0.08),
                    ry1,
                    ry1 + 1.3,
                    sc.wall,
                    sc.trim,
                );
                // The cars, scattered over the rink.
                let cars = [
                    (-0.6, -0.5, 0.0),
                    (-0.1, -0.3, 1.0),
                    (0.5, -0.55, 0.0),
                    (0.3, 0.2, 1.0),
                    (-0.45, 0.35, 1.0),
                    (0.65, 0.5, 0.0),
                    (-0.05, 0.6, 0.0),
                ];
                for (k, (u, v, turn)) in cars.into_iter().enumerate() {
                    if !near && k >= 4 {
                        break;
                    }
                    let p = (c.0 + u * h.0, c.1 + v * h.1);
                    let e = if turn > 0.5 { (0.55, 0.8) } else { (0.8, 0.55) };
                    let col = BLOOMS[k % BLOOMS.len()];
                    block(
                        m,
                        f,
                        p,
                        (e.0 + 0.08, e.1 + 0.08),
                        y + 0.25,
                        y + 0.5,
                        IRON,
                        IRON,
                    );
                    block(m, f, p, e, y + 0.5, y + 0.95, col, col);
                    if near {
                        // The pole to the ceiling grid.
                        block(m, f, p, (0.03, 0.03), y + 0.95, ry0, IRON, IRON);
                    }
                }
            }
        }
        Massing::RollerCoaster => {
            // A track round the lot with a lift hill at the back and a vertical loop in front,
            // on thin supports; a station shed and a train at the platform.
            ground(m, 0.12, GRASS);
            let (a, bb) = (0.78 * hu, 0.62 * hv);
            let steps = if far {
                16
            } else if near {
                48
            } else {
                32
            };
            let base_h = 1.4;
            let (loop_u, loop_r) = (0.25 * hu, 3.6);
            let height = |t: f32| {
                let s = t.sin();
                base_h
                    + if s > 0.0 { 10.5 * s.powf(1.5) } else { 0.0 }
                    + 1.6 * (3.0 * t).cos().max(0.0) * (s > -0.2) as u8 as f32
            };
            let point = |t: f32| {
                let (u, v) = (a * t.cos(), cv + bb * t.sin());
                (u, v, y + height(t))
            };
            let rail_col = sc.roof;
            let w = 0.42;
            for k in 0..steps {
                let (t0, t1) = (
                    k as f32 / steps as f32 * std::f32::consts::TAU,
                    (k + 1) as f32 / steps as f32 * std::f32::consts::TAU,
                );
                let (p0, p1) = (point(t0), point(t1));
                // Lateral offset: the path's normal in the lot plane.
                let d = Vec2::new(p1.0 - p0.0, p1.1 - p0.1).normalize_or_zero();
                let side = Vec2::new(-d.y, d.x) * w;
                for s in [-1.0f32, 1.0] {
                    rod(
                        m,
                        f.at(p0.0 + s * side.x, p0.1 + s * side.y, p0.2),
                        f.at(p1.0 + s * side.x, p1.1 + s * side.y, p1.2),
                        if far { 0.3 } else { 0.18 },
                        rail_col,
                    );
                }
                if near {
                    rod(
                        m,
                        f.at(p0.0 - side.x, p0.1 - side.y, p0.2 - 0.05),
                        f.at(p0.0 + side.x, p0.1 + side.y, p0.2 - 0.05),
                        0.1,
                        IRON,
                    );
                }
                if k % (steps / 16).max(1) == 0 && p0.2 > y + 0.5 {
                    rod(
                        m,
                        f.at(p0.0, p0.1, y),
                        f.at(p0.0, p0.1, p0.2 - 0.1),
                        if far { 0.3 } else { 0.2 },
                        sc.wall,
                    );
                }
            }
            // The vertical loop on the front straight (t = -pi/2 is the front middle).
            let lv = cv - bb;
            let loop_n = if far { 10 } else { 20 };
            let lp = |k: usize, s: f32| {
                let ang = k as f32 / loop_n as f32 * std::f32::consts::TAU;
                f.at(
                    loop_u + loop_r * ang.sin(),
                    lv + s * w + 0.9 * (k as f32 / loop_n as f32 - 0.5),
                    y + base_h + loop_r - loop_r * ang.cos(),
                )
            };
            for k in 0..loop_n {
                for s in [-1.0f32, 1.0] {
                    rod(
                        m,
                        lp(k, s),
                        lp(k + 1, s),
                        if far { 0.3 } else { 0.2 },
                        rail_col,
                    );
                }
            }
            rod(
                m,
                f.at(loop_u, lv, y),
                f.at(loop_u, lv, y + base_h - 0.1),
                0.25,
                sc.wall,
            );
            // The station: a shed over the platform on the front left, and the train.
            let st = (-0.5 * hu, lv);
            block(
                m,
                f,
                (st.0, st.1 + 1.6),
                (3.2, 0.9),
                y,
                y + base_h - 0.1,
                PLANK,
                PLANK,
            );
            for s in [-1.0f32, 1.0] {
                block(
                    m,
                    f,
                    (st.0 + s * 3.0, st.1 + 1.6),
                    (0.12, 0.12),
                    y,
                    y + 4.2,
                    sc.wall,
                    sc.wall,
                );
            }
            block(
                m,
                f,
                (st.0, st.1 + 0.8),
                (3.5, 1.9),
                y + 4.2,
                y + 4.5,
                sc.trim,
                sc.trim,
            );
            if !far {
                for k in 0..4 {
                    let u = st.0 - 2.4 + k as f32 * 1.6;
                    let col = if k == 0 {
                        sc.trim
                    } else {
                        BLOOMS[k % BLOOMS.len()]
                    };
                    block(
                        m,
                        f,
                        (u, st.1),
                        (0.7, 0.5),
                        y + base_h,
                        y + base_h + 0.7,
                        col,
                        col,
                    );
                }
            }
        }
        Massing::FerrisWheel => {
            // Two A-frames hold the axle; the wheel itself is the rotor. A boarding deck and
            // a booth below it.
            ground(m, 0.12, PAVING);
            let hub_y = y + WHEEL_HUB;
            let legs_u = 0.42 * hu;
            for s in [-1.0f32, 1.0] {
                let v = cv + s * 1.5;
                let top = f.at(0.0, v, hub_y);
                for lu in [-legs_u, legs_u] {
                    rod(
                        m,
                        f.at(lu, v + s * 0.8, y),
                        top,
                        if far { 0.6 } else { 0.45 },
                        STEEL,
                    );
                }
                if !far {
                    rod(
                        m,
                        f.at(-legs_u * 0.5, v + s * 0.4, y + WHEEL_HUB * 0.5),
                        f.at(legs_u * 0.5, v + s * 0.4, y + WHEEL_HUB * 0.5),
                        0.3,
                        STEEL,
                    );
                }
            }
            rod(
                m,
                f.at(0.0, cv - 1.8, hub_y),
                f.at(0.0, cv + 1.8, hub_y),
                0.5,
                IRON,
            );
            block(
                m,
                f,
                (0.0, cv - 0.2 * hv),
                (2.6, 0.32 * hv),
                y,
                y + 0.6,
                sc.wall,
                sc.trim,
            );
            if !far {
                booth(m, f, at(0.7, -0.75), y, sc.wall, sc.roof);
                fence(
                    m,
                    f,
                    (0.0, cv - 0.25 * hv),
                    (0.3 * hu, 0.45 * hv),
                    y,
                    1.0,
                    2.0,
                    sc.roof,
                );
            }
            if near {
                lamp(m, f, at(-0.8, -0.8), y, 3.2);
                lamp(m, f, at(0.8, 0.8), y, 3.2);
            }
        }
        _ => {}
    }
}

/// The turning part of a zoo or amusement park building, if it has one.
pub fn rotor(massing: Massing, sc: &ServiceCx) -> Option<Rotor> {
    let y = sc.base + 0.12;
    match massing {
        Massing::FerrisWheel => Some(Rotor {
            hub: sc.f.at(0.0, sc.cv, y + WHEEL_HUB),
            axis: sc.f.dir(0.0, 1.0).normalize_or_zero(),
            radius: WHEEL_R,
            color: sc.wall,
            accent: sc.roof,
            kind: RotorKind::FerrisWheel,
        }),
        Massing::Carousel => Some(Rotor {
            hub: sc.f.at(0.0, sc.cv, y + 0.3),
            axis: Vec3::Y,
            radius: CAROUSEL_R,
            color: sc.roof,
            accent: sc.trim,
            kind: RotorKind::Carousel,
        }),
        _ => None,
    }
}

/// The mesh of a ferris wheel or a carousel round the origin, turning round +Z, at `angle`.
pub fn rotor_mesh(r: &Rotor, angle: f32) -> MeshData {
    let mut m = MeshData::default();
    match r.kind {
        RotorKind::FerrisWheel => {
            // In the XY plane: two rims, spokes from the hub, braces between the rims and the
            // gondolas on the rim.
            let rim_n = 24;
            let p = |k: usize, rad: f32, z: f32| {
                let a = angle + k as f32 / rim_n as f32 * std::f32::consts::TAU;
                Vec3::new(rad * a.cos(), rad * a.sin(), z)
            };
            for z in [-0.9f32, 0.9] {
                for k in 0..rim_n {
                    rod(
                        &mut m,
                        p(k, r.radius, z),
                        p(k + 1, r.radius, z),
                        0.35,
                        r.color,
                    );
                    if k % 2 == 0 {
                        rod(
                            &mut m,
                            Vec3::new(0.0, 0.0, z * 0.6),
                            p(k, r.radius, z),
                            0.15,
                            r.color,
                        );
                    }
                }
            }
            for k in (0..rim_n).step_by(2) {
                rod(
                    &mut m,
                    p(k, r.radius, -0.9),
                    p(k, r.radius, 0.9),
                    0.15,
                    r.color,
                );
            }
            rod(
                &mut m,
                Vec3::new(0.0, 0.0, -1.2),
                Vec3::new(0.0, 0.0, 1.2),
                1.2,
                r.accent,
            );
            let f0 = local_frame();
            for k in (0..rim_n).step_by(2) {
                let c = p(k, r.radius + 0.2, 0.0);
                let col = if k % 4 == 0 {
                    r.accent
                } else {
                    BLOOMS[(k / 2) % BLOOMS.len()]
                };
                // `local_frame` maps (u, v, y) to (u, y, v): the gondola box spans z by `v`.
                block(
                    &mut m,
                    &f0,
                    (c.x, 0.0),
                    (0.7, 0.75),
                    c.y - 0.7,
                    c.y + 0.7,
                    col,
                    col,
                );
            }
        }
        RotorKind::Carousel => {
            // Built upright (Y up) and then turned so that the spin axis is +Z.
            let f0 = local_frame();
            let rad = r.radius;
            let n = 16;
            cyl(
                &mut m,
                &f0,
                (0.0, 0.0),
                (rad, rad),
                (0.0, 0.35),
                n,
                r.accent,
                Some(PLANK),
            );
            cyl(
                &mut m,
                &f0,
                (0.0, 0.0),
                (0.55, 0.55),
                (0.35, 3.8),
                8,
                r.accent,
                None,
            );
            // The canopy: a striped cone over a scalloped valance.
            cyl(
                &mut m,
                &f0,
                (0.0, 0.0),
                (rad + 0.2, rad + 0.2),
                (3.8, 4.4),
                n,
                r.accent,
                None,
            );
            for k in 0..n {
                let (a0, a1) = (
                    angle + k as f32 / n as f32 * std::f32::consts::TAU,
                    angle + (k + 1) as f32 / n as f32 * std::f32::consts::TAU,
                );
                let rr = rad + 0.4;
                let (q0, q1, apex) = (
                    Vec3::new(rr * a0.cos(), 4.4, rr * a0.sin()),
                    Vec3::new(rr * a1.cos(), 4.4, rr * a1.sin()),
                    Vec3::new(0.0, 6.6, 0.0),
                );
                let col = if k % 2 == 0 { r.color } else { WOOL };
                let mid = (a0 + a1) * 0.5;
                let nrm =
                    (Vec3::new(mid.cos(), 0.0, mid.sin()) * 2.2 + Vec3::Y * rr).normalize_or_zero();
                let v = [
                    push_vertex(&mut m, q0, nrm, col),
                    push_vertex(&mut m, q1, nrm, col),
                    push_vertex(&mut m, apex, nrm, col),
                ];
                tri(&mut m, v[0], v[1], v[2], nrm);
                // Underside, seen from the platform.
                let v = [
                    push_vertex(&mut m, q0, -Vec3::Y, col),
                    push_vertex(&mut m, q1, -Vec3::Y, col),
                    push_vertex(&mut m, Vec3::new(0.0, 4.4, 0.0), -Vec3::Y, col),
                ];
                tri(&mut m, v[0], v[1], v[2], -Vec3::Y);
            }
            cyl(
                &mut m,
                &f0,
                (0.0, 0.0),
                (0.1, 0.1),
                (6.6, 7.4),
                4,
                IRON,
                None,
            );
            // Horses on brass poles, in a ring, each in its own frame along the turn.
            let horses = 8;
            for k in 0..horses {
                let a = angle + (k as f32 + 0.25) / horses as f32 * std::f32::consts::TAU;
                let (dir, out) = (Vec2::new(-a.sin(), a.cos()), Vec2::new(a.cos(), a.sin()));
                let hf = Frame {
                    c: out * (rad * 0.7),
                    u: dir,
                    v: out,
                };
                let lift = if k % 2 == 0 { 0.0 } else { 0.35 };
                let col = if k % 2 == 0 { WOOL } else { MANE };
                block(
                    &mut m,
                    &hf,
                    (0.0, 0.0),
                    (0.04, 0.04),
                    0.35,
                    3.8,
                    r.accent,
                    r.accent,
                );
                block(
                    &mut m,
                    &hf,
                    (0.0, 0.0),
                    (0.55, 0.16),
                    1.1 + lift,
                    1.5 + lift,
                    col,
                    col,
                );
                block(
                    &mut m,
                    &hf,
                    (0.5, 0.0),
                    (0.14, 0.12),
                    1.4 + lift,
                    2.0 + lift,
                    col,
                    col,
                );
                for s in [-0.4f32, 0.4] {
                    block(
                        &mut m,
                        &hf,
                        (s, 0.0),
                        (0.05, 0.1),
                        0.75 + lift,
                        1.1 + lift,
                        col,
                        col,
                    );
                }
            }
            // Turn the upright mesh so that its up (+Y) is the rotor's +Z.
            let q = Quat::from_rotation_arc(Vec3::Z, Vec3::Y).inverse();
            for (p, n) in m.positions.iter_mut().zip(m.normals.iter_mut()) {
                *p = (q * Vec3::from_array(*p)).to_array();
                *n = (q * Vec3::from_array(*n)).to_array();
            }
        }
        RotorKind::Turbine => {}
    }
    m
}

/// The identity lot frame: `(u, v, y)` is `(x, y, z) = (u, y, v)`.
fn local_frame() -> Frame {
    Frame {
        c: Vec2::ZERO,
        u: Vec2::X,
        v: Vec2::Y,
    }
}
