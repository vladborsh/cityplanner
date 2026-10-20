//! Who is glad and who is sorry when a city service comes or goes (T-287, user 2026-10-20):
//! the houses that gain a service's coverage show a smile, the ones that lose it a frown, in a
//! wave out from the building. Pure: the old and the new building grid in, the reactions out.
//!
//! * A service with a coverage radius (education, police, fire, health, deathcare, parks,
//!   landmarks, the post; never water, power, sewage, transit, industry or cargo): per kind it touched, the coverage the sim gives
//!   ([`cp_service::kind_coverage`], the same fold) before and after on the current roads; a
//!   residence whose access segment gained is happy, one that lost is sad. A move nets out: the
//!   houses that keep the service show nothing.
//! * Garbage (landfills, incinerators, recycling, depots) is never welcome: a residence within
//!   [`GARBAGE_M`] of a new one's footprint is sad, of one that went happy (relief). Industry
//!   and the rest beside it do not react. Render only: the sim has no nuisance for garbage yet.
//!
//! A diff with more service changes than [`LOAD_EVENTS`] is a load or a scenario, not a
//! player's action: no reaction.

use std::collections::BTreeMap;

use bevy_math::Vec2;
use cp_building::{Building, BuildingGrid, BuildingId, BuildingState};
use cp_content::{ContentDb, ServiceKind, ZoneType};
use cp_net::NetGraph;
use cp_service::{RoadGraph, kind_coverage};

use crate::city_events::{BuildingChange, BuildingEvent, building_events};

/// How near (metres) a residence may stand to a garbage building's footprint and mind it: 8
/// cells (user).
pub const GARBAGE_M: f32 = 64.0;
/// At most this many reactions per change, nearest the service first.
pub const MAX_REACTIONS: usize = 150;
/// Seconds the wave takes from the service out to the farthest reaction.
pub const WAVE_S: f32 = 0.6;
/// More service changes than this in one diff: a load, not a player's action.
pub const LOAD_EVENTS: usize = 12;
/// Smallest coverage change that shows.
const EPS: f32 = 0.01;

/// One house's reaction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mood {
    pub building: BuildingId,
    pub pos: Vec2,
    pub happy: bool,
    /// Seconds after the change it shows (the wave).
    pub delay: f32,
}

/// How a service kind is felt: `Some(true)` by its coverage, `Some(false)` as a nuisance
/// (garbage), `None` not at all (the utilities, roads, transit, industry, cargo).
fn felt(kind: ServiceKind) -> Option<bool> {
    use ServiceKind as K;
    match kind {
        K::Health
        | K::Deathcare
        | K::Fire
        | K::Police
        | K::Education
        | K::Landmark
        | K::Park
        | K::Post => Some(true),
        K::Garbage => Some(false),
        K::Water | K::Power | K::Roads | K::Sewage | K::Transit | K::Industry | K::Cargo => None,
    }
}

fn service_of(db: &ContentDb, b: &Building) -> Option<ServiceKind> {
    let name = db.building(b.def).service.as_deref()?;
    Some(db.service(db.service_id(name)?).kind)
}

fn residential(db: &ContentDb, b: &Building) -> bool {
    b.state == BuildingState::Active
        && matches!(
            db.building(b.def).zone,
            ZoneType::ResLow | ZoneType::ResHigh
        )
}

/// The distance from `p` to the footprint of `b` (0 inside).
fn to_footprint(b: &Building, p: Vec2) -> f32 {
    if b.footprint_contains(p) {
        return 0.0;
    }
    let q = b.footprint_polygon();
    (0..4)
        .map(|i| {
            let (a, c) = (q[i], q[(i + 1) % 4]);
            let ab = c - a;
            let k = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            (a + ab * k).distance(p)
        })
        .fold(f32::MAX, f32::min)
}

/// The reactions to the change from `old` to `new` (the roads of `net`), nearest a source
/// first, at most [`MAX_REACTIONS`], each with its place in the wave.
pub fn moods(net: &NetGraph, old: &BuildingGrid, new: &BuildingGrid, db: &ContentDb) -> Vec<Mood> {
    moods_of(net, old, new, db, &building_events(old, new))
}

/// [`moods`] for changes already diffed.
pub fn moods_of(
    net: &NetGraph,
    old: &BuildingGrid,
    new: &BuildingGrid,
    db: &ContentDb,
    changes: &[BuildingChange],
) -> Vec<Mood> {
    // The service buildings that came (Active) or went, with their kind.
    let mut came: Vec<(&Building, ServiceKind)> = Vec::new();
    let mut went: Vec<(&Building, ServiceKind)> = Vec::new();
    for c in changes {
        let Some(kind) = service_of(db, &c.building).filter(|k| felt(*k).is_some()) else {
            continue;
        };
        match c.event {
            BuildingEvent::Activated => came.push((&c.building, kind)),
            BuildingEvent::Removed | BuildingEvent::Deactivated => {
                if c.event == BuildingEvent::Deactivated
                    || c.building.state == BuildingState::Active
                {
                    went.push((c.before.as_ref().unwrap_or(&c.building), kind));
                }
            }
            BuildingEvent::Moved => {
                came.push((&c.building, kind));
                if let Some(b) = &c.before {
                    went.push((b, kind));
                }
            }
            BuildingEvent::Placed => {}
        }
    }
    if came.is_empty() && went.is_empty() || came.len() + went.len() > LOAD_EVENTS {
        return Vec::new();
    }
    // Per residence: + glad, − sorry.
    let mut score: BTreeMap<BuildingId, f32> = BTreeMap::new();
    // Coverage kinds: the sim's fold before and after.
    let mut kinds: Vec<ServiceKind> = came
        .iter()
        .chain(&went)
        .map(|(_, k)| *k)
        .filter(|k| felt(*k) == Some(true))
        .collect();
    kinds.sort();
    kinds.dedup();
    if !kinds.is_empty() {
        let g = RoadGraph::new(net);
        for kind in kinds {
            let before = kind_coverage(&g, old, db, kind);
            let after = kind_coverage(&g, new, db, kind);
            for (id, b) in new.iter().filter(|(_, b)| residential(db, b)) {
                let Some(seg) = b.access().map(|a| a.segment.index as usize) else {
                    continue;
                };
                let d = after.get(seg).copied().unwrap_or(0.0)
                    - before.get(seg).copied().unwrap_or(0.0);
                if d.abs() > EPS {
                    *score.entry(id).or_default() += d.signum();
                }
            }
        }
    }
    // Garbage: sorry beside a new one, glad when one went.
    for (list, sign) in [(&came, -1.0f32), (&went, 1.0)] {
        for (g, _) in list.iter().filter(|(_, k)| *k == ServiceKind::Garbage) {
            for (id, b) in new.iter().filter(|(_, b)| residential(db, b)) {
                if to_footprint(g, b.pos) <= GARBAGE_M {
                    *score.entry(id).or_default() += sign;
                }
            }
        }
    }
    // The wave: by distance from the nearest source.
    let sources: Vec<Vec2> = came.iter().chain(&went).map(|(b, _)| b.pos).collect();
    let mut out: Vec<(f32, Mood)> = score
        .into_iter()
        .filter(|(_, s)| *s != 0.0)
        .filter_map(|(id, s)| {
            let b = new.get(id)?;
            let d = sources
                .iter()
                .map(|p| p.distance(b.pos))
                .fold(f32::MAX, f32::min);
            Some((
                d,
                Mood {
                    building: id,
                    pos: b.pos,
                    happy: s > 0.0,
                    delay: 0.0,
                },
            ))
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.building.cmp(&b.1.building)));
    out.truncate(MAX_REACTIONS);
    let far = out.last().map_or(1.0, |(d, _)| d.max(1.0));
    out.into_iter()
        .map(|(d, m)| Mood {
            delay: WAVE_S * d / far,
            ..m
        })
        .collect()
}

// --- The emoji (render time only, never the sim tick).

/// Seconds an emoji lives once it shows.
pub const LIFE_S: f32 = 2.0;
/// How far it rises over its life, metres.
pub const RISE_M: f32 = 1.5;
/// Height of its centre above the roof at the start, metres: over the problem icons
/// (`problem::LIFT_M`), which share the roof.
pub const LIFT_M: f32 = 6.0;
/// Its size against a problem icon's at the same distance.
const SIZE_K: f32 = 1.25;
/// Seconds it takes to pop up to full size.
const POP_S: f32 = 0.15;
/// It fades over its last this many seconds.
const FADE_S: f32 = 0.8;

const BORDER: [f32; 3] = [0.08, 0.08, 0.09];
const HAPPY: [f32; 3] = [0.36, 0.80, 0.32];
const SAD: [f32; 3] = [0.90, 0.26, 0.20];
const FACE_DARK: [f32; 3] = [0.10, 0.09, 0.08];

/// A live emoji: where (the roof top), which, and its age in seconds (negative: not yet shown).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emoji {
    pub at: bevy_math::Vec3,
    pub happy: bool,
    pub age: f32,
}

/// Scale and opacity at `age` (0 before it shows and after it lives).
pub fn pop_and_fade(age: f32) -> (f32, f32) {
    if !(0.0..LIFE_S).contains(&age) {
        return (0.0, 0.0);
    }
    let pop = (age / POP_S).min(1.0);
    // A small overshoot as it pops.
    let scale = pop * (1.0 + 0.15 * (1.0 - pop) * 4.0 * pop);
    let alpha = ((LIFE_S - age) / FADE_S).clamp(0.0, 1.0);
    (scale, alpha)
}

/// A disc of radius `r` at `c` in the icon's square, as a fan.
fn disc(c: [f32; 2], r: f32, n: usize) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect()
}

/// The mouth: quads along an arc centred at `c` of radius `r` from angle `a0` to `a1`
/// (radians), `w` thick.
fn arc(c: [f32; 2], r: f32, a0: f32, a1: f32, w: f32) -> Vec<Vec<[f32; 2]>> {
    const N: usize = 8;
    let at = |a: f32, rr: f32| [c[0] + rr * a.cos(), c[1] + rr * a.sin()];
    (0..N)
        .map(|i| {
            let (s, e) = (
                a0 + (a1 - a0) * i as f32 / N as f32,
                a0 + (a1 - a0) * (i + 1) as f32 / N as f32,
            );
            vec![
                at(s, r - w / 2.0),
                at(e, r - w / 2.0),
                at(e, r + w / 2.0),
                at(s, r + w / 2.0),
            ]
        })
        .collect()
}

/// One camera-facing mesh of every shown emoji: a round face with a dark rim, two eyes and a
/// smile (green) or a frown (red), rising and fading with its age. Sized like the problem icons.
pub fn emoji_mesh(emojis: &[Emoji], facing: &crate::problem::Facing) -> crate::chunk::MeshData {
    use std::f32::consts::PI;
    let mut m = crate::chunk::MeshData::default();
    for e in emojis {
        let (scale, alpha) = pop_and_fade(e.age);
        if alpha <= 0.0 || scale <= 0.0 {
            continue;
        }
        let pos = e.at + bevy_math::Vec3::Y * (LIFT_M + RISE_M * e.age / LIFE_S);
        let to_cam = facing.position - pos;
        let dist = to_cam.length();
        let size = crate::problem::icon_size(dist) * SIZE_K * scale;
        let toward = to_cam / dist.max(1e-3);
        let face = if e.happy { HAPPY } else { SAD };
        let rgba = |c: [f32; 3]| [c[0], c[1], c[2], alpha];
        let mut shapes: Vec<(Vec<[f32; 2]>, [f32; 4])> = vec![
            (disc([0.0, 0.0], 0.5, 20), rgba(BORDER)),
            (disc([0.0, 0.0], 0.43, 20), rgba(face)),
            (disc([-0.15, 0.12], 0.065, 8), rgba(FACE_DARK)),
            (disc([0.15, 0.12], 0.065, 8), rgba(FACE_DARK)),
        ];
        let mouth = if e.happy {
            arc([0.0, 0.04], 0.24, PI * 1.15, PI * 1.85, 0.07)
        } else {
            arc([0.0, -0.34], 0.22, PI * 0.2, PI * 0.8, 0.07)
        };
        shapes.extend(mouth.into_iter().map(|q| (q, rgba(FACE_DARK))));
        for (layer, (pts, color)) in shapes.iter().enumerate() {
            let base = m.positions.len() as u32;
            let lift = toward * (layer as f32 * 0.02 * size);
            for p in pts {
                let v = pos + (facing.right * p[0] + facing.up * p[1]) * size + lift;
                m.positions.push(v.to_array());
                m.normals.push(toward.to_array());
                m.colors.push(*color);
            }
            for i in 1..pts.len() as u32 - 1 {
                m.indices.extend([base, base + i, base + i + 1]);
            }
        }
    }
    m
}
