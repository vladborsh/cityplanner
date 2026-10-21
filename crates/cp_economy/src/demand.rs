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
//! * `C = citizens × shop jobs per citizen − commercial job slots + counted unemployed × job
//!   weight + base_C` (BUG-076)
//! * `I = commercial job slots × industry per shop job − industrial job slots + unemployed
//!   adults of education ≤ 1 + counted unemployed beyond the manual ones × job weight + base_I
//!   plus the industrial jobs that would make the goods of the shops out of goods` (BUG-095:
//!   the job slots of the `NO_GOODS` shops × `goods_per_job / goods_per_worker`, counted over
//!   the whole grid when the cycle is published, not in the sums: no save change)
//! * move-in `= Σ weight(level) × free job slots − unemployed adults + base_R`: `R` without the
//!   vacant homes, the gate of households moving into existing homes
//! * `O = unemployed adults of education ≥ 2 − free office job slots + base_O`
//!
//! A free office slot is one nobody holds; the formula's "office jobs" is read as the vacant
//! ones.

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingFlags, BuildingGrid, BuildingState, DEMAND_PERIOD, Demand};
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
        // BUG-076: the jobless ask for jobs. Shops and industry jobs per citizen alone cover
        // ~0.3 jobs against ~0.7 adults, so without this R (negative: unemployed) and C / I
        // (their ratios met) all stood empty and the city stalled. The educated share beyond
        // the manual workers has no I term of its own, so it counts here too.
        let jobs_wanted = counted * b.demand_unemployed_job_weight;
        let educated_counted = (counted - f(self.unemployed_manual)).max(0.0);
        [
            clamp(
                self.free_jobs_weighted - counted - f(self.vacant_homes) * b.demand_adults_per_home
                    + b.demand_base[0],
            ),
            clamp(
                f(self.citizens) * b.demand_shop_jobs_per_citizen - f(self.commercial_jobs)
                    + jobs_wanted
                    + b.demand_base[1],
            ),
            clamp(
                f(self.commercial_jobs) * b.demand_industry_jobs_per_shop_job
                    - f(self.industrial_jobs)
                    + f(self.unemployed_manual) * b.demand_unemployed_industry_weight
                    + educated_counted * b.demand_unemployed_job_weight
                    + b.demand_base[2],
            ),
            clamp(f(self.unemployed_educated) - f(self.free_office_jobs) + b.demand_base[3]),
            clamp(self.free_jobs_weighted - counted + b.demand_base[0]),
        ]
    }
}

/// The industrial jobs the shops out of goods ask for (BUG-095): their job slots × the goods a
/// shop job sells ÷ the goods an industrial job makes. Active shops with `NO_GOODS`, id order.
pub fn shortage_jobs(buildings: &BuildingGrid, db: &cp_content::ContentDb, b: &Balance) -> f32 {
    if b.goods_per_worker <= 0.0 {
        return 0.0;
    }
    let slots: u32 = buildings
        .iter()
        .filter(|(_, bl)| {
            bl.state == BuildingState::Active && bl.flags.0 & BuildingFlags::NO_GOODS != 0
        })
        .map(|(_, bl)| db.building(bl.def))
        .filter(|d| matches!(d.zone, ZoneType::ComLow | ZoneType::ComHigh))
        .map(|d| d.jobs.iter().map(|j| u32::from(*j)).sum::<u32>())
        .sum();
    slots as f32 * b.goods_per_job / b.goods_per_worker
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
        // A service, park or landmark stands on an `Industrial` def (BUG-095): its jobs are no
        // industry and no shop, only free jobs above. The industry chains' buildings stay
        // industry.
        let grown = cp_building::is_growable(def) || def.industry.is_some();
        match def.zone {
            ZoneType::ComLow | ZoneType::ComHigh if grown => acc.commercial_jobs += slots,
            ZoneType::Industrial if grown => acc.industrial_jobs += slots,
            ZoneType::ComLow | ZoneType::ComHigh | ZoneType::Industrial => {}
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
        let mut raw = acc.raw(bal);
        if let Some(db) = db {
            raw[2] = (raw[2] + shortage_jobs(&buildings, db, bal)).clamp(-LIMIT, LIMIT);
        }
        demand.r = smooth(demand.r, raw[0], bal.demand_ema);
        demand.c = smooth(demand.c, raw[1], bal.demand_ema);
        demand.i = smooth(demand.i, raw[2], bal.demand_ema);
        demand.o = smooth(demand.o, raw[3], bal.demand_ema);
        demand.move_in = smooth(demand.move_in, raw[4], bal.demand_ema);
    }
}

#[cfg(test)]
mod raw_tests {
    use super::*;

    /// BUG-076: 454 residents (~318 adults), shops and industry at their ratios, no free job:
    /// R is empty because of the unemployed, so C and I must ask for the jobs. BUG-099: the
    /// ratios are 0.3 shop jobs per resident and 0.8 industry jobs per shop job (were 0.2, 0.5):
    /// 136 and 109 jobs (were 91, 45); the small town 50 residents, 15 and 12 (were 40, 8, 4).
    #[test]
    fn unemployed_adults_above_the_allowance_ask_for_shop_and_industry_jobs() {
        let b = Balance::default();
        let stalled = DemandAcc {
            seen: 64,
            citizens: 454,
            commercial_jobs: 136,
            industrial_jobs: 109,
            unemployed: 182,
            unemployed_educated: 182,
            ..DemandAcc::default()
        };
        let [r, c, i, _, move_in] = stalled.raw(&b);
        assert!(
            r < 0.0 && move_in < 0.0,
            "no homes are wanted while jobless"
        );
        assert!(c > 20.0, "C {c}");
        assert!(i > 20.0, "I {i}");
        // Below the commuter allowance nobody asks (a small town's commuters).
        let small = DemandAcc {
            citizens: 50,
            commercial_jobs: 15,
            industrial_jobs: 12,
            unemployed: 15,
            ..DemandAcc::default()
        };
        let [_, c, i, _, _] = small.raw(&b);
        assert!((c - b.demand_base[1]).abs() < 1e-3 && (i - b.demand_base[2]).abs() < 1e-3);
    }
}
