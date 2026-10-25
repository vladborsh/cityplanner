//! Hotels (§22.2, T-321): every building whose def has a [`cp_content::HotelSpec`] (a placed
//! hotel or a grown Tourism building) has a slot here with its guests and the week's nights
//! and money. The slots follow the building grid ([`track_hotels`]); the lodgers fill them
//! (T-321b).

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingGrid, BuildingId};
use cp_content::{ContentDb, HotelSpec, SimContent};
use cp_core::Cost;
use serde::{Deserialize, Serialize};

/// One hotel's guests now and its week so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotelSlot {
    pub guests: u16,
    pub nights_week: u32,
    pub revenue_week: Cost,
}

/// The hotels of the city, by building (few: a `BTreeMap` keeps the order). Serialized sim
/// state.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotelState {
    pub slots: BTreeMap<BuildingId, HotelSlot>,
    /// The `BuildingGrid::revision` the slots were last matched to; after a load it is unknown
    /// and the next tick matches again (to the same slots).
    #[serde(skip)]
    revision: Option<u64>,
}

impl HotelState {
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Adds a slot for each new hotel and drops those whose building is gone or no longer a
    /// hotel; the slots of the others stay as they are.
    pub fn sync(&mut self, db: &ContentDb, buildings: &BuildingGrid) {
        let hotels: std::collections::BTreeSet<BuildingId> = buildings
            .iter()
            .filter(|(_, b)| db.building(b.def).hotel.is_some())
            .map(|(id, _)| id)
            .collect();
        self.slots.retain(|id, _| hotels.contains(id));
        for id in hotels {
            self.slots.entry(id).or_default();
        }
        self.revision = Some(buildings.revision());
    }

    /// The hotel spec of a building that has a slot.
    pub fn spec<'a>(
        db: &'a ContentDb,
        buildings: &BuildingGrid,
        id: BuildingId,
    ) -> Option<&'a HotelSpec> {
        buildings
            .get(id)
            .and_then(|b| db.building(b.def).hotel.as_ref())
    }

    /// Guests and rooms of every hotel.
    pub fn occupancy(&self, db: &ContentDb, buildings: &BuildingGrid) -> (u32, u32) {
        self.slots.iter().fold((0, 0), |(g, r), (&id, s)| {
            let rooms = Self::spec(db, buildings, id).map_or(0, |h| u32::from(h.rooms));
            (g + u32::from(s.guests), r + rooms)
        })
    }
}

/// `Agents` phase, before `run_tourists`: matches the slots to the buildings when the grid
/// changed.
pub fn track_hotels(
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    mut hotels: ResMut<HotelState>,
) {
    let Some(content) = content else { return };
    if hotels.revision != Some(buildings.revision()) {
        hotels.sync(&content.0, &buildings);
    }
}
