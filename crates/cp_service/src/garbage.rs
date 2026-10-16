//! Garbage (§16 M2 D "Мусор"): buildings pile it up, landfills and incinerators take it away
//! within their daily capacity, and a building that holds too much has the GARBAGE problem.
//!
//! (T-151) With `cp_traffic`'s [`ServiceRuns`] present the hand-over is a trip: a covered
//! building holding `garbage_truck_min` gets a garbage truck from the nearest facility with
//! room, and the garbage is taken (within the facility's room at that moment) when the truck
//! arrives — by driving, or abstractly when the agent pool is full. A failed run takes it at
//! once, as without runs. Without `ServiceRuns` every pass hands the garbage over at once.

use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingGrid, BuildingState};
use cp_citizen::CitizenGrid;
use cp_content::{ServiceKind, SimContent, VehicleKind};
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_net::NetGraph;
use cp_traffic::{RunState, ServiceRuns};

use crate::coverage::{COVERAGE_PERIOD, ServiceCoverage};
use crate::facility::nearest_facility;
use crate::state::{ServiceProblems, ServiceState};

/// Passes a building gets per game day (one every [`COVERAGE_PERIOD`] ticks).
const PASSES_PER_DAY: u64 = TICKS_PER_DAY / COVERAGE_PERIOD;

/// Services phase: every tick the buildings of bucket `index % 64 == tick % 64` pile up the
/// garbage of the last 64 ticks and, when a garbage service covers them, hand it over to the
/// nearest facility with room left.
#[allow(clippy::too_many_arguments)]
pub fn update_garbage(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    buildings: Res<BuildingGrid>,
    citizens: Res<CitizenGrid>,
    coverage: Res<ServiceCoverage>,
    mut state: ResMut<ServiceState>,
    mut runs: Option<ResMut<ServiceRuns>>,
) {
    let Some(content) = content else { return };
    let db = &content.0;
    let balance = db.balance();
    let bucket = tick.0 % COVERAGE_PERIOD;
    let pass = (tick.0 / COVERAGE_PERIOD) % PASSES_PER_DAY;
    for (id, b) in buildings.iter() {
        if u64::from(id.index) % COVERAGE_PERIOD != bucket || b.state != BuildingState::Active {
            continue;
        }
        let workers: u16 = (0..4).map(|l| citizens.jobs_filled(id, l)).sum();
        let per_day = (f32::from(citizens.households_in(id)) * balance.garbage_household
            + f32::from(workers) * balance.garbage_job)
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
                if garbage as f32 >= balance.garbage_truck_min
                    && coverage.coverage_of(ServiceKind::Garbage, b) > 0.0
                    && let Some(facility) =
                        nearest_facility(&net, &buildings, db, &state, ServiceKind::Garbage, id)
                {
                    r.start(VehicleKind::Truck, facility, id);
                }
                false
            }
        };
        if hand_over_now
            && garbage > 0
            && coverage.coverage_of(ServiceKind::Garbage, b) > 0.0
            && let Some(facility) =
                nearest_facility(&net, &buildings, db, &state, ServiceKind::Garbage, id)
        {
            let svc = buildings
                .get(facility)
                .and_then(|f| db.building(f.def).service.as_deref())
                .and_then(|s| db.service_id(s))
                .map(|s| db.service(s));
            if let Some(svc) = svc {
                let used = state.facility(facility).copied().unwrap_or_default();
                let mut room = u64::from(svc.capacity.saturating_sub(used.used_today));
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
