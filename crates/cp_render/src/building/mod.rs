//! Pure building mesh generation (no Bevy types). See `architecture/system-design.md` §13.3.
//!
//! A building is a box on its lot (`Building::lot` × 8 m cells, inset by a front setback
//! and side margins) with floors and a roof (`Flat`, `Gabled`, `Hipped`; the other roofs and
//! massings draw as a flat box until T-112) chosen by [`variant`] from the def's `StyleDef`s, and a
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

mod shapes;

pub use shapes::{Rotor, rotor_mesh, rotor_world};

use std::collections::{BTreeMap, BTreeSet};

use bevy_math::{Vec2, Vec3};
use cp_building::{Building, BuildingGrid, BuildingId, BuildingState, build_ticks};
use cp_content::{
    BuildingDef, ContentDb, PLACEHOLDER_INDEX, Rgb, RoofKind, StyleDef, StyleDefId, ZoneType,
};
use cp_core::Tick;
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

fn rgba(c: Rgb, shade: f32) -> [f32; 4] {
    [c[0] * shade, c[1] * shade, c[2] * shade, 1.0]
}

/// What the hash of a building picks inside its [`StyleDef`] (`system-design.md` §8.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VariantParams {
    /// Floors, within the style's range.
    pub floors: u8,
    /// Share of the lot area the volume covers, within the style's range.
    pub fill: f32,
    pub wall: Rgb,
    pub roof: Rgb,
    pub trim: Rgb,
}

/// FNV-1a 64 over the building's index, generation and def id, then a finaliser so every
/// bit depends on every input (the modulo below reads the low bits).
fn variant_hash(id: BuildingId, def: &BuildingDef) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(&id.index.to_le_bytes());
    eat(&id.generation.to_le_bytes());
    eat(def.id.as_bytes());
    h ^= h >> 32;
    h = h.wrapping_mul(0xd6e8_feb8_6659_fd93);
    h ^= h >> 32;
    h
}

/// The look of a building: one of the def's styles and the floors, lot fill and colours
/// inside it, all from a hash of `(id.index, id.generation, def.id)`. A function of sim
/// state, not RNG: the same building looks the same after a load and on another client, and
/// nothing is written to the sim. A style the content does not have draws as the grey
/// placeholder.
pub fn variant<'a>(
    db: &'a ContentDb,
    id: BuildingId,
    def: &BuildingDef,
) -> (&'a StyleDef, VariantParams) {
    let h = variant_hash(id, def);
    // Independent draws: each takes its own 16 bits of two mixed words.
    let h2 = (h ^ (h >> 29)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let draw = |k: u32| (((if k < 4 { h } else { h2 }) >> ((k % 4) * 16)) & 0xffff) as usize;
    let style = def
        .styles
        .get(draw(0) % def.styles.len().max(1))
        .and_then(|name| db.style_id(name))
        .map_or_else(|| db.style(StyleDefId(PLACEHOLDER_INDEX)), |s| db.style(s));
    let (f0, f1) = style.floors;
    let (g0, g1) = style.footprint_fill;
    let pal = &style.palette;
    let pick = |list: &[Rgb], k: u32| list[draw(k) % list.len()];
    let params = VariantParams {
        floors: f0 + (draw(1) % (usize::from(f1.saturating_sub(f0)) + 1)) as u8,
        fill: g0 + (g1 - g0) * (draw(2) as f32 / 65535.0),
        wall: pick(&pal.walls, 3),
        roof: pick(&pal.roof, 4),
        trim: pick(&pal.trim, 5),
    };
    (style, params)
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

/// Construction stage `0..STAGES` of an `UnderConstruction` building at `tick`;
/// `total` is its whole construction time in ticks (`cp_building::build_ticks`).
pub fn construction_stage(b: &Building, total: u64, tick: Tick) -> u8 {
    let left = b.ready_at.0.saturating_sub(tick.0) as f32 / total.max(1) as f32;
    let progress = (1.0 - left).clamp(0.0, 0.999);
    (progress * f32::from(STAGES)) as u8
}

/// The three detail levels of one building plus its bounds.
#[derive(Clone, Debug, Default)]
pub struct BuildingPiece {
    /// The turning rotor of a wind turbine; its blades are not in the meshes below.
    pub rotor: Option<Rotor>,
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
    let from_road = b
        .access()
        .and_then(|a| net.segment(a.segment).map(|s| (s, a)))
        .and_then(|(s, a)| {
            let p = s.curve.point(a.t);
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

#[derive(Clone, Copy)]
struct Ctx {
    f: Frame,
    /// Centre offset of the box along u.
    cu: f32,
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
        self.f.at(self.cu + su * self.hu, self.cv + sv * self.hv, y)
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
    // Beyond 450 m a one- or two-floor wall has no visible bands: one quad, as in the far level.
    if lod == Lod::Far || floors == 0 || (lod == Lod::Mid && floors <= 2) {
        q(m, 0.0, width, y0, y0 + height, cx.wall);
        return;
    }
    let st = cx.style;
    let fh = height / floors as f32;
    for fl in 0..floors {
        let fy = y0 + fh * fl as f32;
        let ground = fl == 0;
        if lod == Lod::Mid {
            // A glass band per floor and the wall bands between them: the wall above one
            // window and the wall below the next are one quad.
            let sill = fy + fh * st.sill;
            let top = sill + fh * st.win_h;
            let glass = if dead { mul(cx.glass, 0.3) } else { cx.glass };
            if ground {
                q(m, 0.0, width, fy, sill, cx.wall);
            }
            q(m, 0.0, width, sill, top, glass);
            let next = if fl + 1 == floors {
                fy + fh
            } else {
                fy + fh + fh * st.sill
            };
            q(m, 0.0, width, top, next, cx.wall);
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
fn roof(m: &mut MeshData, cx: &Ctx, kind: RoofKind, y: f32, lod: Lod, plain: bool) {
    let c = |su: f32, sv: f32, yy: f32| cx.corner(su, sv, yy);
    let burned = cx.state == BuildingState::Burned;
    let rc = cx.roof;
    match kind {
        RoofKind::Shed => shapes::shed(m, cx, y),
        RoofKind::Mansard => shapes::mansard(m, cx, y),
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
            if lod == Lod::Near && !burned && !plain {
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
                    cx.f.at(
                        cx.cu + su * bu - cx.hu * 0.3,
                        cx.cv + sv * bv - cx.hv * 0.2,
                        yy,
                    )
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
    let cp = |su: f32, sv: f32, y: f32| cx.f.at(cx.cu + su * hu, cx.cv + sv * hv, y);
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
    db: &ContentDb,
    tick: Tick,
) -> BuildingPiece {
    let f = lot_frame(b, net);
    let (w, d) = (
        f32::from(b.size().0) * CELL_SIZE,
        f32::from(b.size().1) * CELL_SIZE,
    );
    let (style_def, vp) = variant(db, id, def);
    // The volume covers `fill` of the lot area: both half-extents shrink by its square root.
    let shrink = vp.fill.clamp(0.0, 1.0).sqrt();
    let hu = ((w * 0.5 - SIDE_MARGIN) * shrink).max(1.0);
    let hv = (((d - FRONT_SETBACK - BACK_MARGIN) * 0.5) * shrink).max(1.0);
    let cv = (FRONT_SETBACK - BACK_MARGIN) * 0.5;

    let sd = seed(id, b);
    let style = facade_style(def.zone, def.level);
    let floors = u32::from(vp.floors.max(1));
    let plan = shapes::plan(style_def.massing, floors, hu, hv, cv);

    // Ground: the highest corner of any box, with a foundation down to the lowest.
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for v in &plan.vols {
        for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = f.c + f.u * (v.cu + su * v.hu) + f.v * (v.cv + sv * v.hv);
            let y = terrain.height(p);
            lo = lo.min(y);
            // A building on water (a pump) stands on the surface, not on the bed.
            hi = hi.max(y + terrain.water_depth(p).max(0.0));
        }
    }
    let base = hi + PLINTH;

    let tone = 0.92 + (sd >> 8) as f32 % 9.0 * 0.02;
    let mut wallc = rgba(vp.wall, tone);
    let mut roofc = rgba(vp.roof, 0.9 + (sd >> 16) as f32 % 5.0 * 0.04);
    let mut trimc = rgba(vp.trim, 1.0);
    let mut glassc = rgba(style_def.palette.glass, 1.0);
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
        cu: 0.0,
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

    // Floors drawn per box: all of them, or 60% of the whole stack once burned.
    let burned = b.state == BuildingState::Burned;
    let shown_total = if burned {
        ((floors as f32 * 0.6).ceil() as u32).max(1)
    } else {
        floors
    };
    let fh = style.floor_h;
    let full_h = floors as f32 * fh;
    let boxes: Vec<(usize, Ctx, u32)> = plan
        .vols
        .iter()
        .enumerate()
        .filter_map(|(i, v)| {
            let shown = shown_total.saturating_sub(v.below).min(v.floors);
            (shown > 0).then_some((
                i,
                Ctx {
                    cu: v.cu,
                    cv: v.cv,
                    hu: v.hu,
                    hv: v.hv,
                    ..cx
                },
                shown,
            ))
        })
        .collect();

    let mut pieces = [
        MeshData::default(),
        MeshData::default(),
        MeshData::default(),
    ];
    let service_cx = |lod| shapes::ServiceCx {
        f: &f,
        hu,
        hv,
        cv,
        base,
        lo,
        wall: wallc,
        roof: roofc,
        trim: trimc,
        glass: glassc,
        lod,
    };
    let silhouette =
        shapes::is_service(style_def.massing) && b.state != BuildingState::UnderConstruction;
    let rotor = (silhouette && b.state == BuildingState::Active)
        .then(|| shapes::rotor(style_def.massing, &service_cx(Lod::Near)))
        .flatten();
    for (m, lod) in pieces.iter_mut().zip([Lod::Near, Lod::Mid, Lod::Far]) {
        if silhouette {
            shapes::service(m, style_def.massing, &service_cx(lod));
            continue;
        }
        // Foundation of every box that stands on the ground.
        let fnd = [0.30, 0.30, 0.30, 1.0];
        let ring = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        for (i, vcx, _) in &boxes {
            let v = &plan.vols[*i];
            // No foundation skirt beyond the mid distance: it is a few dark pixels there.
            if v.below > 0 || lod == Lod::Far {
                continue;
            }
            for k in 0..4 {
                let Some((t0, t1)) = v.vis[k] else { continue };
                let (a, bb) = (ring[k], ring[(k + 1) % 4]);
                let n = f
                    .dir((a.0 + bb.0) * 0.5 * vcx.hu, (a.1 + bb.1) * 0.5 * vcx.hv)
                    .normalize_or_zero();
                let c = |su: f32, sv: f32, y: f32| vcx.corner(su, sv, y);
                let (pa, pb) = (
                    c(a.0, a.1, lo - 0.3).lerp(c(bb.0, bb.1, lo - 0.3), t0),
                    c(a.0, a.1, lo - 0.3).lerp(c(bb.0, bb.1, lo - 0.3), t1),
                );
                flat_quad(
                    m,
                    [
                        pa,
                        pb,
                        Vec3::new(pb.x, base, pb.z),
                        Vec3::new(pa.x, base, pa.z),
                    ],
                    n,
                    fnd,
                );
            }
        }
        if b.state == BuildingState::UnderConstruction {
            let stage = construction_stage(b, build_ticks(def), tick);
            let frac = f32::from(stage + 1) / f32::from(STAGES);
            let slab = [0.55, 0.55, 0.53, 1.0];
            for (i, vcx, _) in &boxes {
                let v = &plan.vols[*i];
                let y0 = base + frac * v.below as f32 * fh;
                let y1 = base + frac * (v.below + v.floors) as f32 * fh;
                solid_box(m, vcx, y0, y1, [0.62, 0.62, 0.60, 1.0], slab);
            }
            if lod == Lod::Near {
                scaffold(m, &cx, base + full_h * frac + fh.min(3.0));
            }
            continue;
        }
        // Walls. Corners counter-clockwise from above: (-u,-v) -> (+u,-v) -> (+u,+v) -> (-u,+v).
        for (i, vcx, shown) in &boxes {
            let v = &plan.vols[*i];
            let y0 = base + v.below as f32 * fh;
            let h = *shown as f32 * fh;
            let c = |su: f32, sv: f32| vcx.corner(su, sv, y0);
            let faces = [
                (
                    c(-1.0, -1.0),
                    c(1.0, -1.0),
                    vcx.f.dir(0.0, -1.0),
                    v.front,
                    0u32,
                ),
                (c(1.0, -1.0), c(1.0, 1.0), vcx.f.dir(1.0, 0.0), false, 1),
                (c(1.0, 1.0), c(-1.0, 1.0), vcx.f.dir(0.0, 1.0), false, 2),
                (c(-1.0, 1.0), c(-1.0, -1.0), vcx.f.dir(-1.0, 0.0), false, 3),
            ];
            for (k, (a, bb, n, front, wid)) in faces.into_iter().enumerate() {
                let Some((t0, t1)) = v.vis[k] else { continue };
                // Separate boxes get separate wall ids so their windows differ.
                let (pa, pb) = (a.lerp(bb, t0), a.lerp(bb, t1));
                wall(
                    m,
                    vcx,
                    pa,
                    pb,
                    n,
                    h,
                    *shown,
                    lod,
                    front,
                    wid + 4 * *i as u32,
                );
            }
            let top = y0 + h;
            if burned {
                quad_up(
                    m,
                    [
                        vcx.corner(-1.0, -1.0, top),
                        vcx.corner(1.0, -1.0, top),
                        vcx.corner(1.0, 1.0, top),
                        vcx.corner(-1.0, 1.0, top),
                    ],
                    vcx.roof,
                );
            } else {
                match v.roof {
                    shapes::RoofMode::Style => {
                        roof(m, vcx, style_def.roof, top, lod, false);
                    }
                    shapes::RoofMode::Plain => roof(m, vcx, RoofKind::Flat, top, lod, true),
                    shapes::RoofMode::Saw => shapes::sawtooth(m, vcx, top),
                }
            }
        }
        if lod == Lod::Near
            && !matches!(b.state, BuildingState::Burned)
            && !style_def.details.is_empty()
        {
            let dc = shapes::DetailCx {
                f: &f,
                terrain,
                vols: &plan.vols,
                plan_top: plan.top,
                plan_front: plan.front,
                base,
                floor_h: fh,
                bay: style.bay,
                lot: (w, d),
                roof_kind: if plan.vols[plan.top].roof == shapes::RoofMode::Style {
                    style_def.roof
                } else {
                    RoofKind::Hipped
                },
                wall: wallc,
                trim: trimc,
                seed: sd,
            };
            shapes::details(m, &dc, &style_def.details);
        }
    }
    let [near, mid, far] = pieces;
    let (min, max) = bounds(&near);
    BuildingPiece {
        rotor,
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
pub fn building_stamp(b: &Building, total: u64, tick: Tick) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    mix(&mut h, b.def.index() as u32);
    mix(&mut h, u32::from(b.level));
    mix(&mut h, b.state as u32);
    mix(&mut h, b.pos.x.to_bits());
    mix(&mut h, b.pos.y.to_bits());
    mix(&mut h, b.rot.to_bits());
    mix(&mut h, u32::from(b.size().0) | u32::from(b.size().1) << 8);
    mix(&mut h, b.access().map_or(0, |a| a.t.to_bits()));
    if b.state == BuildingState::UnderConstruction {
        mix(&mut h, u32::from(construction_stage(b, total, tick)));
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

    /// The rotors of the wind turbines, in building-id order.
    pub fn rotors(&self) -> impl Iterator<Item = (BuildingId, &Rotor)> {
        self.pieces
            .iter()
            .filter_map(|(id, e)| e.piece.rotor.as_ref().map(|r| (*id, r)))
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
            if b.def.index() >= db.buildings().count() {
                continue;
            }
            let stamp = building_stamp(b, build_ticks(db.building(b.def)), tick);
            if self.pieces.get(&id).is_some_and(|e| e.stamp == stamp) {
                continue;
            }
            let piece = build_building(id, b, db.building(b.def), net, terrain, db, tick);
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
