//! Pure building mesh generation (no Bevy types). See `architecture/system-design.md` §13.3.
//!
//! A building is a box on its lot (`Building::lot` × 8 m cells, inset by a front setback
//! and side margins) with `style.floors` floors, a roof (`Flat`, `Gabled`, `Hipped`) and a
//! facade. **Windows are geometry, not a texture atlas:** every wall is *tiled* exactly with
//! wall-coloured and glass-coloured quads (no overlapping surfaces, so no z-fighting), the
//! pattern (bay width, window size, ribbon glazing, door) chosen by zone family and level.
//! This keeps the material texture-free (one small uniform), which is what made WebGL2
//! affordable for terrain and roads (see `material.rs`).
//!
//! Three detail levels per building: `near` (windows), `mid` (one wall band and one glass
//! band per floor) and `far` (plain box and roof). [`BuildingSet`] batches them per 256 m
//! chunk and rebuilds only buildings whose inputs changed.
//!
//! Facade variation (wall colour from the palette, window shades, bay count) comes from a
//! seed made of the building's id and def, never from RNG state.
//!
//! **Construction**: an `UnderConstruction` building shows one of four stages (a growing
//! concrete core inside a scaffolding frame), derived from `ready_at` and the tick;
//! `Abandoned` is dulled with boarded windows; `Burned` is charred, lower and roofless.
//!
//! The building's *front* (the wall facing the road) is derived from the access point: the
//! direction from the road point to the lot centre. Without a usable access point the lot's
//! +v side is assumed to point away from the road.

use std::collections::{BTreeMap, BTreeSet};

use bevy_math::{Vec2, Vec3};
use cp_building::{Building, BuildingGrid, BuildingId, BuildingState};
use cp_content::{BuildingDef, ContentDb, RoofKind, ZoneType};
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_net::NetGraph;
use cp_terrain::Terrain;
use cp_zone::CELL_SIZE;

use crate::chunk::MeshData;
use crate::road::{append, bounds, flat_quad, push_vertex, tri};

/// Chunk side used to batch buildings, metres.
pub const BUILDING_CHUNK: f32 = 256.0;
/// `near` below this distance, `mid` up to [`FAR_DISTANCE`], `far` beyond.
pub const MID_DISTANCE: f32 = 450.0;
/// No facade details beyond this (system-design §13.3), metres.
pub const FAR_DISTANCE: f32 = 1500.0;
/// Construction stages (`0..STAGES`).
pub const STAGES: u8 = 4;

const SIDE_MARGIN: f32 = 1.0;
const FRONT_SETBACK: f32 = 2.0;
const BACK_MARGIN: f32 = 1.0;
const PLINTH: f32 = 0.4;

type Rgb = [f32; 3];

fn rgba(c: Rgb, shade: f32) -> [f32; 4] {
    [c[0] * shade, c[1] * shade, c[2] * shade, 1.0]
}

/// Colours of a style. `walls` are alternatives picked by the seed.
struct Palette {
    walls: &'static [Rgb],
    roof: Rgb,
    trim: Rgb,
    glass: Rgb,
}

fn palette(name: &str) -> Palette {
    match name {
        "suburb_warm" => Palette {
            walls: &[
                [0.78, 0.66, 0.50],
                [0.82, 0.74, 0.62],
                [0.70, 0.52, 0.42],
                [0.86, 0.80, 0.70],
            ],
            roof: [0.50, 0.22, 0.17],
            trim: [0.90, 0.88, 0.82],
            glass: [0.18, 0.24, 0.30],
        },
        "shopfront" => Palette {
            walls: &[[0.72, 0.60, 0.46], [0.60, 0.40, 0.34], [0.78, 0.74, 0.62]],
            roof: [0.36, 0.34, 0.33],
            trim: [0.30, 0.22, 0.18],
            glass: [0.20, 0.28, 0.34],
        },
        "city_concrete" => Palette {
            walls: &[[0.60, 0.60, 0.58], [0.66, 0.65, 0.60], [0.54, 0.55, 0.57]],
            roof: [0.32, 0.32, 0.33],
            trim: [0.45, 0.45, 0.45],
            glass: [0.16, 0.22, 0.28],
        },
        "office_grey" => Palette {
            walls: &[[0.50, 0.53, 0.57], [0.58, 0.60, 0.62], [0.44, 0.47, 0.52]],
            roof: [0.30, 0.31, 0.33],
            trim: [0.36, 0.38, 0.41],
            glass: [0.14, 0.22, 0.30],
        },
        "glass_blue" => Palette {
            walls: &[[0.30, 0.42, 0.55], [0.26, 0.38, 0.52], [0.34, 0.46, 0.58]],
            roof: [0.28, 0.30, 0.33],
            trim: [0.22, 0.28, 0.35],
            glass: [0.12, 0.28, 0.42],
        },
        "industry_rust" => Palette {
            walls: &[[0.55, 0.36, 0.28], [0.62, 0.45, 0.32], [0.46, 0.34, 0.30]],
            roof: [0.40, 0.40, 0.38],
            trim: [0.30, 0.26, 0.24],
            glass: [0.20, 0.22, 0.24],
        },
        "utility_grey" => Palette {
            walls: &[[0.55, 0.57, 0.58]],
            roof: [0.38, 0.40, 0.42],
            trim: [0.40, 0.42, 0.44],
            glass: [0.20, 0.24, 0.28],
        },
        _ => Palette {
            walls: &[[0.6, 0.6, 0.6]],
            roof: [0.35, 0.35, 0.35],
            trim: [0.4, 0.4, 0.4],
            glass: [0.2, 0.25, 0.3],
        },
    }
}

/// Window pattern of a zone family and level (the "atlas", as parameters).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FacadeStyle {
    /// Width of one window bay, metres.
    pub bay: f32,
    /// Window width as a fraction of the bay.
    pub win_w: f32,
    /// Window height as a fraction of the floor height.
    pub win_h: f32,
    /// Sill height as a fraction of the floor height.
    pub sill: f32,
    /// Floor height, metres.
    pub floor_h: f32,
    /// One continuous glass band instead of single windows.
    pub ribbon: bool,
    /// Doors/shopfront on the ground floor of the front wall.
    pub shopfront: bool,
}

/// The facade pattern for `zone` at `level` (higher levels glaze more).
pub fn facade_style(zone: ZoneType, level: u8) -> FacadeStyle {
    let l = f32::from(level.clamp(1, 5));
    match zone {
        ZoneType::ResLow | ZoneType::None => FacadeStyle {
            bay: 3.2,
            win_w: 0.38 + 0.02 * l,
            win_h: 0.46,
            sill: 0.30,
            floor_h: 3.0,
            ribbon: false,
            shopfront: false,
        },
        ZoneType::ResHigh => FacadeStyle {
            bay: 3.0,
            win_w: 0.42 + 0.03 * l,
            win_h: 0.50,
            sill: 0.28,
            floor_h: 3.0,
            ribbon: false,
            shopfront: false,
        },
        ZoneType::ComLow => FacadeStyle {
            bay: 4.0,
            win_w: 0.55 + 0.05 * l,
            win_h: 0.55,
            sill: 0.22,
            floor_h: 3.6,
            ribbon: false,
            shopfront: true,
        },
        ZoneType::ComHigh => FacadeStyle {
            bay: 3.6,
            win_w: 0.70 + 0.04 * l,
            win_h: 0.62,
            sill: 0.20,
            floor_h: 3.8,
            ribbon: l >= 3.0,
            shopfront: true,
        },
        ZoneType::Office => FacadeStyle {
            bay: 3.0,
            win_w: 0.9,
            win_h: 0.56 + 0.04 * l,
            sill: 0.22,
            floor_h: 3.7,
            ribbon: true,
            shopfront: false,
        },
        ZoneType::Industrial => FacadeStyle {
            bay: 8.0,
            win_w: 0.25,
            win_h: 0.18,
            sill: 0.65,
            floor_h: 5.0,
            ribbon: false,
            shopfront: false,
        },
    }
}

/// Stable per-building seed: the id and the def, no RNG state.
pub fn seed(id: BuildingId, b: &Building) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for v in [id.index, id.generation, b.def.index() as u32] {
        h ^= v;
        h = h.wrapping_mul(0x0100_0193);
        h ^= h >> 15;
    }
    h
}

fn hash2(seed: u32, a: u32, b: u32) -> u32 {
    let mut h = seed ^ a.wrapping_mul(0x9e37_79b9) ^ b.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h
}

/// Construction stage `0..STAGES` of an `UnderConstruction` building at `tick`.
pub fn construction_stage(b: &Building, tick: Tick) -> u8 {
    let left = b.ready_at.0.saturating_sub(tick.0) as f32 / TICKS_PER_DAY as f32;
    let progress = (1.0 - left).clamp(0.0, 0.999);
    (progress * f32::from(STAGES)) as u8
}

/// The three detail levels of one building plus its bounds.
#[derive(Clone, Debug, Default)]
pub struct BuildingPiece {
    pub near: MeshData,
    pub mid: MeshData,
    pub far: MeshData,
    pub min: Vec3,
    pub max: Vec3,
}

impl BuildingPiece {
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lod {
    Near,
    Mid,
    Far,
}

/// Local frame of a lot: `u` along the road, `v` away from it, centre in XZ.
#[derive(Clone, Copy, Debug)]
struct Frame {
    c: Vec2,
    u: Vec2,
    v: Vec2,
}

impl Frame {
    fn at(&self, lu: f32, lv: f32, y: f32) -> Vec3 {
        let p = self.c + self.u * lu + self.v * lv;
        Vec3::new(p.x, y, p.y)
    }
    fn dir(&self, du: f32, dv: f32) -> Vec3 {
        let d = self.u * du + self.v * dv;
        Vec3::new(d.x, 0.0, d.y)
    }
}

/// The lot frame: `u` from the yaw, `v` away from the road (towards the lot interior).
fn lot_frame(b: &Building, net: &NetGraph) -> Frame {
    let u = Vec2::new(b.rot.cos(), b.rot.sin());
    let v_default = Vec2::new(u.y, -u.x);
    let from_road = net.segment(b.access.segment).and_then(|s| {
        let p = s.curve.point(b.access.t);
        let d = b.pos - Vec2::new(p.x, p.z);
        // Sane distance from the road (lots are at most 4 cells deep).
        (d.length() > 3.0 && d.length() < 60.0).then(|| d.normalize())
    });
    let v = match from_road {
        // Snap to the perpendicular of `u` on the side the road says.
        Some(d) if d.dot(v_default) < 0.0 => -v_default,
        _ => v_default,
    };
    Frame { c: b.pos, u, v }
}

fn mul(c: [f32; 4], k: f32) -> [f32; 4] {
    [c[0] * k, c[1] * k, c[2] * k, c[3]]
}

struct Ctx {
    f: Frame,
    /// Half extents of the box along u and v, and the box centre offset along v.
    hu: f32,
    hv: f32,
    cv: f32,
    /// Ground height used as the base of the walls.
    base: f32,
    style: FacadeStyle,
    wall: [f32; 4],
    glass: [f32; 4],
    roof: [f32; 4],
    trim: [f32; 4],
    seed: u32,
    state: BuildingState,
}

impl Ctx {
    fn corner(&self, su: f32, sv: f32, y: f32) -> Vec3 {
        self.f.at(su * self.hu, self.cv + sv * self.hv, y)
    }
}

/// One wall tiled with wall and glass quads. `a`→`b` runs counter-clockwise seen from
/// above so the outward normal is `n`.
#[allow(clippy::too_many_arguments)]
fn wall(
    m: &mut MeshData,
    cx: &Ctx,
    a: Vec3,
    b: Vec3,
    n: Vec3,
    height: f32,
    floors: u32,
    lod: Lod,
    front: bool,
    wall_id: u32,
) {
    let width = a.distance(b);
    let dir = (b - a) / width.max(1e-4);
    let y0 = a.y;
    let at = |s: f32, y: f32| Vec3::new(a.x + dir.x * s, y, a.z + dir.z * s);
    let q = |m: &mut MeshData, s0: f32, s1: f32, ya: f32, yb: f32, c: [f32; 4]| {
        if s1 - s0 > 1e-3 && yb - ya > 1e-3 {
            flat_quad(m, [at(s0, ya), at(s1, ya), at(s1, yb), at(s0, yb)], n, c);
        }
    };
    let dead = matches!(cx.state, BuildingState::Burned);
    if lod == Lod::Far || floors == 0 {
        q(m, 0.0, width, y0, y0 + height, cx.wall);
        return;
    }
    let st = cx.style;
    let fh = height / floors as f32;
    for fl in 0..floors {
        let fy = y0 + fh * fl as f32;
        let ground = fl == 0;
        if lod == Lod::Mid {
            // A wall band and a glass band per floor.
            let sill = fy + fh * st.sill;
            let top = sill + fh * st.win_h;
            q(m, 0.0, width, fy, sill, cx.wall);
            let glass = if dead { mul(cx.glass, 0.3) } else { cx.glass };
            q(m, 0.0, width, sill, top, glass);
            q(m, 0.0, width, top, fy + fh, cx.wall);
            continue;
        }
        let (sill_h, win_h) = if ground && front && st.shopfront {
            (0.15 * fh, 0.7 * fh) // shopfront glazing from the pavement
        } else {
            (fh * st.sill, fh * st.win_h)
        };
        let (ya, yb) = (fy + sill_h, fy + sill_h + win_h);
        q(m, 0.0, width, fy, ya, cx.wall);
        q(m, 0.0, width, yb, fy + fh, cx.wall);
        // The window band: piers and glass along the wall.
        let shade = |i: u32| -> f32 {
            let h = hash2(cx.seed, wall_id * 131 + fl, i);
            match cx.state {
                BuildingState::Burned => 0.25,
                BuildingState::Abandoned => 0.45 + (h % 3) as f32 * 0.05,
                _ => 0.85 + (h % 5) as f32 * 0.08,
            }
        };
        let boarded = |i: u32| {
            cx.state == BuildingState::Abandoned
                && hash2(cx.seed, wall_id * 7 + fl, i).is_multiple_of(3)
        };
        let glass_at = |i: u32| {
            if boarded(i) {
                [0.30, 0.22, 0.15, 1.0]
            } else {
                mul(cx.glass, shade(i))
            }
        };
        if st.ribbon {
            let pier = (0.6f32).min(width * 0.2);
            q(m, 0.0, pier, ya, yb, cx.wall);
            q(m, width - pier, width, ya, yb, cx.wall);
            // Glass in panels so the shade varies along the band.
            let panels = (((width - 2.0 * pier) / st.bay).round() as u32).max(1);
            let pw = (width - 2.0 * pier) / panels as f32;
            for i in 0..panels {
                let s0 = pier + pw * i as f32;
                q(m, s0, s0 + pw, ya, yb, glass_at(i));
            }
        } else {
            let bays = ((width / st.bay).round() as u32).max(1);
            let bw = width / bays as f32;
            let ww = (bw * st.win_w).min(bw - 0.3);
            let pier = (bw - ww) * 0.5;
            let mut s = 0.0;
            for i in 0..bays {
                q(m, s, s + pier, ya, yb, cx.wall);
                q(m, s + pier, s + pier + ww, ya, yb, glass_at(i));
                q(m, s + pier + ww, s + bw, ya, yb, cx.wall);
                s += bw;
            }
        }
        // A door in the middle of the front wall's ground floor.
        if ground && front && !st.shopfront && width > 4.0 {
            let (d0, d1) = (width * 0.5 - 0.5, width * 0.5 + 0.5);
            q(m, d0, d1, y0, y0 + 2.1, cx.trim);
        }
    }
}

fn quad_up(m: &mut MeshData, p: [Vec3; 4], c: [f32; 4]) {
    flat_quad(m, p, Vec3::Y, c);
}

/// Roof on top of the box at height `y`.
fn roof(m: &mut MeshData, cx: &Ctx, kind: RoofKind, y: f32, lod: Lod) {
    let c = |su: f32, sv: f32, yy: f32| cx.corner(su, sv, yy);
    let burned = cx.state == BuildingState::Burned;
    let rc = cx.roof;
    match kind {
        RoofKind::Flat => {
            quad_up(
                m,
                [
                    c(-1.0, -1.0, y),
                    c(1.0, -1.0, y),
                    c(1.0, 1.0, y),
                    c(-1.0, 1.0, y),
                ],
                rc,
            );
            if lod == Lod::Near && !burned {
                // Parapet rim (outer faces are the walls; add the inner faces and cap) and a
                // roof box.
                let h = 0.5;
                let ring = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
                for k in 0..4 {
                    let (a, b) = (ring[k], ring[(k + 1) % 4]);
                    let mid = Vec2::new(-(a.0 + b.0) * 0.5, -(a.1 + b.1) * 0.5);
                    let n = cx.f.dir(mid.x * cx.hu, mid.y * cx.hv).normalize_or_zero();
                    flat_quad(
                        m,
                        [
                            c(a.0, a.1, y + h),
                            c(b.0, b.1, y + h),
                            c(b.0, b.1, y),
                            c(a.0, a.1, y),
                        ],
                        n,
                        cx.trim,
                    );
                }
                // Rooftop unit.
                let (bu, bv) = (cx.hu * 0.25, cx.hv * 0.25);
                let top = y + 1.6;
                let p = |su: f32, sv: f32, yy: f32| {
                    cx.f.at(su * bu - cx.hu * 0.3, cx.cv + sv * bv - cx.hv * 0.2, yy)
                };
                let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
                quad_up(
                    m,
                    [
                        p(-1.0, -1.0, top),
                        p(1.0, -1.0, top),
                        p(1.0, 1.0, top),
                        p(-1.0, 1.0, top),
                    ],
                    cx.trim,
                );
                for k in 0..4 {
                    let (a, b) = (corners[k], corners[(k + 1) % 4]);
                    let n =
                        cx.f.dir((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
                            .normalize_or_zero();
                    flat_quad(
                        m,
                        [
                            p(a.0, a.1, y),
                            p(b.0, b.1, y),
                            p(b.0, b.1, top),
                            p(a.0, a.1, top),
                        ],
                        n,
                        mul(cx.trim, 0.9),
                    );
                }
            }
        }
        RoofKind::Gabled | RoofKind::Hipped => {
            let rise = (cx.hu.min(cx.hv) * 0.8).clamp(1.2, 4.5);
            let ridge = y + rise;
            let gable = RoofKind::Gabled == kind;
            // Ridge along u: from -r..r at sv = 0 (hipped pulls the ends in).
            let inset = if gable { 1.0 } else { 0.55 };
            let (r0, r1) = (-inset, inset);
            // Two long slopes.
            for side in [-1.0f32, 1.0] {
                let n = (cx.f.dir(0.0, side) * rise + Vec3::Y * cx.hv).normalize();
                flat_quad(
                    m,
                    if side < 0.0 {
                        [
                            c(-1.0, -1.0, y),
                            c(1.0, -1.0, y),
                            c(r1, 0.0, ridge),
                            c(r0, 0.0, ridge),
                        ]
                    } else {
                        [
                            c(1.0, 1.0, y),
                            c(-1.0, 1.0, y),
                            c(r0, 0.0, ridge),
                            c(r1, 0.0, ridge),
                        ]
                    },
                    n,
                    rc,
                );
            }
            // Ends: a gable wall triangle or a hipped slope.
            for end in [-1.0f32, 1.0] {
                let re = if end < 0.0 { r0 } else { r1 };
                let a = c(end, -1.0, y);
                let b = c(end, 1.0, y);
                let apex = c(re, 0.0, ridge);
                let outward = cx.f.dir(end, 0.0).normalize_or_zero();
                if gable {
                    let v = [
                        push_vertex(m, a, outward, cx.wall),
                        push_vertex(m, b, outward, cx.wall),
                        push_vertex(m, apex, outward, cx.wall),
                    ];
                    tri(m, v[0], v[1], v[2], outward);
                } else {
                    let n = (outward + Vec3::Y).normalize();
                    let v = [
                        push_vertex(m, a, n, rc),
                        push_vertex(m, b, n, rc),
                        push_vertex(m, apex, n, rc),
                    ];
                    tri(m, v[0], v[1], v[2], n);
                }
            }
        }
    }
}

/// Scaffolding around the box: posts and one rail per floor up to `top`.
fn scaffold(m: &mut MeshData, cx: &Ctx, top: f32) {
    let col = [0.80, 0.58, 0.12, 1.0];
    let out = 0.8;
    let (hu, hv) = (cx.hu + out, cx.hv + out);
    let corners = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let cp = |su: f32, sv: f32, y: f32| cx.f.at(su * hu, cx.cv + sv * hv, y);
    let post = |m: &mut MeshData, p: Vec3, h: f32| {
        let s = 0.09;
        for (dx, dz, n) in [
            (s, 0.0, Vec3::X),
            (-s, 0.0, -Vec3::X),
            (0.0, s, Vec3::Z),
            (0.0, -s, -Vec3::Z),
        ] {
            let (px, pz) = (n.z.abs(), n.x.abs());
            let a = Vec3::new(p.x + dx - px * s, p.y, p.z + dz - pz * s);
            let b = Vec3::new(p.x + dx + px * s, p.y, p.z + dz + pz * s);
            flat_quad(m, [a, b, b + Vec3::Y * h, a + Vec3::Y * h], n, col);
        }
    };
    let h = top - cx.base;
    for k in 0..4 {
        let (a, b) = (corners[k], corners[(k + 1) % 4]);
        let (pa, pb) = (cp(a.0, a.1, cx.base), cp(b.0, b.1, cx.base));
        let len = pa.distance(pb);
        let n_posts = (len / 3.5).ceil().max(1.0) as u32;
        for i in 0..n_posts {
            let t = i as f32 / n_posts as f32;
            post(m, pa.lerp(pb, t), h);
        }
        // Rails on the outer face, double-sided via two quads.
        let normal =
            cx.f.dir((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
                .normalize_or_zero();
        let floors = (h / 3.0).floor() as u32;
        for fl in 1..=floors {
            let y = cx.base + fl as f32 * 3.0;
            for nn in [normal, -normal] {
                flat_quad(
                    m,
                    [
                        pa + Vec3::Y * (y - 0.05),
                        pb + Vec3::Y * (y - 0.05),
                        pb + Vec3::Y * (y + 0.05),
                        pa + Vec3::Y * (y + 0.05),
                    ],
                    nn,
                    col,
                );
            }
        }
    }
    post(m, cp(1.0, 1.0, cx.base), h);
}

/// A solid box (4 walls + flat top), used for construction cores and plain masses.
fn solid_box(m: &mut MeshData, cx: &Ctx, y0: f32, y1: f32, wall: [f32; 4], top: [f32; 4]) {
    let c = |su: f32, sv: f32, y: f32| cx.corner(su, sv, y);
    let ring = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    for k in 0..4 {
        let (a, b) = (ring[k], ring[(k + 1) % 4]);
        let n =
            cx.f.dir((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
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
            wall,
        );
    }
    quad_up(
        m,
        [
            c(-1.0, -1.0, y1),
            c(1.0, -1.0, y1),
            c(1.0, 1.0, y1),
            c(-1.0, 1.0, y1),
        ],
        top,
    );
}

fn dull(c: [f32; 4], k: f32) -> [f32; 4] {
    let g = (c[0] + c[1] + c[2]) / 3.0;
    [
        (c[0] * 0.4 + g * 0.6) * k,
        (c[1] * 0.4 + g * 0.6) * k,
        (c[2] * 0.4 + g * 0.6) * k,
        c[3],
    ]
}

/// Builds all three detail levels of one building. `tick` selects the construction stage.
pub fn build_building(
    id: BuildingId,
    b: &Building,
    def: &BuildingDef,
    net: &NetGraph,
    terrain: &Terrain,
    tick: Tick,
) -> BuildingPiece {
    let f = lot_frame(b, net);
    let (w, d) = (
        f32::from(b.lot.w) * CELL_SIZE,
        f32::from(b.lot.d) * CELL_SIZE,
    );
    let hu = (w * 0.5 - SIDE_MARGIN).max(1.0);
    let hv = ((d - FRONT_SETBACK - BACK_MARGIN) * 0.5).max(1.0);
    let cv = (FRONT_SETBACK - BACK_MARGIN) * 0.5;

    // Ground: the highest corner of the box, with a foundation down to the lowest.
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let p = f.c + f.u * (su * hu) + f.v * (cv + sv * hv);
        let y = terrain.height(p);
        lo = lo.min(y);
        hi = hi.max(y);
    }
    let base = hi + PLINTH;

    let pal = palette(&def.style.palette);
    let sd = seed(id, b);
    let style = facade_style(def.zone, def.level);
    let wall_c = pal.walls[sd as usize % pal.walls.len()];
    let tone = 0.92 + (sd >> 8) as f32 % 9.0 * 0.02;
    let mut wallc = rgba(wall_c, tone);
    let mut roofc = rgba(pal.roof, 0.9 + (sd >> 16) as f32 % 5.0 * 0.04);
    let mut trimc = rgba(pal.trim, 1.0);
    let mut glassc = rgba(pal.glass, 1.0);
    match b.state {
        BuildingState::Abandoned => {
            wallc = dull(wallc, 0.62);
            roofc = dull(roofc, 0.6);
            trimc = dull(trimc, 0.6);
            glassc = mul(glassc, 0.55);
        }
        BuildingState::Burned => {
            wallc = [0.09, 0.08, 0.08, 1.0];
            roofc = [0.06, 0.05, 0.05, 1.0];
            trimc = [0.07, 0.06, 0.06, 1.0];
            glassc = [0.04, 0.04, 0.04, 1.0];
        }
        _ => {}
    }
    let cx = Ctx {
        f,
        hu,
        hv,
        cv,
        base,
        style,
        wall: wallc,
        glass: glassc,
        roof: roofc,
        trim: trimc,
        seed: sd,
        state: b.state,
    };

    let floors = u32::from(def.style.floors.max(1));
    let full_h = floors as f32 * style.floor_h;
    let (floors_shown, height) = match b.state {
        BuildingState::Burned => {
            let n = ((floors as f32 * 0.6).ceil() as u32).max(1);
            (n, n as f32 * style.floor_h)
        }
        _ => (floors, full_h),
    };

    let mut pieces = [
        MeshData::default(),
        MeshData::default(),
        MeshData::default(),
    ];
    for (m, lod) in pieces.iter_mut().zip([Lod::Near, Lod::Mid, Lod::Far]) {
        // Foundation: from the lowest terrain point up to the plinth.
        let fnd = [0.30, 0.30, 0.30, 1.0];
        let ring = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        for k in 0..4 {
            let (a, bb) = (ring[k], ring[(k + 1) % 4]);
            let n = f
                .dir((a.0 + bb.0) * 0.5 * hu, (a.1 + bb.1) * 0.5 * hv)
                .normalize_or_zero();
            let c = |su: f32, sv: f32, y: f32| cx.corner(su, sv, y);
            flat_quad(
                m,
                [
                    c(a.0, a.1, lo - 0.3),
                    c(bb.0, bb.1, lo - 0.3),
                    c(bb.0, bb.1, base),
                    c(a.0, a.1, base),
                ],
                n,
                fnd,
            );
        }
        if b.state == BuildingState::UnderConstruction {
            let stage = construction_stage(b, tick);
            let core_h = height * f32::from(stage + 1) / f32::from(STAGES);
            let slab = [0.55, 0.55, 0.53, 1.0];
            solid_box(m, &cx, base, base + core_h, [0.62, 0.62, 0.60, 1.0], slab);
            if lod == Lod::Near {
                scaffold(m, &cx, base + core_h + style.floor_h.min(3.0));
            }
            continue;
        }
        // Walls. Corners counter-clockwise from above: (-u,-v) -> (+u,-v) -> (+u,+v) -> (-u,+v).
        let c = |su: f32, sv: f32| cx.corner(su, sv, base);
        let faces = [
            (c(-1.0, -1.0), c(1.0, -1.0), f.dir(0.0, -1.0), true, 0u32),
            (c(1.0, -1.0), c(1.0, 1.0), f.dir(1.0, 0.0), false, 1),
            (c(1.0, 1.0), c(-1.0, 1.0), f.dir(0.0, 1.0), false, 2),
            (c(-1.0, 1.0), c(-1.0, -1.0), f.dir(-1.0, 0.0), false, 3),
        ];
        for (a, bb, n, front, wid) in faces {
            wall(m, &cx, a, bb, n, height, floors_shown, lod, front, wid);
        }
        let top = base + height;
        if b.state == BuildingState::Burned {
            quad_up(
                m,
                [
                    cx.corner(-1.0, -1.0, top),
                    cx.corner(1.0, -1.0, top),
                    cx.corner(1.0, 1.0, top),
                    cx.corner(-1.0, 1.0, top),
                ],
                cx.roof,
            );
        } else {
            roof(m, &cx, def.style.roof, top, lod);
        }
    }
    let [near, mid, far] = pieces;
    let (min, max) = bounds(&near);
    BuildingPiece {
        near,
        mid,
        far,
        min,
        max,
    }
}

// ---------------------------------------------------------------------------------------
// Incremental set

fn mix(h: &mut u64, v: u32) {
    for b in v.to_le_bytes() {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Hash of everything a building's mesh depends on (except static terrain).
pub fn building_stamp(b: &Building, tick: Tick) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    mix(&mut h, b.def.index() as u32);
    mix(&mut h, u32::from(b.level));
    mix(&mut h, b.state as u32);
    mix(&mut h, b.pos.x.to_bits());
    mix(&mut h, b.pos.y.to_bits());
    mix(&mut h, b.rot.to_bits());
    mix(&mut h, u32::from(b.lot.w) | u32::from(b.lot.d) << 8);
    mix(&mut h, b.access.t.to_bits());
    if b.state == BuildingState::UnderConstruction {
        mix(&mut h, u32::from(construction_stage(b, tick)));
    }
    h
}

/// Chunk cell of a point.
pub fn chunk_of(p: Vec3) -> (i32, i32) {
    (
        (p.x / BUILDING_CHUNK).floor() as i32,
        (p.z / BUILDING_CHUNK).floor() as i32,
    )
}

struct Entry {
    stamp: u64,
    chunk: (i32, i32),
    piece: BuildingPiece,
    constructing: bool,
}

/// What an update changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Update {
    pub rebuilt: usize,
    pub removed: usize,
    pub dirty_chunks: BTreeSet<(i32, i32)>,
}

/// All building pieces, batched by chunk.
#[derive(Default)]
pub struct BuildingSet {
    pieces: BTreeMap<BuildingId, Entry>,
    by_chunk: BTreeMap<(i32, i32), BTreeSet<BuildingId>>,
}

impl BuildingSet {
    pub fn count(&self) -> usize {
        self.pieces.len()
    }

    /// Buildings currently showing a construction stage (their look changes with time).
    pub fn constructing(&self) -> usize {
        self.pieces.values().filter(|e| e.constructing).count()
    }

    pub fn chunks(&self) -> impl Iterator<Item = &(i32, i32)> {
        self.by_chunk.keys()
    }

    pub fn piece(&self, id: BuildingId) -> Option<&BuildingPiece> {
        self.pieces.get(&id).map(|e| &e.piece)
    }

    /// Rebuilds buildings whose stamp changed, drops vanished ones.
    pub fn update(
        &mut self,
        grid: &BuildingGrid,
        net: &NetGraph,
        db: &ContentDb,
        terrain: &Terrain,
        tick: Tick,
    ) -> Update {
        let mut up = Update::default();
        let gone: Vec<BuildingId> = self
            .pieces
            .keys()
            .filter(|id| grid.get(**id).is_none())
            .copied()
            .collect();
        for id in gone {
            self.remove(id, &mut up);
            up.removed += 1;
        }
        for (id, b) in grid.iter() {
            let stamp = building_stamp(b, tick);
            if self.pieces.get(&id).is_some_and(|e| e.stamp == stamp) {
                continue;
            }
            if b.def.index() >= db.buildings().count() {
                continue;
            }
            let piece = build_building(id, b, db.building(b.def), net, terrain, tick);
            self.remove(id, &mut up);
            let chunk = chunk_of(piece.center());
            self.by_chunk.entry(chunk).or_default().insert(id);
            up.dirty_chunks.insert(chunk);
            up.rebuilt += 1;
            self.pieces.insert(
                id,
                Entry {
                    stamp,
                    chunk,
                    piece,
                    constructing: b.state == BuildingState::UnderConstruction,
                },
            );
        }
        up
    }

    fn remove(&mut self, id: BuildingId, up: &mut Update) {
        if let Some(old) = self.pieces.remove(&id) {
            if let Some(set) = self.by_chunk.get_mut(&old.chunk) {
                set.remove(&id);
                if set.is_empty() {
                    self.by_chunk.remove(&old.chunk);
                }
            }
            up.dirty_chunks.insert(old.chunk);
        }
    }

    /// Merged mesh of a chunk at one detail level (0 near, 1 mid, 2 far).
    pub fn chunk_mesh(&self, chunk: (i32, i32), level: usize) -> Option<MeshData> {
        let ids = self.by_chunk.get(&chunk)?;
        let mut m = MeshData::default();
        for id in ids {
            let p = &self.pieces[id].piece;
            append(&mut m, [&p.near, &p.mid, &p.far][level.min(2)]);
        }
        Some(m)
    }

    pub fn chunk_bounds(&self, chunk: (i32, i32)) -> Option<(Vec3, Vec3)> {
        let ids = self.by_chunk.get(&chunk)?;
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for id in ids {
            let p = &self.pieces[id].piece;
            lo = lo.min(p.min);
            hi = hi.max(p.max);
        }
        Some((lo, hi))
    }

    pub fn triangle_count(&self, level: usize) -> usize {
        self.pieces
            .values()
            .map(|e| {
                let p = &e.piece;
                [&p.near, &p.mid, &p.far][level.min(2)].triangle_count()
            })
            .sum()
    }
}

/// Detail level (0 near, 1 mid, 2 far) for a chunk at `distance` metres, with 10 %
/// hysteresis around the thresholds relative to the `current` level.
pub fn select_level(distance: f32, current: Option<usize>) -> usize {
    const LIMITS: [f32; 2] = [MID_DISTANCE, FAR_DISTANCE];
    let band = |d: f32| LIMITS.iter().position(|&l| d < l).unwrap_or(2);
    let fresh = band(distance);
    match current {
        Some(c) if c != fresh => {
            let lo = if c == 0 { 0.0 } else { LIMITS[c - 1] * 0.9 };
            let hi = if c == 2 { f32::MAX } else { LIMITS[c] * 1.1 };
            if (lo..hi).contains(&distance) {
                c
            } else {
                fresh
            }
        }
        _ => fresh,
    }
}

#[cfg(test)]
mod tests;
