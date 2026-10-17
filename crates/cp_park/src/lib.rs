//! `cp_park` — the park areas (T-223, `architecture/system-design.md` §16 M6 (C)).
//!
//! The areas themselves (`AreaKind::Park`, level, xp) live in `cp_district`; the buildings in a
//! park (gate, attractions, props) are `BuildingDef`s with a [`cp_content::ParkSpec`], placed by
//! the rules of `cp_building::park_site`. This crate keeps, per park area, what the visitors
//! (T-224) need: the **attractiveness** (`Σ attract × (1 + park_attract_per_level × level)` of
//! the area's active buildings that fit its kind), the **capacity** (`Σ capacity`), who is
//! inside, the visits of the day and the week, and the entrance fee. Attractiveness and capacity
//! are derived: [`run_park`] recomputes them when the buildings changed and once a game hour
//! (a level may have risen without a change the count could see).

pub mod command;
pub mod visits;

use std::collections::BTreeMap;

use bevy_ecs::change_detection::{DetectChanges, DetectChangesMut};
use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Local, Res, ResMut};
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_citizen::{LeisureTarget, LeisureTargets, TripBook};
use cp_content::SimContent;
use cp_content::{ParkRole, ParkSpec};
use cp_core::Cost;
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_core::time::TICKS_PER_WEEK;
use cp_district::{AreaKind, AreaMap, Areas, area_at};
use cp_transit::Tourists;

pub use command::{ParkCommand, ParkError, apply};
use serde::{Deserialize, Serialize};
pub use visits::{ParkLedger, ParkVisits, Visit, Walker, gate_area, path_access, run_walkers};

/// The level an area with `xp` has reached, given the thresholds for levels 2 to 5.
pub fn level_for_xp(xp: u32, thresholds: &[u32; 4]) -> u8 {
    1 + thresholds.iter().filter(|&&t| xp >= t).count() as u8
}

/// Highest park level.
pub const MAX_PARK_LEVEL: u8 = 5;

/// Ticks between two forced recounts: a game hour.
pub const PARK_RECOUNT_TICKS: u64 = TICKS_PER_DAY / 24;

/// One park area. Saved and in the checksum once any exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ParkAreaState {
    /// Derived: how much the area draws.
    pub attract: f32,
    /// Derived: visitors the area holds at once.
    pub capacity: u32,
    /// Visitors inside now (T-224).
    pub inside: u32,
    /// Visits since the day began, and since the week began (T-224).
    pub visitors_day: u32,
    pub visitors_week: u32,
    /// The entrance fee (T-225), `0..=fee_max`.
    pub fee: f32,
}

/// The park areas by area id (`AreaId.0`).
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ParkState {
    pub areas: BTreeMap<u8, ParkAreaState>,
}

impl ParkState {
    pub fn is_empty(&self) -> bool {
        self.areas.is_empty()
    }

    /// The state of park area `id`, if it is one.
    pub fn get(&self, id: cp_district::AreaId) -> Option<&ParkAreaState> {
        self.areas.get(&id.0)
    }
}

/// Recounts the attractiveness and the capacity of every park area from its buildings; keeps
/// the visitor counters and the fee of the areas that remain, drops the areas that are gone.
pub fn recount(
    state: &mut ParkState,
    db: &cp_content::ContentDb,
    buildings: &BuildingGrid,
    area_map: &AreaMap,
    areas: &Areas,
) {
    let per_level = db.balance().park_attract_per_level;
    let mut sums: BTreeMap<u8, (f32, u32)> = BTreeMap::new();
    for (id, a) in areas.iter() {
        if matches!(a.kind, AreaKind::Park(_)) {
            sums.insert(id.0, (0.0, 0));
        }
    }
    for (_, b) in buildings.iter() {
        if b.state != BuildingState::Active {
            continue;
        }
        let Some(spec) = &db.building(b.def).park else {
            continue;
        };
        let Some(area_id) = area_at(area_map, b.pos) else {
            continue;
        };
        let Some(area) = areas.get(area_id) else {
            continue;
        };
        let AreaKind::Park(kind) = area.kind else {
            continue;
        };
        if spec.park.is_some_and(|k| k != kind) {
            continue;
        }
        if let Some(s) = sums.get_mut(&area_id.0) {
            s.0 += spec.attract;
            s.1 += u32::from(spec.capacity);
        }
    }
    state.areas.retain(|id, _| sums.contains_key(id));
    for (id, (attract, capacity)) in sums {
        let level = areas
            .get(cp_district::AreaId(id))
            .map_or(1.0, |a| f32::from(a.level));
        let e = state.areas.entry(id).or_default();
        e.attract = attract * (1.0 + per_level * level);
        e.capacity = capacity;
    }
}

/// The leisure targets (T-224): the gates of the parks that can take visitors, weighted by
/// the attractiveness of their area and the entrance fee; the city parks; the unique buildings
/// that draw tourists. Sorted by building.
fn leisure_targets(
    db: &cp_content::ContentDb,
    buildings: &BuildingGrid,
    area_map: &AreaMap,
    areas: &Areas,
    state: &ParkState,
) -> Vec<LeisureTarget> {
    let fee_max = db.balance().park_fee_max.max(1.0);
    let mut list = Vec::new();
    for (id, b) in buildings.iter() {
        if b.state != BuildingState::Active || b.access().is_none() {
            continue;
        }
        let def = db.building(b.def);
        let weight = match &def.park {
            Some(ParkSpec {
                role: ParkRole::Gate,
                park: Some(kind),
                ..
            }) => gate_area(areas, area_map, *kind, b.pos)
                .and_then(|a| state.get(a))
                .filter(|a| a.capacity > 0 && a.inside < a.capacity)
                .map_or(0.0, |a| a.attract * (1.0 - a.fee / fee_max * 0.5)),
            // A city park of the Buildings panel.
            Some(ParkSpec {
                role: ParkRole::Attraction,
                park: None,
                attract,
                ..
            }) => *attract,
            Some(_) => 0.0,
            None => def.unique.as_ref().map_or(0.0, |u| u.tourists_per_day),
        };
        if weight > 0.0 {
            list.push(LeisureTarget {
                building: id,
                pos: b.pos,
                weight,
            });
        }
    }
    list.sort_by_key(|t| t.building);
    list
}

/// `Agents` phase, after `run_industry`: keeps [`ParkState`] up to date, counts the arrivals at
/// the gates (residents and tourists), lets visits end and fills [`LeisureTargets`].
#[allow(clippy::too_many_arguments)]
pub fn run_park(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    area_map: Res<AreaMap>,
    mut areas: ResMut<Areas>,
    mut state: ResMut<ParkState>,
    mut book: ResMut<TripBook>,
    mut tourists: ResMut<Tourists>,
    mut visits: ResMut<ParkVisits>,
    mut ledger: ResMut<ParkLedger>,
    mut targets: ResMut<LeisureTargets>,
    mut last_revision: Local<Option<u64>>,
) {
    // The arrivals are drained even without content, so the lists never grow.
    let arrivals: Vec<(BuildingId, Tick)> = book
        .leisure_arrivals
        .drain(..)
        .map(|a| (a.building, a.leave_at))
        .chain(tourists.arrivals.drain(..))
        .collect();
    let Some(content) = content else {
        return;
    };
    let db = &*content.0;
    let now = *tick;
    // A new building, a new or repainted area, a level that rose.
    let changed =
        *last_revision != Some(buildings.revision()) || areas.is_changed() || area_map.is_changed();
    let hour = now.0.is_multiple_of(PARK_RECOUNT_TICKS);
    if changed || hour {
        *last_revision = Some(buildings.revision());
        recount(&mut state, db, &buildings, &area_map, &areas);
    }

    // The day and the week begin.
    if now.0.is_multiple_of(TICKS_PER_DAY) {
        for a in state.areas.values_mut() {
            a.visitors_day = 0;
        }
    }
    if now.0.is_multiple_of(TICKS_PER_WEEK) {
        for a in state.areas.values_mut() {
            a.visitors_week = 0;
        }
    }

    let mut levelled = false;
    // Arrivals at a gate: the visit counts, the fee is paid, the visitor is inside.
    for (building, leave_at) in arrivals {
        let Some(b) = buildings.get(building) else {
            continue;
        };
        let Some(ParkSpec {
            role: ParkRole::Gate,
            park: Some(kind),
            ..
        }) = db.building(b.def).park
        else {
            continue;
        };
        let Some(area) = gate_area(&areas, &area_map, kind, b.pos) else {
            continue;
        };
        let Some(a) = state.areas.get_mut(&area.0) else {
            continue;
        };
        a.visitors_day += 1;
        a.visitors_week += 1;
        ledger.fees += Cost::from_units(a.fee);
        // The area earns a point per visitor and rises at the thresholds of the balance. The
        // points are written past the change detection (they happen on most ticks of a busy
        // park); a new level is a change, and the next count sees it.
        let thresholds = db.balance().park_levels;
        let risen = areas.bypass_change_detection().get_mut(area).map(|rec| {
            rec.xp = rec.xp.saturating_add(1);
            let level = level_for_xp(rec.xp, &thresholds).min(MAX_PARK_LEVEL);
            let up = level > rec.level;
            if up {
                rec.level = level;
            }
            up
        });
        if risen == Some(true) {
            areas.set_changed();
            levelled = true;
        }
        visits.inside.push(Visit {
            area: area.0,
            leave_at,
        });
    }
    // A new level changes the attractiveness at once.
    if levelled {
        recount(&mut state, db, &buildings, &area_map, &areas);
    }
    // Visits end; the areas know who is left.
    visits.inside.retain(|v| v.leave_at > now);
    visits.inside.retain(|v| state.areas.contains_key(&v.area));
    for (id, a) in state.areas.iter_mut() {
        a.inside = visits.inside_of(*id);
    }

    if hour {
        targets.list = leisure_targets(db, &buildings, &area_map, &areas, &state);
    }
}

#[cfg(test)]
mod tests;
