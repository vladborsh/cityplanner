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

use std::collections::BTreeMap;

use bevy_ecs::change_detection::DetectChanges;
use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Local, Res, ResMut};
use cp_building::{BuildingGrid, BuildingState};
use cp_content::SimContent;
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_district::{AreaKind, AreaMap, Areas, area_at};
use serde::{Deserialize, Serialize};

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

/// `Agents` phase, after `run_industry`: keeps [`ParkState`] up to date.
pub fn run_park(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    area_map: Res<AreaMap>,
    areas: Res<Areas>,
    mut state: ResMut<ParkState>,
    mut last_revision: Local<Option<u64>>,
) {
    let Some(content) = content else {
        return;
    };
    // A new building, a new or repainted area, a level that rose.
    let changed =
        *last_revision != Some(buildings.revision()) || areas.is_changed() || area_map.is_changed();
    if !changed && !tick.0.is_multiple_of(PARK_RECOUNT_TICKS) {
        return;
    }
    *last_revision = Some(buildings.revision());
    recount(&mut state, &content.0, &buildings, &area_map, &areas);
}

#[cfg(test)]
mod tests;
