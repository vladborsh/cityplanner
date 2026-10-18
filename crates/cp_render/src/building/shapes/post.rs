//! The post silhouettes (T-248): the post office, a red brick hall with a yellow sign bearing
//! an envelope over its door, a letterbox by the steps and a loading bay with a mail van at the
//! side; and the sorting facility, a big clad shed with a row of loading docks, mail trucks
//! backed up to them, vans parked in the yard and a brick office at the corner.

use cp_content::Massing;

use super::theme_park::{mul, rod};
use super::{DARK, ServiceCx, block, cyl};
use crate::building::{Frame, Lod, lights};
use crate::chunk::MeshData;

/// The post livery: yellow with a red band.
pub const POST_YELLOW: [f32; 4] = [0.95, 0.74, 0.12, 1.0];
pub const POST_RED: [f32; 4] = [0.78, 0.10, 0.08, 1.0];
const PAD: [f32; 4] = [0.58, 0.58, 0.56, 1.0];
const ASPHALT: [f32; 4] = [0.30, 0.31, 0.33, 1.0];
const LINE: [f32; 4] = [0.92, 0.92, 0.88, 1.0];
const DOOR: [f32; 4] = [0.32, 0.34, 0.37, 1.0];
const VAN_GLASS: [f32; 4] = [0.16, 0.20, 0.26, 1.0];
const WHITE: [f32; 4] = [0.95, 0.95, 0.93, 1.0];
const BRICK: [f32; 4] = [0.58, 0.27, 0.21, 1.0];

/// Whether `m` is one of the post silhouettes of this module.
pub fn is_post(m: Massing) -> bool {
    matches!(m, Massing::PostOffice | Massing::PostSortingFacility)
}

/// A mail vehicle standing at `(u, v)` from `y`, its front towards `front` (a unit axis of
/// the lot, `(±1, 0)` or `(0, ±1)`): a yellow body `len` metres long with a red band, a
/// windscreen and, near, dark wheels. `truck`: a cab and a taller box behind it.
#[allow(clippy::too_many_arguments)]
fn mail_vehicle(
    m: &mut MeshData,
    f: &Frame,
    (u, v): (f32, f32),
    y: f32,
    front: (f32, f32),
    len: f32,
    truck: bool,
    near: bool,
) {
    let half = |l: f32, w: f32| {
        if front.0 != 0.0 { (l, w) } else { (w, l) }
    };
    let at = |s: f32| (u + front.0 * s, v + front.1 * s);
    let w = 1.0;
    let l = len * 0.5;
    if truck {
        // Cab in front, the box behind it.
        let cab = 1.0;
        block(
            m,
            f,
            at(l - cab),
            half(cab, w),
            y + 0.4,
            y + 2.4,
            POST_YELLOW,
            POST_YELLOW,
        );
        block(
            m,
            f,
            at(-cab),
            half(l - cab, w + 0.05),
            y + 0.5,
            y + 3.3,
            POST_YELLOW,
            WHITE,
        );
        block(
            m,
            f,
            at(-cab),
            half(l - cab + 0.02, w + 0.08),
            y + 1.6,
            y + 2.0,
            POST_RED,
            POST_RED,
        );
        if near {
            block(
                m,
                f,
                at(l - 0.02),
                half(0.04, w - 0.1),
                y + 1.5,
                y + 2.2,
                VAN_GLASS,
                VAN_GLASS,
            );
        }
    } else {
        // A high-roofed van: the body, the bonnet lower in front.
        block(
            m,
            f,
            at(-0.4),
            half(l - 0.4, w),
            y + 0.35,
            y + 2.3,
            POST_YELLOW,
            POST_YELLOW,
        );
        block(
            m,
            f,
            at(l - 0.4),
            half(0.4, w),
            y + 0.35,
            y + 1.2,
            POST_YELLOW,
            POST_YELLOW,
        );
        block(
            m,
            f,
            at(-0.4),
            half(l - 0.4 + 0.02, w + 0.03),
            y + 1.0,
            y + 1.3,
            POST_RED,
            POST_RED,
        );
        if near {
            block(
                m,
                f,
                at(l - 0.82),
                half(0.04, w - 0.08),
                y + 1.3,
                y + 2.1,
                VAN_GLASS,
                VAN_GLASS,
            );
        }
    }
    if near {
        for s in [l - 1.0, -l + 1.0] {
            block(m, f, at(s), half(0.35, w + 0.04), y, y + 0.6, DARK, DARK);
        }
    }
}

/// A red letterbox (pillar box) at `(u, v)` from `y`: a post with a rounded red box on top.
fn letterbox(m: &mut MeshData, f: &Frame, at: (f32, f32), y: f32) {
    cyl(
        m,
        f,
        at,
        (0.28, 0.28),
        (y, y + 1.25),
        8,
        POST_RED,
        Some(POST_RED),
    );
    cyl(
        m,
        f,
        at,
        (0.3, 0.3),
        (y + 1.25, y + 1.38),
        8,
        mul(POST_RED, 0.7),
        Some(mul(POST_RED, 0.7)),
    );
}

/// A sign board on the wall facing `-v` at `v_wall`: yellow, `hw` metres half wide, from `y0`
/// to `y1`, with a white envelope on it (near: its flap drawn as a dark V). `lit`: it glows at
/// night (see `lights::glow_alpha`).
#[allow(clippy::too_many_arguments)]
fn envelope_sign(
    m: &mut MeshData,
    f: &Frame,
    u: f32,
    v_wall: f32,
    hw: f32,
    (y0, y1): (f32, f32),
    near: bool,
    lit: bool,
) {
    let glow = |c: [f32; 4], k: f32| {
        if lit {
            [c[0], c[1], c[2], lights::glow_alpha(None, 2, k)]
        } else {
            c
        }
    };
    block(
        m,
        f,
        (u, v_wall - 0.1),
        (hw, 0.1),
        y0,
        y1,
        glow(POST_YELLOW, 0.25),
        POST_YELLOW,
    );
    let (eh, ey) = (0.3 * (y1 - y0), 0.5 * (y0 + y1));
    let ew = (eh * 1.5).min(hw * 0.8);
    block(
        m,
        f,
        (u, v_wall - 0.22),
        (ew, 0.04),
        ey - eh,
        ey + eh,
        glow(WHITE, 0.5),
        WHITE,
    );
    if near {
        let p = |du: f32, y: f32| f.at(u + du, v_wall - 0.27, y);
        let col = mul(POST_RED, 0.8);
        let w = (0.12 * eh).max(0.06);
        rod(m, p(-ew, ey + eh - 0.02), p(0.0, ey), w, col);
        rod(m, p(0.0, ey), p(ew, ey + eh - 0.02), w, col);
    }
}

pub fn post(m: &mut MeshData, massing: Massing, sc: &ServiceCx) {
    let (f, b) = (sc.f, sc.base);
    let (hu, hv, cv) = (sc.hu, sc.hv, sc.cv);
    let near = sc.lod == Lod::Near;
    let far = sc.lod == Lod::Far;
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
    // Lot-relative point: `(u, v)` in -1..1 of the half extents; the street is at -v.
    let at = |u: f32, v: f32| (u * hu, cv + v * hv);
    let y = b + 0.12;
    match massing {
        Massing::PostOffice => {
            ground(m, 0.12, PAD);
            // The hall: brick, a cream cornice, a tall window band on the street side.
            let hall = at(-0.2, 0.15);
            let (wu, wv) = (0.72 * hu, 0.62 * hv);
            let top = y + 5.6;
            block(m, f, hall, (wu, wv), y, top, sc.wall, sc.roof);
            block(
                m,
                f,
                hall,
                (wu + 0.25, wv + 0.25),
                top,
                top + 0.5,
                sc.trim,
                sc.roof,
            );
            let front = hall.1 - wv;
            if !far {
                for k in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        (hall.0 + k * 0.55 * wu, front - 0.04),
                        (0.28 * wu, 0.05),
                        y + 1.0,
                        y + 3.6,
                        sc.glass,
                        sc.trim,
                    );
                }
            }
            // The door under a canopy, the yellow sign above it.
            block(
                m,
                f,
                (hall.0, front - 0.05),
                (0.9, 0.06),
                y,
                y + 2.5,
                DOOR,
                DOOR,
            );
            envelope_sign(
                m,
                f,
                hall.0,
                front,
                0.32 * wu,
                (y + 3.4, top - 0.4),
                !far,
                sc.lit,
            );
            if !far {
                block(
                    m,
                    f,
                    (hall.0, front - 0.8),
                    (1.4, 0.8),
                    y + 2.6,
                    y + 2.8,
                    sc.trim,
                    sc.trim,
                );
            }
            // The loading bay at the side: a lower annex with a roll-up door and the van.
            let bay = at(0.74, 0.35);
            let (bu, bv) = (0.22 * hu, 0.42 * hv);
            block(m, f, bay, (bu, bv), y, y + 3.6, mul(sc.wall, 0.9), sc.roof);
            block(
                m,
                f,
                (bay.0, bay.1 - bv - 0.04),
                (bu * 0.75, 0.05),
                y,
                y + 2.8,
                DOOR,
                DOOR,
            );
            block(
                m,
                f,
                (bay.0, bay.1 - bv - 0.04),
                (bu * 0.8, 0.07),
                y + 2.8,
                y + 3.0,
                POST_YELLOW,
                POST_YELLOW,
            );
            block(
                m,
                f,
                at(0.74, -0.45),
                (bu, 0.45 * hv),
                y,
                y + 0.03,
                ASPHALT,
                ASPHALT,
            );
            if !far {
                mail_vehicle(m, f, at(0.74, -0.38), y, (0.0, 1.0), 5.0, false, near);
            }
            if near {
                letterbox(m, f, (hall.0 + 1.9, front - 1.1), y);
            }
        }
        Massing::PostSortingFacility => {
            ground(m, 0.12, PAD);
            // The yard in front of the docks.
            block(
                m,
                f,
                at(0.0, -0.66),
                (0.94 * hu, 0.3 * hv),
                y,
                y + 0.03,
                ASPHALT,
                ASPHALT,
            );
            // The shed: clad walls, a yellow band under the eaves, a low gable roof.
            let shed = at(0.08, 0.35);
            let (wu, wv) = (0.84 * hu, 0.58 * hv);
            let top = y + 9.0;
            block(m, f, shed, (wu, wv), y, top, sc.wall, sc.roof);
            super::park_area::gable(m, f, shed, (wu + 0.2, wv + 0.2), (top, top + 1.6), sc.roof);
            block(
                m,
                f,
                shed,
                (wu + 0.06, wv + 0.06),
                top - 1.2,
                top - 0.5,
                sc.trim,
                sc.trim,
            );
            let front = shed.1 - wv;
            if !far {
                // Clerestory windows high on the front and back.
                for s in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        (shed.0, shed.1 + s * (wv + 0.03)),
                        (wu * 0.9, 0.05),
                        top - 3.2,
                        top - 2.0,
                        sc.glass,
                        sc.glass,
                    );
                }
            }
            // The office at the street corner: two floors of brick with a window band.
            let off = at(-0.72, -0.42);
            let (ou, ov) = (0.2 * hu, 0.2 * hv);
            block(m, f, off, (ou, ov), y, y + 7.0, BRICK, sc.roof);
            if !far {
                for fl in [0.0f32, 1.0] {
                    let ya = y + 1.2 + 3.3 * fl;
                    block(
                        m,
                        f,
                        off,
                        (ou + 0.05, ov + 0.05),
                        ya,
                        ya + 1.4,
                        sc.glass,
                        sc.glass,
                    );
                }
            }
            envelope_sign(
                m,
                f,
                shed.0 + 0.2 * wu,
                front,
                3.2,
                (top - 4.6, top - 1.6),
                !far,
                sc.lit,
            );
            // The docks: dark doors with yellow frames along the front, mail trucks backed up
            // to every other one, vans parked across the yard.
            let docks = 6;
            let d0 = shed.0 - wu + 0.25 * wu;
            let step = 1.5 * wu / (docks - 1) as f32;
            for k in 0..docks {
                let u = d0 + step * k as f32;
                block(
                    m,
                    f,
                    (u, front - 0.05),
                    (1.5, 0.08),
                    y + 1.1,
                    y + 4.4,
                    POST_YELLOW,
                    POST_YELLOW,
                );
                block(
                    m,
                    f,
                    (u, front - 0.09),
                    (1.3, 0.06),
                    y + 1.2,
                    y + 4.2,
                    DOOR,
                    DOOR,
                );
                if !far {
                    // The dock leveller, a step at truck bed height.
                    block(
                        m,
                        f,
                        (u, front - 0.7),
                        (1.4, 0.7),
                        y,
                        y + 1.1,
                        mul(PAD, 0.85),
                        mul(PAD, 0.95),
                    );
                    if near && k > 0 {
                        // The bay lines between the docks.
                        let lu = u - 0.5 * step;
                        block(
                            m,
                            f,
                            (lu, front - 4.0),
                            (0.08, 3.2),
                            y,
                            y + 0.05,
                            LINE,
                            LINE,
                        );
                    }
                }
                if !far && k % 2 == 1 {
                    mail_vehicle(
                        m,
                        f,
                        (u, front - 1.4 - 3.5),
                        y,
                        (0.0, -1.0),
                        7.0,
                        true,
                        near,
                    );
                }
            }
            if !far {
                for k in 0..3 {
                    mail_vehicle(
                        m,
                        f,
                        at(-0.3 + 0.4 * k as f32, -0.9),
                        y,
                        (1.0, 0.0),
                        5.0,
                        false,
                        near,
                    );
                }
            }
        }
        _ => {}
    }
}
