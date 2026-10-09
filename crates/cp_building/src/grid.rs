//! The building arena.

use std::collections::BTreeSet;

use bevy_ecs::resource::Resource;
use cp_core::Arena;
use cp_zone::ZoneCellRef;
use serde::{Deserialize, Serialize};

use crate::model::{Building, BuildingId, BuildingState};

/// All buildings.
#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct BuildingGrid {
    buildings: Arena<Building>,
    /// `(ready_at tick, id)` of buildings under construction; derived, rebuilt on demand.
    #[serde(skip)]
    pending: BTreeSet<(u64, BuildingId)>,
    #[serde(skip)]
    pending_ready: bool,
}

impl PartialEq for BuildingGrid {
    fn eq(&self, other: &Self) -> bool {
        self.buildings == other.buildings
    }
}

impl BuildingGrid {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: BuildingId) -> Option<&Building> {
        self.buildings.get(id)
    }

    /// Buildings in arena (index) order.
    pub fn iter(&self) -> impl Iterator<Item = (BuildingId, &Building)> {
        self.buildings.iter()
    }

    pub fn len(&self) -> usize {
        self.buildings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buildings.is_empty()
    }

    /// Adds a building. The caller occupies its cells in the `ZoneGrid`.
    pub fn insert(&mut self, b: Building) -> BuildingId {
        let under = b.state == BuildingState::UnderConstruction;
        let ready = b.ready_at.0;
        let id = self.buildings.insert(b);
        if under && self.pending_ready {
            self.pending.insert((ready, id));
        }
        id
    }

    /// Removes a building. The caller releases its cells in the `ZoneGrid`.
    pub fn remove(&mut self, id: BuildingId) -> Option<Building> {
        let b = self.buildings.remove(id)?;
        self.pending.remove(&(b.ready_at.0, id));
        Some(b)
    }

    /// Every cell covered by a building, in building order.
    pub fn occupied_cells(&self) -> impl Iterator<Item = ZoneCellRef> + '_ {
        self.buildings.iter().flat_map(|(_, b)| b.lot.cells())
    }

    /// Moves buildings whose construction finished by `tick` to `Active`; returns them.
    pub fn finish_construction(&mut self, tick: u64) -> Vec<BuildingId> {
        if !self.pending_ready {
            self.pending = self
                .buildings
                .iter()
                .filter(|(_, b)| b.state == BuildingState::UnderConstruction)
                .map(|(id, b)| (b.ready_at.0, id))
                .collect();
            self.pending_ready = true;
        }
        let mut done = Vec::new();
        while let Some(&(t, id)) = self.pending.first() {
            if t > tick {
                break;
            }
            self.pending.pop_first();
            if let Some(b) = self.buildings.get_mut(id) {
                b.state = BuildingState::Active;
                done.push(id);
            }
        }
        done
    }
}
