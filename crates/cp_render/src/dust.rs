//! Dust when something is built or knocked down (T-288, user 2026-10-20): a small sandy puff
//! round a placed building and along a new road, a darker, bigger one where a building or a road
//! went. Pure: the city's changes ([`crate::city_events`]) in, the puffs out, and the puffs'
//! mesh at an age; render time only, never the sim tick.

use bevy_math::{Vec2, Vec3};
use cp_building::{Building, FOOTPRINT_CELL_M, is_growable};
use cp_content::ContentDb;
use cp_core::curve::to_xz;

use crate::chunk::MeshData;
use crate::city_events::{BuildingChange, BuildingEvent, SegmentChange};
use crate::problem::Facing;

/// Seconds a puff lives.
pub const PUFF_S: f32 = 2.2;
/// At most this many puffs live at once (the oldest go first).
pub const MAX_PUFFS: usize = 24;
/// One puff per this many metres of a road laid or removed (6 cells; T-299: was 2).
pub const ROAD_STEP_M: f32 = 48.0;
/// More changes than this in one diff: a load or a scenario, no dust.
pub const LOAD_CHANGES: usize = 400;
/// A demolition's puff is this much bigger than a build's.
const DEMOLISH_K: f32 = 1.15;
/// Spheres per puff.
const BLOBS: usize = 4;
/// Opacity of a sphere where it faces the eye, at the start.
const PEAK_ALPHA: f32 = 0.9;

/// Light sandy beige (a build) and a darker grey-brown (a demolition).
pub const BUILD_DUST: [f32; 3] = [0.82, 0.72, 0.52];
pub const DEMOLISH_DUST: [f32; 3] = [0.58, 0.53, 0.46];

/// One puff: where on the ground plane, how big (metres across its ring), what kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Puff {
    pub at: Vec2,
    pub radius: f32,
    pub demolish: bool,
}

/// The radius of a building's puff: half its footprint's diagonal.
fn building_radius(b: &Building) -> f32 {
    let (w, d) = b.size();
    (f32::from(w).hypot(f32::from(d)) * FOOTPRINT_CELL_M * 0.5).max(4.0)
}

/// Puffs along a road piece: one per [`ROAD_STEP_M`], centred in its stretch.
fn along(s: &SegmentChange, demolish: bool, out: &mut Vec<Puff>) {
    let len = s.curve.arc_table().length();
    let n = (len / ROAD_STEP_M).round().max(1.0) as usize;
    for k in 0..n {
        let t = s
            .curve
            .arc_table()
            .s_to_t((k as f32 + 0.5) * len / n as f32);
        out.push(Puff {
            at: to_xz(s.curve.point(t)),
            radius: s.half_width + 2.0,
            demolish,
        });
    }
}

/// The puffs of one diff: a placed building that is not a grown house (the player put it
/// there), a building gone (any), a building moved (dust where it went and where it left), and
/// every road piece laid or removed. Nothing for a load ([`LOAD_CHANGES`]).
pub fn puffs(
    buildings: &[BuildingChange],
    laid: &[SegmentChange],
    removed: &[SegmentChange],
    db: &ContentDb,
) -> Vec<Puff> {
    if buildings.len() + laid.len() + removed.len() > LOAD_CHANGES {
        return Vec::new();
    }
    let mut out = Vec::new();
    for c in buildings {
        let b = &c.building;
        match c.event {
            BuildingEvent::Placed if !is_growable(db.building(b.def)) => out.push(Puff {
                at: b.pos,
                radius: building_radius(b),
                demolish: false,
            }),
            BuildingEvent::Removed => out.push(Puff {
                at: b.pos,
                radius: building_radius(b) * DEMOLISH_K,
                demolish: true,
            }),
            BuildingEvent::Moved => {
                out.push(Puff {
                    at: b.pos,
                    radius: building_radius(b),
                    demolish: false,
                });
                if let Some(o) = &c.before {
                    out.push(Puff {
                        at: o.pos,
                        radius: building_radius(o) * DEMOLISH_K,
                        demolish: true,
                    });
                }
            }
            _ => {}
        }
    }
    for s in laid {
        along(s, false, &mut out);
    }
    for s in removed {
        along(s, true, &mut out);
    }
    out
}

/// The cap: keeps the newest [`MAX_PUFFS`] of `live` (oldest first).
pub fn cap<T>(live: &mut Vec<T>) {
    let over = live.len().saturating_sub(MAX_PUFFS);
    live.drain(..over);
}

/// A cheap hash of `k` to `0..1`, for the blobs' scatter.
fn unit(k: u32) -> f32 {
    let mut v = k.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    v ^= v >> 16;
    v = v.wrapping_mul(0x85eb_ca6b);
    v ^= v >> 13;
    (v & 0xffff) as f32 / 65_535.0
}

/// A puff in the world: on the ground at `at`, `age` seconds old, `seed` for its scatter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LivePuff {
    pub at: Vec3,
    pub radius: f32,
    pub demolish: bool,
    pub age: f32,
    pub seed: u32,
}

/// The unit icosphere (one subdivision: 42 vertices, 80 triangles, wound outward), once.
fn sphere() -> &'static (Vec<Vec3>, Vec<[u32; 3]>) {
    static SPHERE: std::sync::OnceLock<(Vec<Vec3>, Vec<[u32; 3]>)> = std::sync::OnceLock::new();
    SPHERE.get_or_init(|| {
        let t = (1.0 + 5f32.sqrt()) / 2.0;
        let mut v: Vec<Vec3> = [
            (-1.0, t, 0.0),
            (1.0, t, 0.0),
            (-1.0, -t, 0.0),
            (1.0, -t, 0.0),
            (0.0, -1.0, t),
            (0.0, 1.0, t),
            (0.0, -1.0, -t),
            (0.0, 1.0, -t),
            (t, 0.0, -1.0),
            (t, 0.0, 1.0),
            (-t, 0.0, -1.0),
            (-t, 0.0, 1.0),
        ]
        .into_iter()
        .map(|(x, y, z)| Vec3::new(x, y, z).normalize())
        .collect();
        let f: [[u32; 3]; 20] = [
            [0, 11, 5],
            [0, 5, 1],
            [0, 1, 7],
            [0, 7, 10],
            [0, 10, 11],
            [1, 5, 9],
            [5, 11, 4],
            [11, 10, 2],
            [10, 7, 6],
            [7, 1, 8],
            [3, 9, 4],
            [3, 4, 2],
            [3, 2, 6],
            [3, 6, 8],
            [3, 8, 9],
            [4, 9, 5],
            [2, 4, 11],
            [6, 2, 10],
            [8, 6, 7],
            [9, 8, 1],
        ];
        let mut mid = std::collections::BTreeMap::new();
        let mut tris = Vec::new();
        let mut half = |a: u32, b: u32, v: &mut Vec<Vec3>| -> u32 {
            let key = (a.min(b), a.max(b));
            *mid.entry(key).or_insert_with(|| {
                v.push(((v[a as usize] + v[b as usize]) * 0.5).normalize());
                v.len() as u32 - 1
            })
        };
        for [a, b, c] in f {
            let (ab, bc, ca) = (half(a, b, &mut v), half(b, c, &mut v), half(c, a, &mut v));
            tris.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        for t in &mut tris {
            let (p0, p1, p2) = (v[t[0] as usize], v[t[1] as usize], v[t[2] as usize]);
            if (p1 - p0).cross(p2 - p0).dot(p0 + p1 + p2) < 0.0 {
                t.swap(1, 2);
            }
        }
        (v, tris)
    })
}

/// Direction the light comes from, for the blobs' baked shading (the sun stands high).
const LIGHT: Vec3 = Vec3::new(0.35, 0.85, 0.4);

/// One mesh of every live puff (T-299): each a few lit, translucent spheres that billow out
/// low, rise and thin out. The shading is baked into the vertex colour (light from above, the
/// underside darker) and the rim fades where the surface turns away from the eye, so a puff
/// reads as a soft body, not a flat disc. Draw it with back faces culled.
pub fn dust_mesh(puffs: &[LivePuff], facing: &Facing) -> MeshData {
    let (verts, tris) = sphere();
    let light = LIGHT.normalize();
    let mut m = MeshData::default();
    for p in puffs {
        if !(0.0..PUFF_S).contains(&p.age) {
            continue;
        }
        let t = p.age / PUFF_S;
        let grow = 1.0 - (1.0 - t).powi(3);
        let alpha = PEAK_ALPHA * (1.0 - t).sqrt() * (t / 0.1).min(1.0);
        let rgb = if p.demolish {
            DEMOLISH_DUST
        } else {
            BUILD_DUST
        };
        let (k, density) = if p.demolish {
            (DEMOLISH_K, 0.9)
        } else {
            (1.0, 1.0)
        };
        for b in 0..BLOBS {
            let seed = p.seed.wrapping_mul(31).wrapping_add(b as u32);
            let ang = (b as f32 + unit(seed) * 0.6) / BLOBS as f32 * std::f32::consts::TAU;
            let out = p.radius * (0.3 + 0.6 * grow) * (0.8 + 0.4 * unit(seed ^ 0x9e37));
            let size = p.radius * 0.42 * k * (0.45 + 0.75 * grow);
            // The cloud climbs as it thins; each blob at its own pace.
            let rise = (2.0 + 5.0 * unit(seed ^ 0x51)) * grow;
            let centre =
                p.at + Vec3::new(ang.cos(), 0.0, ang.sin()) * out + Vec3::Y * (size * 0.7 + rise);
            let base = m.positions.len() as u32;
            for n in verts {
                let pos = centre + *n * size;
                let to_eye = (facing.position - pos).normalize_or_zero();
                // Facing the eye: thick; turning away: a thin rim.
                let face = n.dot(to_eye).max(0.0).powf(0.6);
                let lit = 0.62 + 0.38 * (0.5 + 0.5 * n.dot(light));
                m.positions.push(pos.to_array());
                m.normals.push(n.to_array());
                m.colors.push([
                    rgb[0] * lit,
                    rgb[1] * lit,
                    rgb[2] * lit,
                    (alpha * density * (0.15 + 0.85 * face)).min(1.0),
                ]);
            }
            for tri in tris {
                m.indices.extend(tri.map(|i| base + i));
            }
        }
    }
    m
}
