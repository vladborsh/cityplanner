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
            _ => {}
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
