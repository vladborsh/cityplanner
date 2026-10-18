//! The silhouettes of the city park and the nature reserve areas (T-227): their gates, four
//! attractions each, and the six props every park area can place. Low and flat like the city
//! parks (T-221); the props are drawn a little larger than life so they read from the camera.

use bevy_math::Vec3;
use cp_content::Massing;

use super::{GRASS, GRAVEL, LOG, PLANK, STEEL, ServiceCx, TRUNK, block, cone, cyl, tree};
use crate::building::{Frame, Lod};
use crate::chunk::MeshData;
use crate::road::{flat_quad, push_vertex, tri};

pub(super) const STONE: [f32; 4] = [0.72, 0.70, 0.66, 1.0];
pub(super) const IRON: [f32; 4] = [0.14, 0.15, 0.16, 1.0];
pub(super) const WATER: [f32; 4] = [0.22, 0.45, 0.62, 1.0];
pub(super) const WATER_TOP: [f32; 4] = [0.30, 0.55, 0.72, 1.0];
const FOREST: [f32; 4] = [0.26, 0.48, 0.24, 1.0];
const PINE: [f32; 4] = [0.13, 0.36, 0.20, 1.0];
pub(super) const ROCK: [f32; 4] = [0.36, 0.35, 0.33, 1.0];
const REED: [f32; 4] = [0.55, 0.58, 0.30, 1.0];
pub(super) const PAVING: [f32; 4] = [0.80, 0.78, 0.72, 1.0];
pub(super) const BLOOMS: [[f32; 4]; 4] = [
    [0.86, 0.20, 0.22, 1.0],
    [0.95, 0.80, 0.20, 1.0],
    [0.62, 0.30, 0.75, 1.0],
    [0.95, 0.55, 0.70, 1.0],
];

/// Whether `m` is one of the park area silhouettes of this module.
pub fn is_park_area(m: Massing) -> bool {
    (m as u8) >= Massing::CityParkGate as u8 && (m as u8) <= Massing::PropStatue as u8
}

/// A gable roof over `(cu, cv) ± (hu, hv)` from eaves at `y0` to a ridge along `u` at `y1`.
pub(super) fn gable(
    m: &mut MeshData,
    f: &Frame,
    (cu, cv): (f32, f32),
    (hu, hv): (f32, f32),
    (y0, y1): (f32, f32),
    col: [f32; 4],
) {
    let p = |su: f32, sv: f32, y: f32| f.at(cu + su * hu, cv + sv * hv, y);
    for s in [-1.0f32, 1.0] {
        let n = (f.dir(0.0, s) * (y1 - y0) + Vec3::Y * hv).normalize_or_zero();
        flat_quad(
            m,
            [
                p(-1.0, s, y0),
                p(1.0, s, y0),
                p(1.0, 0.0, y1),
                p(-1.0, 0.0, y1),
            ],
            n,
            col,
        );
        let n = f.dir(s, 0.0).normalize_or_zero();
        let v = [
            push_vertex(m, p(s, -1.0, y0), n, col),
            push_vertex(m, p(s, 1.0, y0), n, col),
            push_vertex(m, p(s, 0.0, y1), n, col),
        ];
        tri(m, v[0], v[1], v[2], n);
    }
}

/// An annular sector round `c` from radius `r0` to `r1` and angle `a0` to `a1` (radians in the
/// lot frame, 0 = `+u`) up to `y1`: its top, outer wall and inner riser.
#[allow(clippy::too_many_arguments)]
pub(super) fn arc_band(
    m: &mut MeshData,
    f: &Frame,
    c: (f32, f32),
    (r0, r1): (f32, f32),
    (a0, a1): (f32, f32),
    (y0, y1): (f32, f32),
    seg: usize,
    side: [f32; 4],
    top: [f32; 4],
) {
    let p = |r: f32, a: f32, y: f32| f.at(c.0 + r * a.cos(), c.1 + r * a.sin(), y);
    for k in 0..seg {
        let a = a0 + (a1 - a0) * k as f32 / seg as f32;
        let b = a0 + (a1 - a0) * (k + 1) as f32 / seg as f32;
        let top_q = [p(r0, a, y1), p(r1, a, y1), p(r1, b, y1), p(r0, b, y1)];
        flat_quad(m, top_q, Vec3::Y, top);
        let mid = (a + b) * 0.5;
        let out = f.dir(mid.cos(), mid.sin()).normalize_or_zero();
        let outer = [p(r1, a, y0), p(r1, b, y0), p(r1, b, y1), p(r1, a, y1)];
        flat_quad(m, outer, out, side);
        let inner = [p(r0, b, y0), p(r0, a, y0), p(r0, a, y1), p(r0, b, y1)];
        flat_quad(m, inner, -out, side);
    }
}

/// A pine: a trunk and two stacked cones.
pub(super) fn pine(
    m: &mut MeshData,
    f: &Frame,
    at: (f32, f32),
    y: f32,
    h: f32,
    n: usize,
    near: bool,
) {
    cyl(m, f, at, (0.3, 0.25), (y, y + h * 0.3), 5, TRUNK, None);
    cone(m, f, at, h * 0.22, (y + h * 0.2, y + h * 0.75), n, PINE);
    if near {
        cone(m, f, at, h * 0.16, (y + h * 0.5, y + h), n, PINE);
    }
}

/// A bench of `len` metres along `u` at `(u, v)`, its back towards `+v` (`back` = -1 turns it).
pub(super) fn bench(m: &mut MeshData, f: &Frame, (u, v): (f32, f32), y: f32, len: f32, back: f32) {
    block(
        m,
        f,
        (u, v),
        (len * 0.5, 0.25),
        y + 0.4,
        y + 0.5,
        PLANK,
        PLANK,
    );
    block(
        m,
        f,
        (u, v + back * 0.25),
        (len * 0.5, 0.06),
        y + 0.5,
        y + 1.0,
        PLANK,
        PLANK,
    );
    for s in [-1.0f32, 1.0] {
        block(
            m,
            f,
            (u + s * len * 0.4, v),
            (0.06, 0.22),
            y,
            y + 0.4,
            IRON,
            IRON,
        );
    }
}

/// A park lamp: a dark post and a pale head.
pub(super) fn lamp(m: &mut MeshData, f: &Frame, at: (f32, f32), y: f32, h: f32) {
    cyl(m, f, at, (0.09, 0.07), (y, y + h), 6, IRON, None);
    cyl(
        m,
        f,
        at,
        (0.28, 0.18),
        (y + h, y + h + 0.5),
        6,
        [0.98, 0.92, 0.70, 1.0],
        Some(IRON),
    );
}

pub fn park_area(m: &mut MeshData, massing: Massing, sc: &ServiceCx) {
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
    match massing {
        Massing::CityParkGate => {
            // A wrought-iron gate with an arch between two stone pillars, railings either side.
            ground(m, 0.12, GRASS);
            let y = b + 0.12;
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
            let front = -0.72;
            for s in [-1.0f32, 1.0] {
                block(
                    m,
                    f,
                    at(s * 0.32, front),
                    (0.45, 0.45),
                    y,
                    y + 3.4,
                    STONE,
                    STONE,
                );
                block(
                    m,
                    f,
                    at(s * 0.32, front),
                    (0.55, 0.55),
                    y + 3.4,
                    y + 3.7,
                    STONE,
                    STONE,
                );
            }
            // The arch: a half ring of short iron pieces from pillar to pillar.
            let (span, rise) = (0.32 * hu, 1.8);
            let pieces = if far { 4 } else { 9 };
            for k in 0..pieces {
                let a = (k as f32 + 0.5) / pieces as f32 * std::f32::consts::PI;
                let (u, h) = (-span * a.cos(), rise * a.sin());
                block(
                    m,
                    f,
                    (u, cv + front * hv),
                    (span / pieces as f32 + 0.05, 0.08),
                    y + 3.2 + h,
                    y + 3.45 + h,
                    IRON,
                    IRON,
                );
            }
            if !far {
                // The two gate leaves, half open, and the railings.
                for s in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        at(s * 0.17, front + 0.05),
                        (0.13 * hu, 0.05),
                        y,
                        y + 2.6,
                        IRON,
                        IRON,
                    );
                    block(
                        m,
                        f,
                        at(s * 0.68, front),
                        (0.32 * hu, 0.05),
                        y + 1.6,
                        y + 1.7,
                        IRON,
                        IRON,
                    );
                }
            }
            if near {
                for k in 0..12 {
                    let u = 0.38 + 0.6 * k as f32 / 11.0;
                    for s in [-1.0f32, 1.0] {
                        block(m, f, at(s * u, front), (0.04, 0.04), y, y + 1.8, IRON, IRON);
                    }
                }
            }
            for (u, v) in [(-0.6, 0.4), (0.6, 0.4), (0.0, 0.75)] {
                tree(m, f, at(u, v), y, 6.0, n, near);
            }
        }
        Massing::FlowerGarden => {
            // Four flower beds of different colours round a sundial, gravel paths between.
            ground(m, 0.12, GRASS);
            let y = b + 0.12;
            block(
                m,
                f,
                at(0.0, 0.0),
                (0.9 * hu, 0.08 * hv),
                y,
                y + 0.05,
                GRAVEL,
                GRAVEL,
            );
            block(
                m,
                f,
                at(0.0, 0.0),
                (0.08 * hu, 0.9 * hv),
                y,
                y + 0.05,
                GRAVEL,
                GRAVEL,
            );
            for (k, (u, v)) in [(-0.45, -0.45), (0.45, -0.45), (-0.45, 0.45), (0.45, 0.45)]
                .into_iter()
                .enumerate()
            {
                block(
                    m,
                    f,
                    at(u, v),
                    (0.32 * hu, 0.32 * hv),
                    y,
                    y + 0.35,
                    TRUNK,
                    BLOOMS[k],
                );
                if near {
                    block(
                        m,
                        f,
                        at(u, v),
                        (0.12 * hu, 0.12 * hv),
                        y + 0.35,
                        y + 0.55,
                        BLOOMS[(k + 2) % 4],
                        BLOOMS[(k + 2) % 4],
                    );
                }
            }
            cyl(
                m,
                f,
                at(0.0, 0.0),
                (0.3, 0.25),
                (y, y + 1.1),
                6,
                STONE,
                Some(STONE),
            );
            if near {
                cone(
                    m,
                    f,
                    at(0.0, 0.0),
                    0.35,
                    (y + 1.1, y + 1.5),
                    6,
                    [0.75, 0.6, 0.3, 1.0],
                );
                for (u, v) in [(-0.85, 0.0), (0.85, 0.0)] {
                    tree(m, f, at(u, v), y, 4.0, n, near);
                }
            }
        }
        Massing::ParkPond => {
            // A pond with an arched wooden bridge, reeds and willows round the bank.
            ground(m, 0.12, GRASS);
            let y = b + 0.12;
            cyl(
                m,
                f,
                at(0.0, 0.0),
                (0.66 * r, 0.62 * r),
                (y - 0.05, y + 0.04),
                n.max(10),
                WATER,
                Some(WATER_TOP),
            );
            // The bridge: a hump of five planks across the pond along `u`, with rails near.
            let steps = if far { 3 } else { 7 };
            for k in 0..steps {
                let t = (k as f32 + 0.5) / steps as f32;
                let h = 1.4 * (t * std::f32::consts::PI).sin();
                let u = (t - 0.5) * 1.5 * r;
                block(
                    m,
                    f,
                    (u, cv),
                    (0.75 * r / steps as f32 + 0.05, 1.0),
                    y + h,
                    y + h + 0.25,
                    PLANK,
                    PLANK,
                );
                if near {
                    for s in [-1.0f32, 1.0] {
                        block(
                            m,
                            f,
                            (u, cv + s * 0.95),
                            (0.06, 0.06),
                            y + h + 0.25,
                            y + h + 1.1,
                            TRUNK,
                            TRUNK,
                        );
                    }
                }
            }
            if !far {
                for k in 0..8 {
                    let a = k as f32 / 8.0 * std::f32::consts::TAU + 0.3;
                    let p = (0.6 * r * a.cos(), cv + 0.6 * r * a.sin());
                    if k % 2 == 0 {
                        cone(m, f, p, 0.5, (y, y + 1.4), 5, REED);
                    }
                }
            }
            for (u, v) in [(-0.82, -0.8), (0.82, 0.8), (0.85, -0.7)] {
                tree(m, f, at(u, v), y, 6.5, n, near);
            }
        }
        Massing::ParkPavilion => {
            // A white hexagonal gazebo on a low deck, a pointed roof, paths from four sides.
            ground(m, 0.12, GRASS);
            let y = b + 0.12;
            block(
                m,
                f,
                at(0.0, 0.0),
                (0.92 * hu, 0.07 * hv),
                y,
                y + 0.05,
                GRAVEL,
                GRAVEL,
            );
            block(
                m,
                f,
                at(0.0, 0.0),
                (0.07 * hu, 0.92 * hv),
                y,
                y + 0.05,
                GRAVEL,
                GRAVEL,
            );
            let rr = 0.38 * r;
            cyl(
                m,
                f,
                at(0.0, 0.0),
                (rr + 0.4, rr + 0.4),
                (y, y + 0.5),
                6,
                STONE,
                Some(PLANK),
            );
            let posts = 6;
            for k in 0..posts {
                let a = k as f32 / posts as f32 * std::f32::consts::TAU;
                let p = (rr * a.cos(), cv + rr * a.sin());
                cyl(m, f, p, (0.16, 0.16), (y + 0.5, y + 3.2), 5, sc.trim, None);
            }
            cyl(
                m,
                f,
                at(0.0, 0.0),
                (rr + 0.7, rr + 0.6),
                (y + 3.2, y + 3.5),
                6,
                sc.trim,
                None,
            );
            cone(m, f, at(0.0, 0.0), rr + 0.7, (y + 3.5, y + 6.0), 6, sc.roof);
            if near {
                cyl(
                    m,
                    f,
                    at(0.0, 0.0),
                    (0.06, 0.06),
                    (y + 6.0, y + 6.8),
                    4,
                    IRON,
                    None,
                );
                // A rail between the posts, open on the path sides.
                cyl(
                    m,
                    f,
                    at(0.0, 0.0),
                    (rr, rr),
                    (y + 1.3, y + 1.4),
                    6,
                    sc.trim,
                    None,
                );
            }
            for (u, v) in [(-0.7, -0.7), (0.7, 0.7), (-0.7, 0.7), (0.7, -0.7)] {
                tree(m, f, at(u, v), y, 6.0, n, near);
            }
        }
        Massing::Amphitheatre => {
            // Grass tiers in a half ring facing a stage with a curved shell.
            ground(m, 0.12, GRASS);
            let y = b + 0.12;
            let tiers = if far { 2 } else { 4 };
            let seg = if far { 6 } else { 12 };
            let stage = (0.0, cv - 0.45 * hv);
            for t in 0..tiers {
                let rad = (0.42 + 0.13 * t as f32) * r;
                let h = 0.45 * (t + 1) as f32;
                arc_band(
                    m,
                    f,
                    stage,
                    (rad, rad + 0.13 * r),
                    (0.15, std::f32::consts::PI - 0.15),
                    (y, y + h),
                    seg,
                    STONE,
                    GRASS,
                );
            }
            // The stage and its shell: a quarter dome of a few panels behind it.
            cyl(
                m,
                f,
                stage,
                (0.28 * r, 0.28 * r),
                (y, y + 0.9),
                n.max(8),
                STONE,
                Some(PLANK),
            );
            let panels = if far { 3 } else { 7 };
            for k in 0..panels {
                let a =
                    std::f32::consts::PI + (k as f32 + 0.5) / panels as f32 * std::f32::consts::PI;
                let rad = 0.34 * r;
                let p = (stage.0 + rad * a.cos(), stage.1 + rad * a.sin());
                let h = 4.5 - 1.2 * ((k as f32 + 0.5) / panels as f32 - 0.5).abs() * 2.0;
                block(
                    m,
                    f,
                    p,
                    (rad * 0.24, 0.2),
                    y + 0.9,
                    y + 0.9 + h,
                    sc.trim,
                    sc.trim,
                );
            }
            for (u, v) in [(-0.85, -0.85), (0.85, -0.85)] {
                tree(m, f, at(u, v), y, 6.0, n, near);
            }
        }
        Massing::NatureGate => {
            // A timber gate: two log posts under a gabled canopy, a sign board, pines behind.
            ground(m, 0.12, FOREST);
            let y = b + 0.12;
            block(
                m,
                f,
                at(0.0, -0.1),
                (0.2 * hu, 0.85 * hv),
                y,
                y + 0.06,
                [0.55, 0.42, 0.28, 1.0],
                [0.55, 0.42, 0.28, 1.0],
            );
            let front = -0.65;
            for s in [-1.0f32, 1.0] {
                cyl(
                    m,
                    f,
                    at(s * 0.3, front),
                    (0.3, 0.28),
                    (y, y + 3.6),
                    6,
                    LOG,
                    None,
                );
            }
            block(
                m,
                f,
                at(0.0, front),
                (0.4 * hu, 0.3),
                y + 3.0,
                y + 3.6,
                LOG,
                LOG,
            );
            gable(
                m,
                f,
                at(0.0, front),
                (0.48 * hu, 1.4),
                (y + 3.6, y + 5.0),
                sc.roof,
            );
            if !far {
                block(
                    m,
                    f,
                    at(0.0, front - 0.02),
                    (0.22 * hu, 0.08),
                    y + 2.2,
                    y + 2.9,
                    PLANK,
                    PLANK,
                );
            }
            for (u, v) in [(-0.7, 0.2), (0.7, 0.1), (-0.3, 0.75), (0.45, 0.7)] {
                pine(m, f, at(u, v), y, 8.0, n, near);
            }
        }
        Massing::Viewpoint => {
            // A timber watchtower: four splayed legs, a platform with a rail, a pyramid roof.
            ground(m, 0.12, FOREST);
            let y = b + 0.12;
            let (deck, half) = (7.5, 0.22 * r);
            for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let leg = (su * half, cv + sv * half);
                cyl(m, f, leg, (0.22, 0.18), (y, y + deck), 5, LOG, None);
            }
            // Ring beams round the legs: two, one far.
            let beams: &[f32] = if far { &[0.5] } else { &[0.33, 0.66] };
            for h in beams {
                let (y0, y1) = (y + deck * h, y + deck * h + 0.25);
                for s in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        (0.0, cv + s * half),
                        (half + 0.12, 0.12),
                        y0,
                        y1,
                        LOG,
                        LOG,
                    );
                    block(m, f, (s * half, cv), (0.12, half + 0.12), y0, y1, LOG, LOG);
                }
            }
            block(
                m,
                f,
                (0.0, cv),
                (half + 0.4, half + 0.4),
                y + deck,
                y + deck + 0.3,
                PLANK,
                PLANK,
            );
            if !far {
                for (u, v, wu, wv) in [
                    (0.0, -1.0, 1.0, 0.04),
                    (0.0, 1.0, 1.0, 0.04),
                    (-1.0, 0.0, 0.04, 1.0),
                    (1.0, 0.0, 0.04, 1.0),
                ] {
                    block(
                        m,
                        f,
                        (u * (half + 0.4), cv + v * (half + 0.4)),
                        (wu * (half + 0.4) + 0.05, wv * (half + 0.4) + 0.05),
                        y + deck + 0.3,
                        y + deck + 1.2,
                        LOG,
                        LOG,
                    );
                }
            }
            cone(
                m,
                f,
                (0.0, cv),
                (half + 0.8) * std::f32::consts::SQRT_2,
                (y + deck + 2.4, y + deck + 3.8),
                4,
                sc.roof,
            );
            for (su, sv) in [(-1.0f32, -1.0f32), (1.0, 1.0)] {
                block(
                    m,
                    f,
                    (su * half, cv + sv * half),
                    (0.12, 0.12),
                    y + deck + 0.3,
                    y + deck + 2.4,
                    LOG,
                    LOG,
                );
            }
            for (u, v) in [(-0.75, 0.7), (0.75, -0.6)] {
                pine(m, f, at(u, v), y, 7.0, n, near);
            }
        }
        Massing::BirdHide => {
            // A reedy marsh pool and a hut on stilts with a dark viewing slit, a boardwalk to it.
            ground(m, 0.12, FOREST);
            let y = b + 0.12;
            cyl(
                m,
                f,
                at(0.15, 0.25),
                (0.55 * r, 0.5 * r),
                (y - 0.05, y + 0.04),
                n.max(8),
                WATER,
                Some(WATER_TOP),
            );
            let hut = at(-0.35, -0.1);
            for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                block(
                    m,
                    f,
                    (hut.0 + su * 1.6, hut.1 + sv * 1.3),
                    (0.12, 0.12),
                    y,
                    y + 1.2,
                    LOG,
                    LOG,
                );
            }
            block(m, f, hut, (1.9, 1.6), y + 1.2, y + 3.4, PLANK, PLANK);
            gable(m, f, hut, (2.2, 1.9), (y + 3.4, y + 4.6), sc.roof);
            if !far {
                block(
                    m,
                    f,
                    (hut.0, hut.1 + 1.62),
                    (1.4, 0.03),
                    y + 2.4,
                    y + 2.75,
                    [0.06, 0.06, 0.07, 1.0],
                    [0.06, 0.06, 0.07, 1.0],
                );
                block(
                    m,
                    f,
                    (hut.0, hut.1 - 0.35 * hv),
                    (0.5, 0.3 * hv),
                    y + 0.4,
                    y + 0.55,
                    PLANK,
                    PLANK,
                );
            }
            if !far {
                for k in 0..10 {
                    let a = k as f32 / 10.0 * std::f32::consts::TAU;
                    let p = (
                        0.15 * hu + 0.52 * r * a.cos(),
                        cv + 0.25 * hv + 0.5 * r * a.sin(),
                    );
                    if k % 2 == 1 || near {
                        cone(m, f, p, 0.45, (y, y + 1.6), 4, REED);
                    }
                }
            }
            pine(m, f, at(0.8, -0.75), y, 8.0, n, near);
        }
        Massing::Waterfall => {
            // A rocky step with a sheet of water falling into a round pool, pines on top.
            ground(m, 0.12, FOREST);
            let y = b + 0.12;
            let cliff = 5.0;
            block(
                m,
                f,
                at(0.0, 0.62),
                (0.8 * hu, 0.3 * hv),
                y,
                y + cliff,
                ROCK,
                FOREST,
            );
            for (u, v, w, h) in [
                (-0.6, 0.22, 0.22, 3.0),
                (0.55, 0.2, 0.25, 3.8),
                (-0.25, 0.25, 0.12, 1.8),
            ] {
                block(m, f, at(u, v), (w * hu, 0.15 * hv), y, y + h, ROCK, ROCK);
            }
            // The fall: a thin bright sheet down the cliff face, a stream on top, a pool below.
            block(
                m,
                f,
                at(0.1, 0.30),
                (0.12 * hu, 0.03 * hv),
                y + 0.2,
                y + cliff + 0.05,
                WATER_TOP,
                WATER_TOP,
            );
            block(
                m,
                f,
                at(0.1, 0.62),
                (0.1 * hu, 0.3 * hv),
                y + cliff,
                y + cliff + 0.06,
                WATER,
                WATER_TOP,
            );
            cyl(
                m,
                f,
                at(0.1, -0.3),
                (0.42 * r, 0.38 * r),
                (y - 0.05, y + 0.05),
                n.max(8),
                WATER,
                Some(WATER_TOP),
            );
            if !far {
                cyl(
                    m,
                    f,
                    at(0.1, -0.05),
                    (0.6, 0.2),
                    (y, y + 1.0),
                    6,
                    [0.9, 0.95, 1.0, 1.0],
                    None,
                );
            }
            for (u, v) in [(-0.6, 0.7), (0.6, 0.75), (0.75, -0.75)] {
                let base = if v > 0.0 { y + cliff } else { y };
                pine(m, f, at(u, v), base, 7.0, n, near);
            }
        }
        Massing::Lodge => {
            // A log lodge with a porch and a stone chimney, a fire pit and pines round it.
            ground(m, 0.12, FOREST);
            let y = b + 0.12;
            let house = at(0.0, 0.15);
            let (wu, wv) = (0.5 * hu, 0.32 * hv);
            block(m, f, house, (wu, wv), y, y + 3.4, LOG, LOG);
            gable(
                m,
                f,
                house,
                (wu + 0.6, wv + 0.8),
                (y + 3.4, y + 6.4),
                sc.roof,
            );
            block(
                m,
                f,
                (house.0 + 0.6 * wu, house.1 + 0.3 * wv),
                (0.6, 0.6),
                y + 3.0,
                y + 7.2,
                STONE,
                STONE,
            );
            // The porch along the front: a deck and a lean-to roof on posts.
            block(
                m,
                f,
                (house.0, house.1 - wv - 1.0),
                (0.8 * wu, 1.0),
                y,
                y + 0.4,
                PLANK,
                PLANK,
            );
            if !far {
                block(
                    m,
                    f,
                    (house.0, house.1 - wv - 1.0),
                    (0.85 * wu, 1.1),
                    y + 2.8,
                    y + 3.0,
                    sc.roof,
                    sc.roof,
                );
                for s in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        (house.0 + s * 0.75 * wu, house.1 - wv - 1.8),
                        (0.12, 0.12),
                        y + 0.4,
                        y + 2.8,
                        LOG,
                        LOG,
                    );
                }
                // Windows: dark squares on the front wall.
                for s in [-0.55f32, 0.55] {
                    block(
                        m,
                        f,
                        (house.0 + s * wu, house.1 - wv - 0.02),
                        (0.7, 0.03),
                        y + 1.2,
                        y + 2.3,
                        [0.2, 0.25, 0.3, 1.0],
                        [0.2, 0.25, 0.3, 1.0],
                    );
                }
            }
            if near {
                let pit = at(-0.5, -0.7);
                cyl(
                    m,
                    f,
                    pit,
                    (0.9, 0.8),
                    (y, y + 0.3),
                    8,
                    STONE,
                    Some([0.3, 0.2, 0.15, 1.0]),
                );
                cone(
                    m,
                    f,
                    pit,
                    0.4,
                    (y + 0.3, y + 1.0),
                    5,
                    [0.95, 0.55, 0.15, 1.0],
                );
                for k in 0..3 {
                    let a = k as f32 * 2.1;
                    block(
                        m,
                        f,
                        (pit.0 + 2.0 * a.cos(), pit.1 + 2.0 * a.sin()),
                        (0.6, 0.2),
                        y,
                        y + 0.45,
                        LOG,
                        LOG,
                    );
                }
            }
            for (u, v) in [(-0.85, 0.85), (0.85, 0.8), (0.85, -0.6), (-0.85, -0.2)] {
                pine(m, f, at(u, v), y, 9.0, n, near);
            }
        }
        Massing::PropBench => {
            // Two benches facing across a gravel square, a bin between them.
            ground(m, 0.1, GRASS);
            let y = b + 0.1;
            block(
                m,
                f,
                at(0.0, 0.0),
                (0.6 * hu, 0.6 * hv),
                y,
                y + 0.04,
                GRAVEL,
                GRAVEL,
            );
            let y = y + 0.04;
            bench(m, f, at(0.0, -0.4), y, 2.2, -1.0);
            bench(m, f, at(0.0, 0.4), y, 2.2, 1.0);
            if !far {
                cyl(
                    m,
                    f,
                    at(0.45, 0.0),
                    (0.25, 0.25),
                    (y, y + 0.9),
                    6,
                    IRON,
                    Some(DARKEST),
                );
            }
            if near {
                // Armrests at the bench ends.
                for (v, s) in [(-0.4f32, -1.0f32), (0.4, 1.0)] {
                    for e in [-1.0f32, 1.0] {
                        let p = at(0.0, v);
                        block(
                            m,
                            f,
                            (p.0 + e * 1.05, p.1 + s * 0.05),
                            (0.05, 0.22),
                            y + 0.5,
                            y + 0.75,
                            IRON,
                            IRON,
                        );
                    }
                }
            }
        }
        Massing::PropTreeCluster => {
            // Three trees of different heights on a mown patch.
            ground(m, 0.1, GRASS);
            let y = b + 0.1;
            tree(m, f, at(-0.35, -0.3), y, 6.5, n, near);
            tree(m, f, at(0.4, -0.15), y, 5.0, n, near);
            tree(m, f, at(0.0, 0.4), y, 7.5, n, near);
        }
        Massing::PropLamp => {
            // Two park lamps on a paved strip.
            ground(m, 0.1, GRASS);
            let y = b + 0.1;
            block(
                m,
                f,
                at(0.0, 0.0),
                (0.8 * hu, 0.18 * hv),
                y,
                y + 0.04,
                PAVING,
                PAVING,
            );
            for s in [-1.0f32, 1.0] {
                lamp(m, f, at(s * 0.45, 0.0), y + 0.04, 3.6);
                if near {
                    block(
                        m,
                        f,
                        at(s * 0.45, 0.0),
                        (0.2, 0.2),
                        y + 0.04,
                        y + 0.4,
                        IRON,
                        IRON,
                    );
                }
            }
        }
        Massing::PropFlowerBed => {
            // A round raised bed in rings of colour.
            ground(m, 0.1, GRASS);
            let y = b + 0.1;
            cyl(
                m,
                f,
                at(0.0, 0.0),
                (0.7 * r, 0.7 * r),
                (y, y + 0.35),
                n.max(8),
                STONE,
                Some(BLOOMS[0]),
            );
            cyl(
                m,
                f,
                at(0.0, 0.0),
                (0.48 * r, 0.42 * r),
                (y + 0.35, y + 0.55),
                n.max(8),
                BLOOMS[1],
                Some(BLOOMS[2]),
            );
            if !far {
                cyl(
                    m,
                    f,
                    at(0.0, 0.0),
                    (0.22 * r, 0.1 * r),
                    (y + 0.55, y + 0.9),
                    n.max(8),
                    BLOOMS[3],
                    Some(BLOOMS[3]),
                );
            }
        }
        Massing::PropKiosk => {
            // A small kiosk with a striped awning and a counter, a parasol table beside it.
            ground(m, 0.1, PAVING);
            let y = b + 0.1;
            let k = at(-0.15, 0.1);
            block(m, f, k, (1.5, 1.3), y, y + 2.6, sc.wall, sc.wall);
            block(m, f, k, (1.7, 1.5), y + 2.6, y + 2.8, sc.roof, sc.roof);
            if !far {
                for i in 0..4 {
                    let col = if i % 2 == 0 { sc.trim } else { sc.roof };
                    block(
                        m,
                        f,
                        (k.0 - 1.3 + 0.75 * i as f32 + 0.375, k.1 - 1.7),
                        (0.375, 0.4),
                        y + 2.1,
                        y + 2.25,
                        col,
                        col,
                    );
                }
                block(
                    m,
                    f,
                    (k.0, k.1 - 1.4),
                    (1.3, 0.15),
                    y + 0.9,
                    y + 1.05,
                    PLANK,
                    PLANK,
                );
            }
            let t = at(0.6, -0.45);
            cyl(m, f, t, (0.05, 0.05), (y, y + 2.2), 4, STEEL, None);
            cone(
                m,
                f,
                t,
                1.1,
                (y + 1.9, y + 2.4),
                if far { 4 } else { 8 },
                sc.trim,
            );
            if near {
                cyl(
                    m,
                    f,
                    t,
                    (0.5, 0.5),
                    (y + 0.7, y + 0.75),
                    8,
                    PLANK,
                    Some(PLANK),
                );
            }
        }
        Massing::PropStatue => {
            // A figure on a stepped plinth in a small paved square.
            ground(m, 0.1, GRASS);
            let y = b + 0.1;
            block(
                m,
                f,
                at(0.0, 0.0),
                (0.7 * hu, 0.7 * hv),
                y,
                y + 0.05,
                PAVING,
                PAVING,
            );
            let c = at(0.0, 0.0);
            block(m, f, c, (1.0, 1.0), y + 0.05, y + 0.45, STONE, STONE);
            block(m, f, c, (0.7, 0.7), y + 0.45, y + 1.9, STONE, STONE);
            let bronze = [0.36, 0.42, 0.34, 1.0];
            cyl(
                m,
                f,
                c,
                (0.42, 0.3),
                (y + 1.9, y + 3.4),
                if far { 4 } else { 8 },
                bronze,
                None,
            );
            cyl(
                m,
                f,
                c,
                (0.22, 0.2),
                (y + 3.4, y + 3.85),
                if far { 4 } else { 8 },
                bronze,
                Some(bronze),
            );
            if near {
                // A plaque on the front of the plinth.
                block(
                    m,
                    f,
                    (c.0, c.1 - 0.72),
                    (0.35, 0.03),
                    y + 0.9,
                    y + 1.3,
                    [0.55, 0.45, 0.2, 1.0],
                    [0.55, 0.45, 0.2, 1.0],
                );
            }
            if !far {
                // An arm raised to one side.
                block(
                    m,
                    f,
                    (c.0 + 0.45, c.1),
                    (0.1, 0.1),
                    y + 2.9,
                    y + 4.0,
                    bronze,
                    bronze,
                );
            }
        }
        _ => {}
    }
}

const DARKEST: [f32; 4] = [0.08, 0.08, 0.09, 1.0];
