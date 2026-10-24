//! Daily capacity of service buildings and the search for the nearest one with room left
//! (§16 M2 D "Суточная ёмкость").

use bevy_ecs::system::{Res, ResMut};
use cp_building::{AccessPoint, BuildingFlags, BuildingGrid, BuildingId, BuildingState};
use cp_content::{Budgets, ContentDb, ServiceDef, ServiceKind};
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_net::NetGraph;

use crate::coverage::{RoadGraph, Scratch};
use crate::state::{Facility, ServiceState};

/// The nearest Active building of `kind` by road reach (free-flow travel time, see
/// [`crate::coverage`]) from building `from` that still has room at its kind's budget
/// (`budgets`, §20): fewer than `capacity` served today and, when the definition has a `storage`, less than that
/// stored. A building without power (`NO_POWER`) does not work and is skipped. Ties go to the smaller id. `None` when `from` has no access point or nothing
/// reachable has room.
pub fn nearest_facility(
    net: &NetGraph,
    buildings: &BuildingGrid,
    db: &ContentDb,
    budgets: &Budgets,
    state: &ServiceState,
    kind: ServiceKind,
    from: BuildingId,
) -> Option<BuildingId> {
    nearest_facility_with(
        net,
        buildings,
        db,
        budgets,
        state,
        kind,
        from,
        |used, svc| {
            used.used_today < svc.capacity && (svc.storage == 0 || used.stored < svc.storage)
        },
    )
    .map(|(id, _)| id)
}

/// [`nearest_facility`] with the "has room" test given by the caller, which also gets the
/// road reach back (metres of a 40 km/h street, the unit of `radius_m`). Fire stations count
/// engines out in `Facility::stored`. The test gets the definition at the kind's budget.
#[allow(clippy::too_many_arguments)]
pub fn nearest_facility_with(
    net: &NetGraph,
    buildings: &BuildingGrid,
    db: &ContentDb,
    budgets: &Budgets,
    state: &ServiceState,
    kind: ServiceKind,
    from: BuildingId,
    has_room: impl Fn(&Facility, &ServiceDef) -> bool,
) -> Option<(BuildingId, f32)> {
    FacilityFinder::new(&RoadGraph::new(net), buildings, db, budgets, kind)
        .nearest_with(state, from, has_room)
}

/// Searches for the nearest facility of one kind many times in one system run: the road graph
/// and the list of working facilities are built once; the room of each is read per search.
/// The "has room" tests get each definition at its kind's budget (§20).
pub struct FacilityFinder<'a> {
    buildings: &'a BuildingGrid,
    graph: &'a RoadGraph,
    /// Active, powered buildings of the kind in ascending id order, with their definition
    /// (an index into `defs`).
    cands: Vec<(BuildingId, AccessPoint, usize)>,
    /// The definitions of the candidates at the budget, one per service definition.
    defs: Vec<(cp_content::ServiceDefId, ServiceDef)>,
    scratch: Scratch,
}

impl<'a> FacilityFinder<'a> {
    pub fn new(
        graph: &'a RoadGraph,
        buildings: &'a BuildingGrid,
        db: &'a ContentDb,
        budgets: &Budgets,
        kind: ServiceKind,
    ) -> Self {
        let mut cands = Vec::new();
        let mut defs: Vec<(cp_content::ServiceDefId, ServiceDef)> = Vec::new();
        for (id, b) in buildings.iter() {
            if b.state != BuildingState::Active || b.flags.has(BuildingFlags::NO_POWER) {
                continue;
            }
            let Some(to) = b.access() else { continue };
            let Some(sid) = db.building_service(b.def) else {
                continue;
            };
            let svc = db.service(sid);
            if svc.kind == kind {
                let k = match defs.iter().position(|(d, _)| *d == sid) {
                    Some(k) => k,
                    None => {
                        defs.push((sid, budgets.def(svc, db.balance())));
                        defs.len() - 1
                    }
                };
                cands.push((id, to, k));
            }
        }
        let scratch = Scratch::new(graph);
        Self {
            buildings,
            graph,
            cands,
            defs,
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
        self.nearest_with_id(state, from, |_, used, svc| has_room(used, svc))
    }

    /// [`Self::nearest_with`] where the "has room" test also gets the facility's id (garbage
    /// trucks in flight count against a facility's room, BUG-113).
    pub fn nearest_with_id(
        &mut self,
        state: &ServiceState,
        from: BuildingId,
        has_room: impl Fn(BuildingId, &Facility, &ServiceDef) -> bool,
    ) -> Option<(BuildingId, f32)> {
        let origin = self.buildings.get(from)?.access()?;
        let g = self.graph;
        let open: Vec<(BuildingId, AccessPoint)> = self
            .cands
            .iter()
            .filter(|&&(id, _, k)| {
                has_room(
                    id,
                    &state.facility(id).copied().unwrap_or_default(),
                    &self.defs[k].1,
                )
            })
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
