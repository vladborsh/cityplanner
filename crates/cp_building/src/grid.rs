//! The building arena.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

use bevy_ecs::resource::Resource;
use cp_core::Arena;
use cp_zone::ZoneCellRef;
use serde::{Deserialize, Serialize};

use crate::model::{Building, BuildingId, BuildingState};

/// All buildings.
/// Process-wide source of grid revisions: every change of any grid, and every new, cloned-then-
/// changed or deserialized grid, gets a value no other grid has had, so a cache keyed by the
/// revision can never mistake one grid for another.
static REVISIONS: AtomicU64 = AtomicU64::new(1);

fn next_revision() -> u64 {
    REVISIONS.fetch_add(1, Ordering::Relaxed)
}

#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
pub struct BuildingGrid {
    buildings: Arena<Building>,
    /// `(ready_at tick, id)` of buildings under construction; derived, rebuilt on demand.
    #[serde(skip)]
    pending: BTreeSet<(u64, BuildingId)>,
    #[serde(skip)]
    pending_ready: bool,
    /// Set from a process-wide counter on creation, load and every change (insert, remove,
    /// state change, `get_mut`); lets clients that cache a snapshot (`SimView::buildings`)
    /// notice changes cheaply. Not saved, not hashed.
    #[serde(skip, default = "next_revision")]
    revision: u64,
}

impl Default for BuildingGrid {
    fn default() -> Self {
        Self {
            buildings: Arena::default(),
            pending: BTreeSet::new(),
            pending_ready: false,
            revision: next_revision(),
        }
    }
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

    /// Mutable access; counts as a change for [`BuildingGrid::revision`].
    pub fn get_mut(&mut self, id: BuildingId) -> Option<&mut Building> {
        self.revision = next_revision();
        self.buildings.get_mut(id)
    }

    /// Changes whenever the contents change, and is never shared between grids (see the
    /// field docs).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Buildings in arena (index) order.
    pub fn iter(&self) -> impl Iterator<Item = (BuildingId, &Building)> {
        self.buildings.iter()
    }

    /// Buildings of one bucket (`index % buckets == bucket`), ascending.
    pub fn iter_bucket(
        &self,
        buckets: u32,
        bucket: u32,
    ) -> impl Iterator<Item = (BuildingId, &Building)> {
        self.buildings.iter_bucket(buckets, bucket)
    }

    pub fn len(&self) -> usize {
        self.buildings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buildings.is_empty()
    }

    /// Adds a building. The caller occupies its cells in the `ZoneGrid`.
    pub fn insert(&mut self, b: Building) -> BuildingId {
        self.revision = next_revision();
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
        self.revision = next_revision();
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
        if !done.is_empty() {
            self.revision = next_revision();
        }
        done
    }
}

/// Removes a building and frees its lot cells in the zone grid. The future `Demolish`
/// command uses this; citizens living or working there leave on their next update.
pub fn demolish_building(
    buildings: &mut BuildingGrid,
    zones: &mut cp_zone::ZoneGrid,
    id: BuildingId,
) -> bool {
    match buildings.remove(id) {
        Some(b) => {
            let cells: Vec<ZoneCellRef> = b.lot.cells().collect();
            zones.release(&cells);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod revision_tests {
    use super::*;
    use crate::model::{AccessPoint, BuildingFlags, Lot};
    use cp_core::{Id, Tick};

    fn building() -> Building {
        Building {
            def: cp_content::BuildingDefId(0),
            lot: Lot {
                block: Id::new(0, 0),
                col: 0,
                w: 1,
                d: 1,
            },
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos: bevy_math::Vec2::ZERO,
            rot: 0.0,
            access: AccessPoint {
                segment: Id::new(0, 0),
                t: 0.0,
            },
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        }
    }

    #[test]
    fn grids_never_share_a_revision() {
        let (mut a, mut b) = (BuildingGrid::new(), BuildingGrid::new());
        let mut seen = BTreeSet::from([a.revision(), b.revision()]);
        assert_eq!(seen.len(), 2, "two fresh grids differ");
        for _ in 0..3 {
            a.insert(building());
            b.insert(building());
            assert!(seen.insert(a.revision()) && seen.insert(b.revision()));
        }
        let id = a.iter().next().unwrap().0;
        a.get_mut(id).unwrap().level = 2;
        assert!(seen.insert(a.revision()));
        a.remove(id);
        assert!(seen.insert(a.revision()));
    }

    #[test]
    fn a_deserialized_grid_does_not_reuse_an_earlier_revision() {
        let mut g = BuildingGrid::new();
        g.insert(building());
        let before = g.revision();
        let bytes = postcard::to_stdvec(&g).unwrap();
        let loaded: BuildingGrid = postcard::from_bytes(&bytes).unwrap();
        let loaded2: BuildingGrid = postcard::from_bytes(&bytes).unwrap();
        assert_ne!(loaded.revision(), before);
        assert_ne!(loaded.revision(), loaded2.revision());
        assert_eq!(loaded, g, "contents survive; only the revision is new");
    }
}
