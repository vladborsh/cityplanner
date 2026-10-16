//! Daily capacity of service buildings and the search for the nearest one with room left
//! (§16 M2 D "Суточная ёмкость").

use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingFlags, BuildingGrid, BuildingId, BuildingState};
use cp_content::{ContentDb, ServiceKind};
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_net::NetGraph;

use crate::coverage::RoadGraph;
use crate::state::ServiceState;

/// The nearest Active building of `kind` by road from building `from` that still has room:
/// fewer than `capacity` served today and, when the definition has a `storage`, less than that
/// stored. A building without power (`NO_POWER`) does not work and is skipped. Ties go to the smaller id. `None` when `from` has no access point or nothing
/// reachable has room.
pub fn nearest_facility(
    net: &NetGraph,
    buildings: &BuildingGrid,
    db: &ContentDb,
    state: &ServiceState,
    kind: ServiceKind,
    from: BuildingId,
) -> Option<BuildingId> {
    let origin = buildings.get(from)?.access()?;
    let g = RoadGraph::new(net);
    let dist = g.node_distances(origin, f32::INFINITY);
    let mut best: Option<(f32, BuildingId)> = None;
    for (id, b) in buildings.iter() {
        if b.state != BuildingState::Active || b.flags.has(BuildingFlags::NO_POWER) {
            continue;
        }
        let Some(to) = b.access() else { continue };
        let Some(svc) = db
            .building(b.def)
            .service
            .as_deref()
            .and_then(|s| db.service_id(s))
            .map(|s| db.service(s))
        else {
            continue;
        };
        if svc.kind != kind {
            continue;
        }
        let used = state.facility(id).copied().unwrap_or_default();
        if used.used_today >= svc.capacity || (svc.storage > 0 && used.stored >= svc.storage) {
            continue;
        }
        let d = g.distance_to(&dist, origin, to);
        // Buildings come in ascending id order, so a strict `<` keeps the smaller id on a tie.
        if d.is_finite() && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, id));
        }
    }
    best.map(|(_, id)| id)
}

/// Services phase: at the start of every game day the daily throughput of all facilities
/// starts over.
pub fn reset_daily_capacity(tick: Res<Tick>, mut state: ResMut<ServiceState>) {
    if tick.0 != 0 && tick.0.is_multiple_of(TICKS_PER_DAY) {
        state.reset_day();
    }
}

/// Drops the service state of demolished buildings. Runs before the system that drains the
/// grid's removed list (`cp_traffic::purge_removed_buildings`).
pub fn purge_removed_services(buildings: Res<BuildingGrid>, mut state: ResMut<ServiceState>) {
    for &id in buildings.removed() {
        state.remove(id);
    }
}
