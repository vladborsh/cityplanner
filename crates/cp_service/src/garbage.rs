//! Garbage (§16 M2 D "Мусор"): buildings pile it up, landfills and incinerators take it away
//! within their daily capacity, and a building that holds too much has the GARBAGE problem.
//!
//! (T-151) With `cp_traffic`'s [`ServiceRuns`] present the hand-over is a trip: a covered
//! building holding `garbage_truck_min` gets a garbage truck from the nearest facility with
//! room, and the garbage is taken (within the facility's room at that moment) when the truck
//! arrives — by driving, or abstractly when the agent pool is full. A failed run takes it at
//! once, as without runs. Without `ServiceRuns` every pass hands the garbage over at once.

use std::collections::BTreeMap;

use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_citizen::CitizenGrid;
use cp_content::{PolicyKind, ServiceKind, ServiceUnlocks, SimContent, VehicleKind};
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_district::PolicyParams;
use cp_traffic::{RunPoint, RunState, ServiceRuns};

use crate::coverage::{COVERAGE_PERIOD, ServiceCoverage};
use crate::facility::FacilityFinder;
use crate::state::{ServiceProblems, ServiceState};

/// A building holding this share of the problem level may use a facility's reserved last
/// quarter of the day (BUG-113).
const URGENT: f32 = 0.75;

/// The garbage the trucks on their way will bring to each facility: what the buildings they
/// go to hold now.
fn booked_by_trucks(runs: &ServiceRuns, state: &ServiceState) -> BTreeMap<BuildingId, u64> {
    let mut out = BTreeMap::new();
    for (_, r) in runs.iter() {
        if r.kind != VehicleKind::Truck as u8 {
            continue;
        }
        if let (RunPoint::Building(from), RunPoint::Building(to)) = (r.from, r.to) {
            *out.entry(from).or_default() += u64::from(state.get(to).map_or(0, |s| s.garbage));
        }
    }
    out
}

/// Passes a building gets per game day (one every [`COVERAGE_PERIOD`] ticks).
const PASSES_PER_DAY: u64 = TICKS_PER_DAY / COVERAGE_PERIOD;

/// Services phase: every tick the buildings of bucket `index % 64 == tick % 64` pile up the
/// garbage of the last 64 ticks and, when a garbage service covers them, hand it over to the
/// nearest facility with room left.
#[allow(clippy::too_many_arguments)]
pub fn update_garbage(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: crate::ServiceNet,
    buildings: Res<BuildingGrid>,
    citizens: Res<CitizenGrid>,
    coverage: Res<ServiceCoverage>,
    mut state: ResMut<ServiceState>,
    mut runs: Option<ResMut<ServiceRuns>>,
    policies: PolicyParams,
    unlocks: Option<Res<ServiceUnlocks>>,
) {
    let Some(content) = content else { return };
    // Worlds built without the resource have every service open.
    let unlocks = unlocks.map(|u| *u).unwrap_or_default();
    let db = &content.0;
    let balance = db.balance();
    let budgets = net.budgets();
    let bucket = tick.0 % COVERAGE_PERIOD;
    let pass = (tick.0 / COVERAGE_PERIOD) % PASSES_PER_DAY;
    let roads = net.graph();
    let mut finder = None;
    let mut booked: Option<BTreeMap<BuildingId, u64>> = None;
    // Only this pass's bucket, in id order (T-256: the whole grid was walked to find it).
    for (id, b) in buildings.iter_bucket(COVERAGE_PERIOD as u32, bucket as u32) {
        // No garbage service is open yet: no garbage piles up (BUG-051).
        if !unlocks.open(ServiceKind::Garbage) {
            if state
                .get(id)
                .is_some_and(|s| s.garbage > 0 || s.problems.has(ServiceProblems::GARBAGE))
            {
                let s = state.get_mut(id);
                s.garbage = 0;
                s.problems.set(ServiceProblems::GARBAGE, false);
            }
            continue;
        }
        if b.state != BuildingState::Active {
            continue;
        }
        let workers: u16 = (0..4).map(|l| citizens.jobs_filled(id, l)).sum();
        // Recycling (T-192) cuts what the building makes.
        let recycling = policies
            .get()
            .map_or(1.0, |p| p.factor_at(db, b.pos, PolicyKind::Recycling));
        let per_day = ((f32::from(citizens.households_in(id)) * balance.garbage_household
            + f32::from(workers) * balance.garbage_job)
            * recycling)
            .round() as u64;
        let held = state.get(id).map_or(0, |s| s.garbage);
        if per_day == 0 && held == 0 {
            continue;
        }
        // The day's total spread over its 64 passes without losing the remainder.
        let made = per_day * (pass + 1) / PASSES_PER_DAY - per_day * pass / PASSES_PER_DAY;
        let mut garbage = u64::from(held) + made;
        let truck = runs
            .as_deref()
            .and_then(|r| r.find(VehicleKind::Truck, id))
            .map(|(run, r)| (run, r.state));
        let hand_over_now = match (runs.as_deref_mut(), truck) {
            (None, _) => true,
            // The truck is here (or the run failed): take the garbage now.
            (Some(r), Some((run, RunState::Arrived { .. } | RunState::Failed))) => {
                r.remove(run);
                true
            }
            (Some(_), Some(_)) => false,
            (Some(r), None) => {
                // BUG-113: a truck is worth the trip with half a problem's worth on board,
                // and only to a facility whose day is not already spoken for by the trucks on
                // their way. The last quarter of its day is for buildings near the problem.
                let trigger = balance.garbage_truck_min.max(balance.garbage_problem * 0.5);
                if garbage as f32 >= trigger && coverage.coverage_of(ServiceKind::Garbage, b) > 0.0
                {
                    let booked = booked.get_or_insert_with(|| booked_by_trucks(r, &state));
                    let urgent = garbage as f32 >= balance.garbage_problem * URGENT;
                    let finder = finder.get_or_insert_with(|| {
                        FacilityFinder::new(&roads, &buildings, db, &budgets, ServiceKind::Garbage)
                    });
                    let found = finder.nearest_with_id(&state, id, |f, used, svc| {
                        let committed =
                            u64::from(used.used_today) + booked.get(&f).copied().unwrap_or(0);
                        let free = u64::from(svc.capacity).saturating_sub(committed);
                        let kept = if urgent {
                            0
                        } else {
                            u64::from(svc.capacity) / 4
                        };
                        free > kept && (svc.storage == 0 || used.stored < svc.storage)
                    });
                    if let Some((facility, _)) = found {
                        r.start(VehicleKind::Truck, facility, id);
                        *booked.entry(facility).or_default() += garbage;
                    }
                }
                false
            }
        };
        if hand_over_now
            && garbage > 0
            && coverage.coverage_of(ServiceKind::Garbage, b) > 0.0
            && let Some(facility) = finder
                .get_or_insert_with(|| {
                    FacilityFinder::new(&roads, &buildings, db, &budgets, ServiceKind::Garbage)
                })
                .nearest(&state, id)
        {
            let svc = buildings
                .get(facility)
                .and_then(|f| db.building_service(f.def))
                .map(|s| db.service(s));
            if let Some(svc) = svc {
                let used = state.facility(facility).copied().unwrap_or_default();
                let capacity = budgets.capacity(svc, balance);
                let mut room = u64::from(capacity.saturating_sub(used.used_today));
                if svc.storage > 0 {
                    room = room.min(u64::from(svc.storage.saturating_sub(used.stored)));
                }
                let take = garbage.min(room);
                if take > 0 {
                    let f = state.facility_mut(facility);
                    f.used_today += take as u32;
                    if svc.storage > 0 {
                        f.stored += take as u32;
                    }
                    garbage -= take;
                }
            }
        }
        let garbage = garbage.min(u64::from(u16::MAX)) as u16;
        let problem = f32::from(garbage) >= balance.garbage_problem;
        if garbage != held || problem != state.problems_of(id).has(ServiceProblems::GARBAGE) {
            let s = state.get_mut(id);
            s.garbage = garbage;
            s.problems.set(ServiceProblems::GARBAGE, problem);
        }
    }
}
