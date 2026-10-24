//! Vehicles and pedestrians: per-frame instance lists (interpolation, camera-height gates,
//! frustum culling) and the procedural meshes they are drawn with. Pure data, no Bevy render
//! types; [`crate::agent_render`] uploads and draws the result. See `system-design.md` §13.3.
//!
//! Which vehicle looks to use comes from content (`VehicleDef` length/width per
//! `VehicleKind`); the colours are per-agent hashes of the stable agent key.

pub mod mesh;
#[cfg(test)]
mod tests;

use bevy_math::{Vec3, Vec4};
use cp_content::ContentDb;
use cp_sim::{PedestrianPose, VehiclePose};

/// Vehicles are drawn when the camera is below this height, metres (§13.3).
pub const VEHICLE_MAX_HEIGHT: f32 = 800.0;
/// Pedestrians are drawn when the camera is below this height, metres (§13.3).
pub const PEDESTRIAN_MAX_HEIGHT: f32 = 250.0;
/// Below this camera height vehicles use the detailed mesh, else the single-box one.
pub const VEHICLE_DETAIL_HEIGHT: f32 = 220.0;
/// Below this camera height pedestrians use the detailed mesh, else the single-box one.
pub const PEDESTRIAN_DETAIL_HEIGHT: f32 = 90.0;
/// Number of vehicle kinds (`VehicleKind` variants).
pub const VEHICLE_KINDS: usize = cp_content::VehicleKind::COUNT;
/// A pose that moved further than this in one snapshot step is a teleport (new lane after a
/// network change, ...): not interpolated.
const TELEPORT: f32 = 60.0;
/// Culling sphere radii, metres.
const VEHICLE_RADIUS: f32 = 7.0;
const PEDESTRIAN_RADIUS: f32 = 1.5;
/// A pedestrian near a door is drawn up to this far from where the sim has it, metres: it is
/// culled by the sim position with this much room (§25.3).
const PORTAL_REACH: f32 = 12.0;
/// The slow side-to-side drift of a walker: amplitude and wavelength, metres (§25.3).
const DRIFT_M: f32 = 0.35;
const DRIFT_WAVE_M: f32 = 9.0;
/// Pedestrian gait: metres walked per full stride (two steps). The swing phase is the
/// distance walked along the heading over this, so the feet do not slide (T-297).
pub const STRIDE: f32 = 1.4;

/// One drawn agent, as the instance vertex buffer holds it (48 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct InstanceData {
    /// Centre x, y, z and heading (`atan2(dir.x, dir.z)`).
    pub pos_yaw: [f32; 4],
    /// Width (x), height (y), length (z) scale of the unit mesh, and the gait phase.
    pub scale_phase: [f32; 4],
    /// Tint rgb (linear) and sway amplitude, metres (pedestrians; 0 for vehicles).
    pub color_sway: [f32; 4],
}

/// Instance lists for one frame: vehicles by `[kind][detail]`, pedestrians by `[detail]`
/// (detail 0 = full mesh, 1 = single box).
#[derive(Clone, Debug, Default)]
pub struct AgentInstances {
    pub vehicles: [[Vec<InstanceData>; 2]; VEHICLE_KINDS],
    pub pedestrians: [Vec<InstanceData>; 2],
    /// Cyclists (T-322) by `[detail]`: the bicycle mesh.
    pub bikes: [Vec<InstanceData>; 2],
}

impl AgentInstances {
    pub fn clear(&mut self) {
        for k in &mut self.vehicles {
            k[0].clear();
            k[1].clear();
        }
        self.pedestrians[0].clear();
        self.pedestrians[1].clear();
        self.bikes[0].clear();
        self.bikes[1].clear();
    }

    pub fn vehicle_count(&self) -> usize {
        self.vehicles.iter().flatten().map(Vec::len).sum()
    }

    pub fn pedestrian_count(&self) -> usize {
        self.pedestrians.iter().map(Vec::len).sum()
    }

    pub fn bike_count(&self) -> usize {
        self.bikes.iter().map(Vec::len).sum()
    }
}

/// Size of each vehicle kind, from content (`VehicleDef` length/width), `(width, height,
/// length)` metres.
#[derive(Clone, Debug, PartialEq)]
pub struct VehicleLooks {
    pub sizes: [Vec3; VEHICLE_KINDS],
}

impl Default for VehicleLooks {
    fn default() -> Self {
        Self {
            sizes: [
                Vec3::new(1.8, 1.45, 4.2),
                Vec3::new(2.4, 3.2, 8.5),
                Vec3::new(2.5, 3.1, 12.0),
                Vec3::new(2.0, 1.9, 5.2),
                // Metro (drawn elsewhere) and the goods truck.
                Vec3::new(2.0, 1.9, 5.2),
                Vec3::new(2.5, 3.4, 12.0),
                // The intercity train (drawn by `train_render`; the size is for a look-up).
                Vec3::new(3.0, 3.6, 120.0),
                // The chain trucks (T-213b): farm, logs, ore, tanker.
                Vec3::new(2.5, 3.4, 12.0),
                Vec3::new(2.5, 3.4, 12.0),
                Vec3::new(2.5, 3.4, 12.0),
                Vec3::new(2.5, 3.4, 12.0),
                // The cargo ship (T-217; not an agent, drawn at the port's berth).
                Vec3::new(20.0, 14.0, 120.0),
                // The cargo train (T-216b; drawn like the intercity train).
                Vec3::new(3.0, 3.6, 140.0),
                // The mail van (T-247).
                Vec3::new(2.0, 2.3, 5.5),
                // The tram and the trolleybus (T-302; their meshes come with T-302b/c).
                Vec3::new(2.6, 3.4, 30.0),
                Vec3::new(2.5, 3.2, 12.0),
            ],
        }
    }
}

impl VehicleLooks {
    /// The first definition (in id order) of each kind sets that kind's footprint; kinds
    /// without a definition keep the defaults.
    pub fn from_content(db: &ContentDb) -> Self {
        let mut looks = Self::default();
        let mut seen = [false; VEHICLE_KINDS];
        for (_, def) in db.vehicles() {
            let k = def.kind as usize;
            if k < VEHICLE_KINDS && !seen[k] {
                seen[k] = true;
                let s = &mut looks.sizes[k];
                // Height keeps the default's proportion to its length.
                let ratio = s.y / s.z;
                *s = Vec3::new(def.width, def.length * ratio, def.length);
            }
        }
        looks
    }
}

/// What the camera sees: position/height, frustum planes (as `Vec4(normal, d)`, inside when
/// `dot(n, p) + d >= 0`) and the interpolation state.
#[derive(Clone, Debug)]
pub struct Cull {
    /// Camera height above the terrain below it; the §13.3 thresholds apply to this.
    pub height: f32,
    pub planes: [Vec4; 6],
    /// Real-time fraction between the previous and the current snapshot, `0..=1`.
    pub alpha: f32,
    /// Wall time in seconds, for the gait.
    pub time: f32,
}

impl Cull {
    fn sees(&self, p: Vec3, radius: f32) -> bool {
        self.planes
            .iter()
            .all(|pl| pl.truncate().dot(p) + pl.w >= -radius)
    }
}

/// splitmix64: a cheap, well-mixed hash of an agent key.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn unit(h: u64, shift: u32) -> f32 {
    ((h >> shift) & 0xFFFF) as f32 / 65535.0
}

const CAR_COLORS: [[f32; 3]; 8] = [
    [0.80, 0.80, 0.82],
    [0.08, 0.08, 0.10],
    [0.55, 0.06, 0.06],
    [0.10, 0.22, 0.50],
    [0.60, 0.60, 0.62],
    [0.12, 0.35, 0.20],
    [0.75, 0.62, 0.15],
    [0.35, 0.18, 0.10],
];
const SHIRT_COLORS: [[f32; 3]; 6] = [
    [0.55, 0.12, 0.10],
    [0.12, 0.25, 0.55],
    [0.80, 0.78, 0.70],
    [0.15, 0.40, 0.25],
    [0.85, 0.55, 0.12],
    [0.20, 0.20, 0.24],
];

/// Fixed colour of a kind (buses, trucks and emergency vehicles are not randomised).
fn vehicle_color(kind: usize, h: u64) -> [f32; 3] {
    match kind {
        0 => CAR_COLORS[(h % CAR_COLORS.len() as u64) as usize],
        1 => [0.70, 0.45, 0.12],
        7 => [0.25, 0.55, 0.22],
        8 => [0.60, 0.30, 0.12],
        9 => [0.45, 0.42, 0.40],
        10 => [0.78, 0.62, 0.12],
        2 => [0.12, 0.45, 0.70],
        // The mail van's post yellow (T-248).
        13 => [0.95, 0.74, 0.12],
        _ => [0.85, 0.85, 0.88],
    }
}

fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let mut d = (b - a) % std::f32::consts::TAU;
    if d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    } else if d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    a + d * t
}

/// Walks `prev` and `cur` (both ascending by key) in step: calls `f(cur, matching prev)`.
fn zip_by_key<T: Copy>(
    prev: &[T],
    cur: &[T],
    key: impl Fn(&T) -> u64,
    mut f: impl FnMut(&T, Option<&T>),
) {
    let mut j = 0;
    for c in cur {
        let k = key(c);
        while j < prev.len() && key(&prev[j]) < k {
            j += 1;
        }
        f(c, prev.get(j).filter(|p| key(p) == k));
    }
}

/// Where the renderer draws a vehicle `alpha` of the way from its pose in the previous snapshot
/// to `cur` (a jump of 60 m or more is a teleport and not interpolated). Picking uses it so
/// the player clicks what is on screen.
pub fn vehicle_position(prev: Option<&VehiclePose>, cur: &VehiclePose, alpha: f32) -> Vec3 {
    match prev {
        Some(p) if p.pos.distance_squared(cur.pos) < TELEPORT * TELEPORT => {
            p.pos.lerp(cur.pos, alpha.clamp(0.0, 1.0))
        }
        _ => cur.pos,
    }
}

/// [`vehicle_position`] for pedestrians (ground plane).
pub fn pedestrian_position(
    prev: Option<&PedestrianPose>,
    cur: &PedestrianPose,
    alpha: f32,
) -> bevy_math::Vec2 {
    match prev {
        Some(p) if p.pos.distance_squared(cur.pos) < TELEPORT * TELEPORT => {
            p.pos.lerp(cur.pos, alpha.clamp(0.0, 1.0))
        }
        _ => cur.pos,
    }
}

/// Cars waiting in one yard that are drawn there, one behind the other; the rest wait unseen
/// (§25.4).
pub const YARD_QUEUE: u32 = 3;

/// A car still in its yard, waiting to pull out (portal progress 0).
fn waiting(v: &VehiclePose) -> bool {
    v.portal.is_some_and(|p| p.leaving && p.progress <= 0.0)
}

/// Place of every car of `list` in the queue of its yard, by key order (`0` for the cars not
/// waiting).
fn queue_ranks(list: &[VehiclePose]) -> Vec<u32> {
    let mut count = std::collections::BTreeMap::<cp_building::BuildingId, u32>::new();
    list.iter()
        .map(|v| match v.portal {
            Some(p) if waiting(v) => {
                let k = count.entry(p.building).or_insert(0);
                *k += 1;
                *k - 1
            }
            _ => 0,
        })
        .collect()
}

/// Where vehicle `c` is drawn and its heading (§25.4): near the yard of its building
/// (`yard(building)`) it is drawn on its way between the yard and the sim pose, `len` metres
/// long; `rank` is its place in the queue of cars waiting there (`k` stands `k × (len + 1)`
/// deeper in the yard).
pub fn vehicle_drawn(
    c: &VehiclePose,
    len: f32,
    rank: u32,
    yard: &impl Fn(cp_building::BuildingId) -> Option<cp_building::Portals>,
) -> (Vec3, f32) {
    let Some((pt, g)) = c.portal.and_then(|pt| Some((pt, yard(pt.building)?))) else {
        return (c.pos, c.yaw);
    };
    let deep = len / 2.0 + 1.0 + rank as f32 * (len + 1.0);
    let y = g.drive - g.out * deep;
    let at = Vec3::new(y.x, c.pos.y, y.y);
    let sim = bevy_math::Vec2::new(c.pos.x, c.pos.z);
    let lane = bevy_math::Vec2::new(c.yaw.sin(), c.yaw.cos());
    let s = smoothstep(pt.progress);
    let (pos, dir) = if pt.leaving {
        let d = (sim - y).try_normalize().unwrap_or(g.out);
        (at.lerp(c.pos, s), d.lerp(lane, s))
    } else {
        let d = (y - sim).try_normalize().unwrap_or(-g.out);
        (c.pos.lerp(at, s), lane.lerp(d, s))
    };
    let yaw = if dir.length_squared() > 1e-6 {
        dir.x.atan2(dir.y)
    } else {
        c.yaw
    };
    (pos, yaw)
}

/// Walks the snapshot pair like [`zip_by_key`], with each car drawn by [`vehicle_drawn`] in
/// both snapshots and interpolated by `alpha` (a sim jump of 60 m or more is a teleport and not
/// interpolated): `f(cur, drawn position, heading)`. Cars waiting beyond the first
/// [`YARD_QUEUE`] of their yard are skipped.
fn zip_drawn(
    prev: &[VehiclePose],
    cur: &[VehiclePose],
    alpha: f32,
    looks: &VehicleLooks,
    yard: &impl Fn(cp_building::BuildingId) -> Option<cp_building::Portals>,
    mut f: impl FnMut(&VehiclePose, Vec3, f32),
) {
    let a = alpha.clamp(0.0, 1.0);
    let len = |v: &VehiclePose| looks.sizes[usize::from(v.kind).min(VEHICLE_KINDS - 1)].z;
    let prev_rank = queue_ranks(prev);
    let cur_rank = queue_ranks(cur);
    let mut j = 0;
    for (i, c) in cur.iter().enumerate() {
        while j < prev.len() && prev[j].key < c.key {
            j += 1;
        }
        if cur_rank[i] >= YARD_QUEUE {
            continue;
        }
        let now = vehicle_drawn(c, len(c), cur_rank[i], yard);
        let (pos, yaw) = match prev.get(j).filter(|p| p.key == c.key) {
            Some(p) if p.pos.distance_squared(c.pos) < TELEPORT * TELEPORT => {
                let before = vehicle_drawn(p, len(p), prev_rank[j], yard);
                (before.0.lerp(now.0, a), lerp_angle(before.1, now.1, a))
            }
            _ => now,
        };
        f(c, pos, yaw);
    }
}

/// Every vehicle `(key, drawn position)` of the snapshot pair at `alpha`, where
/// [`build_vehicles`] draws it (picking uses it so the player clicks what is on screen).
pub fn vehicle_positions(
    prev: &[VehiclePose],
    cur: &[VehiclePose],
    alpha: f32,
    looks: &VehicleLooks,
    yard: impl Fn(cp_building::BuildingId) -> Option<cp_building::Portals>,
) -> Vec<(u64, Vec3)> {
    let mut out = Vec::with_capacity(cur.len());
    zip_drawn(prev, cur, alpha, looks, &yard, |c, pos, _| {
        out.push((c.key, pos))
    });
    out
}

/// [`vehicle_positions`] of the view's snapshot pair, with the looks from its content and the
/// yards of its buildings.
pub fn view_vehicle_positions(view: &cp_sim::SimView, alpha: f32) -> Vec<(u64, Vec3)> {
    let looks = view
        .content
        .as_deref()
        .map_or_else(VehicleLooks::default, VehicleLooks::from_content);
    vehicle_positions(
        &view.agents.prev.vehicles,
        &view.agents.cur.vehicles,
        alpha,
        &looks,
        |id| view.buildings.get(id)?.portals(&view.net),
    )
}

/// Every pedestrian `(key, drawn position)` of the snapshot pair at `alpha`.
pub fn pedestrian_positions(
    prev: &[PedestrianPose],
    cur: &[PedestrianPose],
    alpha: f32,
) -> Vec<(u64, bevy_math::Vec2)> {
    let mut out = Vec::with_capacity(cur.len());
    zip_by_key(
        prev,
        cur,
        |p| p.key,
        |c, p| {
            out.push((c.key, pedestrian_position(p, c, alpha)));
        },
    );
    out
}

/// Fills `out.vehicles` with the interpolated, culled vehicles, drawn where
/// [`vehicle_drawn`] has them (`yard` gives a building's yard). Nothing is drawn when the
/// camera is above [`VEHICLE_MAX_HEIGHT`].
pub fn build_vehicles(
    prev: &[VehiclePose],
    cur: &[VehiclePose],
    looks: &VehicleLooks,
    yard: impl Fn(cp_building::BuildingId) -> Option<cp_building::Portals>,
    cull: &Cull,
    out: &mut AgentInstances,
) {
    let h = cull.height;
    if h > VEHICLE_MAX_HEIGHT {
        return;
    }
    let detail = usize::from(h > VEHICLE_DETAIL_HEIGHT);
    zip_drawn(prev, cur, cull.alpha, looks, &yard, |c, pos, yaw| {
        if !cull.sees(pos, VEHICLE_RADIUS) {
            return;
        }
        debug_assert!(
            usize::from(c.kind) < VEHICLE_KINDS,
            "vehicle kind {} has no look",
            c.kind
        );
        let kind = usize::from(c.kind).min(VEHICLE_KINDS - 1);
        let hash = mix(c.key);
        let size = looks.sizes[kind];
        let col = c
            .tint
            .map(|t| t.map(|v| f32::from(v) / 255.0))
            .unwrap_or_else(|| vehicle_color(kind, hash));
        out.vehicles[kind][detail].push(InstanceData {
            pos_yaw: [pos.x, pos.y, pos.z, yaw],
            scale_phase: [size.x, size.y, size.z, 0.0],
            color_sway: [col[0], col[1], col[2], 0.0],
        });
    });
}

/// Fills `out.vehicles` (cars, kind 0) with the parked cars of the bays (T-276), culled like
/// the moving ones: none above [`VEHICLE_MAX_HEIGHT`], the box from [`VEHICLE_DETAIL_HEIGHT`].
pub fn build_parked(
    buckets: &[crate::parked::ParkedBucket],
    looks: &VehicleLooks,
    cull: &Cull,
    out: &mut AgentInstances,
) {
    let h = cull.height;
    if h > VEHICLE_MAX_HEIGHT {
        return;
    }
    let detail = usize::from(h > VEHICLE_DETAIL_HEIGHT);
    let size = looks.sizes[0];
    for c in buckets
        .iter()
        .filter(|b| cull.sees(b.center, b.radius + VEHICLE_RADIUS))
        .flat_map(|b| &b.cars)
    {
        if !cull.sees(c.pos, VEHICLE_RADIUS) {
            continue;
        }
        let col = vehicle_color(0, mix(c.key));
        out.vehicles[0][detail].push(InstanceData {
            pos_yaw: [c.pos.x, c.pos.y, c.pos.z, c.yaw],
            scale_phase: [size.x, size.y, size.z, 0.0],
            color_sway: [col[0], col[1], col[2], 0.0],
        });
    }
}

/// Last heading and sideways direction of every pedestrian `(key, yaw, side)`, ascending by
/// key: one that stops keeps facing the way it walked instead of snapping to a random
/// direction (BUG-064), and keeps its place across the sidewalk.
#[derive(Clone, Debug, Default)]
pub struct Headings {
    list: Vec<(u64, f32, bevy_math::Vec2)>,
    next: Vec<(u64, f32, bevy_math::Vec2)>,
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Where pedestrian `c` is drawn (§25.3): `side` metres across its lane from the sim position,
/// where `side` is the unit direction across the way it walks and `along` the distance walked
/// along it (for the drift). The spread is the key's own share of the pose's `room` plus a slow
/// drift, squeezed into `clip(pos, side, lo, hi)` (the bays: nobody walks through a parked
/// car). Near
/// a door (`portal`) the walker is drawn on its way between `door(building)` and that point.
pub fn pedestrian_drawn(
    c: &PedestrianPose,
    side: bevy_math::Vec2,
    along: f32,
    clip: &impl Fn(bevy_math::Vec2, bevy_math::Vec2, f32, f32) -> (f32, f32),
    door: &impl Fn(cp_building::BuildingId) -> Option<bevy_math::Vec2>,
) -> bevy_math::Vec2 {
    let h = mix(c.key);
    let room = c.room.max(0.0);
    let amp = DRIFT_M.min(room);
    let phase = unit(mix(h), 0) * std::f32::consts::TAU;
    let lat = (unit(h, 0) * 2.0 - 1.0) * room
        + amp * (along / DRIFT_WAVE_M * std::f32::consts::TAU + phase).sin();
    // The whole spread is squeezed into what the bays leave clear, not cut at its edge: nobody
    // lines up on the bay's edge.
    let full = room + amp;
    let (lo, hi) = clip(c.pos, side, -full, full);
    let x = if full > 1e-4 {
        lo + (lat + full) / (2.0 * full) * (hi - lo).max(0.0)
    } else {
        0.0f32.clamp(lo, hi.max(lo))
    };
    let p = c.pos + side * x;
    match c.portal.and_then(|pt| Some((pt, door(pt.building)?))) {
        Some((pt, d)) if pt.leaving => d.lerp(p, smoothstep(pt.progress)),
        Some((pt, d)) => p.lerp(d, smoothstep(pt.progress)),
        None => p,
    }
}

/// Fills `out.pedestrians`. Nothing is drawn above [`PEDESTRIAN_MAX_HEIGHT`]. `ground` gives
/// the terrain height at (x, z). Each walker is drawn at [`pedestrian_drawn`] in both snapshots
/// and interpolated between them; the heading follows that drawn motion. A pedestrian that did
/// not move stands (no sway) facing its last heading in `headings` (a random one the first
/// time it is seen) and keeps its side direction. `clip` and `door` are those of
/// [`pedestrian_drawn`].
#[allow(clippy::too_many_arguments)]
pub fn build_pedestrians(
    prev: &[PedestrianPose],
    cur: &[PedestrianPose],
    ground: impl Fn(f32, f32) -> f32,
    clip: impl Fn(bevy_math::Vec2, bevy_math::Vec2, f32, f32) -> (f32, f32),
    door: impl Fn(cp_building::BuildingId) -> Option<bevy_math::Vec2>,
    cull: &Cull,
    headings: &mut Headings,
    out: &mut AgentInstances,
) {
    let h = cull.height;
    if h > PEDESTRIAN_MAX_HEIGHT {
        headings.list.clear();
        return;
    }
    let detail = usize::from(h > PEDESTRIAN_DETAIL_HEIGHT);
    let a = cull.alpha.clamp(0.0, 1.0);
    let Headings { list, next } = headings;
    next.clear();
    let mut j = 0;
    zip_by_key(
        prev,
        cur,
        |p| p.key,
        |c, p| {
            let p = p.filter(|p| p.pos.distance_squared(c.pos) < TELEPORT * TELEPORT);
            let hash = mix(c.key);
            while j < list.len() && list[j].0 < c.key {
                j += 1;
            }
            let last = list.get(j).filter(|(k, ..)| *k == c.key);
            // Across the sidewalk: square to the way the sim moved it, else as last frame.
            let step = p.map_or(bevy_math::Vec2::ZERO, |p| c.pos - p.pos);
            let side = if step.length_squared() > 1e-4 {
                let d = step.normalize();
                bevy_math::Vec2::new(-d.y, d.x)
            } else {
                last.map_or(bevy_math::Vec2::X, |l| l.2)
            };
            let fwd = bevy_math::Vec2::new(side.y, -side.x);
            let drawn =
                |q: &PedestrianPose| pedestrian_drawn(q, side, q.pos.dot(fwd), &clip, &door);
            let now = drawn(c);
            let (pos, dir) = match p {
                Some(p) => {
                    let before = drawn(p);
                    (before.lerp(now, a), now - before)
                }
                None => (now, bevy_math::Vec2::ZERO),
            };
            let moving = dir.length_squared() > 1e-4;
            let yaw = if moving {
                dir.x.atan2(dir.y)
            } else {
                last.map_or(unit(hash, 32) * std::f32::consts::TAU, |l| l.1)
            };
            next.push((c.key, yaw, side));
            // Culled where the sim has it, with room for the way to a door.
            let y = ground(c.pos.x, c.pos.y);
            if !cull.sees(
                Vec3::new(c.pos.x, y + 0.9, c.pos.y),
                PEDESTRIAN_RADIUS + PORTAL_REACH,
            ) {
                return;
            }
            let y = ground(pos.x, pos.y);
            let col = SHIRT_COLORS[(hash % SHIRT_COLORS.len() as u64) as usize];
            // Stateless odometer: the position projected on the heading grows by exactly the
            // distance walked, so the swing follows the interpolated motion (BUG-104).
            let along = if moving {
                let d = dir.normalize();
                pos.x * d.x + pos.y * d.y
            } else {
                0.0
            };
            let phase = (along / STRIDE + unit(hash, 16)) * std::f32::consts::TAU;
            let height = 1.65 + 0.2 * unit(hash, 48);
            if c.bike {
                // T-322: a rider on a bicycle; no stride, the wheels carry it.
                out.bikes[detail].push(InstanceData {
                    pos_yaw: [pos.x, y, pos.y, yaw],
                    scale_phase: [1.0, height / 1.8, 1.0, 0.0],
                    color_sway: [col[0], col[1], col[2], 0.0],
                });
                return;
            }
            out.pedestrians[detail].push(InstanceData {
                pos_yaw: [pos.x, y, pos.y, yaw],
                scale_phase: [1.0, height / 1.8, 1.0, phase],
                color_sway: [col[0], col[1], col[2], if moving { 0.06 } else { 0.0 }],
            });
        },
    );
    std::mem::swap(list, next);
}
