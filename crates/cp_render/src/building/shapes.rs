//! Massing plans, the extra roofs and the details of a building (`system-design.md` §8.2).
//!
//! A massing is a short list of [`Vol`]s: boxes inside the lot that never overlap (stacked
//! ones touch at a floor, side-by-side ones share at most a wall). Each box is built with the
//! same wall tiling as the single-box building; a wall covered by a neighbour is cut away
//! (`vis`), so no two surfaces coincide. Details are `near`-only.

use bevy_math::Vec3;
use cp_content::{Detail, Massing};
use cp_terrain::Terrain;

use super::{Ctx, Frame, mul};
use crate::chunk::MeshData;
use crate::road::{flat_quad, push_vertex, tri};

mod factory;
mod park_area;
mod post;
mod theme_park;

/// Which part `(t0, t1)` of each of the four walls (front, right, back, left; corners
/// counter-clockwise from above) is not hidden by a neighbouring volume.
pub type Vis = [Option<(f32, f32)>; 4];

const FULL: Vis = [Some((0.0, 1.0)); 4];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RoofMode {
    /// The style's roof.
    Style,
    /// A bare flat cap: something stands on it.
    Plain,
    /// Sawtooth teeth.
    Saw,
}

/// One box of a massing, in lot-frame metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vol {
    pub cu: f32,
    pub cv: f32,
    pub hu: f32,
    pub hv: f32,
    /// Floors below this box (the stack under it).
    pub below: u32,
    pub floors: u32,
    pub vis: Vis,
    pub roof: RoofMode,
    /// The front door / shopfront belongs to this box.
    pub front: bool,
}

pub struct Plan {
    pub vols: Vec<Vol>,
    /// The box whose roof carries roof details.
    pub top: usize,
    /// The box whose front wall carries facade details.
    pub front: usize,
}

/// The boxes of `massing` for a lot volume of half extents `hu`, `hv` centred at `cv`.
/// Shapes that do not fit the floor count fall back to the plain box.
pub fn plan(massing: Massing, floors: u32, hu: f32, hv: f32, cv: f32) -> Plan {
    let floors = floors.max(1);
    let one = |roof| Vol {
        cu: 0.0,
        cv,
        hu,
        hv,
        below: 0,
        floors,
        vis: FULL,
        roof,
        front: true,
    };
    let single = |roof| Plan {
        vols: vec![one(roof)],
        top: 0,
        front: 0,
    };
    match massing {
        Massing::LShape => {
            let wing = Vol {
                cu: -0.5 * hu,
                cv: cv - 0.5 * hv,
                hu: 0.5 * hu,
                hv: 0.5 * hv,
                vis: [FULL[0], FULL[1], None, FULL[3]],
                ..one(RoofMode::Style)
            };
            let bar = Vol {
                cu: 0.0,
                cv: cv + 0.5 * hv,
                hu,
                hv: 0.5 * hv,
                vis: [Some((0.5, 1.0)), FULL[1], FULL[2], FULL[3]],
                front: false,
                ..one(RoofMode::Style)
            };
            Plan {
                vols: vec![wing, bar],
                top: 1,
                front: 0,
            }
        }
        Massing::Stepped if floors >= 3 => {
            let tiers = if floors >= 6 { 3 } else { 2 };
            let mut vols = Vec::new();
            let mut below = 0;
            for i in 0..tiers {
                let s = 1.0 - 0.22 * i as f32;
                let fl = floors / tiers + u32::from(i < floors % tiers);
                vols.push(Vol {
                    cu: 0.0,
                    cv: cv + (1.0 - s) * hv * 0.3,
                    hu: hu * s,
                    hv: hv * s,
                    below,
                    floors: fl,
                    vis: FULL,
                    roof: if i + 1 == tiers {
                        RoofMode::Style
                    } else {
                        RoofMode::Plain
                    },
                    front: i == 0,
                });
                below += fl;
            }
            Plan {
                top: vols.len() - 1,
                vols,
                front: 0,
            }
        }
        Massing::TowerOnPodium if floors >= 3 => {
            let p = (floors / 4).clamp(1, 2);
            let podium = Vol {
                floors: p,
                ..one(RoofMode::Plain)
            };
            let tower = Vol {
                cu: 0.0,
                cv: cv + 0.3 * hv,
                hu: 0.6 * hu,
                hv: 0.6 * hv,
                below: p,
                floors: floors - p,
                vis: FULL,
                roof: RoofMode::Style,
                front: false,
            };
            Plan {
                vols: vec![podium, tower],
                top: 1,
                front: 0,
            }
        }
        Massing::TwinBlocks => {
            let a = Vol {
                cu: -0.55 * hu,
                hu: 0.45 * hu,
                ..one(RoofMode::Style)
            };
            let b = Vol {
                cu: 0.55 * hu,
                hu: 0.45 * hu,
                floors: ((floors * 3 + 2) / 4).max(1),
                ..one(RoofMode::Style)
            };
            Plan {
                vols: vec![a, b],
                top: 0,
                front: 0,
            }
        }
        Massing::Sawtooth => single(RoofMode::Saw),
        // The service massings are T-114; they draw as a plain box until then.
        _ => single(RoofMode::Style),
    }
}

// ---------------------------------------------------------------------------------------
// Roofs

fn n_up(side: Vec3, run: f32, rise: f32) -> Vec3 {
    (side * rise + Vec3::Y * run).normalize_or_zero()
}

/// One slope from the front eave up to the back wall, with the back wall and the two end
/// triangles closing the high side.
pub fn shed(m: &mut MeshData, cx: &Ctx, y: f32) {
    let c = |su: f32, sv: f32, yy: f32| cx.corner(su, sv, yy);
    let rise = (cx.hv * 0.5).clamp(1.0, 3.5);
    let n = n_up(cx.f.dir(0.0, -1.0), 2.0 * cx.hv, rise);
    flat_quad(
        m,
        [
            c(-1.0, -1.0, y),
            c(1.0, -1.0, y),
            c(1.0, 1.0, y + rise),
            c(-1.0, 1.0, y + rise),
        ],
        n,
        cx.roof,
    );
    let back = cx.f.dir(0.0, 1.0).normalize_or_zero();
    flat_quad(
        m,
        [
            c(-1.0, 1.0, y),
            c(1.0, 1.0, y),
            c(1.0, 1.0, y + rise),
            c(-1.0, 1.0, y + rise),
        ],
        back,
        cx.wall,
    );
    for s in [-1.0f32, 1.0] {
        let out = cx.f.dir(s, 0.0).normalize_or_zero();
        let v = [
            push_vertex(m, c(s, -1.0, y), out, cx.wall),
            push_vertex(m, c(s, 1.0, y), out, cx.wall),
            push_vertex(m, c(s, 1.0, y + rise), out, cx.wall),
        ];
        tri(m, v[0], v[1], v[2], out);
    }
}

/// A steep skirt around a flat top.
pub fn mansard(m: &mut MeshData, cx: &Ctx, y: f32) {
    let inset = (cx.hu.min(cx.hv) * 0.35).max(0.3);
    let (iu, iv) = (
        (1.0 - inset / cx.hu).max(0.1),
        (1.0 - inset / cx.hv).max(0.1),
    );
    let rise = (cx.hu.min(cx.hv) * 0.5).clamp(1.0, 3.0);
    let c = |su: f32, sv: f32, yy: f32| cx.corner(su, sv, yy);
    let ring = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    for k in 0..4 {
        let (a, b) = (ring[k], ring[(k + 1) % 4]);
        let out =
            cx.f.dir((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
                .normalize_or_zero();
        flat_quad(
            m,
            [
                c(a.0, a.1, y),
                c(b.0, b.1, y),
                c(b.0 * iu, b.1 * iv, y + rise),
                c(a.0 * iu, a.1 * iv, y + rise),
            ],
            n_up(out, inset, rise),
            cx.roof,
        );
    }
    flat_quad(
        m,
        [
            c(-iu, -iv, y + rise),
            c(iu, -iv, y + rise),
            c(iu, iv, y + rise),
            c(-iu, iv, y + rise),
        ],
        Vec3::Y,
        mul(cx.roof, 0.92),
    );
}

/// Teeth with their ridges along `u`: a gentle slope to the back, then a glazed rise.
pub fn sawtooth(m: &mut MeshData, cx: &Ctx, y: f32) {
    let teeth = ((cx.hv * 2.0 / 6.0).round() as u32).clamp(2, 6);
    let rise = 1.6f32;
    let c = |su: f32, sv: f32, yy: f32| cx.corner(su, sv, yy);
    let step = 2.0 / teeth as f32;
    let back = cx.f.dir(0.0, 1.0).normalize_or_zero();
    for i in 0..teeth {
        let (a, b) = (-1.0 + step * i as f32, -1.0 + step * (i + 1) as f32);
        let n = n_up(cx.f.dir(0.0, -1.0), step * cx.hv, rise);
        flat_quad(
            m,
            [
                c(-1.0, a, y),
                c(1.0, a, y),
                c(1.0, b, y + rise),
                c(-1.0, b, y + rise),
            ],
            n,
            cx.roof,
        );
        flat_quad(
            m,
            [
                c(-1.0, b, y),
                c(1.0, b, y),
                c(1.0, b, y + rise),
                c(-1.0, b, y + rise),
            ],
            back,
            cx.glass,
        );
        for s in [-1.0f32, 1.0] {
            let out = cx.f.dir(s, 0.0).normalize_or_zero();
            let v = [
                push_vertex(m, c(s, a, y), out, cx.wall),
                push_vertex(m, c(s, b, y), out, cx.wall),
                push_vertex(m, c(s, b, y + rise), out, cx.wall),
            ];
            tri(m, v[0], v[1], v[2], out);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Details (near only)

/// An upright box in lot-frame metres: four sides and a top.
#[allow(clippy::too_many_arguments)]
pub fn block(
    m: &mut MeshData,
    f: &Frame,
    (cu, cv): (f32, f32),
    (hu, hv): (f32, f32),
    y0: f32,
    y1: f32,
    side: [f32; 4],
    top: [f32; 4],
) {
    let c = |su: f32, sv: f32, y: f32| f.at(cu + su * hu, cv + sv * hv, y);
    let ring = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    for k in 0..4 {
        let (a, b) = (ring[k], ring[(k + 1) % 4]);
        let n = f
            .dir((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
            .normalize_or_zero();
        flat_quad(
            m,
            [
                c(a.0, a.1, y0),
                c(b.0, b.1, y0),
                c(b.0, b.1, y1),
                c(a.0, a.1, y1),
            ],
            n,
            side,
        );
    }
    flat_quad(
        m,
        [
            c(-1.0, -1.0, y1),
            c(1.0, -1.0, y1),
            c(1.0, 1.0, y1),
            c(-1.0, 1.0, y1),
        ],
        Vec3::Y,
        top,
    );
}

/// An 8-sided upright cylinder.
fn tank(
    m: &mut MeshData,
    f: &Frame,
    (cu, cv): (f32, f32),
    r: f32,
    y0: f32,
    y1: f32,
    col: [f32; 4],
) {
    const N: usize = 8;
    let p = |k: usize, y: f32| {
        let a = k as f32 / N as f32 * std::f32::consts::TAU;
        f.at(cu + r * a.cos(), cv + r * a.sin(), y)
    };
    for k in 0..N {
        let mid = (k as f32 + 0.5) / N as f32 * std::f32::consts::TAU;
        let n = f.dir(mid.cos(), mid.sin()).normalize_or_zero();
        flat_quad(m, [p(k, y0), p(k + 1, y0), p(k + 1, y1), p(k, y1)], n, col);
    }
    let centre = push_vertex(m, f.at(cu, cv, y1), Vec3::Y, mul(col, 1.1));
    let ring: Vec<u32> = (0..N)
        .map(|k| push_vertex(m, p(k, y1), Vec3::Y, mul(col, 1.1)))
        .collect();
    for k in 0..N {
        tri(m, centre, ring[k], ring[(k + 1) % N], Vec3::Y);
    }
}

/// What the detail builders need to know about the building.
pub struct DetailCx<'a> {
    pub f: &'a Frame,
    pub terrain: &'a Terrain,
    pub vols: &'a [Vol],
    pub plan_top: usize,
    pub plan_front: usize,
    pub base: f32,
    pub floor_h: f32,
    pub bay: f32,
    pub lot: (f32, f32),
    pub roof_kind: cp_content::RoofKind,
    pub wall: [f32; 4],
    pub trim: [f32; 4],
    pub seed: u32,
}

fn accent(seed: u32) -> [f32; 4] {
    const A: [[f32; 4]; 4] = [
        [0.62, 0.16, 0.14, 1.0],
        [0.18, 0.36, 0.56, 1.0],
        [0.20, 0.46, 0.28, 1.0],
        [0.78, 0.60, 0.16, 1.0],
    ];
    A[(seed >> 5) as usize % 4]
}

fn rise_of(v: &Vol, kind: cp_content::RoofKind) -> f32 {
    use cp_content::RoofKind::*;
    match kind {
        Gabled | Hipped => (v.hu.min(v.hv) * 0.8).clamp(1.2, 4.5),
        _ => 0.0,
    }
}

/// Draws `details` for the volumes of `dc`.
pub fn details(m: &mut MeshData, dc: &DetailCx, list: &[Detail]) {
    let f = dc.f;
    let top = &dc.vols[dc.plan_top];
    let ty = dc.base + (top.below + top.floors) as f32 * dc.floor_h;
    let flat = matches!(dc.roof_kind, cp_content::RoofKind::Flat);
    let front = &dc.vols[dc.plan_front];
    let fy0 = dc.base + front.below as f32 * dc.floor_h;
    let front_full = front.vis[0] == Some((0.0, 1.0));
    let front_v = front.cv - front.hv;
    let dark = mul(dc.trim, 0.7);
    for d in list {
        match d {
            Detail::Chimney => {
                let r = rise_of(top, dc.roof_kind);
                block(
                    m,
                    f,
                    (top.cu + 0.55 * top.hu, top.cv),
                    (0.4, 0.4),
                    ty,
                    ty + r + 1.3,
                    dark,
                    mul(dc.trim, 0.5),
                );
            }
            Detail::RooftopUnits if flat => {
                let u = [
                    ((0.3, 0.5), (0.2, 0.2), 1.4),
                    ((0.3, -0.5), (0.15, 0.12), 2.2),
                ];
                for ((pu, pv), (su, sv), h) in u {
                    block(
                        m,
                        f,
                        (top.cu + pu * top.hu, top.cv + pv * top.hv),
                        (su * top.hu, sv * top.hv),
                        ty,
                        ty + h,
                        mul(dc.trim, 0.9),
                        dc.trim,
                    );
                }
            }
            Detail::Tanks if flat => {
                let r = (0.2 * top.hu).min(0.2 * top.hv).min(2.0);
                for sv in [-0.45f32, 0.45] {
                    tank(
                        m,
                        f,
                        (top.cu + 0.7 * top.hu, top.cv + sv * top.hv),
                        r,
                        ty,
                        ty + 3.0,
                        mul(dc.wall, 0.9),
                    );
                }
            }
            Detail::Sign => {
                let half = (0.3 * front.hu).min(2.5);
                let y = dc.base + (front.below + front.floors) as f32 * dc.floor_h;
                block(
                    m,
                    f,
                    (front.cu, front_v + 0.2),
                    (half, 0.15),
                    y,
                    y + 1.0,
                    accent(dc.seed),
                    mul(accent(dc.seed), 0.8),
                );
            }
            Detail::Awning if front_full => {
                let half = (front.hu - 0.4).max(0.5);
                let (y_hi, y_lo, out) = (fy0 + 3.0, fy0 + 2.6, 1.0);
                let c = |su: f32, o: f32, y: f32| f.at(front.cu + su * half, front_v - o, y);
                let col = accent(dc.seed);
                let n = (Vec3::Y * out + f.dir(0.0, -1.0) * (y_hi - y_lo)).normalize_or_zero();
                flat_quad(
                    m,
                    [
                        c(-1.0, 0.0, y_hi),
                        c(1.0, 0.0, y_hi),
                        c(1.0, out, y_lo),
                        c(-1.0, out, y_lo),
                    ],
                    n,
                    col,
                );
                for s in [-1.0f32, 1.0] {
                    let side = f.dir(s, 0.0).normalize_or_zero();
                    let v = [
                        push_vertex(m, c(s, 0.0, y_hi), side, mul(col, 0.8)),
                        push_vertex(m, c(s, 0.0, y_lo), side, mul(col, 0.8)),
                        push_vertex(m, c(s, out, y_lo), side, mul(col, 0.8)),
                    ];
                    tri(m, v[0], v[1], v[2], side);
                }
            }
            Detail::Balconies if front_full && front.floors >= 2 => {
                let width = front.hu * 2.0;
                let bays = ((width / dc.bay).round() as u32).max(1);
                let bw = width / bays as f32;
                let mut left = 10;
                'floors: for fl in 1..front.floors {
                    for i in (1..bays).step_by(2) {
                        if left == 0 {
                            break 'floors;
                        }
                        left -= 1;
                        let cu = front.cu - front.hu + bw * (i as f32 + 0.5);
                        let y = fy0 + fl as f32 * dc.floor_h;
                        block(
                            m,
                            f,
                            (cu, front_v - 0.6),
                            ((bw * 0.35).max(0.3), 0.6),
                            y,
                            y + 0.18,
                            mul(dc.wall, 0.85),
                            dc.trim,
                        );
                    }
                }
            }
            Detail::LoadingDock => {
                // On a right-hand wall that no neighbour covers; 0.8 m deep, inside the 1 m
                // side margin.
                if let Some(v) = dc.vols.iter().find(|v| v.vis[1] == Some((0.0, 1.0))) {
                    let by = dc.base + v.below as f32 * dc.floor_h;
                    let half = (0.25 * v.hv).min(2.0);
                    let cvv = v.cv + 0.2 * v.hv;
                    block(
                        m,
                        f,
                        (v.cu + v.hu + 0.4, cvv),
                        (0.4, half),
                        by,
                        by + 1.1,
                        mul(dc.wall, 0.8),
                        dc.trim,
                    );
                    let n = f.dir(1.0, 0.0).normalize_or_zero();
                    let c = |sv: f32, y: f32| f.at(v.cu + v.hu, cvv + sv * half, y);
                    flat_quad(
                        m,
                        [
                            c(-1.0, by + 1.1),
                            c(1.0, by + 1.1),
                            c(1.0, by + 3.3),
                            c(-1.0, by + 3.3),
                        ],
                        n,
                        mul(dc.trim, 0.35),
                    );
                }
            }
            Detail::Fence => {
                // Front and the two sides, 0.3 m inside the lot edge, one board per cell.
                let (hw, hd) = (dc.lot.0 * 0.5 - 0.3, dc.lot.1 * 0.5 - 0.3);
                let board = mul(dc.wall, 0.8);
                let ground = |u: f32, v: f32| dc.terrain.height(f.at(u, v, 0.0).xz_pair());
                let mut run = |a: (f32, f32), b: (f32, f32), n: Vec3| {
                    let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
                    let segs = ((len / 8.0).ceil() as u32).max(1);
                    for s in 0..segs {
                        let (t0, t1) = (s as f32 / segs as f32, (s + 1) as f32 / segs as f32);
                        let p = |t: f32, h: f32| {
                            let (u, v) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                            f.at(u, v, ground(u, v) + h)
                        };
                        {
                            let nn = n;
                            flat_quad(
                                m,
                                [p(t0, 0.4), p(t1, 0.4), p(t1, 1.1), p(t0, 1.1)],
                                nn,
                                board,
                            );
                        }
                    }
                };
                run((-hw, -hd), (hw, -hd), f.dir(0.0, -1.0).normalize_or_zero());
                run((hw, -hd), (hw, hd), f.dir(1.0, 0.0).normalize_or_zero());
                run((-hw, hd), (-hw, -hd), f.dir(-1.0, 0.0).normalize_or_zero());
            }
            Detail::Pool | Detail::Driveway => {
                yard(m, dc, *d);
            }
            _ => {}
        }
    }
}

/// Where the pool of a detached house goes (T-296): beside the house, in the wider of the two
/// side margins when it is at least 7 m deep; `((u, v), (half u, half v))` in lot metres. A
/// pure function of the lot (`w` by `d` m) and the house's half extents, so the garden can keep
/// its plants off it.
pub fn pool_rect(
    w: f32,
    d: f32,
    (hu, hv, cv): (f32, f32, f32),
) -> Option<((f32, f32), (f32, f32))> {
    let margin = w * 0.5 - hu;
    if margin < 7.0 || d < 12.0 {
        return None;
    }
    let half_u = (margin * 0.5 - 1.5).min(3.2);
    let half_v = (hv * 0.8).min(5.0).min(d * 0.5 - 2.0);
    (half_u > 1.0 && half_v > 1.5).then_some(((hu + margin * 0.5, cv), (half_u, half_v)))
}

/// The yard of a detached house (T-296): a pool beside it or a drive in front. Laid out from
/// the boxes of the house, so a bigger lot (a higher level) gets the bigger yard.
fn yard(m: &mut MeshData, dc: &DetailCx, d: Detail) {
    let f = dc.f;
    let ld = dc.lot.1 * 0.5;
    let (mut u0, mut u1, mut v0, mut v1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for v in dc.vols {
        u0 = u0.min(v.cu - v.hu);
        u1 = u1.max(v.cu + v.hu);
        v0 = v0.min(v.cv - v.hv);
        v1 = v1.max(v.cv + v.hv);
    }
    let y = dc.base;
    match d {
        Detail::Pool => {
            let hu = u1.max(-u0);
            if let Some(((cu, cv), (pu, pv))) =
                pool_rect(dc.lot.0, dc.lot.1, (hu, (v1 - v0) * 0.5, (v0 + v1) * 0.5))
            {
                block(
                    m,
                    f,
                    (cu, cv),
                    (pu + 0.4, pv + 0.4),
                    y,
                    y + 0.25,
                    [0.78, 0.78, 0.74, 1.0],
                    [0.84, 0.84, 0.80, 1.0],
                );
                block(
                    m,
                    f,
                    (cu, cv),
                    (pu, pv),
                    y,
                    y + 0.3,
                    [0.10, 0.38, 0.62, 1.0],
                    [0.12, 0.45, 0.72, 1.0],
                );
            }
        }
        _ => {
            // From the front edge to the house, beside the door.
            let cu = (u0 + u1) * 0.5 + 0.2 * (u1 - u0);
            let c = |u: f32, v: f32| f.at(u, v, y + 0.14);
            flat_quad(
                m,
                [
                    c(cu - 1.3, -ld),
                    c(cu + 1.3, -ld),
                    c(cu + 1.3, v0 + 0.5),
                    c(cu - 1.3, v0 + 0.5),
                ],
                Vec3::Y,
                [0.56, 0.55, 0.53, 1.0],
            );
        }
    }
}

trait XzPair {
    fn xz_pair(self) -> bevy_math::Vec2;
}

impl XzPair for Vec3 {
    fn xz_pair(self) -> bevy_math::Vec2 {
        bevy_math::Vec2::new(self.x, self.z)
    }
}

// ---------------------------------------------------------------------------------------
// Service buildings (T-114): one silhouette per service massing

/// Whether `m` is one of the service silhouettes (built by [`service`], not from boxes).
pub fn is_service(m: Massing) -> bool {
    matches!(
        m,
        Massing::PumpHouse
            | Massing::Outflow
            | Massing::CoalPlant
            | Massing::WindTurbine
            | Massing::WaterTower
            | Massing::TreatmentPlant
            | Massing::Landfill
            | Massing::Incinerator
            | Massing::Clinic
            | Massing::Hospital
            | Massing::Cemetery
            | Massing::Crematorium
            | Massing::School
            | Massing::HighSchool
            | Massing::University
            | Massing::FireStation
            | Massing::PoliceStation
            | Massing::BusDepot
            | Massing::MetroStation
            | Massing::PlazaFountain
            | Massing::ClockTower
            | Massing::GrandLibrary
            | Massing::OperaHouse
            | Massing::ScienceMuseum
            | Massing::Stadium
            | Massing::TvTower
            | Massing::Cathedral
            | Massing::GoldenArch
            | Massing::SkylineSpire
            | Massing::SpecLeisure
            | Massing::SpecTourism
            | Massing::SpecOrganic
            | Massing::SpecItCluster
            | Massing::SpecSelfSufficient
            | Massing::FarmMain
            | Massing::FarmExtractorS
            | Massing::FarmExtractorL
            | Massing::FarmProcessorA
            | Massing::FarmProcessorB
            | Massing::FarmWarehouse
            | Massing::ForestMain
            | Massing::ForestExtractorS
            | Massing::ForestExtractorL
            | Massing::ForestProcessorA
            | Massing::ForestProcessorB
            | Massing::ForestWarehouse
            | Massing::OreMain
            | Massing::OreExtractorS
            | Massing::OreExtractorL
            | Massing::OreProcessorA
            | Massing::OreProcessorB
            | Massing::OreWarehouse
            | Massing::OilMain
            | Massing::OilExtractorS
            | Massing::OilExtractorL
            | Massing::OilProcessorA
            | Massing::OilProcessorB
            | Massing::OilWarehouse
            | Massing::ParkSmall
            | Massing::ParkPlayground
            | Massing::ParkPlaza
            | Massing::ParkSports
    ) || park_area::is_park_area(m)
        || theme_park::is_theme_park(m)
        || factory::is_factory(m)
        || post::is_post(m)
}

/// An upright (optionally tapered) cylinder with `n` sides; `top` closes it with a cap.
#[allow(clippy::too_many_arguments)]
fn cyl(
    m: &mut MeshData,
    f: &Frame,
    (cu, cv): (f32, f32),
    (r0, r1): (f32, f32),
    (y0, y1): (f32, f32),
    n: usize,
    side: [f32; 4],
    top: Option<[f32; 4]>,
) {
    let p = |k: usize, r: f32, y: f32| {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        f.at(cu + r * a.cos(), cv + r * a.sin(), y)
    };
    let slope = (r0 - r1) / (y1 - y0).max(1e-3);
    for k in 0..n {
        let mid = (k as f32 + 0.5) / n as f32 * std::f32::consts::TAU;
        let nrm = (f.dir(mid.cos(), mid.sin()) + Vec3::Y * slope).normalize_or_zero();
        flat_quad(
            m,
            [
                p(k, r0, y0),
                p(k + 1, r0, y0),
                p(k + 1, r1, y1),
                p(k, r1, y1),
            ],
            nrm,
            side,
        );
    }
    if let Some(col) = top.filter(|_| r1 > 1e-3) {
        let centre = push_vertex(m, f.at(cu, cv, y1), Vec3::Y, col);
        let ring: Vec<u32> = (0..n)
            .map(|k| push_vertex(m, p(k, r1, y1), Vec3::Y, col))
            .collect();
        for k in 0..n {
            tri(m, centre, ring[k], ring[(k + 1) % n], Vec3::Y);
        }
    }
}

/// A cone of `n` triangles from a base circle to an apex.
fn cone(
    m: &mut MeshData,
    f: &Frame,
    (cu, cv): (f32, f32),
    r: f32,
    (y0, y1): (f32, f32),
    n: usize,
    col: [f32; 4],
) {
    let p = |k: usize| {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        f.at(cu + r * a.cos(), cv + r * a.sin(), y0)
    };
    let apex = f.at(cu, cv, y1);
    let slope = r / (y1 - y0).max(1e-3);
    for k in 0..n {
        let mid = (k as f32 + 0.5) / n as f32 * std::f32::consts::TAU;
        let nrm = (f.dir(mid.cos(), mid.sin()) + Vec3::Y * slope).normalize_or_zero();
        let v = [
            push_vertex(m, p(k), nrm, col),
            push_vertex(m, p(k + 1), nrm, col),
            push_vertex(m, apex, nrm, col),
        ];
        tri(m, v[0], v[1], v[2], nrm);
    }
}

/// What a service silhouette is built from.
pub struct ServiceCx<'a> {
    pub f: &'a Frame,
    /// Half extents of the lot volume and its centre offset along `v` (as for the boxes).
    pub hu: f32,
    pub hv: f32,
    pub cv: f32,
    /// Top of the plinth, and the ground under the lowest corner.
    pub base: f32,
    pub lo: f32,
    pub wall: [f32; 4],
    pub roof: [f32; 4],
    pub trim: [f32; 4],
    pub glass: [f32; 4],
    pub lod: super::Lod,
    /// Its lights are on at night (active and powered): signs may glow.
    pub lit: bool,
}

/// A turning part: drawn as its own mesh (it turns with the frame clock, which is a view
/// effect only), hub at `hub`, turning round `axis`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rotor {
    pub hub: Vec3,
    pub axis: Vec3,
    pub radius: f32,
    pub color: [f32; 4],
    /// Second colour (the ferris wheel's hub and gondolas, the carousel's platform and poles).
    pub accent: [f32; 4],
    pub kind: RotorKind,
}

/// What turns: a wind turbine's blades, a ferris wheel or a carousel (T-228).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotorKind {
    Turbine,
    FerrisWheel,
    Carousel,
}

impl Rotor {
    /// Radians per second.
    pub fn speed(&self) -> f32 {
        match self.kind {
            RotorKind::Turbine => 1.1,
            RotorKind::FerrisWheel => 0.12,
            RotorKind::Carousel => 0.6,
        }
    }
}

/// Blade length of the turbine, metres.
const BLADE: f32 = 17.0;
/// Hub height above the plinth, metres.
const HUB_H: f32 = 38.0;

/// The rotor of a building standing on `sc`, if `massing` has a turning part.
pub fn rotor(massing: Massing, sc: &ServiceCx) -> Option<Rotor> {
    if theme_park::is_theme_park(massing) {
        return theme_park::rotor(massing, sc);
    }
    (massing == Massing::WindTurbine).then(|| {
        let axis = sc.f.dir(0.0, -1.0).normalize_or_zero();
        let p = sc.f.at(0.0, sc.cv - 1.6, sc.base + HUB_H + 0.9);
        Rotor {
            hub: p,
            axis,
            radius: BLADE,
            color: sc.wall,
            accent: sc.wall,
            kind: RotorKind::Turbine,
        }
    })
}

/// The rotor mesh around the origin, turning round +Z, turned by `angle` (a turbine's blades
/// in the XY plane).
pub fn rotor_mesh(r: &Rotor, angle: f32) -> MeshData {
    if r.kind != RotorKind::Turbine {
        return theme_park::rotor_mesh(r, angle);
    }
    let mut m = MeshData::default();
    let n = Vec3::Z;
    for k in 0..3 {
        let a = angle + k as f32 * std::f32::consts::TAU / 3.0;
        let (d, p) = (
            Vec3::new(a.cos(), a.sin(), 0.0),
            Vec3::new(-a.sin(), a.cos(), 0.0),
        );
        let pts = [
            d * 1.0 - p * 0.7,
            d * 1.0 + p * 0.7,
            d * r.radius + p * 0.2,
            d * r.radius - p * 0.2,
        ];
        for nn in [n, -n] {
            flat_quad(&mut m, pts, nn, r.color);
        }
    }
    // The hub: a small octagon.
    let centre = push_vertex(&mut m, Vec3::new(0.0, 0.0, 0.0), n, mul(r.color, 0.8));
    let ring: Vec<u32> = (0..8)
        .map(|k| {
            let a = k as f32 / 8.0 * std::f32::consts::TAU;
            push_vertex(
                &mut m,
                Vec3::new(a.cos(), a.sin(), 0.0) * 0.9,
                n,
                mul(r.color, 0.8),
            )
        })
        .collect();
    for k in 0..8 {
        tri(&mut m, centre, ring[k], ring[(k + 1) % 8], n);
        tri(&mut m, centre, ring[k], ring[(k + 1) % 8], -n);
    }
    m
}

/// `rotor_mesh` placed in the world at `angle`: for tests and rasters.
pub fn rotor_world(r: &Rotor, angle: f32) -> MeshData {
    let q = bevy_math::Quat::from_rotation_arc(Vec3::Z, r.axis);
    let mut m = rotor_mesh(r, angle);
    for (p, n) in m.positions.iter_mut().zip(m.normals.iter_mut()) {
        *p = (q * Vec3::from_array(*p) + r.hub).to_array();
        *n = (q * Vec3::from_array(*n)).to_array();
    }
    m
}

/// Draws the silhouette of a service `massing`. `near` has every part, `mid` the main solids,
/// `far` the main solids with fewer sides.
pub fn service(m: &mut MeshData, massing: Massing, sc: &ServiceCx) {
    use super::Lod;
    let (f, b) = (sc.f, sc.base);
    let (hu, hv, cv) = (sc.hu, sc.hv, sc.cv);
    let near = sc.lod == Lod::Near;
    let far = sc.lod == Lod::Far;
    let n = if near { 8 } else { 6 };
    let fnd = [0.30, 0.30, 0.30, 1.0];
    let dark = mul(sc.trim, 0.55);
    // The pad every silhouette stands on, reaching down to the lowest ground.
    let pad = |m: &mut MeshData, (cu, cvv): (f32, f32), (pu, pv): (f32, f32), top: f32| {
        block(
            m,
            f,
            (cu, cvv),
            (pu, pv),
            sc.lo - 0.3,
            b + top,
            fnd,
            [0.55, 0.55, 0.53, 1.0],
        );
    };
    match massing {
        Massing::PumpHouse => {
            pad(m, (0.0, cv), (0.85 * hu, 0.8 * hv), 0.25);
            let y = b + 0.25;
            block(
                m,
                f,
                (-0.2 * hu, cv + 0.15 * hv),
                (0.38 * hu, 0.38 * hv),
                y,
                y + 3.4,
                sc.wall,
                sc.roof,
            );
            if !far {
                // The riser: a pipe standing beside the hut.
                cyl(
                    m,
                    f,
                    (0.55 * hu, cv - 0.4 * hv),
                    (0.5, 0.5),
                    (y, y + 4.2),
                    n,
                    sc.trim,
                    Some(dark),
                );
            }
            if near {
                block(
                    m,
                    f,
                    (-0.2 * hu, cv + 0.15 * hv),
                    (0.42 * hu, 0.42 * hv),
                    y + 3.4,
                    y + 3.7,
                    sc.roof,
                    mul(sc.roof, 0.9),
                );
                // The intake: a pipe from the pad edge down into the water.
                cyl(
                    m,
                    f,
                    (0.55 * hu, cv - 0.9 * hv),
                    (0.42, 0.42),
                    (sc.lo - 0.3, y + 2.2),
                    n,
                    dark,
                    Some(sc.trim),
                );
                block(
                    m,
                    f,
                    (0.55 * hu, cv - 0.65 * hv),
                    (0.3, 0.3 * hv),
                    y + 1.9,
                    y + 2.2,
                    dark,
                    sc.trim,
                );
            }
        }
        Massing::Outflow => {
            pad(m, (0.0, cv), (0.95 * hu, 0.9 * hv), 0.25);
            let y = b + 0.25;
            block(
                m,
                f,
                (-0.5 * hu, cv + 0.1 * hv),
                (0.34 * hu, 0.6 * hv),
                y,
                y + 3.0,
                sc.wall,
                sc.roof,
            );
            // The outfall: a wide culvert running out to the water, with a headwall.
            block(
                m,
                f,
                (0.3 * hu, cv),
                (0.55 * hu, 0.4 * hv),
                y,
                y + 1.3,
                sc.trim,
                mul(sc.trim, 0.8),
            );
            if !far {
                block(
                    m,
                    f,
                    (0.88 * hu, cv),
                    (0.1 * hu, 0.8 * hv),
                    y,
                    y + 2.0,
                    sc.wall,
                    sc.roof,
                );
            }
            if near {
                for s in [-1.0f32, 1.0] {
                    cyl(
                        m,
                        f,
                        (0.1 * hu, cv + s * 0.2 * hv),
                        (0.3, 0.3),
                        (y + 1.3, y + 2.1),
                        6,
                        dark,
                        Some(sc.trim),
                    );
                }
                block(
                    m,
                    f,
                    (-0.5 * hu, cv + 0.1 * hv),
                    (0.38 * hu, 0.64 * hv),
                    y + 3.0,
                    y + 3.3,
                    sc.roof,
                    mul(sc.roof, 0.9),
                );
            }
        }
        Massing::CoalPlant => {
            pad(m, (0.0, cv), (0.95 * hu, 0.95 * hv), 0.3);
            let y = b + 0.3;
            block(
                m,
                f,
                (-0.3 * hu, cv + 0.1 * hv),
                (0.55 * hu, 0.4 * hv),
                y,
                y + 9.0,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (0.55 * hu, cv + 0.1 * hv),
                (0.28 * hu, 0.3 * hv),
                y,
                y + 15.0,
                mul(sc.wall, 0.85),
                sc.roof,
            );
            for s in [0.38f32, 0.72] {
                cyl(
                    m,
                    f,
                    (s * hu, cv - 0.5 * hv),
                    (1.2, 0.85),
                    (y, y + 30.0),
                    n,
                    sc.wall,
                    Some(dark),
                );
                if !far {
                    cyl(
                        m,
                        f,
                        (s * hu, cv - 0.5 * hv),
                        (0.95, 0.9),
                        (y + 26.0, y + 28.0),
                        n,
                        sc.trim,
                        None,
                    );
                }
            }
            if !far {
                cone(
                    m,
                    f,
                    (-0.62 * hu, cv - 0.6 * hv),
                    0.24 * hu.min(hv) * 2.0,
                    (y, y + 4.5),
                    n,
                    mul(sc.roof, 1.4),
                );
            }
            if near {
                block(
                    m,
                    f,
                    (-0.3 * hu, cv - 0.35 * hv),
                    (0.4 * hu, 0.08 * hv),
                    y,
                    y + 2.2,
                    mul(sc.wall, 0.8),
                    sc.trim,
                );
                block(
                    m,
                    f,
                    (-0.3 * hu, cv + 0.1 * hv),
                    (0.57 * hu, 0.42 * hv),
                    y + 9.0,
                    y + 9.5,
                    sc.roof,
                    mul(sc.roof, 0.9),
                );
            }
        }
        Massing::WindTurbine => {
            cyl(
                m,
                f,
                (0.0, cv),
                (1.7, 1.7),
                (sc.lo - 0.3, b + 0.3),
                n,
                fnd,
                Some(sc.trim),
            );
            let y = b + 0.3;
            cyl(
                m,
                f,
                (0.0, cv),
                (0.95, 0.45),
                (y, b + HUB_H),
                n,
                sc.wall,
                None,
            );
            // The nacelle, long axis along v.
            block(
                m,
                f,
                (0.0, cv - 0.2),
                (0.6, 1.5),
                b + HUB_H,
                b + HUB_H + 1.8,
                mul(sc.wall, 0.95),
                sc.wall,
            );
            if near {
                block(
                    m,
                    f,
                    (0.0, cv - 1.1),
                    (0.45, 0.2),
                    y,
                    y + 2.1,
                    sc.trim,
                    sc.trim,
                );
                cyl(
                    m,
                    f,
                    (0.0, cv),
                    (0.98, 0.98),
                    (y + 8.0, y + 8.6),
                    n,
                    sc.trim,
                    None,
                );
            }
        }
        Massing::WaterTower => {
            let (lu, lv) = (0.42 * hu, 0.42 * hv);
            let top = b + 13.0;
            if far {
                cyl(
                    m,
                    f,
                    (0.0, cv),
                    (1.4, 1.4),
                    (sc.lo - 0.3, top),
                    6,
                    sc.trim,
                    None,
                );
            } else {
                for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    block(
                        m,
                        f,
                        (su * lu, cv + sv * lv),
                        (0.35, 0.35),
                        sc.lo - 0.3,
                        top,
                        sc.trim,
                        sc.trim,
                    );
                }
            }
            let r = 0.62 * hu.min(hv);
            cyl(
                m,
                f,
                (0.0, cv),
                (r, r),
                (top, top + 6.0),
                n,
                sc.wall,
                Some(sc.roof),
            );
            cone(m, f, (0.0, cv), r, (top + 6.0, top + 8.4), n, sc.roof);
            if near {
                for sv in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        (0.0, cv + sv * lv),
                        (lu, 0.15),
                        b + 6.0,
                        b + 6.3,
                        dark,
                        dark,
                    );
                }
                for su in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        (su * lu, cv),
                        (0.15, lv),
                        b + 6.0,
                        b + 6.3,
                        dark,
                        dark,
                    );
                }
                cyl(
                    m,
                    f,
                    (0.0, cv),
                    (r + 0.15, r + 0.15),
                    (top + 2.6, top + 3.0),
                    n,
                    sc.trim,
                    None,
                );
            }
        }
        Massing::TreatmentPlant => {
            pad(m, (0.0, cv), (0.97 * hu, 0.95 * hv), 0.25);
            let y = b + 0.25;
            block(
                m,
                f,
                (-0.4 * hu, cv + 0.5 * hv),
                (0.45 * hu, 0.36 * hv),
                y,
                y + 4.2,
                sc.wall,
                sc.roof,
            );
            let r = (0.34 * hu).min(0.36 * hv);
            for s in [-0.45f32, 0.45] {
                cyl(
                    m,
                    f,
                    (s * hu, cv - 0.38 * hv),
                    (r, r),
                    (y, y + 1.8),
                    if near { 12 } else { 8 },
                    sc.trim,
                    Some(sc.glass),
                );
            }
            if !far {
                block(
                    m,
                    f,
                    (0.0, cv - 0.38 * hv),
                    (0.45 * hu, 0.12 * hv),
                    y + 0.4,
                    y + 0.9,
                    dark,
                    dark,
                );
            }
            if near {
                block(
                    m,
                    f,
                    (-0.4 * hu, cv + 0.5 * hv),
                    (0.49 * hu, 0.4 * hv),
                    y + 4.2,
                    y + 4.5,
                    sc.roof,
                    mul(sc.roof, 0.9),
                );
                block(
                    m,
                    f,
                    (0.5 * hu, cv + 0.55 * hv),
                    (0.12 * hu, 0.2 * hv),
                    y,
                    y + 2.6,
                    mul(sc.wall, 0.9),
                    sc.roof,
                );
                for s in [-0.45f32, 0.45] {
                    block(
                        m,
                        f,
                        (s * hu, cv - 0.38 * hv),
                        (0.1, r),
                        y + 1.8,
                        y + 2.1,
                        dark,
                        dark,
                    );
                }
            }
        }
        Massing::Landfill => {
            pad(m, (0.0, cv), (0.95 * hu, 0.95 * hv), 0.25);
            let y = b + 0.25;
            let r = hu.min(hv);
            cone(
                m,
                f,
                (-0.15 * hu, cv + 0.1 * hv),
                0.55 * r,
                (y, y + 8.0),
                n,
                sc.wall,
            );
            cone(
                m,
                f,
                (0.4 * hu, cv + 0.3 * hv),
                0.32 * r,
                (y, y + 4.5),
                n,
                mul(sc.wall, 0.85),
            );
            if !far {
                // The fence round the tip.
                for (c, h) in [
                    ((0.0, cv - 0.93 * hv), (0.93 * hu, 0.03 * hv)),
                    ((0.0, cv + 0.93 * hv), (0.93 * hu, 0.03 * hv)),
                    ((-0.93 * hu, cv), (0.03 * hu, 0.93 * hv)),
                    ((0.93 * hu, cv), (0.03 * hu, 0.93 * hv)),
                ] {
                    block(m, f, c, h, y, y + 1.8, sc.trim, sc.trim);
                }
            }
            if near {
                // The bulldozer: body, cab and blade.
                let yel = [0.88, 0.68, 0.08, 1.0];
                block(
                    m,
                    f,
                    (0.45 * hu, cv - 0.5 * hv),
                    (1.5, 1.0),
                    y,
                    y + 1.5,
                    yel,
                    yel,
                );
                block(
                    m,
                    f,
                    (0.45 * hu, cv - 0.5 * hv),
                    (0.8, 0.8),
                    y + 1.5,
                    y + 2.6,
                    yel,
                    dark,
                );
                block(
                    m,
                    f,
                    (0.45 * hu, cv - 0.5 * hv - 1.4),
                    (1.7, 0.2),
                    y,
                    y + 1.3,
                    dark,
                    dark,
                );
            }
        }
        Massing::Incinerator => {
            pad(m, (0.0, cv), (0.95 * hu, 0.95 * hv), 0.25);
            let y = b + 0.25;
            block(
                m,
                f,
                (-0.25 * hu, cv + 0.1 * hv),
                (0.55 * hu, 0.6 * hv),
                y,
                y + 8.0,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (-0.25 * hu, cv - 0.7 * hv),
                (0.3 * hu, 0.2 * hv),
                y,
                y + 4.0,
                mul(sc.wall, 0.8),
                sc.trim,
            );
            // One tall chimney.
            cyl(
                m,
                f,
                (0.62 * hu, cv - 0.2 * hv),
                (1.6, 1.0),
                (y, y + 36.0),
                n,
                sc.wall,
                Some(dark),
            );
            if !far {
                cyl(
                    m,
                    f,
                    (0.62 * hu, cv - 0.2 * hv),
                    (1.1, 1.05),
                    (y + 30.0, y + 32.0),
                    n,
                    sc.trim,
                    None,
                );
            }
            if near {
                cyl(
                    m,
                    f,
                    (-0.5 * hu, cv + 0.1 * hv),
                    (0.7, 0.7),
                    (y + 8.0, y + 10.0),
                    n,
                    dark,
                    Some(sc.trim),
                );
            }
        }
        Massing::Clinic => {
            pad(m, (0.0, cv), (0.9 * hu, 0.9 * hv), 0.25);
            let y = b + 0.25;
            block(
                m,
                f,
                (0.0, cv + 0.1 * hv),
                (0.7 * hu, 0.55 * hv),
                y,
                y + 4.5,
                sc.wall,
                sc.roof,
            );
            // The cross on the roof.
            let red = [0.85, 0.15, 0.15, 1.0];
            block(
                m,
                f,
                (0.0, cv - 0.1 * hv),
                (0.3, 0.3),
                y + 4.5,
                y + 9.0,
                red,
                red,
            );
            block(
                m,
                f,
                (0.0, cv - 0.1 * hv),
                (1.3, 0.3),
                y + 6.2,
                y + 7.4,
                red,
                red,
            );
            if near {
                block(
                    m,
                    f,
                    (0.0, cv - 0.5 * hv),
                    (0.2 * hu, 0.12 * hv),
                    y,
                    y + 3.0,
                    sc.trim,
                    sc.trim,
                );
            }
        }
        Massing::Hospital => {
            pad(m, (0.0, cv), (0.95 * hu, 0.95 * hv), 0.25);
            let y = b + 0.25;
            block(
                m,
                f,
                (0.15 * hu, cv + 0.2 * hv),
                (0.7 * hu, 0.5 * hv),
                y,
                y + 14.0,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (-0.6 * hu, cv - 0.45 * hv),
                (0.28 * hu, 0.32 * hv),
                y,
                y + 8.0,
                mul(sc.wall, 0.92),
                sc.roof,
            );
            let red = [0.85, 0.15, 0.15, 1.0];
            block(
                m,
                f,
                (0.15 * hu, cv + 0.2 * hv),
                (0.4, 0.4),
                y + 14.0,
                y + 19.0,
                red,
                red,
            );
            block(
                m,
                f,
                (0.15 * hu, cv + 0.2 * hv),
                (1.6, 0.4),
                y + 16.0,
                y + 17.4,
                red,
                red,
            );
            // The helipad on the wing.
            let rad = (0.28 * hu).min(0.32 * hv) * 0.9;
            cyl(
                m,
                f,
                (-0.6 * hu, cv - 0.45 * hv),
                (rad, rad),
                (y + 8.0, y + 8.3),
                n,
                sc.trim,
                Some(mul(sc.trim, 1.1)),
            );
            if near {
                let w = [0.95, 0.95, 0.95, 1.0];
                for du in [-0.35f32, 0.35] {
                    block(
                        m,
                        f,
                        (-0.6 * hu + du * rad, cv - 0.45 * hv),
                        (0.15, 0.55 * rad),
                        y + 8.3,
                        y + 8.4,
                        w,
                        w,
                    );
                }
                block(
                    m,
                    f,
                    (-0.6 * hu, cv - 0.45 * hv),
                    (0.35 * rad, 0.15),
                    y + 8.3,
                    y + 8.4,
                    w,
                    w,
                );
            }
        }
        Massing::Cemetery => {
            // A walled lawn with rows of gravestones and a chapel.
            block(
                m,
                f,
                (0.0, cv),
                (0.95 * hu, 0.95 * hv),
                sc.lo - 0.3,
                b + 0.2,
                sc.wall,
                mul(sc.wall, 1.05),
            );
            let y = b + 0.2;
            if !far {
                for (c, h) in [
                    ((0.0, cv - 0.95 * hv), (0.95 * hu, 0.03 * hv)),
                    ((0.0, cv + 0.95 * hv), (0.95 * hu, 0.03 * hv)),
                    ((-0.95 * hu, cv), (0.03 * hu, 0.95 * hv)),
                    ((0.95 * hu, cv), (0.03 * hu, 0.95 * hv)),
                ] {
                    block(m, f, c, h, y, y + 1.6, sc.trim, sc.trim);
                }
            }
            block(
                m,
                f,
                (0.0, cv + 0.62 * hv),
                (0.22 * hu, 0.2 * hv),
                y,
                y + 4.2,
                sc.roof,
                mul(sc.roof, 0.9),
            );
            if !far {
                cone(
                    m,
                    f,
                    (0.0, cv + 0.62 * hv),
                    0.26 * hu.min(hv) * 1.4,
                    (y + 4.2, y + 7.0),
                    n,
                    mul(sc.roof, 1.2),
                );
                let (cols, rows) = if near { (6, 3) } else { (4, 2) };
                for i in 0..cols {
                    for j in 0..rows {
                        let u = (-0.7 + 1.4 * (i as f32 + 0.5) / cols as f32) * hu;
                        let v = cv - 0.6 * hv + j as f32 * 0.32 * hv;
                        block(m, f, (u, v), (0.4, 0.12), y, y + 1.2, sc.trim, sc.trim);
                    }
                }
            }
        }
        Massing::Crematorium => {
            pad(m, (0.0, cv), (0.9 * hu, 0.9 * hv), 0.25);
            let y = b + 0.25;
            block(
                m,
                f,
                (-0.1 * hu, cv + 0.1 * hv),
                (0.6 * hu, 0.5 * hv),
                y,
                y + 5.0,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (0.55 * hu, cv - 0.25 * hv),
                (0.25 * hu, 0.3 * hv),
                y,
                y + 3.4,
                mul(sc.wall, 0.9),
                sc.roof,
            );
            // A short, thin chimney.
            cyl(
                m,
                f,
                (-0.5 * hu, cv - 0.15 * hv),
                (0.75, 0.55),
                (y, y + 15.0),
                n,
                sc.trim,
                Some(dark),
            );
            if near {
                block(
                    m,
                    f,
                    (-0.1 * hu, cv - 0.45 * hv),
                    (0.2 * hu, 0.1 * hv),
                    y,
                    y + 3.0,
                    sc.trim,
                    sc.trim,
                );
            }
        }
        Massing::School => {
            pad(m, (0.0, cv), (0.95 * hu, 0.95 * hv), 0.25);
            let y = b + 0.25;
            // Two wings round a yard.
            block(
                m,
                f,
                (-0.15 * hu, cv + 0.5 * hv),
                (0.75 * hu, 0.3 * hv),
                y,
                y + 5.0,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (-0.6 * hu, cv - 0.1 * hv),
                (0.3 * hu, 0.45 * hv),
                y,
                y + 5.0,
                mul(sc.wall, 0.92),
                sc.roof,
            );
            block(
                m,
                f,
                (0.35 * hu, cv - 0.2 * hv),
                (0.55 * hu, 0.55 * hv),
                y,
                y + 0.12,
                mul(sc.trim, 0.8),
                sc.trim,
            );
            // The bell turret and the flagpole.
            block(
                m,
                f,
                (-0.15 * hu, cv + 0.5 * hv),
                (1.4, 1.4),
                y + 5.0,
                y + 7.6,
                sc.wall,
                sc.roof,
            );
            if !far {
                cone(
                    m,
                    f,
                    (-0.15 * hu, cv + 0.5 * hv),
                    2.0,
                    (y + 7.6, y + 10.2),
                    n,
                    sc.roof,
                );
                cyl(
                    m,
                    f,
                    (0.8 * hu, cv - 0.7 * hv),
                    (0.14, 0.12),
                    (y, y + 9.0),
                    6,
                    [0.9, 0.9, 0.9, 1.0],
                    None,
                );
            }
            if near {
                let red = [0.85, 0.2, 0.2, 1.0];
                block(
                    m,
                    f,
                    (0.8 * hu + 0.7, cv - 0.7 * hv),
                    (0.7, 0.06),
                    y + 7.4,
                    y + 8.8,
                    red,
                    red,
                );
            }
        }
        Massing::HighSchool => {
            pad(m, (0.0, cv), (0.95 * hu, 0.95 * hv), 0.25);
            let y = b + 0.25;
            // A centre block with a wing each side.
            block(
                m,
                f,
                (0.0, cv + 0.35 * hv),
                (0.3 * hu, 0.35 * hv),
                y,
                y + 10.0,
                sc.wall,
                sc.roof,
            );
            for s in [-1.0f32, 1.0] {
                block(
                    m,
                    f,
                    (s * 0.68 * hu, cv),
                    (0.28 * hu, 0.6 * hv),
                    y,
                    y + 6.5,
                    mul(sc.wall, 0.92),
                    sc.roof,
                );
            }
            block(
                m,
                f,
                (0.0, cv + 0.62 * hv),
                (0.7 * hu, 0.12 * hv),
                y,
                y + 6.5,
                mul(sc.wall, 0.88),
                sc.roof,
            );
            if !far {
                cyl(
                    m,
                    f,
                    (0.0, cv + 0.35 * hv),
                    (0.16, 0.14),
                    (y + 10.0, y + 18.0),
                    6,
                    [0.9, 0.9, 0.9, 1.0],
                    None,
                );
            }
            if near {
                let blue = [0.2, 0.35, 0.8, 1.0];
                block(
                    m,
                    f,
                    (0.9, cv + 0.35 * hv),
                    (0.9, 0.06),
                    y + 15.8,
                    y + 17.6,
                    blue,
                    blue,
                );
                block(
                    m,
                    f,
                    (0.0, cv + 0.0 * hv),
                    (0.2 * hu, 0.1 * hv),
                    y,
                    y + 3.0,
                    sc.trim,
                    sc.trim,
                );
            }
        }
        Massing::University => {
            pad(m, (0.0, cv), (0.95 * hu, 0.95 * hv), 0.25);
            let y = b + 0.25;
            block(
                m,
                f,
                (0.0, cv),
                (0.9 * hu, 0.85 * hv),
                y,
                y + 1.2,
                sc.trim,
                mul(sc.trim, 0.95),
            );
            let y = y + 1.2;
            block(
                m,
                f,
                (0.0, cv + 0.3 * hv),
                (0.78 * hu, 0.4 * hv),
                y,
                y + 9.0,
                sc.wall,
                sc.roof,
            );
            // The portico: a row of columns under a beam and a pediment.
            let cols = if near {
                6
            } else if far {
                4
            } else {
                5
            };
            for k in 0..cols {
                let u = (-0.68 + 1.36 * k as f32 / (cols - 1) as f32) * hu;
                cyl(
                    m,
                    f,
                    (u, cv - 0.35 * hv),
                    (0.7, 0.7),
                    (y, y + 9.0),
                    if far { 4 } else { n },
                    mul(sc.trim, 1.1),
                    None,
                );
            }
            block(
                m,
                f,
                (0.0, cv - 0.35 * hv),
                (0.78 * hu, 0.2 * hv),
                y + 9.0,
                y + 10.4,
                sc.trim,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv - 0.35 * hv),
                (0.55 * hu, 0.2 * hv),
                y + 10.4,
                y + 12.0,
                sc.trim,
                sc.roof,
            );
            // The dome behind it.
            cyl(
                m,
                f,
                (0.0, cv + 0.3 * hv),
                (0.2 * hu, 0.2 * hu),
                (y + 9.0, y + 12.5),
                n,
                sc.wall,
                None,
            );
            cone(
                m,
                f,
                (0.0, cv + 0.3 * hv),
                0.2 * hu,
                (y + 12.5, y + 18.0),
                n,
                sc.roof,
            );
        }
        Massing::FireStation => {
            pad(m, (0.0, cv), (0.95 * hu, 0.95 * hv), 0.25);
            let y = b + 0.25;
            block(
                m,
                f,
                (-0.1 * hu, cv),
                (0.75 * hu, 0.55 * hv),
                y,
                y + 6.5,
                sc.wall,
                sc.roof,
            );
            // The hose tower.
            block(
                m,
                f,
                (0.75 * hu, cv + 0.3 * hv),
                (0.12 * hu, 0.16 * hv),
                y,
                y + 15.0,
                mul(sc.wall, 0.9),
                sc.trim,
            );
            // The engine doors.
            for k in [-1.0f32, 0.0, 1.0] {
                block(
                    m,
                    f,
                    (-0.1 * hu + k * 0.45 * hu, cv - 0.56 * hv),
                    (0.18 * hu, 0.04 * hv),
                    y,
                    y + 4.2,
                    dark,
                    dark,
                );
            }
            if near {
                block(
                    m,
                    f,
                    (-0.1 * hu, cv - 0.85 * hv),
                    (0.75 * hu, 0.12 * hv),
                    y,
                    y + 0.1,
                    mul(sc.trim, 0.7),
                    sc.trim,
                );
                block(
                    m,
                    f,
                    (-0.1 * hu, cv),
                    (0.77 * hu, 0.57 * hv),
                    y + 6.5,
                    y + 6.9,
                    sc.roof,
                    mul(sc.roof, 0.9),
                );
            }
        }
        Massing::PoliceStation => {
            pad(m, (0.0, cv), (0.95 * hu, 0.95 * hv), 0.25);
            let y = b + 0.25;
            block(
                m,
                f,
                (-0.15 * hu, cv + 0.05 * hv),
                (0.6 * hu, 0.5 * hv),
                y,
                y + 7.0,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (-0.15 * hu, cv - 0.55 * hv),
                (0.2 * hu, 0.1 * hv),
                y,
                y + 3.6,
                sc.trim,
                sc.trim,
            );
            // The blue beacon on the roof.
            let blue = [0.15, 0.35, 1.0, 1.0];
            cyl(
                m,
                f,
                (-0.15 * hu, cv),
                (0.6, 0.6),
                (y + 7.0, y + 8.2),
                n,
                sc.trim,
                None,
            );
            cyl(
                m,
                f,
                (-0.15 * hu, cv),
                (0.7, 0.5),
                (y + 8.2, y + 9.8),
                n,
                blue,
                Some(blue),
            );
            // The car park beside it.
            block(
                m,
                f,
                (0.65 * hu, cv - 0.2 * hv),
                (0.28 * hu, 0.6 * hv),
                y,
                y + 0.1,
                dark,
                mul(dark, 1.2),
            );
            if near {
                for k in 0..3 {
                    block(
                        m,
                        f,
                        (0.65 * hu, cv - 0.5 * hv + k as f32 * 0.35 * hv),
                        (1.1, 0.8),
                        y + 0.1,
                        y + 1.1,
                        [0.9, 0.9, 0.95, 1.0],
                        blue,
                    );
                }
            }
        }
        Massing::BusDepot => {
            pad(m, (0.0, cv), (0.95 * hu, 0.95 * hv), 0.25);
            let y = b + 0.25;
            // The hangar.
            block(
                m,
                f,
                (-0.15 * hu, cv + 0.15 * hv),
                (0.7 * hu, 0.62 * hv),
                y,
                y + 6.0,
                sc.wall,
                sc.roof,
            );
            // The office at the corner.
            block(
                m,
                f,
                (0.72 * hu, cv + 0.5 * hv),
                (0.2 * hu, 0.25 * hv),
                y,
                y + 8.5,
                mul(sc.wall, 0.9),
                sc.trim,
            );
            // Three wide gates in the front.
            for k in [-1.0f32, 0.0, 1.0] {
                block(
                    m,
                    f,
                    (-0.15 * hu + k * 0.42 * hu, cv - 0.48 * hv),
                    (0.17 * hu, 0.03 * hv),
                    y,
                    y + 4.4,
                    dark,
                    dark,
                );
            }
            // The yard in front, with a bay stripe.
            block(
                m,
                f,
                (-0.1 * hu, cv - 0.72 * hv),
                (0.8 * hu, 0.22 * hv),
                y,
                y + 0.08,
                mul(dark, 1.15),
                mul(dark, 1.3),
            );
            if near {
                for k in 0..3 {
                    block(
                        m,
                        f,
                        (-0.15 * hu + (k as f32 - 1.0) * 0.42 * hu, cv - 0.72 * hv),
                        (2.2, 1.1),
                        y + 0.08,
                        y + 3.0,
                        [0.95, 0.75, 0.12, 1.0],
                        [0.98, 0.82, 0.2, 1.0],
                    );
                }
            }
        }
        Massing::MetroStation => {
            // The entrance pavilion (T-164): the platform is 12 m underground, this is the
            // way down: a glass hall under a wide flat roof, a dark portal in front and a
            // red sign pylon beside it.
            pad(m, (0.0, cv), (0.92 * hu, 0.92 * hv), 0.2);
            let y = b + 0.2;
            let glass = [0.30, 0.36, 0.44, 1.0];
            block(
                m,
                f,
                (0.0, cv + 0.05 * hv),
                (0.55 * hu, 0.5 * hv),
                y,
                y + 4.2,
                glass,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv + 0.05 * hv),
                (0.7 * hu, 0.62 * hv),
                y + 4.2,
                y + 4.8,
                sc.roof,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv - 0.45 * hv),
                (0.22 * hu, 0.04 * hv),
                y,
                y + 3.2,
                dark,
                dark,
            );
            block(
                m,
                f,
                (0.78 * hu, cv - 0.7 * hv),
                (0.05 * hu, 0.05 * hv),
                y,
                y + 7.0,
                sc.trim,
                sc.trim,
            );
            block(
                m,
                f,
                (0.78 * hu, cv - 0.7 * hv),
                (0.13 * hu, 0.04 * hv),
                y + 6.0,
                y + 8.2,
                sc.trim,
                mul(sc.trim, 1.1),
            );
            if near {
                // A hand rail down the stair on each side of the portal.
                for k in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        (k * 0.3 * hu, cv - 0.62 * hv),
                        (0.02 * hu, 0.16 * hv),
                        y,
                        y + 1.0,
                        [0.7, 0.7, 0.72, 1.0],
                        [0.75, 0.75, 0.78, 1.0],
                    );
                }
            }
        }
        _ => unique(m, massing, sc),
    }
}

/// The unique buildings and monuments (T-202, T-203): ten silhouettes that read from far away.
/// The front is `-v`. Monuments (stadium to spire) are the biggest things in the city.
fn unique(m: &mut MeshData, massing: Massing, sc: &ServiceCx) {
    use super::Lod;
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
    let stone = [0.78, 0.76, 0.70, 1.0];
    let r = hu.min(hv);
    // The pad every silhouette stands on, reaching down to the lowest ground.
    let pad = |m: &mut MeshData, (pu, pv): (f32, f32), top: f32, col: [f32; 4]| {
        block(m, f, (0.0, cv), (pu, pv), sc.lo - 0.3, b + top, fnd, col);
    };
    match massing {
        Massing::PlazaFountain => {
            // A paved square with a basin and jet in the middle and a colonnade on three sides.
            pad(m, (hu, hv), 0.3, stone);
            let y = b + 0.3;
            cyl(
                m,
                f,
                (0.0, cv),
                (0.34 * r, 0.3 * r),
                (y, y + 1.1),
                n,
                stone,
                Some(sc.glass),
            );
            cyl(
                m,
                f,
                (0.0, cv),
                (0.07 * r, 0.04 * r),
                (y, y + 3.2),
                n.min(8),
                sc.trim,
                None,
            );
            if !far {
                cone(
                    m,
                    f,
                    (0.0, cv),
                    0.1 * r,
                    (y + 3.2, y + 4.6),
                    n.min(8),
                    sc.glass,
                );
            }
            let cols = if near { 7 } else { 4 };
            for k in 0..cols {
                let t = -0.9 + 1.8 * k as f32 / (cols - 1) as f32;
                // Back row and both side rows.
                for (u, v) in [
                    (t * hu, cv + 0.9 * hv),
                    (-0.9 * hu, cv + t * hv),
                    (0.9 * hu, cv + t * hv),
                ] {
                    cyl(
                        m,
                        f,
                        (u, v),
                        (0.45, 0.45),
                        (y, y + 5.0),
                        n.min(8),
                        sc.wall,
                        None,
                    );
                }
            }
            block(
                m,
                f,
                (0.0, cv + 0.9 * hv),
                (0.95 * hu, 0.08 * hv),
                y + 5.0,
                y + 5.8,
                sc.trim,
                sc.roof,
            );
            block(
                m,
                f,
                (-0.9 * hu, cv),
                (0.08 * hu, 0.95 * hv),
                y + 5.0,
                y + 5.8,
                sc.trim,
                sc.roof,
            );
            block(
                m,
                f,
                (0.9 * hu, cv),
                (0.08 * hu, 0.95 * hv),
                y + 5.0,
                y + 5.8,
                sc.trim,
                sc.roof,
            );
        }
        Massing::ClockTower => {
            pad(m, (0.8 * hu, 0.8 * hv), 0.3, stone);
            let y = b + 0.3;
            block(
                m,
                f,
                (0.0, cv),
                (0.36 * r, 0.36 * r),
                y,
                y + 26.0,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv),
                (0.42 * r, 0.42 * r),
                y + 26.0,
                y + 27.6,
                sc.trim,
                sc.trim,
            );
            // The clock stage: a lighter box with a glass face on every side.
            block(
                m,
                f,
                (0.0, cv),
                (0.4 * r, 0.4 * r),
                y + 27.6,
                y + 34.0,
                sc.wall,
                sc.roof,
            );
            if near {
                for (du, dv, wu, wv) in [
                    (0.0, -0.405, 0.26, 0.02),
                    (0.0, 0.405, 0.26, 0.02),
                    (-0.405, 0.0, 0.02, 0.26),
                    (0.405, 0.0, 0.02, 0.26),
                ] {
                    block(
                        m,
                        f,
                        (du * r, cv + dv * r),
                        (wu * r, wv * r),
                        y + 28.6,
                        y + 33.0,
                        sc.glass,
                        sc.glass,
                    );
                }
            }
            block(
                m,
                f,
                (0.0, cv),
                (0.46 * r, 0.46 * r),
                y + 34.0,
                y + 35.2,
                sc.trim,
                sc.trim,
            );
            cone(
                m,
                f,
                (0.0, cv),
                0.4 * r,
                (y + 35.2, y + 50.0),
                n.min(8),
                sc.roof,
            );
        }
        Massing::GrandLibrary => {
            // Two steps, a long hall, a portico of columns, a pediment and a dome.
            pad(m, (0.96 * hu, 0.96 * hv), 0.3, stone);
            let y = b + 0.3;
            block(
                m,
                f,
                (0.0, cv),
                (0.92 * hu, 0.9 * hv),
                y,
                y + 1.0,
                sc.trim,
                sc.trim,
            );
            let y = y + 1.0;
            block(
                m,
                f,
                (0.0, cv + 0.15 * hv),
                (0.82 * hu, 0.62 * hv),
                y,
                y + 11.0,
                sc.wall,
                sc.roof,
            );
            let cols = if near {
                8
            } else if far {
                4
            } else {
                6
            };
            for k in 0..cols {
                let u = (-0.74 + 1.48 * k as f32 / (cols - 1) as f32) * hu;
                cyl(
                    m,
                    f,
                    (u, cv - 0.55 * hv),
                    (0.75, 0.75),
                    (y, y + 11.0),
                    n.min(8),
                    mul(sc.trim, 1.08),
                    None,
                );
            }
            block(
                m,
                f,
                (0.0, cv - 0.55 * hv),
                (0.82 * hu, 0.16 * hv),
                y + 11.0,
                y + 12.4,
                sc.trim,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv - 0.55 * hv),
                (0.6 * hu, 0.14 * hv),
                y + 12.4,
                y + 14.0,
                sc.trim,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv - 0.55 * hv),
                (0.3 * hu, 0.12 * hv),
                y + 14.0,
                y + 15.2,
                sc.trim,
                sc.roof,
            );
            // The dome over the hall.
            cyl(
                m,
                f,
                (0.0, cv + 0.2 * hv),
                (0.3 * r, 0.3 * r),
                (y + 11.0, y + 16.0),
                n,
                sc.wall,
                None,
            );
            cyl(
                m,
                f,
                (0.0, cv + 0.2 * hv),
                (0.3 * r, 0.22 * r),
                (y + 16.0, y + 19.0),
                n,
                sc.glass,
                None,
            );
            cyl(
                m,
                f,
                (0.0, cv + 0.2 * hv),
                (0.22 * r, 0.1 * r),
                (y + 19.0, y + 23.0),
                n,
                sc.roof,
                None,
            );
            cone(
                m,
                f,
                (0.0, cv + 0.2 * hv),
                0.1 * r,
                (y + 23.0, y + 27.0),
                n.min(8),
                sc.trim,
            );
        }
        Massing::OperaHouse => {
            // A glass foyer in front and a fan of white shell roofs rising towards the back.
            pad(m, (0.96 * hu, 0.96 * hv), 0.3, stone);
            let y = b + 0.3;
            block(
                m,
                f,
                (0.0, cv - 0.55 * hv),
                (0.8 * hu, 0.3 * hv),
                y,
                y + 5.0,
                sc.glass,
                sc.roof,
            );
            let shells = if far { 3 } else { 5 };
            for k in 0..shells {
                let t = -1.0 + 2.0 * k as f32 / (shells - 1) as f32;
                let u = t * 0.62 * hu;
                let v = cv + (0.25 - 0.35 * t * t) * hv;
                let h = 15.0 - 5.0 * t.abs();
                let w = 0.36 * r;
                cyl(
                    m,
                    f,
                    (u, v),
                    (w, 0.55 * w),
                    (y, y + 0.6 * h),
                    n,
                    sc.trim,
                    None,
                );
                cyl(
                    m,
                    f,
                    (u, v),
                    (0.55 * w, 0.2 * w),
                    (y + 0.6 * h, y + h),
                    n,
                    sc.trim,
                    None,
                );
                cone(
                    m,
                    f,
                    (u, v),
                    0.2 * w,
                    (y + h, y + h + 2.5),
                    n.min(8),
                    sc.roof,
                );
            }
            block(
                m,
                f,
                (0.0, cv + 0.55 * hv),
                (0.7 * hu, 0.25 * hv),
                y,
                y + 9.0,
                sc.wall,
                sc.roof,
            );
        }
        Massing::ScienceMuseum => {
            // A low plinth under a big glass dome.
            pad(m, (0.96 * hu, 0.96 * hv), 0.3, stone);
            let y = b + 0.3;
            block(
                m,
                f,
                (0.0, cv),
                (0.92 * hu, 0.9 * hv),
                y,
                y + 4.0,
                sc.wall,
                sc.roof,
            );
            let y = y + 4.0;
            let r0 = 0.7 * r;
            cyl(m, f, (0.0, cv), (r0, r0), (y, y + 1.0), n, sc.trim, None);
            let rings = if far { 2 } else { 4 };
            let mut rad = r0;
            let mut h = y + 1.0;
            for k in 0..rings {
                let next = r0 * (1.0 - (k as f32 + 1.0) / rings as f32).powf(0.7).max(0.08);
                let rise = 11.0 / rings as f32;
                cyl(
                    m,
                    f,
                    (0.0, cv),
                    (rad, next),
                    (h, h + rise),
                    n,
                    sc.glass,
                    if k + 1 == rings { Some(sc.glass) } else { None },
                );
                rad = next;
                h += rise;
            }
            if near {
                // Ribs over the glass.
                for k in 0..4 {
                    let a = k as f32 / 4.0 * std::f32::consts::PI;
                    block(
                        m,
                        f,
                        (0.0, cv),
                        (
                            0.04 * r0 + 0.04 * r0 * a.cos().abs(),
                            0.04 * r0 + 0.04 * r0 * a.sin().abs(),
                        ),
                        y + 1.0,
                        y + 12.4,
                        sc.trim,
                        sc.trim,
                    );
                }
            }
        }
        Massing::Stadium => {
            // An oval bowl with a rim and four tall floodlight masts.
            pad(m, (hu, hv), 0.4, fnd);
            let y = b + 0.4;
            let big = 0.9 * r;
            cyl(
                m,
                f,
                (0.0, cv),
                (big, 0.8 * big),
                (y, y + 13.0),
                n.max(10),
                sc.wall,
                Some([0.30, 0.55, 0.28, 1.0]),
            );
            cyl(
                m,
                f,
                (0.0, cv),
                (0.83 * big, 0.83 * big),
                (y + 12.0, y + 14.2),
                n.max(10),
                sc.trim,
                None,
            );
            if !far {
                // The canopy ring over the stands.
                cyl(
                    m,
                    f,
                    (0.0, cv),
                    (0.95 * big, 0.95 * big),
                    (y + 14.2, y + 15.0),
                    n.max(10),
                    sc.roof,
                    None,
                );
            }
            for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let (u, v) = (su * 0.88 * hu, cv + sv * 0.88 * hv);
                cyl(m, f, (u, v), (0.7, 0.5), (y, y + 34.0), 6, sc.trim, None);
                block(
                    m,
                    f,
                    (u, v),
                    (2.2, 1.0),
                    y + 34.0,
                    y + 36.0,
                    [0.95, 0.95, 0.8, 1.0],
                    sc.trim,
                );
            }
        }
        Massing::TvTower => {
            // A concrete mast with a sphere restaurant and a long antenna.
            pad(m, (0.5 * hu, 0.5 * hv), 0.4, stone);
            let y = b + 0.4;
            cyl(
                m,
                f,
                (0.0, cv),
                (0.3 * r, 0.1 * r),
                (y, y + 14.0),
                n,
                sc.wall,
                None,
            );
            cyl(
                m,
                f,
                (0.0, cv),
                (0.1 * r, 0.065 * r),
                (y + 14.0, y + 62.0),
                n.min(10),
                sc.wall,
                None,
            );
            cyl(
                m,
                f,
                (0.0, cv),
                (0.065 * r, 0.3 * r),
                (y + 62.0, y + 66.0),
                n,
                sc.trim,
                None,
            );
            cyl(
                m,
                f,
                (0.0, cv),
                (0.3 * r, 0.3 * r),
                (y + 66.0, y + 70.0),
                n,
                sc.glass,
                Some(sc.trim),
            );
            cyl(
                m,
                f,
                (0.0, cv),
                (0.3 * r, 0.06 * r),
                (y + 70.0, y + 73.0),
                n,
                sc.trim,
                None,
            );
            cone(
                m,
                f,
                (0.0, cv),
                0.035 * r,
                (y + 73.0, y + 100.0),
                4,
                sc.roof,
            );
        }
        Massing::Cathedral => {
            // A nave and a transept under a steep roof, two west towers with spires.
            pad(m, (hu, hv), 0.3, stone);
            let y = b + 0.3;
            block(
                m,
                f,
                (0.0, cv + 0.1 * hv),
                (0.42 * hu, 0.8 * hv),
                y,
                y + 15.0,
                sc.wall,
                sc.wall,
            );
            block(
                m,
                f,
                (0.0, cv + 0.1 * hv),
                (0.34 * hu, 0.8 * hv),
                y + 15.0,
                y + 18.0,
                sc.roof,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv + 0.1 * hv),
                (0.2 * hu, 0.8 * hv),
                y + 18.0,
                y + 20.0,
                sc.roof,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv + 0.2 * hv),
                (0.9 * hu, 0.22 * hv),
                y,
                y + 12.0,
                sc.wall,
                sc.roof,
            );
            for s in [-1.0f32, 1.0] {
                let u = s * 0.3 * hu;
                block(
                    m,
                    f,
                    (u, cv - 0.7 * hv),
                    (0.16 * hu, 0.16 * hv),
                    y,
                    y + 32.0,
                    sc.wall,
                    sc.trim,
                );
                block(
                    m,
                    f,
                    (u, cv - 0.7 * hv),
                    (0.19 * hu, 0.19 * hv),
                    y + 32.0,
                    y + 33.2,
                    sc.trim,
                    sc.trim,
                );
                cone(
                    m,
                    f,
                    (u, cv - 0.7 * hv),
                    0.2 * r,
                    (y + 33.2, y + 52.0),
                    4,
                    sc.roof,
                );
            }
            // The rose window and the portal between the towers.
            block(
                m,
                f,
                (0.0, cv - 0.74 * hv),
                (0.12 * hu, 0.02 * hv),
                y + 15.0,
                y + 24.0,
                sc.glass,
                sc.glass,
            );
            block(
                m,
                f,
                (0.0, cv - 0.76 * hv),
                (0.1 * hu, 0.02 * hv),
                y,
                y + 8.0,
                mul(sc.trim, 0.6),
                sc.trim,
            );
            if near {
                cone(
                    m,
                    f,
                    (0.0, cv + 0.2 * hv),
                    0.1 * r,
                    (y + 12.0, y + 30.0),
                    4,
                    sc.roof,
                );
            }
        }
        Massing::GoldenArch => {
            // A tall gold arch on a stepped podium.
            pad(m, (hu, hv), 0.3, stone);
            let y = b + 0.3;
            block(
                m,
                f,
                (0.0, cv),
                (0.95 * hu, 0.6 * hv),
                y,
                y + 1.2,
                stone,
                stone,
            );
            block(
                m,
                f,
                (0.0, cv),
                (0.8 * hu, 0.45 * hv),
                y + 1.2,
                y + 2.4,
                sc.trim,
                sc.trim,
            );
            let y = y + 2.4;
            let segs = if near {
                18
            } else if far {
                8
            } else {
                12
            };
            let (a, h, t) = (0.72 * hu, 44.0, 2.6);
            for k in 0..segs {
                let ang = |i: usize| i as f32 / segs as f32 * std::f32::consts::PI;
                let (a0, a1) = (ang(k), ang(k + 1));
                let (u0, u1) = (a * a0.cos(), a * a1.cos());
                let (y0, y1) = (h * a0.sin(), h * a1.sin());
                let (lo, hi) = (y0.min(y1), y0.max(y1));
                let thick = t + 0.5 * (u1 - u0).abs();
                block(
                    m,
                    f,
                    ((u0 + u1) * 0.5, cv),
                    (thick * 0.5 + 0.6, 0.18 * hv),
                    y + lo,
                    y + hi.max(lo + 0.5),
                    sc.wall,
                    sc.wall,
                );
            }
        }
        Massing::SkylineSpire => {
            // A very tall glass tower tapering in steps to a needle.
            pad(m, (0.9 * hu, 0.9 * hv), 0.4, stone);
            let y = b + 0.4;
            let steps: [(f32, f32); 5] = [
                (0.62, 34.0),
                (0.5, 28.0),
                (0.39, 24.0),
                (0.29, 20.0),
                (0.2, 16.0),
            ];
            let mut h = y;
            let take = if far { 4 } else { 5 };
            for (i, &(w, rise)) in steps.iter().enumerate().take(take) {
                let col = if i % 2 == 0 { sc.glass } else { sc.wall };
                block(m, f, (0.0, cv), (w * hu, w * hv), h, h + rise, col, sc.roof);
                if near {
                    block(
                        m,
                        f,
                        (0.0, cv),
                        ((w + 0.03) * hu, (w + 0.03) * hv),
                        h + rise - 1.0,
                        h + rise,
                        sc.trim,
                        sc.trim,
                    );
                }
                h += rise;
            }
            cone(m, f, (0.0, cv), 0.06 * r, (h, h + 38.0), 4, sc.trim);
        }
        _ => spec(m, massing, sc),
    }
}

/// The district specialisation buildings (T-198, T-199): low-rise shapes that scale with the lot
/// (a small, medium or large one of each).
fn spec(m: &mut MeshData, massing: Massing, sc: &ServiceCx) {
    use super::Lod;
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
    let r = hu.min(hv);
    let pad = |m: &mut MeshData, top: f32| {
        block(
            m,
            f,
            (0.0, cv),
            (0.96 * hu, 0.96 * hv),
            sc.lo - 0.3,
            b + top,
            fnd,
            [0.55, 0.55, 0.53, 1.0],
        );
    };
    match massing {
        Massing::SpecLeisure => {
            // A club: a flat-roofed box with neon bands on the front and a roof terrace.
            pad(m, 0.2);
            let y = b + 0.2;
            let h = 6.5 + 0.12 * r;
            block(
                m,
                f,
                (0.0, cv),
                (0.86 * hu, 0.8 * hv),
                y,
                y + h,
                sc.wall,
                sc.roof,
            );
            for (k, col) in [[1.0, 0.3, 0.85, 1.0], [0.3, 0.9, 1.0, 1.0]]
                .into_iter()
                .enumerate()
            {
                let y0 = y + 1.6 + 2.2 * k as f32;
                block(
                    m,
                    f,
                    (0.0, cv - 0.8 * hv),
                    (0.86 * hu, 0.04 * hv),
                    y0,
                    y0 + 0.5,
                    col,
                    col,
                );
            }
            // The terrace: a parapet and a few tables with umbrellas.
            block(
                m,
                f,
                (0.0, cv),
                (0.86 * hu, 0.8 * hv),
                y + h,
                y + h + 0.8,
                sc.trim,
                sc.trim,
            );
            if !far {
                let tables = if near { 4 } else { 2 };
                for k in 0..tables {
                    let u = (-0.55 + 1.1 * k as f32 / (tables - 1).max(1) as f32) * hu;
                    cyl(
                        m,
                        f,
                        (u, cv + 0.1 * hv),
                        (0.9, 0.9),
                        (y + h + 0.8, y + h + 1.2),
                        n,
                        sc.glass,
                        Some(sc.glass),
                    );
                }
            }
            // A sign pylon.
            block(
                m,
                f,
                (0.7 * hu, cv - 0.6 * hv),
                (0.05 * hu, 0.05 * hv),
                y,
                y + h + 5.0,
                sc.trim,
                sc.trim,
            );
            if near {
                block(
                    m,
                    f,
                    (0.7 * hu, cv - 0.6 * hv),
                    (0.12 * hu, 0.03 * hv),
                    y + h + 2.0,
                    y + h + 5.0,
                    [1.0, 0.3, 0.85, 1.0],
                    sc.trim,
                );
            }
        }
        Massing::SpecTourism => {
            // A hotel behind, souvenir shops with striped awnings in front, a sign tower.
            pad(m, 0.2);
            let y = b + 0.2;
            let tall = 10.0 + 0.2 * r;
            block(
                m,
                f,
                (0.0, cv + 0.35 * hv),
                (0.8 * hu, 0.5 * hv),
                y,
                y + tall,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv - 0.5 * hv),
                (0.86 * hu, 0.32 * hv),
                y,
                y + 4.0,
                sc.trim,
                sc.roof,
            );
            let stripes = if far {
                3
            } else if near {
                8
            } else {
                6
            };
            for k in 0..stripes {
                let u = (-0.8 + 1.6 * (k as f32 + 0.5) / stripes as f32) * hu;
                let col = if k % 2 == 0 {
                    [0.85, 0.2, 0.18, 1.0]
                } else {
                    [0.96, 0.94, 0.9, 1.0]
                };
                block(
                    m,
                    f,
                    (u, cv - 0.86 * hv),
                    (0.8 * hu / stripes as f32, 0.16 * hv),
                    y + 3.0,
                    y + 3.7,
                    col,
                    col,
                );
            }
            // The sign tower on the roof.
            block(
                m,
                f,
                (0.5 * hu, cv + 0.35 * hv),
                (0.06 * hu, 0.06 * hv),
                y + tall,
                y + tall + 7.0,
                sc.trim,
                sc.trim,
            );
            if !far {
                block(
                    m,
                    f,
                    (0.5 * hu, cv + 0.35 * hv),
                    (0.2 * hu, 0.04 * hv),
                    y + tall + 4.0,
                    y + tall + 7.0,
                    sc.glass,
                    sc.trim,
                );
            }
        }
        Massing::SpecOrganic => {
            // Wooden shops under a green roof, a canopy on posts in front, planters.
            pad(m, 0.2);
            let y = b + 0.2;
            let h = 4.5 + 0.06 * r;
            block(
                m,
                f,
                (0.0, cv + 0.15 * hv),
                (0.84 * hu, 0.6 * hv),
                y,
                y + h,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv + 0.15 * hv),
                (0.9 * hu, 0.66 * hv),
                y + h,
                y + h + 0.7,
                sc.roof,
                sc.roof,
            );
            let posts = if far { 2 } else { 4 };
            for k in 0..posts {
                let u = (-0.8 + 1.6 * k as f32 / (posts - 1) as f32) * hu;
                cyl(
                    m,
                    f,
                    (u, cv - 0.7 * hv),
                    (0.25, 0.25),
                    (y, y + 3.0),
                    6,
                    sc.trim,
                    None,
                );
            }
            block(
                m,
                f,
                (0.0, cv - 0.5 * hv),
                (0.9 * hu, 0.3 * hv),
                y + 3.0,
                y + 3.5,
                sc.roof,
                sc.roof,
            );
            if near {
                for k in 0..3 {
                    let u = (-0.6 + 0.6 * k as f32) * hu;
                    block(
                        m,
                        f,
                        (u, cv - 0.86 * hv),
                        (0.12 * hu, 0.06 * hv),
                        y,
                        y + 0.9,
                        [0.4, 0.75, 0.3, 1.0],
                        [0.3, 0.7, 0.25, 1.0],
                    );
                }
            }
        }
        Massing::SpecItCluster => {
            // A glass campus: two round-cornered wings around a taller atrium.
            pad(m, 0.2);
            let y = b + 0.2;
            let h = 9.0 + 0.2 * r;
            for s in [-1.0f32, 1.0] {
                cyl(
                    m,
                    f,
                    (s * 0.5 * hu, cv),
                    (0.46 * r, 0.46 * r),
                    (y, y + h),
                    n,
                    sc.glass,
                    Some(sc.roof),
                );
            }
            block(
                m,
                f,
                (0.0, cv),
                (0.5 * hu, 0.2 * hv),
                y,
                y + h * 0.8,
                sc.wall,
                sc.roof,
            );
            cyl(
                m,
                f,
                (0.0, cv),
                (0.26 * r, 0.22 * r),
                (y, y + h + 5.0),
                n,
                sc.glass,
                Some(sc.trim),
            );
            if near {
                for s in [-1.0f32, 1.0] {
                    cyl(
                        m,
                        f,
                        (s * 0.5 * hu, cv),
                        (0.47 * r, 0.47 * r),
                        (y + h * 0.5, y + h * 0.5 + 0.4),
                        n,
                        sc.trim,
                        None,
                    );
                }
            }
        }
        Massing::SpecSelfSufficient => {
            // A house with solar panels on the roof slope and a small wind mast on the lot.
            pad(m, 0.2);
            let y = b + 0.2;
            let h = 3.6 + 0.04 * r;
            block(
                m,
                f,
                (-0.15 * hu, cv),
                (0.55 * hu, 0.6 * hv),
                y,
                y + h,
                sc.wall,
                sc.wall,
            );
            block(
                m,
                f,
                (-0.15 * hu, cv),
                (0.6 * hu, 0.66 * hv),
                y + h,
                y + h + 1.4,
                sc.roof,
                sc.roof,
            );
            block(
                m,
                f,
                (-0.15 * hu, cv),
                (0.4 * hu, 0.6 * hv),
                y + h + 1.4,
                y + h + 2.6,
                sc.roof,
                sc.roof,
            );
            let panel = [0.16, 0.26, 0.55, 1.0];
            block(
                m,
                f,
                (-0.15 * hu, cv - 0.3 * hv),
                (0.32 * hu, 0.25 * hv),
                y + h + 1.4,
                y + h + 1.9,
                panel,
                panel,
            );
            // The mast: a thin pole and a hub with three short blades.
            let top = 10.0 + 0.1 * r;
            cyl(
                m,
                f,
                (0.72 * hu, cv + 0.6 * hv),
                (0.22, 0.14),
                (y, y + top),
                6,
                sc.trim,
                None,
            );
            if !far {
                block(
                    m,
                    f,
                    (0.72 * hu, cv + 0.6 * hv),
                    (0.3, 0.3),
                    y + top,
                    y + top + 0.6,
                    sc.trim,
                    sc.trim,
                );
                for k in 0..3 {
                    let a = k as f32 * std::f32::consts::TAU / 3.0 + 0.4;
                    block(
                        m,
                        f,
                        (0.72 * hu + 1.3 * a.cos(), cv + 0.6 * hv - 0.2),
                        (0.12, 0.08),
                        y + top + 0.3 + 1.3 * a.sin(),
                        y + top + 0.5 + 1.3 * a.sin(),
                        sc.trim,
                        sc.trim,
                    );
                }
            }
            if near {
                block(
                    m,
                    f,
                    (-0.7 * hu, cv - 0.55 * hv),
                    (0.1 * hu, 0.06 * hv),
                    y,
                    y + 1.0,
                    [0.4, 0.75, 0.3, 1.0],
                    [0.3, 0.7, 0.25, 1.0],
                );
            }
        }
        _ => chain(m, massing, sc),
    }
}

const SOIL: [f32; 4] = [0.45, 0.34, 0.22, 1.0];
const CROP_GOLD: [f32; 4] = [0.88, 0.76, 0.28, 1.0];
const CROP_GREEN: [f32; 4] = [0.45, 0.65, 0.25, 1.0];
const BARN_RED: [f32; 4] = [0.66, 0.22, 0.18, 1.0];
const LOG: [f32; 4] = [0.50, 0.33, 0.18, 1.0];
const PLANK: [f32; 4] = [0.86, 0.74, 0.52, 1.0];
const STEEL: [f32; 4] = [0.60, 0.62, 0.66, 1.0];
const DARK: [f32; 4] = [0.20, 0.20, 0.22, 1.0];
const ORE_PILE: [f32; 4] = [0.45, 0.40, 0.38, 1.0];
const TANK_WHITE: [f32; 4] = [0.90, 0.90, 0.88, 1.0];

/// The M5 chain buildings (T-212, T-213): farm, forest, ore and oil, each with a main building,
/// two extractors (small and large), two processors and a warehouse. Massing `40 + 6 r + k` is
/// resource `r` (farm, forest, ore, oil) and role `k` (main, extractor S, extractor L,
/// processor A, processor B, warehouse). Fields and clearings are flat (no box under them).
fn chain(m: &mut MeshData, massing: Massing, sc: &ServiceCx) {
    use super::Lod;
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
    let r = hu.min(hv);
    let idx = massing as u8 - Massing::FarmMain as u8;
    let (res, role) = (idx / 6, idx % 6);
    // The pad the building stands on; a field or clearing has soil instead.
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
    let concrete = [0.55, 0.55, 0.53, 1.0];
    match (res, role) {
        // ---- Farming ----
        (0, 0) => {
            // The homestead: a house and a red barn with a silo.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            block(
                m,
                f,
                (-0.45 * hu, cv - 0.2 * hv),
                (0.35 * hu, 0.4 * hv),
                y,
                y + 4.5,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (-0.45 * hu, cv - 0.2 * hv),
                (0.4 * hu, 0.45 * hv),
                y + 4.5,
                y + 6.0,
                sc.roof,
                sc.roof,
            );
            block(
                m,
                f,
                (0.35 * hu, cv + 0.2 * hv),
                (0.45 * hu, 0.5 * hv),
                y,
                y + 6.5,
                BARN_RED,
                sc.roof,
            );
            block(
                m,
                f,
                (0.35 * hu, cv + 0.2 * hv),
                (0.4 * hu, 0.45 * hv),
                y + 6.5,
                y + 8.5,
                sc.roof,
                sc.roof,
            );
            cyl(
                m,
                f,
                (0.75 * hu, cv - 0.5 * hv),
                (0.2 * r, 0.2 * r),
                (y, y + 11.0),
                n,
                STEEL,
                Some(sc.trim),
            );
            if near {
                block(
                    m,
                    f,
                    (0.35 * hu, cv - 0.31 * hv),
                    (0.18 * hu, 0.02 * hv),
                    y,
                    y + 4.0,
                    sc.trim,
                    sc.trim,
                );
            }
        }
        (0, 1) | (0, 2) => {
            // A field with stripes of crops and a shed; the large one has a silo tower.
            ground(m, 0.1, SOIL);
            let y = b + 0.1;
            let rows = if far {
                4
            } else if near {
                10
            } else {
                7
            };
            for k in 0..rows {
                let t = -0.85 + 1.7 * (k as f32 + 0.5) / rows as f32;
                let col = if k % 2 == 0 { CROP_GOLD } else { CROP_GREEN };
                block(
                    m,
                    f,
                    (0.0, cv + t * hv * 0.85),
                    (0.9 * hu, 0.8 * hv / rows as f32),
                    y,
                    y + 0.35,
                    col,
                    col,
                );
            }
            block(
                m,
                f,
                (0.7 * hu, cv - 0.8 * hv),
                (0.2 * hu, 0.14 * hv),
                y,
                y + 3.0,
                sc.wall,
                sc.roof,
            );
            if role == 2 {
                cyl(
                    m,
                    f,
                    (0.7 * hu, cv + 0.6 * hv),
                    (0.16 * r, 0.16 * r),
                    (y, y + 12.0),
                    n,
                    STEEL,
                    Some(sc.roof),
                );
            }
        }
        (0, 3) => {
            // The cowshed: a long low barn under a long roof.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            block(
                m,
                f,
                (0.0, cv),
                (0.85 * hu, 0.5 * hv),
                y,
                y + 4.0,
                BARN_RED,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv),
                (0.92 * hu, 0.58 * hv),
                y + 4.0,
                y + 4.8,
                sc.roof,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv),
                (0.7 * hu, 0.4 * hv),
                y + 4.8,
                y + 6.2,
                sc.roof,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv - 0.5 * hv),
                (0.7 * hu, 0.04 * hv),
                y,
                y + 3.0,
                DARK,
                DARK,
            );
            if near {
                for k in 0..3 {
                    block(
                        m,
                        f,
                        ((-0.5 + 0.5 * k as f32) * hu, cv + 0.78 * hv),
                        (0.14 * hu, 0.1 * hv),
                        y,
                        y + 1.0,
                        STEEL,
                        STEEL,
                    );
                }
            }
        }
        (0, 4) => {
            // The mill: a tall elevator with three silos beside it.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            block(
                m,
                f,
                (-0.3 * hu, cv),
                (0.3 * hu, 0.35 * hv),
                y,
                y + 16.0,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (-0.3 * hu, cv),
                (0.12 * hu, 0.15 * hv),
                y + 16.0,
                y + 20.0,
                sc.trim,
                sc.roof,
            );
            for k in 0..3 {
                cyl(
                    m,
                    f,
                    (0.35 * hu, cv + (-0.5 + 0.5 * k as f32) * hv),
                    (0.16 * r, 0.16 * r),
                    (y, y + 9.0),
                    n,
                    STEEL,
                    Some(sc.roof),
                );
            }
            block(
                m,
                f,
                (0.05 * hu, cv),
                (0.12 * hu, 0.05 * hv),
                y + 8.0,
                y + 8.6,
                DARK,
                DARK,
            );
        }
        (0, 5) => {
            // The grain store: a hangar with a barrel roof.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            block(
                m,
                f,
                (0.0, cv),
                (0.9 * hu, 0.8 * hv),
                y,
                y + 5.0,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv),
                (0.9 * hu, 0.6 * hv),
                y + 5.0,
                y + 6.5,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv),
                (0.9 * hu, 0.3 * hv),
                y + 6.5,
                y + 7.5,
                sc.roof,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv - 0.8 * hv),
                (0.4 * hu, 0.03 * hv),
                y,
                y + 4.0,
                DARK,
                DARK,
            );
            if near {
                block(
                    m,
                    f,
                    (0.0, cv + 0.8 * hv),
                    (0.12 * hu, 0.04 * hv),
                    y + 5.0,
                    y + 6.2,
                    STEEL,
                    STEEL,
                );
            }
        }
        // ---- Forestry ----
        (1, 0) => {
            // A log office with a stone chimney.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            block(
                m,
                f,
                (0.0, cv),
                (0.6 * hu, 0.5 * hv),
                y,
                y + 4.5,
                LOG,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv),
                (0.68 * hu, 0.56 * hv),
                y + 4.5,
                y + 5.5,
                sc.roof,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv),
                (0.45 * hu, 0.4 * hv),
                y + 5.5,
                y + 7.0,
                sc.roof,
                sc.roof,
            );
            block(
                m,
                f,
                (0.45 * hu, cv + 0.2 * hv),
                (0.07 * hu, 0.07 * hv),
                y,
                y + 9.0,
                STEEL,
                STEEL,
            );
            if near {
                block(
                    m,
                    f,
                    (-0.7 * hu, cv - 0.6 * hv),
                    (0.2 * hu, 0.1 * hv),
                    y,
                    y + 1.2,
                    PLANK,
                    PLANK,
                );
            }
        }
        (1, 1) | (1, 2) => {
            // A clearing with rows of log piles; the large one also has a loader post.
            ground(m, 0.1, SOIL);
            let y = b + 0.1;
            let piles = if far { 2 } else { 4 };
            for k in 0..piles {
                let v = (-0.6 + 1.2 * k as f32 / (piles - 1).max(1) as f32) * hv;
                let h = if near { 1.8 } else { 1.6 };
                block(
                    m,
                    f,
                    (-0.2 * hu, cv + v),
                    (0.6 * hu, 0.12 * hv),
                    y,
                    y + h,
                    LOG,
                    PLANK,
                );
                if near {
                    block(
                        m,
                        f,
                        (-0.2 * hu, cv + v),
                        (0.55 * hu, 0.08 * hv),
                        y + h,
                        y + h + 1.0,
                        LOG,
                        PLANK,
                    );
                }
            }
            block(
                m,
                f,
                (0.7 * hu, cv - 0.7 * hv),
                (0.18 * hu, 0.12 * hv),
                y,
                y + 3.0,
                sc.wall,
                sc.roof,
            );
            if role == 2 {
                cyl(
                    m,
                    f,
                    (0.7 * hu, cv + 0.4 * hv),
                    (0.4, 0.3),
                    (y, y + 10.0),
                    6,
                    STEEL,
                    None,
                );
                block(
                    m,
                    f,
                    (0.5 * hu, cv + 0.4 * hv),
                    (0.2 * hu, 0.04 * hv),
                    y + 9.0,
                    y + 9.6,
                    STEEL,
                    STEEL,
                );
            }
        }
        (1, 3) => {
            // The sawmill: a long shed with an open awning, logs on one side, planks on the other.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            block(
                m,
                f,
                (0.0, cv + 0.3 * hv),
                (0.85 * hu, 0.4 * hv),
                y,
                y + 5.0,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (0.0, cv + 0.3 * hv),
                (0.9 * hu, 0.45 * hv),
                y + 5.0,
                y + 5.7,
                sc.roof,
                sc.roof,
            );
            for k in 0..4 {
                let u = (-0.8 + 1.6 * k as f32 / 3.0) * hu;
                cyl(
                    m,
                    f,
                    (u, cv - 0.75 * hv),
                    (0.3, 0.3),
                    (y, y + 3.5),
                    n,
                    LOG,
                    None,
                );
            }
            block(
                m,
                f,
                (0.0, cv - 0.6 * hv),
                (0.9 * hu, 0.2 * hv),
                y + 3.5,
                y + 4.0,
                sc.roof,
                sc.roof,
            );
            block(
                m,
                f,
                (-0.6 * hu, cv - 0.55 * hv),
                (0.2 * hu, 0.12 * hv),
                y,
                y + 1.4,
                LOG,
                LOG,
            );
            block(
                m,
                f,
                (0.6 * hu, cv - 0.55 * hv),
                (0.2 * hu, 0.12 * hv),
                y,
                y + 1.2,
                PLANK,
                PLANK,
            );
        }
        (1, 4) => {
            // The paper mill: a big hall, a tall chimney and two tanks.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            block(
                m,
                f,
                (-0.2 * hu, cv),
                (0.6 * hu, 0.6 * hv),
                y,
                y + 8.0,
                sc.wall,
                sc.roof,
            );
            cyl(
                m,
                f,
                (0.65 * hu, cv + 0.4 * hv),
                (0.12 * r, 0.09 * r),
                (y, y + 22.0),
                n,
                STEEL,
                Some(DARK),
            );
            for k in 0..2 {
                cyl(
                    m,
                    f,
                    (0.6 * hu, cv + (-0.6 + 0.4 * k as f32) * hv),
                    (0.18 * r, 0.18 * r),
                    (y, y + 5.0),
                    n,
                    TANK_WHITE,
                    Some(sc.roof),
                );
            }
            if near {
                for k in 0..3 {
                    block(
                        m,
                        f,
                        ((-0.6 + 0.4 * k as f32) * hu, cv),
                        (0.08 * hu, 0.08 * hv),
                        y + 8.0,
                        y + 9.4,
                        STEEL,
                        STEEL,
                    );
                }
            }
        }
        (1, 5) => {
            // An open shed on posts with stacks of planks under it.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            for (u, v) in [
                (-0.85, -0.7),
                (0.85, -0.7),
                (-0.85, 0.7),
                (0.85, 0.7),
                (0.0, -0.7),
                (0.0, 0.7),
            ] {
                block(
                    m,
                    f,
                    (u * hu, cv + v * hv),
                    (0.3, 0.3),
                    y,
                    y + 5.0,
                    LOG,
                    LOG,
                );
            }
            block(
                m,
                f,
                (0.0, cv),
                (0.95 * hu, 0.85 * hv),
                y + 5.0,
                y + 5.8,
                sc.roof,
                sc.roof,
            );
            for k in 0..3 {
                let u = (-0.55 + 0.55 * k as f32) * hu;
                block(
                    m,
                    f,
                    (u, cv),
                    (0.2 * hu, 0.5 * hv),
                    y,
                    y + 2.5 - 0.4 * k as f32,
                    PLANK,
                    PLANK,
                );
            }
        }
        // ---- Ore ----
        (2, 0) => {
            // An office with a head frame.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            block(
                m,
                f,
                (-0.4 * hu, cv),
                (0.45 * hu, 0.5 * hv),
                y,
                y + 4.5,
                sc.wall,
                sc.roof,
            );
            headframe(m, f, (0.5 * hu, cv), 0.28 * r, y, 15.0, sc.trim, near);
        }
        (2, 1) => {
            // An open pit: stepped terraces with a conveyor and a hopper.
            ground(m, 0.15, ORE_PILE);
            let y = b + 0.15;
            for (k, w) in [0.9f32, 0.7, 0.5, 0.3].into_iter().enumerate() {
                let c = 0.9 - 0.18 * k as f32;
                block(
                    m,
                    f,
                    (0.0, cv),
                    (w * hu, w * hv),
                    y + 0.3 * k as f32,
                    y + 0.3 * (k + 1) as f32,
                    [0.5 * c, 0.42 * c, 0.34 * c, 1.0],
                    [0.55 * c, 0.46 * c, 0.38 * c, 1.0],
                );
            }
            block(
                m,
                f,
                (0.55 * hu, cv),
                (0.35 * hu, 0.05 * hv),
                y + 1.0,
                y + 1.5,
                STEEL,
                DARK,
            );
            block(
                m,
                f,
                (0.85 * hu, cv),
                (0.08 * hu, 0.12 * hv),
                y,
                y + 3.5,
                sc.wall,
                sc.roof,
            );
            if near {
                block(
                    m,
                    f,
                    (-0.3 * hu, cv - 0.8 * hv),
                    (0.1 * hu, 0.08 * hv),
                    y,
                    y + 1.5,
                    sc.trim,
                    sc.trim,
                );
            }
        }
        (2, 2) => {
            // A mine: a tall head frame over a shaft house, ore heaps beside it.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            block(
                m,
                f,
                (-0.2 * hu, cv),
                (0.4 * hu, 0.4 * hv),
                y,
                y + 4.5,
                sc.wall,
                sc.roof,
            );
            headframe(
                m,
                f,
                (-0.2 * hu, cv),
                0.34 * r,
                y + 4.5,
                20.0,
                sc.trim,
                near,
            );
            cone(
                m,
                f,
                (0.6 * hu, cv - 0.4 * hv),
                0.28 * r,
                (y, y + 4.0),
                n,
                ORE_PILE,
            );
            if !far {
                cone(
                    m,
                    f,
                    (0.65 * hu, cv + 0.4 * hv),
                    0.22 * r,
                    (y, y + 3.0),
                    n,
                    ORE_PILE,
                );
            }
        }
        (2, 3) => {
            // A blast furnace: a tall tapered stack, hot-blast stoves and pipes.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            cyl(
                m,
                f,
                (-0.2 * hu, cv),
                (0.3 * r, 0.2 * r),
                (y, y + 8.0),
                n,
                STEEL,
                None,
            );
            cyl(
                m,
                f,
                (-0.2 * hu, cv),
                (0.2 * r, 0.12 * r),
                (y + 8.0, y + 20.0),
                n,
                sc.roof,
                Some(DARK),
            );
            for k in 0..2 {
                cyl(
                    m,
                    f,
                    (0.5 * hu, cv + (-0.4 + 0.8 * k as f32) * hv),
                    (0.14 * r, 0.14 * r),
                    (y, y + 14.0),
                    n,
                    sc.wall,
                    Some(sc.roof),
                );
            }
            block(
                m,
                f,
                (0.15 * hu, cv),
                (0.35 * hu, 0.04 * hv),
                y + 10.0,
                y + 10.8,
                DARK,
                DARK,
            );
            if near {
                block(
                    m,
                    f,
                    (-0.2 * hu, cv - 0.6 * hv),
                    (0.2 * hu, 0.15 * hv),
                    y,
                    y + 3.0,
                    sc.wall,
                    sc.roof,
                );
            }
        }
        (2, 4) => {
            // A glassworks: a hall, a domed furnace and a chimney.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            block(
                m,
                f,
                (-0.2 * hu, cv),
                (0.65 * hu, 0.6 * hv),
                y,
                y + 6.0,
                sc.wall,
                sc.roof,
            );
            cyl(
                m,
                f,
                (0.5 * hu, cv),
                (0.3 * r, 0.3 * r),
                (y, y + 5.0),
                n,
                sc.glass,
                None,
            );
            cyl(
                m,
                f,
                (0.5 * hu, cv),
                (0.3 * r, 0.1 * r),
                (y + 5.0, y + 9.0),
                n,
                sc.glass,
                Some(sc.trim),
            );
            cyl(
                m,
                f,
                (-0.7 * hu, cv + 0.6 * hv),
                (0.1 * r, 0.08 * r),
                (y, y + 16.0),
                n,
                STEEL,
                Some(DARK),
            );
            if near {
                block(
                    m,
                    f,
                    (-0.2 * hu, cv - 0.6 * hv),
                    (0.4 * hu, 0.03 * hv),
                    y + 2.0,
                    y + 4.0,
                    sc.glass,
                    sc.glass,
                );
            }
        }
        (2, 5) => {
            // Bunkers: hoppers on legs.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            let k = if far { 2 } else { 3 };
            for i in 0..k {
                let u = (-0.6 + 1.2 * i as f32 / (k - 1) as f32) * hu;
                for (du, dv) in [(-0.8, -0.8), (0.8, 0.8)] {
                    block(
                        m,
                        f,
                        (u + du * 0.16 * r, cv + dv * 0.16 * r),
                        (0.3, 0.3),
                        y,
                        y + 4.0,
                        STEEL,
                        STEEL,
                    );
                }
                cyl(
                    m,
                    f,
                    (u, cv),
                    (0.1 * r, 0.2 * r),
                    (y + 3.0, y + 9.0),
                    n,
                    sc.wall,
                    Some(sc.roof),
                );
            }
        }
        // ---- Oil ----
        (3, 0) => {
            // An office with a derrick as its logo.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            block(
                m,
                f,
                (-0.4 * hu, cv),
                (0.45 * hu, 0.5 * hv),
                y,
                y + 4.5,
                sc.wall,
                sc.roof,
            );
            cyl(
                m,
                f,
                (0.5 * hu, cv),
                (0.28 * r, 0.05 * r),
                (y, y + 22.0),
                4,
                STEEL,
                Some(DARK),
            );
            if near {
                block(
                    m,
                    f,
                    (0.5 * hu, cv),
                    (0.3 * r, 0.3 * r),
                    y + 8.0,
                    y + 8.5,
                    DARK,
                    DARK,
                );
            }
        }
        (3, 1) => {
            // A pumpjack: a base, an A-frame, a walking beam with a horse head and a weight.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            block(
                m,
                f,
                (0.0, cv),
                (0.3 * hu, 0.3 * hv),
                y,
                y + 0.8,
                DARK,
                DARK,
            );
            block(
                m,
                f,
                (0.0, cv),
                (0.06 * hu, 0.1 * hv),
                y + 0.8,
                y + 5.5,
                STEEL,
                STEEL,
            );
            block(
                m,
                f,
                (0.0, cv),
                (0.6 * hu, 0.06 * hv),
                y + 5.5,
                y + 6.2,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (-0.55 * hu, cv),
                (0.08 * hu, 0.08 * hv),
                y + 3.8,
                y + 6.2,
                sc.wall,
                sc.roof,
            );
            block(
                m,
                f,
                (0.55 * hu, cv),
                (0.1 * hu, 0.08 * hv),
                y + 4.0,
                y + 5.5,
                DARK,
                DARK,
            );
            if near {
                block(
                    m,
                    f,
                    (0.8 * hu, cv),
                    (0.1 * hu, 0.15 * hv),
                    y,
                    y + 2.0,
                    sc.trim,
                    sc.trim,
                );
            }
        }
        (3, 2) => {
            // A drilling rig: a tall tapered derrick on a platform with a shed.
            ground(m, 0.3, concrete);
            let y = b + 0.3;
            block(
                m,
                f,
                (0.0, cv),
                (0.5 * hu, 0.5 * hv),
                y,
                y + 1.0,
                DARK,
                DARK,
            );
            cyl(
                m,
                f,
                (0.0, cv),
                (0.3 * r, 0.05 * r),
                (y + 1.0, y + 32.0),
                4,
                STEEL,
                Some(sc.trim),
            );
            block(
                m,
                f,
                (0.7 * hu, cv - 0.6 * hv),
                (0.2 * hu, 0.14 * hv),
                y,
                y + 3.0,
                sc.wall,
                sc.roof,
            );
            if near {
                block(
                    m,
                    f,
                    (0.0, cv),
                    (0.2 * r, 0.2 * r),
                    y + 12.0,
                    y + 12.6,
                    DARK,
                    DARK,
                );
            }
        }
        (3, 3) => {
            // A refinery: three distillation columns, a flare and a pipe rack.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            for (k, h) in [24.0f32, 19.0, 15.0].into_iter().enumerate() {
                cyl(
                    m,
                    f,
                    ((-0.6 + 0.5 * k as f32) * hu, cv),
                    (0.14 * r, 0.12 * r),
                    (y, y + h),
                    n,
                    STEEL,
                    Some(sc.roof),
                );
            }
            cyl(
                m,
                f,
                (0.7 * hu, cv + 0.5 * hv),
                (0.06 * r, 0.05 * r),
                (y, y + 20.0),
                6,
                DARK,
                None,
            );
            cone(
                m,
                f,
                (0.7 * hu, cv + 0.5 * hv),
                0.07 * r,
                (y + 20.0, y + 23.0),
                6,
                [1.0, 0.6, 0.15, 1.0],
            );
            block(
                m,
                f,
                (0.0, cv - 0.5 * hv),
                (0.8 * hu, 0.06 * hv),
                y + 3.0,
                y + 3.6,
                DARK,
                DARK,
            );
            if near {
                cyl(
                    m,
                    f,
                    (0.3 * hu, cv - 0.6 * hv),
                    (0.15 * r, 0.15 * r),
                    (y, y + 3.0),
                    n,
                    TANK_WHITE,
                    Some(sc.roof),
                );
            }
        }
        (3, 4) => {
            // A plastics works: a hall with spherical-looking tanks.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            block(
                m,
                f,
                (-0.25 * hu, cv),
                (0.6 * hu, 0.55 * hv),
                y,
                y + 6.0,
                sc.wall,
                sc.roof,
            );
            for k in 0..3 {
                let v = (-0.55 + 0.55 * k as f32) * hv;
                cyl(
                    m,
                    f,
                    (0.6 * hu, cv + v),
                    (0.12 * r, 0.2 * r),
                    (y, y + 3.0),
                    n,
                    TANK_WHITE,
                    None,
                );
                cyl(
                    m,
                    f,
                    (0.6 * hu, cv + v),
                    (0.2 * r, 0.06 * r),
                    (y + 3.0, y + 6.0),
                    n,
                    TANK_WHITE,
                    Some(sc.trim),
                );
            }
            if near {
                block(
                    m,
                    f,
                    (-0.25 * hu, cv - 0.55 * hv),
                    (0.4 * hu, 0.02 * hv),
                    y + 2.0,
                    y + 3.5,
                    sc.glass,
                    sc.glass,
                );
            }
        }
        (3, 5) => {
            // A tank farm: short fat tanks with cone roofs.
            ground(m, 0.2, concrete);
            let y = b + 0.2;
            let tanks = if far { 3 } else { 6 };
            for k in 0..tanks {
                let (c, rw) = (k % 3, k / 3);
                let u = (-0.6 + 0.6 * c as f32) * hu;
                let v = (-0.45 + 0.9 * rw as f32) * hv * if tanks == 3 { 0.0 } else { 1.0 };
                cyl(
                    m,
                    f,
                    (u, cv + v),
                    (0.22 * r, 0.22 * r),
                    (y, y + 5.0),
                    n,
                    TANK_WHITE,
                    None,
                );
                cone(m, f, (u, cv + v), 0.23 * r, (y + 5.0, y + 6.4), n, sc.roof);
            }
        }
        _ => park(m, massing, sc),
    }
    // Near only: a lamp post at the front corner of every chain building (not of the park
    // areas, T-227: their lots are open ground and the props bring their own lamps).
    if near && !park_area::is_park_area(massing) && !theme_park::is_theme_park(massing) {
        let (u, v) = (-0.92 * hu, cv - 0.92 * hv);
        block(m, f, (u, v), (0.12, 0.12), sc.lo, b + 4.5, STEEL, STEEL);
        block(
            m,
            f,
            (u, v),
            (0.45, 0.2),
            b + 4.5,
            b + 4.8,
            DARK,
            [1.0, 0.95, 0.7, 1.0],
        );
    }
}

/// A head frame: four slanted legs under a wheel house, `h` metres tall, with a pulley wheel.
#[allow(clippy::too_many_arguments)]
fn headframe(
    m: &mut MeshData,
    f: &Frame,
    (cu, cv): (f32, f32),
    w: f32,
    y: f32,
    h: f32,
    col: [f32; 4],
    near: bool,
) {
    cyl(m, f, (cu, cv), (w, 0.3 * w), (y, y + h), 4, col, None);
    block(
        m,
        f,
        (cu, cv),
        (0.45 * w, 0.45 * w),
        y + h,
        y + h + 1.4,
        DARK,
        DARK,
    );
    if near {
        cyl(
            m,
            f,
            (cu, cv),
            (0.3 * w, 0.3 * w),
            (y + h + 1.4, y + h + 1.8),
            8,
            STEEL,
            Some(col),
        );
    }
}

const GRASS: [f32; 4] = [0.36, 0.62, 0.30, 1.0];
const GRAVEL: [f32; 4] = [0.84, 0.78, 0.62, 1.0];
const LEAF: [f32; 4] = [0.20, 0.50, 0.24, 1.0];
const TRUNK: [f32; 4] = [0.42, 0.28, 0.16, 1.0];

/// A tree: a trunk and a cone of leaves (a rounder second cone near).
fn tree(m: &mut MeshData, f: &Frame, at: (f32, f32), y: f32, h: f32, n: usize, near: bool) {
    cyl(m, f, at, (0.35, 0.3), (y, y + h * 0.4), 6, TRUNK, None);
    cone(m, f, at, h * 0.32, (y + h * 0.3, y + h), n, LEAF);
    if near {
        cone(
            m,
            f,
            at,
            h * 0.25,
            (y + h * 0.55, y + h * 1.15),
            n,
            [0.24, 0.56, 0.28, 1.0],
        );
    }
}

/// One plant of a house garden, in lot-frame metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub u: f32,
    pub v: f32,
    /// 0 conifer, 1 broadleaf, 2 bush.
    pub kind: u8,
    /// Height, metres.
    pub h: f32,
    /// Crown radius, metres.
    pub r: f32,
    /// Leaf tint, `0..1`.
    pub tint: f32,
}

/// Two to four plants for a `w` x `d` m lot (T-234): a real tree 6 to 9 m tall (a conifer or a
/// round broadleaf, in the back or side yard when there is room) and bushes as the extras. The
/// whole crown stays inside the lot and the trunk off the house (`hu`, `hv` half extents
/// centred `cv` along v), plants clear of each other. A function of `seed` alone (the
/// building's id), never of the sim RNG. A lot that cannot take the tree gets what fits.
pub fn garden_spots(w: f32, d: f32, (hu, hv, cv): (f32, f32, f32), seed: u32) -> Vec<Spot> {
    let mut x = seed.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b | 1;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    let frac = |n: u32| (n % 1000) as f32 / 1000.0;
    // A bigger plot takes more green: one more plant per 350 m2 of free ground, two at most.
    let free = (w * d - 4.0 * hu * hv).max(0.0);
    let want = 2 + next() % 3 + ((free / 350.0) as u32).min(2);
    let mut out: Vec<Spot> = Vec::new();
    // The tree first: a smaller one if the lot is tight.
    let kind = (next() % 2) as u8;
    let tint = frac(next());
    let h0 = 6.0 + frac(next()) * 3.0;
    let side_first = next() % 2 == 0;
    for scale in [1.0f32, 0.9, 0.8, 0.7] {
        let h = (h0 * scale).max(5.5);
        let r = if kind == 0 { h * 0.24 } else { h * 0.27 };
        // Where a trunk may stand so that the whole crown is inside the lot ...
        let (u_lo, u_hi) = (-w * 0.5 + r, w * 0.5 - r);
        let (v_lo, v_hi) = (-d * 0.5 + r, d * 0.5 - r);
        if u_lo > u_hi || v_lo > v_hi {
            continue;
        }
        // ... and off the house: the yards, the back one first.
        let back = (u_lo, u_hi, v_lo.max(cv + hv + 0.3), v_hi);
        let left = (u_lo, u_hi.min(-(hu + 0.3)), v_lo, v_hi);
        let right = (u_lo.max(hu + 0.3), u_hi, v_lo, v_hi);
        let front = (u_lo, u_hi, v_lo, v_hi.min(cv - hv - 0.3));
        let sides = if side_first {
            [left, right]
        } else {
            [right, left]
        };
        let yards = [back, sides[0], sides[1], front];
        let Some(&(a, b, c, e)) = yards.iter().find(|y| y.0 <= y.1 && y.2 <= y.3) else {
            continue;
        };
        let (fu, fv) = (frac(next()), frac(next()));
        out.push(Spot {
            u: a + fu * (b - a),
            v: c + fv * (e - c),
            kind,
            h,
            r,
            tint,
        });
        break;
    }
    for _ in 0..40 {
        if out.len() as u32 >= want {
            break;
        }
        let tint = frac(next());
        let h = 1.2 + frac(next()) * 0.9;
        let r = h * 0.5;
        let (ru, rv) = (w - 2.0 * r, d - 2.0 * r);
        let (fu, fv) = (frac(next()), frac(next()));
        if ru <= 0.0 || rv <= 0.0 {
            continue;
        }
        let u = -w * 0.5 + r + fu * ru;
        let v = -d * 0.5 + r + fv * rv;
        if u.abs() < hu + 0.3 + r * 0.4 && (v - cv).abs() < hv + 0.3 + r * 0.4 {
            continue;
        }
        if out
            .iter()
            .any(|o| ((o.u - u).powi(2) + (o.v - v).powi(2)).sqrt() < (o.r + r) * 0.8)
        {
            continue;
        }
        out.push(Spot {
            u,
            v,
            kind: 2,
            h,
            r,
            tint,
        });
    }
    out
}

/// Draws a garden: conifers, round broadleaf trees and bushes standing at `ground(u, v)`.
pub fn garden(
    m: &mut MeshData,
    f: &Frame,
    spots: &[Spot],
    ground: impl Fn(f32, f32) -> f32,
    near: bool,
) {
    let n = if near { 8 } else { 5 };
    for s in spots {
        let y = ground(s.u, s.v);
        let at = (s.u, s.v);
        let t = s.tint;
        let leaf = [0.18 + 0.10 * t, 0.46 + 0.14 * t, 0.2 + 0.08 * t, 1.0];
        match s.kind {
            0 => {
                // Conifer: a trunk and a tall cone (a second, narrower one near).
                cyl(m, f, at, (0.32, 0.26), (y, y + s.h * 0.35), 6, TRUNK, None);
                cone(m, f, at, s.r, (y + s.h * 0.22, y + s.h), n, leaf);
                if near {
                    cone(
                        m,
                        f,
                        at,
                        s.r * 0.72,
                        (y + s.h * 0.5, y + s.h * 1.08),
                        n,
                        mul(leaf, 1.12),
                    );
                }
            }
            1 => {
                cyl(m, f, at, (0.34, 0.26), (y, y + s.h * 0.55), 6, TRUNK, None);
                cyl(
                    m,
                    f,
                    at,
                    (s.r * 0.8, s.r),
                    (y + s.h * 0.42, y + s.h * 0.78),
                    n,
                    leaf,
                    None,
                );
                cone(m, f, at, s.r, (y + s.h * 0.78, y + s.h), n, mul(leaf, 1.12));
            }
            _ => {
                cyl(
                    m,
                    f,
                    at,
                    (s.r, s.r * 0.7),
                    (y, y + s.h * 0.7),
                    n,
                    leaf,
                    None,
                );
                cone(
                    m,
                    f,
                    at,
                    s.r * 0.7,
                    (y + s.h * 0.7, y + s.h),
                    n,
                    mul(leaf, 1.1),
                );
            }
        }
    }
}

/// The city parks (T-221): flat lawns and pads with a few things standing on them.
fn park(m: &mut MeshData, massing: Massing, sc: &ServiceCx) {
    use super::Lod;
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
    let fnd = [0.30, 0.30, 0.30, 1.0];
    let r = hu.min(hv);
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
    match massing {
        Massing::ParkSmall => {
            // A lawn with a looping gravel path and trees round it.
            ground(m, 0.12, GRASS);
            let y = b + 0.12;
            for (u, v, wu, wv) in [
                (0.0, -0.55, 0.6, 0.06),
                (0.0, 0.55, 0.6, 0.06),
                (-0.6, 0.0, 0.06, 0.55),
                (0.6, 0.0, 0.06, 0.55),
            ] {
                block(
                    m,
                    f,
                    (u * hu, cv + v * hv),
                    (wu * hu, wv * hv),
                    y,
                    y + 0.08,
                    GRAVEL,
                    GRAVEL,
                );
            }
            let trees = if far { 3 } else { 6 };
            for k in 0..trees {
                let a = k as f32 / trees as f32 * std::f32::consts::TAU + 0.4;
                tree(
                    m,
                    f,
                    (0.78 * hu * a.cos(), cv + 0.78 * hv * a.sin()),
                    y,
                    5.0 + (k % 3) as f32,
                    n,
                    near,
                );
            }
            tree(m, f, (0.0, cv), y, 7.0, n, near);
            if near {
                block(
                    m,
                    f,
                    (0.0, cv - 0.55 * hv),
                    (0.12 * hu, 0.04 * hv),
                    y + 0.08,
                    y + 0.7,
                    TRUNK,
                    TRUNK,
                );
            }
        }
        Massing::ParkPlayground => {
            // A sandpit with a slide, a swing frame and a low fence.
            ground(m, 0.12, GRASS);
            let y = b + 0.12;
            block(
                m,
                f,
                (0.0, cv),
                (0.8 * hu, 0.8 * hv),
                y,
                y + 0.3,
                [0.88, 0.80, 0.55, 1.0],
                [0.9, 0.83, 0.6, 1.0],
            );
            // The slide: a tower and a ramp of three steps.
            block(
                m,
                f,
                (-0.5 * hu, cv + 0.3 * hv),
                (0.12 * hu, 0.12 * hv),
                y,
                y + 2.4,
                sc.roof,
                sc.roof,
            );
            for k in 0..3 {
                block(
                    m,
                    f,
                    ((-0.3 + 0.12 * k as f32) * hu, cv + 0.3 * hv),
                    (0.08 * hu, 0.08 * hv),
                    y,
                    y + 1.8 - 0.55 * k as f32,
                    sc.trim,
                    sc.trim,
                );
            }
            // The swings: two posts, a beam, two seats.
            for s in [-1.0f32, 1.0] {
                block(
                    m,
                    f,
                    ((0.3 + 0.35 * s) * hu, cv - 0.3 * hv),
                    (0.2, 0.2),
                    y,
                    y + 2.6,
                    STEEL,
                    STEEL,
                );
            }
            block(
                m,
                f,
                (0.3 * hu, cv - 0.3 * hv),
                (0.4 * hu, 0.15),
                y + 2.6,
                y + 2.9,
                STEEL,
                STEEL,
            );
            if !far {
                for s in [-0.15f32, 0.15] {
                    block(
                        m,
                        f,
                        ((0.3 + s) * hu, cv - 0.3 * hv),
                        (0.12 * hu, 0.15),
                        y + 0.7,
                        y + 0.9,
                        sc.roof,
                        sc.roof,
                    );
                }
            }
            if near {
                for k in 0..6 {
                    let u = (-0.9 + 0.36 * k as f32) * hu;
                    block(
                        m,
                        f,
                        (u, cv - 0.92 * hv),
                        (0.1, 0.1),
                        y,
                        y + 1.0,
                        TRUNK,
                        TRUNK,
                    );
                }
            }
        }
        Massing::ParkPlaza => {
            // A paved square: a round basin with a jet, benches and a few trees.
            ground(m, 0.2, [0.80, 0.78, 0.72, 1.0]);
            let y = b + 0.2;
            cyl(
                m,
                f,
                (0.0, cv),
                (0.34 * r, 0.3 * r),
                (y, y + 1.0),
                n.max(8),
                [0.7, 0.7, 0.68, 1.0],
                Some(sc.glass),
            );
            cyl(
                m,
                f,
                (0.0, cv),
                (0.06 * r, 0.04 * r),
                (y, y + 3.4),
                6,
                TRUNK,
                None,
            );
            if !far {
                cone(m, f, (0.0, cv), 0.12 * r, (y + 3.4, y + 4.6), 6, sc.glass);
            }
            for (u, v) in [(-0.75, -0.75), (0.75, 0.75), (-0.75, 0.75), (0.75, -0.75)] {
                tree(m, f, (u * hu, cv + v * hv), y, 5.0, n, near);
            }
            if near {
                for s in [-1.0f32, 1.0] {
                    block(
                        m,
                        f,
                        (0.0, cv + s * 0.55 * hv),
                        (0.3 * hu, 0.04 * hv),
                        y,
                        y + 0.7,
                        TRUNK,
                        TRUNK,
                    );
                }
            }
        }
        Massing::ParkSports => {
            // A pitch with white lines, goals and a small stand along one side.
            ground(m, 0.12, GRASS);
            let y = b + 0.12;
            block(
                m,
                f,
                (0.0, cv),
                (0.7 * hu, 0.8 * hv),
                y,
                y + 0.1,
                [0.30, 0.62, 0.28, 1.0],
                [0.32, 0.66, 0.3, 1.0],
            );
            let white = [0.95, 0.95, 0.92, 1.0];
            block(
                m,
                f,
                (0.0, cv),
                (0.7 * hu, 0.03 * hv),
                y + 0.1,
                y + 0.16,
                white,
                white,
            );
            for s in [-1.0f32, 1.0] {
                block(
                    m,
                    f,
                    (0.0, cv + s * 0.8 * hv),
                    (0.7 * hu, 0.03 * hv),
                    y + 0.1,
                    y + 0.16,
                    white,
                    white,
                );
                block(
                    m,
                    f,
                    (s * 0.7 * hu, cv),
                    (0.03 * hu, 0.8 * hv),
                    y + 0.1,
                    y + 0.16,
                    white,
                    white,
                );
                // The goals.
                block(
                    m,
                    f,
                    (s * 0.7 * hu, cv),
                    (0.3, 0.14 * hv),
                    y,
                    y + 2.4,
                    white,
                    white,
                );
            }
            // The stand: three rising rows.
            for k in 0..3 {
                block(
                    m,
                    f,
                    (0.88 * hu, cv),
                    (0.08 * hu, 0.7 * hv),
                    y,
                    y + 1.0 + 0.9 * k as f32,
                    sc.trim,
                    sc.roof,
                );
                if far {
                    break;
                }
            }
            if near {
                block(
                    m,
                    f,
                    (0.88 * hu, cv),
                    (0.1 * hu, 0.72 * hv),
                    y + 3.0,
                    y + 3.3,
                    sc.roof,
                    sc.roof,
                );
            }
        }
        _ if theme_park::is_theme_park(massing) => theme_park::theme_park(m, massing, sc),
        _ if factory::is_factory(massing) => factory::factory(m, massing, sc),
        _ if post::is_post(massing) => post::post(m, massing, sc),
        _ => park_area::park_area(m, massing, sc),
    }
}
