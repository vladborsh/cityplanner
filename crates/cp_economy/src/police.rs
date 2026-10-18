//! Crime (§16 M2 D "Полиция"): residential and commercial buildings pile it up, more on cheap
//! land, police stations within reach take it away by their daily capacity, and a building
//! that holds too much has the CRIME problem. Same pacing as garbage: every tick the buildings
//! of bucket `index % 64 == tick % 64` get the share of a game day that passed.

use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingGrid, BuildingState};
use cp_content::{ServiceKind, ServiceUnlocks, SimContent, ZoneType};
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_service::{COVERAGE_PERIOD, FacilityFinder, ServiceCoverage, ServiceProblems, ServiceState};

use crate::land::LandGrids;

const PASSES_PER_DAY: u64 = TICKS_PER_DAY / COVERAGE_PERIOD;

/// Economy phase, after `update_city` and the service coverage rebuild.
#[allow(clippy::too_many_arguments)]
pub fn update_police(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: cp_service::ServiceNet,
    buildings: Res<BuildingGrid>,
    land: Res<LandGrids>,
    coverage: Res<ServiceCoverage>,
    mut state: ResMut<ServiceState>,
    policies: cp_district::PolicyParams,
    unlocks: Option<Res<ServiceUnlocks>>,
) {
    let Some(content) = content else { return };
    // Worlds built without the resource have every service open.
    let unlocks = unlocks.map(|u| *u).unwrap_or_default();
    let db = &content.0;
    let bal = db.balance();
    let bucket = tick.0 % COVERAGE_PERIOD;
    let pass = (tick.0 / COVERAGE_PERIOD) % PASSES_PER_DAY;
    let roads = net.graph();
    let mut finder = None;
    for (id, b) in buildings.iter_bucket(COVERAGE_PERIOD as u32, bucket as u32) {
        if b.state != BuildingState::Active {
            continue;
        }
        let def = db.building(b.def);
        if def.service.is_some()
            || !matches!(
                def.zone,
                ZoneType::ResLow | ZoneType::ResHigh | ZoneType::ComLow | ZoneType::ComHigh
            )
        {
            continue;
        }
        let held = state.get(id).map_or(0, |s| s.crime);
        // No police station is open yet: no crime (BUG-051).
        if !unlocks.open(ServiceKind::Police) {
            if held > 0 || state.problems_of(id).has(ServiceProblems::CRIME) {
                let s = state.get_mut(id);
                s.crime = 0;
                s.problems.set(ServiceProblems::CRIME, false);
            }
            continue;
        }
        let cover = coverage.coverage_of(ServiceKind::Police, b).clamp(0.0, 1.0);
        let cheap = (1.0 - land.value_at(b.pos) / 100.0).clamp(0.0, 1.0);
        // NeighborhoodWatch (T-192) cuts the crime that grows.
        let watch = policies.get().map_or(1.0, |p| {
            p.factor_at(db, b.pos, cp_content::PolicyKind::NeighborhoodWatch)
        });
        let per_day = bal.crime_base
            * (1.0 - bal.police_cover * cover)
            * (1.0 + bal.crime_poor * cheap)
            * watch;
        // The day's total spread over its 64 passes without losing the remainder.
        let upto = |p: u64| (per_day * p as f32 / PASSES_PER_DAY as f32).round() as i64;
        let made = (upto(pass + 1) - upto(pass)).max(0) as u64;
        let mut crime = u64::from(held) + made;
        if crime > 0
            && cover > 0.0
            && let Some(station) = finder
                .get_or_insert_with(|| {
                    FacilityFinder::new(&roads, &buildings, db, ServiceKind::Police)
                })
                .nearest(&state, id)
        {
            let svc = buildings
                .get(station)
                .and_then(|f| db.building(f.def).service.as_deref())
                .and_then(|s| db.service_id(s))
                .map(|s| db.service(s));
            if let Some(svc) = svc {
                let used = state.facility(station).copied().unwrap_or_default();
                let take = crime.min(u64::from(svc.capacity.saturating_sub(used.used_today)));
                if take > 0 {
                    state.facility_mut(station).used_today += take as u32;
                    crime -= take;
                }
            }
        }
        let crime = crime.min(u64::from(u16::MAX)) as u16;
        let problem = f32::from(crime) >= bal.crime_problem;
        if crime != held || problem != state.problems_of(id).has(ServiceProblems::CRIME) {
            let s = state.get_mut(id);
            s.crime = crime;
            s.problems.set(ServiceProblems::CRIME, problem);
        }
    }
}
