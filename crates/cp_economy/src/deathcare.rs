//! Deathcare (§16 M2 D "Ритуальные услуги"): the dead wait in the home they lived in
//! (`CitizenGrid::dead`), cemeteries and crematoria within reach take them by their daily
//! capacity, and a building with bodies nobody has taken has the DEAD problem.
//!
//! Per tick the buildings of bucket `index % 64 == tick % 64` that hold bodies are served:
//! with Deathcare coverage the nearest powered facility with room (a cemetery also needs
//! `stored < storage`; a crematorium without power does not work) takes
//! `min(dead, capacity left today, storage left)`; a cemetery keeps them in `Facility::stored`,
//! a crematorium (no `storage`) burns them. A building with none left drops its record and the
//! DEAD bit.

use bevy_ecs::system::{Res, ResMut};
use cp_building::BuildingGrid;
use cp_citizen::CitizenGrid;
use cp_content::{ServiceKind, ServiceUnlocks, SimContent};
use cp_core::Tick;
use cp_net::NetGraph;
use cp_service::{COVERAGE_PERIOD, FacilityFinder, ServiceCoverage, ServiceProblems, ServiceState};

/// Economy phase, after `update_health`.
#[allow(clippy::too_many_arguments)]
pub fn update_deathcare(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    buildings: Res<BuildingGrid>,
    coverage: Res<ServiceCoverage>,
    mut grid: ResMut<CitizenGrid>,
    mut state: ResMut<ServiceState>,
    unlocks: Option<Res<ServiceUnlocks>>,
) {
    let Some(content) = content else { return };
    // Worlds built without the resource have every service open.
    let unlocks = unlocks.map(|u| *u).unwrap_or_default();
    let db = &content.0;
    let bucket = tick.0 % COVERAGE_PERIOD;
    let homes: Vec<_> = grid
        .dead
        .keys()
        .copied()
        .filter(|id| u64::from(id.index) % COVERAGE_PERIOD == bucket)
        .collect();
    // No cemetery or crematorium is open yet: no bodies wait (BUG-051).
    if !unlocks.open(ServiceKind::Deathcare) {
        for id in homes {
            grid.dead.remove(&id);
            if state.problems_of(id).has(ServiceProblems::DEAD) {
                state.get_mut(id).problems.set(ServiceProblems::DEAD, false);
            }
        }
        return;
    }
    let mut finder = None;
    for id in homes {
        let mut dead = u32::from(grid.dead.get(&id).copied().unwrap_or(0));
        let covered = buildings
            .get(id)
            .is_some_and(|b| coverage.coverage_of(ServiceKind::Deathcare, b) > 0.0);
        while dead > 0 && covered {
            let Some(station) = finder
                .get_or_insert_with(|| {
                    FacilityFinder::new(&net, &buildings, db, ServiceKind::Deathcare)
                })
                .nearest(&state, id)
            else {
                break;
            };
            let Some(svc) = buildings
                .get(station)
                .and_then(|f| db.building(f.def).service.as_deref())
                .and_then(|s| db.service_id(s))
                .map(|s| db.service(s))
            else {
                break;
            };
            let used = state.facility(station).copied().unwrap_or_default();
            let mut room = svc.capacity.saturating_sub(used.used_today);
            if svc.storage > 0 {
                room = room.min(svc.storage.saturating_sub(used.stored));
            }
            let take = dead.min(room);
            if take == 0 {
                break;
            }
            let f = state.facility_mut(station);
            f.used_today += take;
            if svc.storage > 0 {
                f.stored += take;
            }
            dead -= take;
        }
        if dead == 0 {
            grid.dead.remove(&id);
        } else {
            grid.dead.insert(id, dead.min(u32::from(u16::MAX)) as u16);
        }
        if (dead > 0) != state.problems_of(id).has(ServiceProblems::DEAD) {
            state
                .get_mut(id)
                .problems
                .set(ServiceProblems::DEAD, dead > 0);
        }
    }
}
