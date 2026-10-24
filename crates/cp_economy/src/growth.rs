//! The 256-tick city pass (`architecture/system-design.md` §8, §11): refreshes the land grids,
//! moves buildings up and down a level, and runs the abandonment timers.
//!
//! One system, [`update_city`], does everything on ticks with `tick % 256 == 0` (not tick 0),
//! visiting buildings in id order. Between passes the only state is what is saved: the
//! [`LandGrids`](crate::land::LandGrids), each building's `level_progress` and the
//! [`Decay`] timers, so a load mid-cycle changes nothing.
//!
//! **Levels.** Each pass an `Active` building of a zone with levels computes the level its
//! surroundings allow, and `level_progress` (signed) moves by `1/level_checks` towards it:
//! * residential 1–5 and commercial 1–3: the highest level whose land value threshold the
//!   land value of the cell it stands in reaches (`level_res_land`, `level_com_land`); a
//!   residential or commercial building without water, power or sewage does not rise;
//! * industrial and office 1–3: the highest level whose threshold the average education of
//!   its workers reaches (`level_ind_edu`, `level_office_edu`), if it has any;
//! * at `+1` the building is rebuilt one level up, at `-1` one level down
//!   ([`cp_building::rebuild`], same lot, see there); when there is no definition or room to
//!   do it the progress is reset and the building stays.
//!
//! **Decay.** A building with any problem flag (`NO_WATER`, `NO_POWER`, `NO_SEWAGE`, `NO_ROAD`,
//! `NO_PATH`, `NO_WORKERS`, `NO_CUSTOMERS`, `NO_GOODS`) gets a [`Decay`] entry with the tick
//! the pass first saw it. After `abandon_days` of unbroken trouble it becomes `Abandoned`
//! (its households and workers leave on the citizens' next update), after `demolish_days` it
//! is demolished through [`cp_building::demolish_building`] (the removed-ids list then purges
//! the households, jobs, trips and agents that pointed at it). If the problems go away first,
//! the entry is dropped and an abandoned building is active again. Only the flags in the
//! balance `decay_mask` count (`NO_WORKERS` is off by default).
//!
//! **Workers.** An `Active` workplace with fewer filled jobs than `no_workers_ratio` of its
//! slots for `no_workers_days` gets `NO_WORKERS` (timer in [`WorkerShortage`]); more workers
//! clear it.

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use cp_building::{
    BuildingFlags, BuildingGrid, BuildingId, BuildingState, demolish_building, rebuild,
    rebuild_candidate,
};
use cp_citizen::CitizenGrid;
use cp_content::{Balance, ContentDb, SimContent, ZoneType};
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_net::NetGraph;
use cp_service::{ServiceCoverage, ServiceState};
use cp_traffic::Agents;
use cp_utility::UtilityGraph;
use cp_zone::ZoneGrid;
use serde::{Deserialize, Serialize};

use crate::land::{LandGrids, LandInputs, SERVICE_KINDS};

/// Ticks between two passes.
pub const CITY_PERIOD: u64 = 256;

/// The abandonment timers: for each building in trouble, the tick of the pass that first saw
/// the trouble. Serialized sim state.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decay {
    /// Scenario/test hook: while false the pass starts no timers and abandons or demolishes
    /// nothing (levels still change).
    pub enabled: bool,
    pub since: BTreeMap<BuildingId, Tick>,
}

impl Default for Decay {
    fn default() -> Self {
        Self {
            enabled: true,
            since: BTreeMap::new(),
        }
    }
}

impl Decay {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            since: BTreeMap::new(),
        }
    }
}

/// When each Active workplace first had too few workers (`no_workers_ratio`), for the
/// `NO_WORKERS` flag (T-145). Serialized sim state (saved since v23).
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerShortage {
    pub since: BTreeMap<BuildingId, Tick>,
}

/// The level `value` earns against ascending `thresholds` (level 1 is always earned).
pub fn level_for(thresholds: &[f32], value: f32) -> u8 {
    let n = thresholds.iter().filter(|&&t| value >= t).count().max(1);
    n as u8
}

/// The highest level up to `wanted` a home may stand at with `covered` of Health, Police, Fire
/// and Education covered (`res_level_services`, T-146).
pub fn service_cap(bal: &Balance, wanted: u8, covered: u8) -> u8 {
    let mut level = wanted.clamp(1, 5);
    while level > 1 && bal.res_level_services[usize::from(level) - 1] > covered {
        level -= 1;
    }
    level
}

/// The level the surroundings of a building allow, or `None` if it has no say (an unstaffed
/// industrial or office building keeps its level).
fn wanted_level(
    bal: &Balance,
    zone: ZoneType,
    land: f32,
    workers: Option<(u32, u32)>,
) -> Option<u8> {
    match zone {
        ZoneType::ResLow | ZoneType::ResHigh => Some(level_for(&bal.level_res_land, land)),
        ZoneType::ComLow | ZoneType::ComHigh => Some(level_for(&bal.level_com_land, land)),
        ZoneType::Industrial | ZoneType::Office => {
            let (sum, n) = workers.filter(|&(_, n)| n > 0)?;
            let avg = sum as f32 / n as f32;
            let t = if zone == ZoneType::Industrial {
                &bal.level_ind_edu
            } else {
                &bal.level_office_edu
            };
            Some(level_for(t, avg))
        }
        ZoneType::None => None,
    }
}

/// `(education sum, workers)` per workplace, dense by arena index with the generation of the
/// workplace (a stale slot reads as nobody works there).
struct Workers(Vec<Option<(u32, u32, u32)>>);

impl Workers {
    fn get(&self, id: BuildingId) -> Option<(u32, u32)> {
        match self.0.get(id.index as usize) {
            Some(Some((g, e, n))) if *g == id.generation => Some((*e, *n)),
            _ => None,
        }
    }
}

fn worker_education(citizens: &CitizenGrid) -> Workers {
    let mut m: Vec<Option<(u32, u32, u32)>> = Vec::new();
    for (_, c) in citizens.citizens() {
        if let Some(w) = c.work {
            let i = w.index as usize;
            if m.len() <= i {
                m.resize(i + 1, None);
            }
            let e = match &mut m[i] {
                Some(e) if e.0 == w.generation => e,
                slot => slot.insert((w.generation, 0, 0)),
            };
            e.1 += u32::from(c.education);
            e.2 += 1;
        }
    }
    Workers(m)
}

/// Moves `level_progress` of building `id` and rebuilds it when it reaches a whole step.
#[allow(clippy::too_many_arguments)]
fn step_level(
    db: &ContentDb,
    buildings: &mut BuildingGrid,
    zones: &mut ZoneGrid,
    net: &NetGraph,
    id: BuildingId,
    wanted: u8,
    may_rise: bool,
) {
    let step = 1.0 / db.balance().level_checks.max(1) as f32;
    let Some(b) = buildings.get(id) else { return };
    let toward = match wanted.cmp(&b.level) {
        std::cmp::Ordering::Greater if may_rise => 1.0,
        std::cmp::Ordering::Less => -1.0,
        _ => 0.0,
    };
    let old = b.level_progress;
    // Towards the wanted direction by one step; with nothing to do, back towards zero.
    let mut p = if toward == 0.0 {
        if old.abs() <= step {
            0.0
        } else {
            old - step * old.signum()
        }
    } else {
        (old + toward * step).clamp(-1.0, 1.0)
    };
    if p.abs() >= 1.0 - 1e-6 {
        let up = p > 0.0;
        let b = buildings.get(id).expect("checked");
        match rebuild_candidate(db, zones, b, up) {
            Some((def, lot)) => {
                rebuild(buildings, zones, net, db, id, def, lot);
                return;
            }
            None => p = 0.0,
        }
    }
    if p != old
        && let Some(b) = buildings.get_mut(id)
    {
        b.level_progress = p;
    }
}

/// `Economy`-phase system, see the module docs.
#[allow(clippy::too_many_arguments)]
pub fn update_city(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    utilities: Res<UtilityGraph>,
    agents: Res<Agents>,
    citizens: Res<CitizenGrid>,
    mut buildings: ResMut<BuildingGrid>,
    mut zones: ResMut<ZoneGrid>,
    (mut grids, mut emissions, water): (
        ResMut<LandGrids>,
        ResMut<crate::Emissions>,
        Option<Res<cp_utility::WaterPools>>,
    ),
    (mut decay, corruption): (ResMut<Decay>, Option<Res<crate::CorruptionState>>),
    mut shortage: ResMut<WorkerShortage>,
    coverage: Option<Res<ServiceCoverage>>,
    services: Option<Res<ServiceState>>,
    areas: Option<Res<cp_district::Areas>>,
    area_map: Option<Res<cp_district::AreaMap>>,
    policies: cp_district::PolicyParams,
) {
    if tick.0 == 0 || !tick.0.is_multiple_of(CITY_PERIOD) {
        return;
    }
    let Some(content) = content else { return };
    let db = &*content.0;
    let bal = db.balance();
    // Pollution and noise first (T-300): the land value reads them.
    let boost = corruption
        .as_deref()
        .map_or_else(Vec::new, |c| c.pollution_boost(db));
    emissions.update(&crate::EmissionInputs {
        content: db,
        buildings: &buildings,
        net: &net,
        load: &agents.load,
        boost: &boost,
    });
    // The dirt of the water each home drinks (T-300 step 2): the supply pass of this tick (every
    // 64 ticks, Utilities before Economy) has just rewritten the pools.
    if let Some(water) = water.as_deref() {
        emissions.update_water(water, &buildings);
    }
    *grids = LandGrids::compute(&LandInputs {
        content: db,
        buildings: &buildings,
        net: &net,
        utilities: &utilities,
        emissions: &emissions,
        services: coverage.as_deref().zip(services.as_deref()),
        parks: areas.as_deref().zip(area_map.as_deref()),
        factor: corruption
            .as_deref()
            .map_or(1.0, |c| c.land_factor(&bal.corruption)),
    });

    let workers = worker_education(&citizens);
    let ids: Vec<BuildingId> = buildings.iter().map(|(id, _)| id).collect();
    decay.since.retain(|id, _| buildings.get(*id).is_some());
    shortage.since.retain(|id, _| buildings.get(*id).is_some());
    // NO_GOODS counts only when the balance switch is on (T-169).
    let mask = if bal.goods_decay {
        bal.decay_mask
    } else {
        bal.decay_mask & !BuildingFlags::NO_GOODS
    };
    let short_after = (bal.no_workers_days * TICKS_PER_DAY as f32) as u64;
    let decaying = decay.enabled;
    let abandon = (bal.abandon_days * TICKS_PER_DAY as f32) as u64;
    let demolish = (bal.demolish_days * TICKS_PER_DAY as f32) as u64;
    let now = tick.0;
    for id in ids {
        let Some(b) = buildings.get(id) else { continue };
        let (state, pos) = (b.state, b.pos);
        if !matches!(state, BuildingState::Active | BuildingState::Abandoned) {
            decay.since.remove(&id);
            shortage.since.remove(&id);
            continue;
        }
        if state == BuildingState::Active {
            let def = db.building(b.def);
            let slots: u32 = def.jobs.iter().map(|&j| u32::from(j)).sum();
            let filled = workers.get(id).map_or(0, |(_, n)| n);
            let short = def.service.is_none()
                && slots > 0
                && (filled as f32) < bal.no_workers_ratio * slots as f32;
            if short {
                let since = shortage.since.entry(id).or_insert(Tick(now)).0;
                if now - since >= short_after {
                    buildings.set_flag(id, BuildingFlags::NO_WORKERS, true);
                }
            } else {
                shortage.since.remove(&id);
                buildings.set_flag(id, BuildingFlags::NO_WORKERS, false);
            }
        }
        let flags = buildings.get(id).expect("checked").flags;
        let service_trouble = services
            .as_ref()
            .is_some_and(|s| s.problems_of(id).0 & bal.decay_service_mask != 0);
        let problem = decaying && (flags.0 & mask != 0 || service_trouble);
        if !decaying {
            // Hook off: nothing is timed, abandoned or demolished.
        } else if !problem {
            decay.since.remove(&id);
            if state == BuildingState::Abandoned
                && let Some(b) = buildings.get_mut(id)
            {
                b.state = BuildingState::Active;
            }
        } else {
            // An abandoned building nobody timed (a scene, an old save) is already a week in.
            let first = if state == BuildingState::Abandoned {
                now.saturating_sub(abandon)
            } else {
                now
            };
            let since = decay.since.entry(id).or_insert(Tick(first)).0;
            let age = now - since;
            if age >= demolish {
                demolish_building(&mut buildings, &mut zones, id);
                decay.since.remove(&id);
                continue;
            }
            if age >= abandon && state == BuildingState::Active {
                if let Some(b) = buildings.get_mut(id) {
                    b.state = BuildingState::Abandoned;
                }
                continue;
            }
        }

        let still = buildings.get(id).map(|b| b.state);
        if still != Some(BuildingState::Active) {
            continue;
        }
        let def = db.building(buildings.get(id).expect("active").def);
        // Services and specialised buildings (T-196) keep their level.
        if def.service.is_some() || def.spec.is_some() {
            continue;
        }
        let utility_trouble = flags.0 & BuildingFlags::UTILITY_MASK != 0;
        if let Some(mut wanted) = wanted_level(
            bal,
            def.zone,
            grids.value_for(def.zone, pos),
            workers.get(id),
        ) {
            // Homes also need services in reach for the higher levels (T-146).
            if matches!(def.zone, ZoneType::ResLow | ZoneType::ResHigh)
                && let Some(cov) = coverage.as_deref()
            {
                let b = buildings.get(id).expect("active");
                let covered = SERVICE_KINDS
                    .into_iter()
                    .filter(|&k| cov.coverage_of(k, b) >= bal.res_service_cover)
                    .count() as u8;
                wanted = service_cap(bal, wanted, covered);
            }
            let mut may_rise = !utility_trouble;
            // HighRiseBan (T-194): tall zones stop rising at level `value`; a building already
            // above it stays as it is.
            if matches!(
                def.zone,
                ZoneType::ResHigh | ZoneType::ComHigh | ZoneType::Office
            ) && let Some(p) = policies.get()
                && let Some(ban) = db.policy(cp_content::PolicyKind::HighRiseBan)
                && p.at(pos, cp_content::PolicyKind::HighRiseBan)
                && f32::from(buildings.get(id).expect("active").level) >= ban.value
            {
                may_rise = false;
            }
            step_level(db, &mut buildings, &mut zones, &net, id, wanted, may_rise);
        }
    }
}
