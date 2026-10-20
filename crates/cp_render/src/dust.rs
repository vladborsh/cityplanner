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
pub const PUFF_S: f32 = 1.5;
/// At most this many puffs live at once (the oldest go first).
pub const MAX_PUFFS: usize = 64;
/// One puff per this many metres of a road laid or removed (2 cells).
pub const ROAD_STEP_M: f32 = 16.0;
/// More changes than this in one diff: a load or a scenario, no dust.
pub const LOAD_CHANGES: usize = 400;
/// A demolition's puff is this much bigger than a build's.
const DEMOLISH_K: f32 = 1.15;
/// Blobs per puff.
const BLOBS: usize = 6;

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

/// One camera-facing mesh of every live puff: each a ring of soft blobs (a fan from an opaque
/// centre to a clear rim) that billows out low, rises a little and fades.
pub fn dust_mesh(puffs: &[LivePuff], facing: &Facing) -> MeshData {
    const RIM: usize = 12;
    let mut m = MeshData::default();
    for p in puffs {
        if !(0.0..PUFF_S).contains(&p.age) {
            continue;
        }
        let t = p.age / PUFF_S;
        let grow = 1.0 - (1.0 - t).powi(3);
        let alpha = (1.0 - t).sqrt() * (t / 0.08).min(1.0);
        let rgb = if p.demolish {
            DEMOLISH_DUST
        } else {
            BUILD_DUST
        };
        // Overlapping blobs add up: a demolition's are bigger, each a little thinner.
        let (k, density) = if p.demolish {
            (DEMOLISH_K, 0.85)
        } else {
            (1.0, 1.0)
        };
        for b in 0..BLOBS {
            let seed = p.seed.wrapping_mul(31).wrapping_add(b as u32);
            let ang = (b as f32 + unit(seed) * 0.6) / BLOBS as f32 * std::f32::consts::TAU;
            let out = p.radius * (0.35 + 0.65 * grow) * (0.8 + 0.4 * unit(seed ^ 0x9e37));
            let size = p.radius * 0.45 * k * (0.5 + 0.7 * grow);
            // Lifted by its own size: a camera-facing disc must not sink into the ground.
            let centre = p.at
                + Vec3::new(ang.cos(), 0.0, ang.sin()) * out
                + Vec3::Y * (size * 0.8 + 1.2 * grow + unit(seed ^ 0x51) * 0.6);
            let toward = (facing.position - centre).normalize_or_zero();
            // A soft blob: as thick as the centre out to 75 %, then a clear rim.
            let a0 = (alpha * density).min(1.0);
            let base = m.positions.len() as u32;
            m.positions.push(centre.to_array());
            m.normals.push(toward.to_array());
            m.colors.push([rgb[0], rgb[1], rgb[2], a0]);
            for (r, a) in [(0.75, a0), (1.0, 0.0)] {
                for i in 0..RIM {
                    let ang = i as f32 / RIM as f32 * std::f32::consts::TAU;
                    let v = centre + (facing.right * ang.cos() + facing.up * ang.sin()) * size * r;
                    m.positions.push(v.to_array());
                    m.normals.push(toward.to_array());
                    m.colors.push([rgb[0], rgb[1], rgb[2], a]);
                }
            }
            let (inner, outer) = (base + 1, base + 1 + RIM as u32);
            for i in 0..RIM as u32 {
                let j = (i + 1) % RIM as u32;
                m.indices.extend([base, inner + i, inner + j]);
                m.indices.extend([
                    inner + i,
                    outer + i,
                    outer + j,
                    inner + i,
                    outer + j,
                    inner + j,
                ]);
            }
        }
    }
    m
}
