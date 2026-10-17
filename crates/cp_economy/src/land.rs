//! Pollution, noise and land value on a 64 m grid over the whole map
//! (`architecture/system-design.md` §11). Everything is recomputed from scratch every 256
//! ticks by [`update_city`](crate::update_city) and read from the saved grids in between, so
//! a save taken mid-cycle resumes with the very values the continuous run is using (the
//! grids are part of the save and of the checksum).
//!
//! All three are `0..=100`:
//! * **Pollution**: every active industrial-zone building adds `pollution_per_level × level`
//!   at its own cell, fading linearly to nothing at `pollution_radius_m`; contributions add
//!   up.
//! * **Noise**: points every 32 m along each road segment carry `noise_per_load × load`
//!   (the 2-hour average load, `Agents::load.ema`), fading linearly to nothing at
//!   `noise_radius_m`; a cell takes the loudest.
//! * **Land value** = `land_base` + up to `land_access` for the nearest road (full within
//!   32 m of the cell centre, none from 96 m) + `land_water` / `land_power` for a pipe / power
//!   line within `land_utility_reach_m` − `land_pollution_weight × pollution` −
//!   `land_noise_weight × noise`, clamped.
//!
//! Cells are visited in index order and sources in arena order, so the floats add up the same
//! way every run.

use std::sync::atomic::{AtomicU64, Ordering};

use bevy_ecs::resource::Resource;
use bevy_math::Vec2;
use cp_building::{BuildingGrid, BuildingState};
use cp_content::{Balance, ContentDb, ServiceKind, ZoneType};
use cp_net::NetGraph;
use cp_service::{ServiceCoverage, ServiceProblems, ServiceState};
use cp_terrain::MAP_SIZE;
use cp_traffic::SegmentLoad;
use cp_utility::{UtilityGraph, UtilityNet};
use serde::{Deserialize, Serialize};

/// Side of a grid cell, metres.
pub const CELL_M: f32 = 64.0;
/// Cells per side.
pub const SIDE: usize = (MAP_SIZE / CELL_M) as usize;
/// Distance between sample points along roads and lines, metres.
const SAMPLE_M: f32 = 32.0;
/// A road this close to a cell centre gives the full access bonus; none from `ACCESS_REACH_M`.
/// Reach of the land value a specialised building adds, metres (T-196).
const SPEC_LAND_RADIUS_M: f32 = 64.0;
const ACCESS_FULL_M: f32 = 32.0;
const ACCESS_REACH_M: f32 = 96.0;

/// The three grids, row-major (`z * SIDE + x`). Serialized sim state.
#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
pub struct LandGrids {
    pub pollution: Vec<f32>,
    pub noise: Vec<f32>,
    pub land_value: Vec<f32>,
    /// Changes whenever the grids are replaced and is never shared between instances, so a
    /// snapshot cache can key on it. Not saved, not compared.
    #[serde(skip, default = "next_revision")]
    revision: u64,
}

static REVISIONS: AtomicU64 = AtomicU64::new(1);

fn next_revision() -> u64 {
    REVISIONS.fetch_add(1, Ordering::Relaxed)
}

impl PartialEq for LandGrids {
    fn eq(&self, other: &Self) -> bool {
        self.pollution == other.pollution
            && self.noise == other.noise
            && self.land_value == other.land_value
    }
}

impl Default for LandGrids {
    /// An empty map: nothing pollutes, nothing is loud, and the land is worth its base value
    /// (as [`LandGrids::compute`] gives an empty city with the default `Balance`).
    fn default() -> Self {
        Self {
            pollution: vec![0.0; SIDE * SIDE],
            noise: vec![0.0; SIDE * SIDE],
            land_value: vec![Balance::default().land_base; SIDE * SIDE],
            revision: next_revision(),
        }
    }
}

/// Everything [`LandGrids::compute`] reads.
pub struct LandInputs<'a> {
    pub content: &'a ContentDb,
    pub buildings: &'a BuildingGrid,
    pub net: &'a NetGraph,
    pub utilities: &'a UtilityGraph,
    pub load: &'a SegmentLoad,
    /// Service coverage and problems (T-146); `None` = no services in the land value.
    pub services: Option<(&'a ServiceCoverage, &'a ServiceState)>,
}

/// The kinds whose coverage raises land value and housing levels, in `land_service` order.
pub const SERVICE_KINDS: [ServiceKind; 4] = [
    ServiceKind::Health,
    ServiceKind::Police,
    ServiceKind::Fire,
    ServiceKind::Education,
];

fn centre(i: usize) -> Vec2 {
    Vec2::new(
        ((i % SIDE) as f32 + 0.5) * CELL_M,
        ((i / SIDE) as f32 + 0.5) * CELL_M,
    )
}

/// Calls `f(cell index, distance from the cell centre to p)` for every cell whose centre is
/// within `radius` of `p`, in index order.
fn cells_near(p: Vec2, radius: f32, mut f: impl FnMut(usize, f32)) {
    let lo = |v: f32| (((v - radius) / CELL_M).floor().max(0.0)) as usize;
    let hi = |v: f32| (((v + radius) / CELL_M).floor().max(0.0) as usize).min(SIDE - 1);
    for z in lo(p.y)..=hi(p.y) {
        for x in lo(p.x)..=hi(p.x) {
            let i = z * SIDE + x;
            let d = centre(i).distance(p);
            if d <= radius {
                f(i, d);
            }
        }
    }
}

/// Points about every 32 m along `curve` (both ends included).
fn samples(curve: &cp_core::curve::CubicBezier3, length: f32) -> impl Iterator<Item = Vec2> + '_ {
    let n = (length / SAMPLE_M).ceil().max(1.0) as usize;
    (0..=n).map(move |k| {
        let p = curve.point(k as f32 / n as f32);
        Vec2::new(p.x, p.z)
    })
}

impl LandGrids {
    /// See the field of the same name.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Land value at a world position (the cell it is in; positions off the map clamp to the
    /// edge).
    pub fn value_at(&self, p: Vec2) -> f32 {
        self.land_value[Self::index(p)]
    }

    /// [`Self::value_at`], named for tests in other crates.
    pub fn land_value_for_test(&self, p: Vec2) -> f32 {
        self.value_at(p)
    }

    pub fn pollution_at(&self, p: Vec2) -> f32 {
        self.pollution[Self::index(p)]
    }

    pub fn noise_at(&self, p: Vec2) -> f32 {
        self.noise[Self::index(p)]
    }

    fn index(p: Vec2) -> usize {
        let c = |v: f32| ((v / CELL_M).floor().max(0.0) as usize).min(SIDE - 1);
        c(p.y) * SIDE + c(p.x)
    }

    /// The grids of the city as it stands.
    pub fn compute(i: &LandInputs) -> Self {
        let bal = i.content.balance();
        let mut g = Self {
            pollution: vec![0.0; SIDE * SIDE],
            noise: vec![0.0; SIDE * SIDE],
            land_value: vec![0.0; SIDE * SIDE],
            revision: next_revision(),
        };

        for (_, b) in i.buildings.iter() {
            let def = i.content.building(b.def);
            // A unique building is filed under Industrial for want of a better zone; it does not pollute.
            if b.state != BuildingState::Active
                || def.zone != ZoneType::Industrial
                || def.unique.is_some()
            {
                continue;
            }
            let strength = bal.pollution_per_level * f32::from(def.level);
            let r = bal.pollution_radius_m;
            cells_near(b.pos, r, |c, d| g.pollution[c] += strength * (1.0 - d / r));
        }

        // Specialised buildings (T-196) make noise and add land value around themselves.
        let mut spec_land = vec![0.0f32; SIDE * SIDE];
        for (_, b) in i.buildings.iter() {
            if b.state != BuildingState::Active {
                continue;
            }
            let Some(spec) = i
                .content
                .building(b.def)
                .spec
                .and_then(|k| i.content.spec(k))
            else {
                continue;
            };
            if spec.noise_add > 0.0 {
                let r = bal.noise_radius_m;
                cells_near(b.pos, r, |c, d| {
                    g.noise[c] = g.noise[c].max(spec.noise_add * (1.0 - d / r))
                });
            }
            if spec.land_value_add > 0.0 {
                cells_near(b.pos, SPEC_LAND_RADIUS_M, |c, d| {
                    spec_land[c] += spec.land_value_add * (1.0 - d / SPEC_LAND_RADIUS_M)
                });
            }
        }

        let mut road_dist = vec![f32::INFINITY; SIDE * SIDE];
        let mut near_seg = vec![u32::MAX; SIDE * SIDE];
        let r = bal.noise_radius_m;
        for (id, seg) in i.net.segments() {
            let loud =
                bal.noise_per_load * i.load.ema.get(id.index as usize).copied().unwrap_or(0.0);
            for p in samples(&seg.curve, seg.length) {
                cells_near(p, ACCESS_REACH_M, |c, d| {
                    if d < road_dist[c] {
                        road_dist[c] = d;
                        near_seg[c] = id.index;
                    }
                });
                if loud > 0.0 {
                    cells_near(p, r, |c, d| {
                        g.noise[c] = g.noise[c].max(loud * (1.0 - d / r))
                    });
                }
            }
        }

        let mut water = vec![false; SIDE * SIDE];
        let mut power = vec![false; SIDE * SIDE];
        for (_, link) in i.utilities.links() {
            let near = match link.net {
                UtilityNet::Pipe => &mut water,
                UtilityNet::Power => &mut power,
            };
            for p in samples(&link.curve, link.length) {
                cells_near(p, bal.land_utility_reach_m, |c, _| near[c] = true);
            }
        }

        // Services (T-146): coverage on the nearest road of the cell, and the garbage and crime
        // of the buildings standing in it.
        let mut service = vec![0.0f32; SIDE * SIDE];
        if let Some((coverage, state)) = i.services {
            for c in 0..SIDE * SIDE {
                if near_seg[c] == u32::MAX {
                    continue;
                }
                for (k, kind) in SERVICE_KINDS.into_iter().enumerate() {
                    service[c] +=
                        bal.land_service[k] * coverage.on_segment_index(kind, near_seg[c]);
                }
            }
            for (id, b) in i.buildings.iter() {
                let p = state.problems_of(id);
                let cell = Self::index(b.pos);
                if p.has(ServiceProblems::GARBAGE) {
                    service[cell] -= bal.land_garbage;
                }
                if p.has(ServiceProblems::CRIME) {
                    service[cell] -= bal.land_crime;
                }
            }
        }

        // Buildings that lift the land around them (T-200), falling off linearly to the radius.
        let mut effect = vec![0.0f32; SIDE * SIDE];
        for (_, b) in i.buildings.iter() {
            if b.state != BuildingState::Active {
                continue;
            }
            if let Some(e) = i.content.building(b.def).land_effect
                && e.radius_m > 0.0
            {
                cells_near(b.pos, e.radius_m, |c, d| {
                    effect[c] += e.add * (1.0 - d / e.radius_m)
                });
            }
        }

        for c in 0..SIDE * SIDE {
            g.pollution[c] = g.pollution[c].min(100.0);
            g.noise[c] = g.noise[c].min(100.0);
            let access = if road_dist[c].is_finite() {
                bal.land_access
                    * ((ACCESS_REACH_M - road_dist[c]) / (ACCESS_REACH_M - ACCESS_FULL_M))
                        .clamp(0.0, 1.0)
            } else {
                0.0
            };
            let v = bal.land_base
                + access
                + if water[c] { bal.land_water } else { 0.0 }
                + if power[c] { bal.land_power } else { 0.0 }
                - bal.land_pollution_weight * g.pollution[c]
                - bal.land_noise_weight * g.noise[c]
                + service[c]
                + effect[c]
                + spec_land[c];
            g.land_value[c] = v.clamp(0.0, 100.0);
        }
        g
    }
}
