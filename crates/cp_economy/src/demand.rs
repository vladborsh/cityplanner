//! Residential, commercial, industrial and office demand (`architecture/system-design.md`
//! §11). The result is [`cp_building::Demand`], which gates spawning and move-in.
//!
//! **Bucketing.** Every tick [`update_demand`] reads the buildings and the citizens of one of
//! 64 buckets (`index % 64 == tick % 64`, id order) into the [`DemandAcc`] sums; after the
//! 64th bucket of a cycle (`tick % 64 == 63`) the raw values are computed from the sums,
//! smoothed (EMA) and published. A cycle that was not scanned from its first bucket (a load
//! from an older save mid-cycle) is dropped. The sums are serialized with the save, so a
//! save in the middle of a cycle resumes exactly.
//!
//! **Raw values** (clamped to ±100; `base[..]`, weights and factors are `Balance` knobs).
//! Buildings under construction count as already standing, so the city does not order
//! housing, shops or jobs again while the first ones are still being built:
//! * unemployed adults count against R and move-in only above the commuter allowance
//!   `max(commuter_min, commuter_share × adults)` (BUG-057); manual unemployed weigh
//!   `demand_unemployed_industry_weight` in I.
//! * `R = Σ weight(level) × free job slots − counted unemployed adults − vacant homes × adults per
//!   home + base_R`
//! * `C = citizens × shop jobs per citizen − commercial job slots + base_C`
//! * `I = commercial job slots × industry per shop job − industrial job slots + unemployed
//!   adults of education ≤ 1 + base_I`
//! * move-in `= Σ weight(level) × free job slots − unemployed adults + base_R`: `R` without the
//!   vacant homes, the gate of households moving into existing homes
//! * `O = unemployed adults of education ≥ 2 − free office job slots + base_O`
//!
//! A free office slot is one nobody holds; the formula's "office jobs" is read as the vacant
//! ones.

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingGrid, BuildingState, DEMAND_PERIOD, Demand};
use cp_citizen::CitizenGrid;
use cp_content::{Balance, SimContent, ZoneType};
use cp_core::Tick;
use serde::{Deserialize, Serialize};

/// Largest magnitude of a raw demand value.
const LIMIT: f32 = 100.0;

/// Estimated share of the citizens that are adults, for the commuter allowance.
const ADULT_SHARE: f32 = 0.7;

/// The sums of the current cycle. Serialized sim state.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DemandAcc {
    /// Buckets scanned in this cycle.
    pub seen: u32,
    pub free_jobs_weighted: f32,
    pub vacant_homes: u32,
    pub commercial_jobs: u32,
    pub industrial_jobs: u32,
    pub free_office_jobs: u32,
    pub citizens: u32,
    pub unemployed: u32,
    /// Unemployed adults of education 0..=1.
    pub unemployed_manual: u32,
    /// Unemployed adults of education 2..=3.
    pub unemployed_educated: u32,
}

impl DemandAcc {
    /// The raw `[R, C, I, O, move-in]` of the sums, before smoothing.
    pub fn raw(&self, b: &Balance) -> [f32; 5] {
        let clamp = |v: f32| v.clamp(-LIMIT, LIMIT);
        let f = |n: u32| n as f32;
        // Commuter allowance (BUG-057): a small town's unemployed work elsewhere. `DemandAcc`
        // has no adult count (it is saved), so the adults are estimated from the citizens.
        let adults = f(self.citizens) * ADULT_SHARE;
        let allowance = b.demand_commuter_min.max(b.demand_commuter_share * adults);
        let counted = (f(self.unemployed) - allowance).max(0.0);
        [
            clamp(
                self.free_jobs_weighted - counted - f(self.vacant_homes) * b.demand_adults_per_home
                    + b.demand_base[0],
            ),
            clamp(
                f(self.citizens) * b.demand_shop_jobs_per_citizen - f(self.commercial_jobs)
                    + b.demand_base[1],
            ),
            clamp(
                f(self.commercial_jobs) * b.demand_industry_jobs_per_shop_job
                    - f(self.industrial_jobs)
                    + f(self.unemployed_manual) * b.demand_unemployed_industry_weight
                    + b.demand_base[2],
            ),
            clamp(f(self.unemployed_educated) - f(self.free_office_jobs) + b.demand_base[3]),
            clamp(self.free_jobs_weighted - counted + b.demand_base[0]),
        ]
    }
}

/// One step of the smoothing: closes `ema` of the gap to `raw`.
pub fn smooth(old: f32, raw: f32, ema: f32) -> f32 {
    old + (raw - old) * ema.clamp(0.0, 1.0)
}

/// `Economy`-phase system: scans bucket `tick % 64` and, at the end of a cycle, publishes the
/// new [`Demand`].
pub fn update_demand(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    citizens: Res<CitizenGrid>,
    mut demand: ResMut<Demand>,
    mut acc: ResMut<DemandAcc>,
) {
    if demand.frozen {
        return;
    }
    // Without content there are no buildings either: an empty city that wants what a new one
    // wants (a sim with and without content then stay in the same state).
    let default_balance = Balance::default();
    let (db, bal) = content
        .as_ref()
        .map_or((None, &default_balance), |c| (Some(&*c.0), c.0.balance()));
    let bucket = (tick.0 % DEMAND_PERIOD) as u32;
    if bucket == 0 {
        *acc = DemandAcc::default();
    }

    for (id, b) in buildings
        .iter_bucket(DEMAND_PERIOD as u32, bucket)
        .filter(|_| db.is_some())
    {
        let Some(db) = db else { continue };
        let active = match b.state {
            BuildingState::Active => true,
            BuildingState::UnderConstruction => false,
            BuildingState::Abandoned | BuildingState::Burned => continue,
        };
        let def = db.building(b.def);
        let mut slots = 0u32;
        let mut free = 0u32;
        for (level, &jobs) in def.jobs.iter().enumerate() {
            let filled = if active {
                citizens.jobs_filled(id, level)
            } else {
                0
            };
            let open = jobs.saturating_sub(filled);
            acc.free_jobs_weighted += bal.demand_job_weight[level] * f32::from(open);
            slots += u32::from(jobs);
            free += u32::from(open);
        }
        match def.zone {
            ZoneType::ComLow | ZoneType::ComHigh => acc.commercial_jobs += slots,
            ZoneType::Industrial => acc.industrial_jobs += slots,
            ZoneType::Office => acc.free_office_jobs += free,
            ZoneType::ResLow | ZoneType::ResHigh => {
                let living = if active {
                    citizens.households_in(id)
                } else {
                    0
                };
                acc.vacant_homes += u32::from(def.households.saturating_sub(living));
            }
            ZoneType::None => {}
        }
    }
    for (_, c) in citizens.citizens_bucket(DEMAND_PERIOD as u32, bucket) {
        acc.citizens += 1;
        if c.is_adult() && c.work.is_none() {
            acc.unemployed += 1;
            if c.education <= 1 {
                acc.unemployed_manual += 1;
            } else {
                acc.unemployed_educated += 1;
            }
        }
    }
    acc.seen += 1;

    if bucket as u64 == DEMAND_PERIOD - 1 && u64::from(acc.seen) == DEMAND_PERIOD {
        let raw = acc.raw(bal);
        demand.r = smooth(demand.r, raw[0], bal.demand_ema);
        demand.c = smooth(demand.c, raw[1], bal.demand_ema);
        demand.i = smooth(demand.i, raw[2], bal.demand_ema);
        demand.o = smooth(demand.o, raw[3], bal.demand_ema);
        demand.move_in = smooth(demand.move_in, raw[4], bal.demand_ema);
    }
}
