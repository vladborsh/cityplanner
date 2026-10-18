//! Daily capacity of service buildings and the search for the nearest one with room left
//! (§16 M2 D "Суточная ёмкость").

use bevy_ecs::system::{Res, ResMut};
use cp_building::{AccessPoint, BuildingFlags, BuildingGrid, BuildingId, BuildingState};
use cp_content::{ContentDb, ServiceDef, ServiceKind};
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_net::NetGraph;

use crate::coverage::{RoadGraph, Scratch};
use crate::state::{Facility, ServiceState};

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
    nearest_facility_with(net, buildings, db, state, kind, from, |used, svc| {
        used.used_today < svc.capacity && (svc.storage == 0 || used.stored < svc.storage)
    })
    .map(|(id, _)| id)
}

/// [`nearest_facility`] with the "has room" test given by the caller, which also gets the
/// road distance (metres) back. Fire stations count engines out in `Facility::stored`.
pub fn nearest_facility_with(
    net: &NetGraph,
    buildings: &BuildingGrid,
    db: &ContentDb,
    state: &ServiceState,
    kind: ServiceKind,
    from: BuildingId,
    has_room: impl Fn(&Facility, &ServiceDef) -> bool,
) -> Option<(BuildingId, f32)> {
    FacilityFinder::new(net, buildings, db, kind).nearest_with(state, from, has_room)
}

/// Searches for the nearest facility of one kind many times in one system run: the road graph
/// and the list of working facilities are built once; the room of each is read per search.
pub struct FacilityFinder<'a> {
    buildings: &'a BuildingGrid,
    graph: RoadGraph,
    /// Active, powered buildings of the kind in ascending id order.
    cands: Vec<(BuildingId, AccessPoint, &'a ServiceDef)>,
    scratch: Scratch,
}

impl<'a> FacilityFinder<'a> {
    pub fn new(
        net: &'a NetGraph,
        buildings: &'a BuildingGrid,
        db: &'a ContentDb,
        kind: ServiceKind,
    ) -> Self {
        let mut cands = Vec::new();
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
            if svc.kind == kind {
                cands.push((id, to, svc));
            }
        }
        let graph = RoadGraph::new(net);
        let scratch = Scratch::new(&graph);
        Self {
            buildings,
            graph,
            cands,
            scratch,
        }
    }

    /// [`nearest_facility`] against the facilities this finder was built with.
    pub fn nearest(&mut self, state: &ServiceState, from: BuildingId) -> Option<BuildingId> {
        self.nearest_with(state, from, |used, svc| {
            used.used_today < svc.capacity && (svc.storage == 0 || used.stored < svc.storage)
        })
        .map(|(id, _)| id)
    }

    /// [`nearest_facility_with`] against the facilities this finder was built with.
    pub fn nearest_with(
        &mut self,
        state: &ServiceState,
        from: BuildingId,
        has_room: impl Fn(&Facility, &ServiceDef) -> bool,
    ) -> Option<(BuildingId, f32)> {
        let origin = self.buildings.get(from)?.access()?;
        let g = &self.graph;
        let open: Vec<(BuildingId, AccessPoint)> = self
            .cands
            .iter()
            .filter(|(id, _, svc)| has_room(&state.facility(*id).copied().unwrap_or_default(), svc))
            .map(|&(id, to, _)| (id, to))
            .collect();
        if open.is_empty() {
            return None;
        }
        let sc = &mut self.scratch;
        sc.clear_tails();
        let mut bound = f32::INFINITY;
        for &(_, to) in &open {
            g.add_tail(&mut sc.tail, &mut sc.tail_touched, to);
            if to.segment == origin.segment {
                bound = bound.min(g.same_segment(origin, to));
            }
        }
        let tail = std::mem::take(&mut sc.tail);
        g.search_scratch(sc, origin, f32::INFINITY, |n, d| {
            bound = bound.min(d + tail[n]);
            bound
        });
        sc.tail = tail;
        let mut best: Option<(f32, BuildingId)> = None;
        for &(id, to) in &open {
            let d = g.distance_to(&sc.dist, origin, to);
            // Ascending ids, so a strict `<` keeps the smaller id on a tie.
            if d.is_finite() && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, id));
            }
        }
        best.map(|(d, id)| (id, d))
    }
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
pub fn purge_removed_services(
    buildings: Res<BuildingGrid>,
    mut state: ResMut<ServiceState>,
    mut mail: ResMut<crate::MailState>,
) {
    for &id in buildings.removed() {
        state.remove(id);
        mail.remove(id);
    }
}
