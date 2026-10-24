//! Pollution, noise and land value on a 64 m grid over the whole map
//! (`architecture/system-design.md` §11). Everything is recomputed from scratch every 256
//! ticks by [`update_city`](crate::update_city) and read from the saved grids in between, so
//! a save taken mid-cycle resumes with the very values the continuous run is using (the
//! grids are part of the save and of the checksum).
//!
//! All three are `0..=100`:
//! * **Pollution** and **noise**: the means of the 16 m [`Emissions`] grids under each cell
//!   (pollution from the ground, so it stays after a source is gone; T-300, §11.1).
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
use cp_utility::{UtilityGraph, UtilityNet};
use serde::{Deserialize, Serialize};

use crate::emissions::Emissions;

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
    /// The land a garbage facility took off a cell (T-290): already out of `land_value`, kept
    /// apart so that only homes feel it ([`LandGrids::value_for`]). Saved with the others.
    pub garbage: Vec<f32>,
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
            && self.garbage == other.garbage
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
            garbage: vec![0.0; SIDE * SIDE],
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
    /// The 16 m pollution and noise, already stepped for this pass.
    pub emissions: &'a Emissions,
    /// Service coverage and problems (T-146); `None` = no services in the land value.
    pub services: Option<(&'a ServiceCoverage, &'a ServiceState)>,
    /// The park areas and their cells (T-225); `None` = no park land value.
    pub parks: Option<(&'a cp_district::Areas, &'a cp_district::AreaMap)>,
}

/// The kinds whose coverage raises land value and housing levels, in `land_service` order.
pub const SERVICE_KINDS: [ServiceKind; 4] = [
    ServiceKind::Health,
    ServiceKind::Police,
    ServiceKind::Fire,
    ServiceKind::Education,
];

/// Whether `def` is a garbage facility: a service building of the Garbage kind.
pub fn is_garbage_facility(db: &ContentDb, def: &cp_content::BuildingDef) -> bool {
    def.service
        .as_deref()
        .and_then(|s| db.service_id(s))
        .is_some_and(|s| db.service(s).kind == ServiceKind::Garbage)
}

/// Half the longer side of a building's lot, metres: the nuisance counts from its footprint.
fn footprint_half_m(def: &cp_content::BuildingDef) -> f32 {
    f32::from(def.lot.0.max(def.lot.1)) * cp_building::FOOTPRINT_CELL_M * 0.5
}

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

    /// What the garbage facilities near `p` take off its land (T-290).
    pub fn garbage_at(&self, p: Vec2) -> f32 {
        self.garbage.get(Self::index(p)).copied().unwrap_or(0.0)
    }

    /// The land value a building of `zone` at `p` sees: homes feel the nuisance of a garbage
    /// facility near them, industry, shops and offices do not (T-290).
    pub fn value_for(&self, zone: ZoneType, p: Vec2) -> f32 {
        let v = self.value_at(p);
        if matches!(zone, ZoneType::ResLow | ZoneType::ResHigh) {
            v
        } else {
            v + self.garbage_at(p)
        }
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
            garbage: vec![0.0; SIDE * SIDE],
            revision: next_revision(),
        };

        // Garbage facilities (T-290): within `garbage_nuisance_m` of the footprint, falling to 0.
        // Buildings are visited in arena order, so the sums come out the same every run.
        if bal.garbage_nuisance_m > 0.0 && bal.land_nuisance_garbage > 0.0 {
            let reach = bal.garbage_nuisance_m;
            for (_, b) in i.buildings.iter() {
                if b.state != BuildingState::Active {
                    continue;
                }
                let def = i.content.building(b.def);
                if !is_garbage_facility(i.content, def) {
                    continue;
                }
                let half = footprint_half_m(def);
                cells_near(b.pos, reach + half, |c, d| {
                    let away = (d - half).max(0.0);
                    g.garbage[c] += bal.land_nuisance_garbage * (1.0 - away / reach)
                });
            }
        }

        let (pollution, noise) = i.emissions.downsample(SIDE);
        g.pollution = pollution;
        g.noise = noise;

        // Specialised buildings (T-196) add land value around themselves (their noise is in
        // `Emissions`).
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
            if spec.land_value_add > 0.0 {
                cells_near(b.pos, SPEC_LAND_RADIUS_M, |c, d| {
                    spec_land[c] += spec.land_value_add * (1.0 - d / SPEC_LAND_RADIUS_M)
                });
            }
        }

        let mut road_dist = vec![f32::INFINITY; SIDE * SIDE];
        let mut near_seg = vec![u32::MAX; SIDE * SIDE];
        for (id, seg) in i.net.segments() {
            for p in samples(&seg.curve, seg.length) {
                cells_near(p, ACCESS_REACH_M, |c, d| {
                    if d < road_dist[c] {
                        road_dist[c] = d;
                        near_seg[c] = id.index;
                    }
                });
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
                if p.has(ServiceProblems::MAIL) {
                    service[cell] -= bal.land_mail;
                    // Shops and offices feel it most (T-247).
                    if matches!(
                        i.content.building(b.def).zone,
                        ZoneType::ComLow | ZoneType::ComHigh | ZoneType::Office
                    ) {
                        service[cell] -= bal.land_mail_work;
                    }
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

        // The cells of a park area lift the land around them by the area's level, falling off
        // to `park_land_radius_m`; where areas overlap the higher value counts (T-225).
        let mut park = vec![0.0f32; SIDE * SIDE];
        if let Some((areas, area_map)) = i.parks
            && bal.park_land_radius_m > 0.0
        {
            let r = bal.park_land_radius_m;
            for (id, a) in areas.iter() {
                if !matches!(a.kind, cp_district::AreaKind::Park(_)) {
                    continue;
                }
                let add = bal.park_land[usize::from(a.level.clamp(1, 5)) - 1];
                if add <= 0.0 {
                    continue;
                }
                for (cx, cz) in area_map.cells_of(id) {
                    let p = Vec2::new(
                        (f32::from(cx) + 0.5) * cp_district::CELL_M,
                        (f32::from(cz) + 0.5) * cp_district::CELL_M,
                    );
                    cells_near(p, r, |c, d| park[c] = park[c].max(add * (1.0 - d / r)));
                }
            }
        }

        // Street trees (T-323, §24.1): the largest bonus among the rows near a cell, not the sum.
        let mut tree = vec![0.0f32; SIDE * SIDE];
        let r = bal.tree_land_radius_m;
        if r > 0.0 && bal.tree_land > 0.0 {
            for (_, seg) in i.net.segments().filter(|(_, s)| s.trees) {
                for p in samples(&seg.curve, seg.length) {
                    cells_near(p, r, |c, d| {
                        tree[c] = tree[c].max(bal.tree_land * (1.0 - d / r))
                    });
                }
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
                + park[c]
                + tree[c]
                + spec_land[c];
            // The garbage nuisance (T-290) is taken off the clamped value, and what it took is
            // kept, so that adding it back gives exactly the land without the facility.
            let without = v.clamp(0.0, 100.0);
            let with = (v - g.garbage[c]).clamp(0.0, 100.0);
            g.land_value[c] = with;
            g.garbage[c] = without - with;
        }
        g
    }
}
