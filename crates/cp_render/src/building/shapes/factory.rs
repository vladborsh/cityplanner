//! The silhouettes of the four unique M5 factories and the cargo station (T-218): the food
//! factory's canning towers, the furniture factory's sawtooth hall and plank stacks, the white
//! electronics block with its antennas, the car factory's long assembly hall over a lot of new
//! cars, the cargo station's gantry crane over its container yard, and the cargo port's quay
//! with its ship-to-shore crane.

use bevy_math::Vec3;
use cp_content::Massing;

use super::park_area::gable;
use super::theme_park::{mul, rod};
use super::{DARK, GRASS, LOG, PLANK, STEEL, ServiceCx, TANK_WHITE, block, cone, cyl, tree};
use crate::building::{Frame, Lod};
use crate::chunk::MeshData;
use crate::road::{flat_quad, push_vertex, tri};

const PAD: [f32; 4] = [0.58, 0.58, 0.56, 1.0];
const ASPHALT: [f32; 4] = [0.30, 0.31, 0.33, 1.0];
const LINE: [f32; 4] = [0.92, 0.92, 0.88, 1.0];
const DOOR: [f32; 4] = [0.32, 0.34, 0.37, 1.0];
const CAR_GLASS: [f32; 4] = [0.16, 0.20, 0.26, 1.0];
const SOLAR: [f32; 4] = [0.16, 0.22, 0.42, 1.0];
const QUAY: [f32; 4] = [0.66, 0.65, 0.62, 1.0];
const BRICK: [f32; 4] = [0.56, 0.30, 0.24, 1.0];
/// Paints of the parked cars and of the containers, cycled.
const PAINTS: [[f32; 4]; 6] = [
    [0.80, 0.16, 0.14, 1.0],
    [0.18, 0.36, 0.72, 1.0],
    [0.92, 0.92, 0.90, 1.0],
    [0.16, 0.16, 0.18, 1.0],
    [0.86, 0.64, 0.14, 1.0],
    [0.30, 0.56, 0.36, 1.0],
];

/// Whether `m` is one of the factory, cargo station or cargo port silhouettes of this module.
pub fn is_factory(m: Massing) -> bool {
    (m as u8) >= Massing::FoodFactory as u8 && (m as u8) <= Massing::CargoPort as u8
}

/// A thin shell round the hull `(cu, cv) ± (hu, hv)` from `y0` to `y1`: a window band or a
/// coloured stripe on all four walls.
#[allow(clippy::too_many_arguments)]
fn band(
    m: &mut MeshData,
    f: &Frame,
    at: (f32, f32),
    (hu, hv): (f32, f32),
    (y0, y1): (f32, f32),
    col: [f32; 4],
) {
    block(m, f, at, (hu + 0.05, hv + 0.05), y0, y1, col, col);
}

/// A sawtooth (north-light) roof over `(cu, cv) ± (hu, hv)` from `y`: `teeth` ridges across
/// `u`, each a slope rising `rise` towards `+v` and a glazed upright face.
#[allow(clippy::too_many_arguments)]
fn sawtooth(
    m: &mut MeshData,
    f: &Frame,
    (cu, cv): (f32, f32),
    (hu, hv): (f32, f32),
    y: f32,
    teeth: usize,
    rise: f32,
    (roof, glass, wall): ([f32; 4], [f32; 4], [f32; 4]),
) {
    let step = 2.0 * hv / teeth as f32;
    for k in 0..teeth {
        let v0 = cv - hv + step * k as f32;
        let v1 = v0 + step;
        let n = (f.dir(0.0, -1.0) * rise + Vec3::Y * step).normalize_or_zero();
        flat_quad(
            m,
            [
                f.at(cu - hu, v0, y),
                f.at(cu + hu, v0, y),
                f.at(cu + hu, v1, y + rise),
                f.at(cu - hu, v1, y + rise),
            ],
            n,
            roof,
        );
        flat_quad(
            m,
            [
                f.at(cu - hu, v1, y),
                f.at(cu + hu, v1, y),
                f.at(cu + hu, v1, y + rise),
                f.at(cu - hu, v1, y + rise),
            ],
            f.dir(0.0, 1.0).normalize_or_zero(),
            glass,
        );
        for s in [-1.0f32, 1.0] {
            let n = f.dir(s, 0.0).normalize_or_zero();
            let v = [
                push_vertex(m, f.at(cu + s * hu, v0, y), n, wall),
                push_vertex(m, f.at(cu + s * hu, v1, y), n, wall),
                push_vertex(m, f.at(cu + s * hu, v1, y + rise), n, wall),
            ];
            tri(m, v[0], v[1], v[2], n);
        }
    }
}

/// A parked car at `at`, its length along `v` (`along_u` turns it), body colour `col`; `near`
/// adds the cabin.
fn car(
    m: &mut MeshData,
    f: &Frame,
    at: (f32, f32),
    y: f32,
    along_u: bool,
    col: [f32; 4],
    near: bool,
) {
    let (l, w) = (2.1, 0.88);
    let half = if along_u { (l, w) } else { (w, l) };
    block(m, f, at, half, y + 0.2, y + 0.85, col, col);
    if near {
        let c = if along_u { (1.05, 0.8) } else { (0.8, 1.05) };
        block(m, f, at, c, y + 0.85, y + 1.4, CAR_GLASS, col);
    }
}

/// A shipping container (6.1 x 2.44 x 2.6 m) standing at `at` from `y`, its length along `u`
/// (`along_u` false turns it).
fn container(m: &mut MeshData, f: &Frame, at: (f32, f32), y: f32, along_u: bool, col: [f32; 4]) {
    let half = if along_u { (3.05, 1.22) } else { (1.22, 3.05) };
    block(m, f, at, half, y, y + 2.6, col, mul(col, 0.85));
}

/// A truck facing `+u` at `(u, v)`: cab, chassis, wheels and, with `load`, a container.
fn truck(
    m: &mut MeshData,
    f: &Frame,
    (u, v): (f32, f32),
    y: f32,
    cab: [f32; 4],
    load: Option<[f32; 4]>,
) {
    block(m, f, (u + 3.0, v), (0.9, 1.2), y + 0.5, y + 3.0, cab, cab);
    block(
        m,
        f,
        (u + 3.88, v),
        (0.03, 1.0),
        y + 1.9,
        y + 2.7,
        CAR_GLASS,
        CAR_GLASS,
    );
    block(
        m,
        f,
        (u - 0.6, v),
        (2.8, 1.15),
        y + 0.5,
        y + 1.1,
        DARK,
        DARK,
    );
    for du in [-2.6f32, -1.4, 3.0] {
        for s in [-1.0f32, 1.0] {
            block(
                m,
                f,
                (u + du, v + s * 1.0),
                (0.5, 0.18),
                y,
                y + 1.0,
                DARK,
                DARK,
            );
        }
    }
    if let Some(col) = load {
        container(m, f, (u - 0.6, v), y + 1.1, true, col);
    }
}

pub fn factory(m: &mut MeshData, massing: Massing, sc: &ServiceCx) {
    let (f, b) = (sc.f, sc.base);
    let (hu, hv, cv) = (sc.hu, sc.hv, sc.cv);
    let near = sc.lod == Lod::Near;
    let far = sc.lod == Lod::Far;
    let n = if near {
        12
    } else if far {
        6
    } else {
        8
    };
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
        Massing::FoodFactory => {
            // A gabled hall, three canning towers in a row beside it, a brick chimney and a
            // giant can on a post by the street.
            ground(m, 0.12, PAD);
            let hall = at(-0.22, 0.12);
            let (wu, wv) = (0.68 * hu, 0.62 * hv);
            block(m, f, hall, (wu, wv), y, y + 7.0, sc.wall, sc.wall);
            gable(
                m,
                f,
                hall,
                (wu + 0.3, wv + 0.3),
                (y + 7.0, y + 9.5),
                sc.roof,
            );
            if !far {
                band(m, f, hall, (wu, wv), (y + 3.8, y + 5.2), sc.glass);
            }
            for k in 0..3 {
                let p = at(0.72, -0.5 + 0.5 * k as f32);
                cyl(m, f, p, (2.2, 2.2), (y, y + 13.0), n, TANK_WHITE, None);
                cone(m, f, p, 2.3, (y + 13.0, y + 14.6), n, sc.roof);
                if !far {
                    cyl(m, f, p, (2.25, 2.25), (y + 9.0, y + 10.2), n, sc.trim, None);
                }
            }
            // The brick chimney with a band near the top.
            let ch = at(-0.78, 0.72);
            cyl(m, f, ch, (0.95, 0.7), (y, y + 16.0), n, BRICK, Some(DARK));
            if !far {
                cyl(
                    m,
                    f,
                    ch,
                    (0.78, 0.78),
                    (y + 14.6, y + 15.3),
                    n,
                    sc.trim,
                    None,
                );
                // The conveyor bridge from the hall to the towers and the catwalk over them.
                let (hr, tu) = (hall.0 + wu, at(0.72, 0.0).0);
                block(
                    m,
                    f,
                    ((hr + tu) * 0.5, hall.1 - 0.1 * hv),
                    ((tu - hr) * 0.5, 0.6),
                    y + 8.6,
                    y + 9.6,
                    STEEL,
                    STEEL,
                );
                block(
                    m,
                    f,
                    at(0.72, 0.0),
                    (0.4, 0.62 * hv),
                    y + 13.0,
                    y + 13.3,
                    STEEL,
                    STEEL,
                );
            }
            if near {
                // Three loading doors on the street side and a truck at the dock.
                for k in 0..3 {
                    block(
                        m,
                        f,
                        (hall.0 - 0.45 * wu + 0.45 * wu * k as f32, hall.1 - wv),
                        (1.5, 0.05),
                        y,
                        y + 3.4,
                        DOOR,
                        DOOR,
                    );
                }
                truck(m, f, at(0.25, -0.8), y, sc.trim, None);
                // The can on its post: red with a white label.
                let c = at(-0.8, -0.75);
                block(m, f, c, (0.15, 0.15), y, y + 3.0, STEEL, STEEL);
                cyl(
                    m,
                    f,
                    c,
                    (1.3, 1.3),
                    (y + 3.0, y + 5.8),
                    n,
                    sc.trim,
                    Some(STEEL),
                );
                cyl(
                    m,
                    f,
                    c,
                    (1.33, 1.33),
                    (y + 3.8, y + 5.0),
                    n,
                    TANK_WHITE,
                    None,
                );
            }
        }
        Massing::FurnitureFactory => {
            // A hall under a sawtooth roof, a dust cyclone beside it, plank stacks and a log
            // pile in the yard.
            ground(m, 0.12, PAD);
            let hall = at(-0.2, 0.18);
            let (wu, wv) = (0.72 * hu, 0.6 * hv);
            block(m, f, hall, (wu, wv), y, y + 5.5, sc.wall, sc.wall);
            sawtooth(
                m,
                f,
                hall,
                (wu, wv),
                y + 5.5,
                if far { 3 } else { 4 },
                2.4,
                (sc.roof, sc.glass, sc.wall),
            );
            if !far {
                band(m, f, hall, (wu, wv), (y + 2.2, y + 3.5), sc.glass);
            }
            // The dust cyclone: a cone on legs under a drum, a duct to the roof.
            let cy = at(0.72, 0.55);
            cyl(m, f, cy, (0.3, 1.5), (y + 3.0, y + 6.0), n, STEEL, None);
            cyl(
                m,
                f,
                cy,
                (1.5, 1.5),
                (y + 6.0, y + 8.6),
                n,
                STEEL,
                Some(sc.trim),
            );
            if !far {
                for (du, dv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    block(
                        m,
                        f,
                        (cy.0 + du * 1.0, cy.1 + dv * 1.0),
                        (0.1, 0.1),
                        y,
                        y + 6.2,
                        DARK,
                        DARK,
                    );
                }
                let roof_u = hall.0 + wu;
                block(
                    m,
                    f,
                    ((roof_u + cy.0) * 0.5, cy.1),
                    ((cy.0 - roof_u) * 0.5, 0.45),
                    y + 7.4,
                    y + 8.2,
                    STEEL,
                    STEEL,
                );
            }
            // Plank stacks in the yard: a column of six, layered with stickers when near.
            for (i, (u, v)) in [
                (10.0f32, -9.6f32),
                (13.2, -9.6),
                (10.0, -6.8),
                (13.2, -6.8),
                (10.0, -4.0),
                (13.2, -4.0),
            ]
            .into_iter()
            .enumerate()
            {
                let h = 1.2 + 0.4 * (i % 3) as f32;
                let p = (u, cv + v);
                if near {
                    let layers = (h / 0.4) as usize;
                    for l in 0..layers {
                        let y0 = y + l as f32 * 0.4;
                        block(m, f, p, (1.3, 1.0), y0, y0 + 0.08, LOG, LOG);
                        let c = if l % 2 == 0 { PLANK } else { mul(PLANK, 0.9) };
                        block(m, f, p, (1.3, 1.0), y0 + 0.08, y0 + 0.4, c, c);
                    }
                } else {
                    block(m, f, p, (1.3, 1.0), y, y + h, PLANK, PLANK);
                }
            }
            // The log pile by the street: logs along `u`, three layers.
            let lp = at(-0.5, -0.83);
            if far {
                block(m, f, lp, (3.0, 1.2), y, y + 1.2, LOG, LOG);
            } else {
                for layer in 0..3 {
                    let count = 4 - layer;
                    for k in 0..count {
                        let dv = (k as f32 - (count - 1) as f32 * 0.5) * 0.6;
                        let yy = y + 0.25 + layer as f32 * 0.45;
                        rod(
                            m,
                            f.at(lp.0 - 3.0, lp.1 + dv, yy),
                            f.at(lp.0 + 3.0, lp.1 + dv, yy),
                            0.5,
                            if near && k % 2 == 1 {
                                mul(LOG, 0.85)
                            } else {
                                LOG
                            },
                        );
                    }
                }
            }
            if near {
                // Two big doors on the street side.
                for s in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        (hall.0 + s * 0.4 * wu, hall.1 - wv),
                        (1.8, 0.05),
                        y,
                        y + 4.0,
                        DOOR,
                        DOOR,
                    );
                }
            }
        }
        Massing::ElectronicsFactory => {
            // A white block with glass bands and a coloured stripe, a glass lobby and a low
            // wing in front, antennas, dishes and solar panels on the roof, a lawn and parking.
            ground(m, 0.12, PAD);
            block(
                m,
                f,
                at(0.0, -0.8),
                (0.95 * hu, 0.15 * hv),
                y,
                y + 0.05,
                GRASS,
                GRASS,
            );
            let main = at(0.05, 0.22);
            let (wu, wv) = (0.72 * hu, 0.52 * hv);
            let top = y + 9.0;
            block(m, f, main, (wu, wv), y, top, sc.wall, sc.roof);
            block(
                m,
                f,
                main,
                (wu + 0.1, wv + 0.1),
                top - 0.1,
                top + 0.5,
                sc.wall,
                sc.wall,
            );
            block(
                m,
                f,
                main,
                (wu - 0.2, wv - 0.2),
                top + 0.4,
                top + 0.45,
                sc.roof,
                sc.roof,
            );
            band(m, f, main, (wu, wv), (y + 2.2, y + 3.6), sc.glass);
            if !far {
                band(m, f, main, (wu, wv), (y + 5.6, y + 7.0), sc.glass);
                band(m, f, main, (wu, wv), (y + 8.0, y + 8.4), sc.trim);
            }
            // The low wing and the glass lobby.
            let wing = at(0.62, -0.25);
            block(m, f, wing, (3.0, 3.6), y, y + 5.0, sc.wall, sc.roof);
            let lobby = at(-0.35, -0.42);
            block(m, f, lobby, (3.2, 2.2), y, y + 4.2, sc.glass, sc.glass);
            block(m, f, lobby, (3.8, 2.8), y + 4.2, y + 4.5, sc.wall, sc.wall);
            if !far {
                band(m, f, wing, (3.0, 3.6), (y + 2.0, y + 3.4), sc.glass);
                // Antennas: three masts of different heights.
                for (u, v, h) in [
                    (-0.55f32, 0.6f32, 6.5f32),
                    (-0.4, 0.62, 4.5),
                    (0.6, 0.62, 5.5),
                ] {
                    let p = at(u, v);
                    block(m, f, p, (0.08, 0.08), top + 0.45, top + h, STEEL, STEEL);
                    if near {
                        for k in 1..3 {
                            let yy = top + h * k as f32 / 3.0;
                            block(m, f, p, (0.5, 0.04), yy, yy + 0.06, STEEL, STEEL);
                        }
                        block(
                            m,
                            f,
                            p,
                            (0.12, 0.12),
                            top + h,
                            top + h + 0.2,
                            sc.trim,
                            sc.trim,
                        );
                    }
                }
                // Two dishes on posts.
                for u in [0.15f32, 0.4] {
                    let p = at(u, 0.55);
                    block(m, f, p, (0.12, 0.12), top + 0.45, top + 1.6, STEEL, STEEL);
                    cyl(
                        m,
                        f,
                        p,
                        (0.2, 1.3),
                        (top + 1.6, top + 2.3),
                        n,
                        TANK_WHITE,
                        Some(STEEL),
                    );
                }
            }
            if near {
                // Solar panel rows, air handlers, trees on the lawn and parked cars.
                for k in 0..4 {
                    block(
                        m,
                        f,
                        (main.0 - 0.6 * wu + k as f32 * 2.6, main.1 - 0.35 * wv),
                        (1.0, 2.6),
                        top + 0.5,
                        top + 0.8,
                        SOLAR,
                        SOLAR,
                    );
                }
                for k in 0..3 {
                    block(
                        m,
                        f,
                        (main.0 + 0.2 * wu + k as f32 * 2.4, main.1 - 0.3 * wv),
                        (0.9, 0.9),
                        top + 0.45,
                        top + 1.5,
                        STEEL,
                        mul(STEEL, 0.8),
                    );
                }
                for u in [-0.85f32, -0.6, 0.0] {
                    tree(m, f, at(u, -0.8), y, 4.0, n, near);
                }
                for k in 0..4 {
                    car(
                        m,
                        f,
                        at(0.25 + 0.15 * k as f32, -0.8),
                        y,
                        false,
                        PAINTS[k % PAINTS.len()],
                        near,
                    );
                }
            }
        }
        Massing::CarFactory => {
            // A long assembly hall with skylights and a taller paint shop, rows of new cars on
            // the lot in front.
            ground(m, 0.12, ASPHALT);
            let hall = at(-0.06, 0.38);
            let (wu, wv) = (0.86 * hu, 0.44 * hv);
            block(m, f, hall, (wu, wv), y, y + 8.0, sc.wall, sc.roof);
            let shop = at(0.86, 0.5);
            block(m, f, shop, (2.0, 0.36 * hv), y, y + 12.0, sc.wall, sc.roof);
            if !far {
                band(m, f, hall, (wu, wv), (y + 5.0, y + 6.0), sc.glass);
                band(m, f, hall, (wu, wv), (y + 6.6, y + 7.4), sc.trim);
                for k in 0..4 {
                    let v = hall.1 - wv + (k as f32 + 0.5) * 2.0 * wv / 4.0;
                    gable(
                        m,
                        f,
                        (hall.0, v),
                        (0.85 * wu, 0.6),
                        (y + 8.0, y + 8.9),
                        sc.glass,
                    );
                }
                for s in [-1.0f32, 1.0] {
                    cyl(
                        m,
                        f,
                        (shop.0, shop.1 + s * 2.0),
                        (0.45, 0.45),
                        (y + 12.0, y + 16.0),
                        n,
                        STEEL,
                        Some(DARK),
                    );
                }
            }
            // The car lot: three rows of new cars between white lines.
            if !far {
                for (r, v) in [-0.82f32, -0.55, -0.28].into_iter().enumerate() {
                    if near {
                        block(
                            m,
                            f,
                            at(0.0, v - 0.135),
                            (0.82 * hu, 0.04),
                            y,
                            y + 0.02,
                            LINE,
                            LINE,
                        );
                    }
                    for k in 0..10 {
                        let u = -0.8 + 1.6 * k as f32 / 9.0;
                        car(
                            m,
                            f,
                            at(u, v),
                            y,
                            false,
                            PAINTS[(k + 2 * r) % PAINTS.len()],
                            near,
                        );
                    }
                }
            }
            if near {
                // The doors the new cars leave by.
                for k in 0..3 {
                    block(
                        m,
                        f,
                        (hall.0 - 0.5 * wu + 0.5 * wu * k as f32, hall.1 - wv),
                        (2.0, 0.05),
                        y,
                        y + 4.0,
                        DOOR,
                        DOOR,
                    );
                }
            }
        }
        Massing::CargoStation => {
            // A gantry crane on rails across the container yard, its girders reaching the back
            // edge towards the track; the office and a truck under the crane.
            ground(m, 0.12, PAD);
            let (vf, vb) = (-0.78 * hv, 0.84 * hv);
            for v in [vf, vb] {
                block(
                    m,
                    f,
                    (-3.25, cv + v),
                    (12.25, 0.12),
                    y,
                    y + 0.1,
                    DARK,
                    STEEL,
                );
            }
            // The stacks: two rows along `u`, mostly one or two high, well under the crane (the
            // front row leaves room for the office).
            for (r, v) in [-0.3f32, 0.2].into_iter().enumerate() {
                for (k, u) in [-11.0f32, -4.6, 1.8, 8.6, 13.0].into_iter().enumerate() {
                    if u > 8.0 && r == 0 {
                        continue;
                    }
                    let high = [[1, 2, 2, 0, 0], [3, 1, 2, 2, 1]][r][k];
                    let high = if far { high.min(2) } else { high };
                    for l in 0..high {
                        container(
                            m,
                            f,
                            (u.min(12.8), cv + v * hv),
                            y + l as f32 * 2.6,
                            true,
                            PAINTS[(k + l + r * 3) % PAINTS.len()],
                        );
                    }
                }
            }
            // The crane: two portals of two legs under girders along `v`.
            let (u0, du) = (-1.4f32, 3.6f32);
            let (g0, g1) = (-0.9 * hv, 0.98 * hv);
            let gtop = y + 15.0;
            for s in [-1.0f32, 1.0] {
                let u = u0 + s * du;
                for v in [vf, vb] {
                    block(
                        m,
                        f,
                        (u, cv + v),
                        (0.35, 0.35),
                        y + 0.8,
                        gtop - 1.0,
                        sc.trim,
                        sc.trim,
                    );
                }
                block(
                    m,
                    f,
                    (u, cv + (g0 + g1) * 0.5),
                    (0.4, (g1 - g0) * 0.5),
                    gtop - 1.0,
                    gtop,
                    sc.trim,
                    mul(sc.trim, 0.9),
                );
            }
            // Bogies on the rails and the ties between the portals.
            for v in [vf, vb] {
                block(
                    m,
                    f,
                    (u0, cv + v),
                    (du + 0.6, 0.2),
                    y + 0.1,
                    y + 0.8,
                    DARK,
                    DARK,
                );
            }
            for v in [g0 + 0.3, g1 - 0.3] {
                block(
                    m,
                    f,
                    (u0, cv + v),
                    (du, 0.25),
                    gtop - 0.8,
                    gtop - 0.2,
                    sc.trim,
                    sc.trim,
                );
            }
            // The trolley on the girders.
            let tv = cv + 0.62 * hv;
            block(
                m,
                f,
                (u0, tv),
                (du + 0.5, 1.2),
                gtop,
                gtop + 1.2,
                sc.wall,
                sc.roof,
            );
            if !far {
                // Hoist cables, the spreader and the container it lifts.
                let sy = y + 6.8;
                for (a, c) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    rod(
                        m,
                        f.at(u0 + a * 2.6, tv + c * 0.9, gtop),
                        f.at(u0 + a * 2.6, tv + c * 0.9, sy + 0.4),
                        0.06,
                        DARK,
                    );
                }
                block(m, f, (u0, tv), (3.1, 1.25), sy, sy + 0.4, DARK, sc.trim);
                container(m, f, (u0, tv), sy - 2.6, true, PAINTS[1]);
                // The office by the street.
                let off = at(0.79, -0.62);
                block(m, f, off, (2.4, 1.5), y, y + 3.0, sc.wall, sc.wall);
                block(m, f, off, (2.6, 1.7), y + 3.0, y + 3.3, sc.roof, sc.roof);
            }
            if near {
                // Diagonal braces in each portal, the cab under the girder, a truck loading.
                for s in [-1.0f32, 1.0] {
                    let u = u0 + s * du;
                    rod(
                        m,
                        f.at(u, cv + vf, y + 0.8),
                        f.at(u, cv + vf + 3.0, gtop - 1.0),
                        0.2,
                        sc.trim,
                    );
                    rod(
                        m,
                        f.at(u, cv + vb, y + 0.8),
                        f.at(u, cv + vb - 3.0, gtop - 1.0),
                        0.2,
                        sc.trim,
                    );
                }
                block(
                    m,
                    f,
                    (u0 + du - 0.6, cv + vf + 1.4),
                    (0.8, 0.8),
                    gtop - 2.8,
                    gtop - 1.0,
                    sc.wall,
                    sc.wall,
                );
                block(
                    m,
                    f,
                    (u0 + du - 0.6 - 0.81, cv + vf + 1.4),
                    (0.02, 0.6),
                    gtop - 2.4,
                    gtop - 1.4,
                    CAR_GLASS,
                    CAR_GLASS,
                );
                truck(m, f, (u0 - 0.6, cv - 0.6 * hv), y, sc.trim, Some(PAINTS[4]));
                let off = at(0.79, -0.62);
                block(
                    m,
                    f,
                    (off.0, off.1 - 1.52),
                    (1.6, 0.02),
                    y + 1.2,
                    y + 2.4,
                    sc.glass,
                    sc.glass,
                );
            }
        }
        Massing::CargoPort => {
            // A quay along the back edge (the side away from the street, towards the water), a
            // ship-to-shore crane on rails with its A-frame and boom over the quay edge, the
            // apron with container stacks, a gabled shed and a light mast.
            ground(m, 0.12, PAD);
            let qv = 0.92 * hv;
            block(
                m,
                f,
                (0.0, cv + qv),
                (0.96 * hu, 0.04 * hv),
                y,
                y + 0.3,
                QUAY,
                QUAY,
            );
            let (vf, vb) = (0.3 * hv, 0.78 * hv);
            for v in [vf, vb] {
                block(
                    m,
                    f,
                    (0.0, cv + v),
                    (0.95 * hu, 0.12),
                    y,
                    y + 0.1,
                    DARK,
                    STEEL,
                );
            }
            // The crane: four legs, two girders along `v` from the backreach to the boom tip
            // over the quay edge, the machinery house, the A-frame and its stays.
            let (u0, du) = (-0.25 * hu, 3.4f32);
            let (g0, g1) = (-0.12 * hv, 0.98 * hv);
            let gtop = y + 14.5;
            let apex = (cv + 0.54 * hv, y + 20.5);
            for s in [-1.0f32, 1.0] {
                let u = u0 + s * du;
                for v in [vf, vb] {
                    block(
                        m,
                        f,
                        (u, cv + v),
                        (0.4, 0.4),
                        y + 0.8,
                        gtop - 1.0,
                        sc.trim,
                        sc.trim,
                    );
                }
                block(
                    m,
                    f,
                    (u, cv + (g0 + g1) * 0.5),
                    (0.4, (g1 - g0) * 0.5),
                    gtop - 1.0,
                    gtop,
                    sc.trim,
                    mul(sc.trim, 0.9),
                );
                if !far {
                    for v in [vf, vb] {
                        rod(
                            m,
                            f.at(u, cv + v, gtop),
                            f.at(u, apex.0, apex.1),
                            0.35,
                            sc.trim,
                        );
                    }
                    rod(
                        m,
                        f.at(u, apex.0, apex.1),
                        f.at(u, cv + g1, gtop),
                        0.12,
                        DARK,
                    );
                    rod(
                        m,
                        f.at(u, apex.0, apex.1),
                        f.at(u, cv + g0, gtop),
                        0.12,
                        DARK,
                    );
                }
            }
            for v in [vf, vb] {
                block(
                    m,
                    f,
                    (u0, cv + v),
                    (du + 0.7, 0.25),
                    y + 0.1,
                    y + 0.8,
                    DARK,
                    DARK,
                );
            }
            block(
                m,
                f,
                (u0, cv + vf),
                (du, 0.3),
                gtop - 0.9,
                gtop - 0.2,
                sc.trim,
                sc.trim,
            );
            block(
                m,
                f,
                (u0, cv + 0.02 * hv),
                (du + 0.5, 1.6),
                gtop,
                gtop + 2.2,
                sc.wall,
                sc.roof,
            );
            if !far {
                block(
                    m,
                    f,
                    (u0, apex.0),
                    (du + 0.4, 0.3),
                    apex.1 - 0.4,
                    apex.1,
                    sc.trim,
                    sc.trim,
                );
                // The trolley near the tip, the hoist and the container it lifts off the quay.
                let tv = cv + 0.86 * hv;
                block(
                    m,
                    f,
                    (u0, tv),
                    (1.6, 1.0),
                    gtop - 1.6,
                    gtop - 1.0,
                    sc.wall,
                    sc.wall,
                );
                for (a, c) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    rod(
                        m,
                        f.at(u0 + a * 1.2, tv + c * 0.7, gtop - 1.6),
                        f.at(u0 + a * 1.2, tv + c * 0.7, y + 6.4),
                        0.06,
                        DARK,
                    );
                }
                block(m, f, (u0, tv), (3.1, 1.25), y + 6.0, y + 6.4, DARK, sc.trim);
                container(m, f, (u0, tv), y + 3.4, true, PAINTS[0]);
            }
            // The shed and the stacks on the apron.
            let shed = at(-0.55, -0.6);
            let (su, sv) = (0.36 * hu, 0.28 * hv);
            block(m, f, shed, (su, sv), y, y + 6.0, sc.wall, sc.wall);
            gable(
                m,
                f,
                shed,
                (su + 0.3, sv + 0.3),
                (y + 6.0, y + 8.0),
                sc.roof,
            );
            for (r, v) in [-0.78f32, -0.52, -0.26].into_iter().enumerate() {
                for (k, u) in [5.0f32, 11.6].into_iter().enumerate() {
                    let high = [[2, 1], [3, 2], [1, 2]][r][k];
                    let high = if far { high.min(2) } else { high };
                    for l in 0..high {
                        container(
                            m,
                            f,
                            (u, cv + v * hv),
                            y + l as f32 * 2.6,
                            true,
                            PAINTS[(r + k * 2 + l) % PAINTS.len()],
                        );
                    }
                }
            }
            if !far {
                // Containers waiting on the apron under the crane.
                for (k, u) in [6.0f32, 12.6].into_iter().enumerate() {
                    container(m, f, (u, cv + 0.55 * hv), y, true, PAINTS[k + 3]);
                }
            }
            if near {
                // Fenders on the quay face and bollards along its edge.
                for k in 0..8 {
                    let u = -0.84 * hu + k as f32 * 0.24 * hu;
                    block(
                        m,
                        f,
                        (u, cv + qv + 0.04 * hv),
                        (0.5, 0.15),
                        y - 0.6,
                        y + 0.2,
                        DARK,
                        DARK,
                    );
                    cyl(
                        m,
                        f,
                        (u + 0.12 * hu, cv + qv),
                        (0.22, 0.18),
                        (y + 0.3, y + 0.8),
                        6,
                        DARK,
                        Some(DARK),
                    );
                }
                // The crane cab, the shed doors, the light mast, a truck at the stacks.
                block(
                    m,
                    f,
                    (u0 + du - 0.5, cv + 0.7 * hv),
                    (0.8, 0.8),
                    gtop - 2.8,
                    gtop - 1.0,
                    sc.wall,
                    sc.wall,
                );
                for s in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        (shed.0 + s * 0.45 * su, shed.1 - sv),
                        (1.6, 0.05),
                        y,
                        y + 3.6,
                        DOOR,
                        DOOR,
                    );
                }
                let mast = at(-0.9, 0.12);
                block(m, f, mast, (0.15, 0.15), y, y + 13.0, STEEL, STEEL);
                block(
                    m,
                    f,
                    mast,
                    (0.8, 0.3),
                    y + 13.0,
                    y + 13.5,
                    DARK,
                    [1.0, 0.95, 0.7, 1.0],
                );
                truck(
                    m,
                    f,
                    (at(0.1, -0.05).0, cv - 0.05 * hv),
                    y,
                    sc.trim,
                    Some(PAINTS[5]),
                );
            }
        }
        _ => {}
    }
}
