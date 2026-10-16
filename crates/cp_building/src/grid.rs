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
    /// Ids removed since the last [`BuildingGrid::take_removed`]: lets other domains drop
    /// their references (citizens, trips, agents) without `cp_building` depending on them.
    /// Produced by the `Commands` phase and drained a phase later, so a save taken on pause
    /// right after a `Demolish` still holds it: it is saved (since format 13) and hashed.
    removed: Vec<BuildingId>,
    /// Ids of the free-standing buildings (§8.1); derived, rebuilt on demand like `pending`.
    #[serde(skip)]
    free: BTreeSet<BuildingId>,
    #[serde(skip)]
    free_ready: bool,
    /// Changes whenever a free-standing building is added, removed or touched through
    /// `get_mut`, and on creation and load: the zone grid's blockers need a refresh only then.
    #[serde(skip, default = "next_revision")]
    free_epoch: u64,
}

impl Default for BuildingGrid {
    fn default() -> Self {
        Self {
            buildings: Arena::default(),
            pending: BTreeSet::new(),
            pending_ready: false,
            revision: next_revision(),
            removed: Vec::new(),
            free: BTreeSet::new(),
            free_ready: false,
            free_epoch: next_revision(),
        }
    }
}

impl PartialEq for BuildingGrid {
    fn eq(&self, other: &Self) -> bool {
        self.buildings == other.buildings && self.removed == other.removed
    }
}

impl BuildingGrid {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: BuildingId) -> Option<&Building> {
        self.buildings.get(id)
    }

    /// Replaces every building's definition id by `f(old)` (a save loaded against content
    /// whose definitions sit at other indices).
    pub fn remap_defs(
        &mut self,
        f: impl Fn(cp_content::BuildingDefId) -> cp_content::BuildingDefId,
    ) {
        for (_, b) in self.buildings.iter_mut() {
            b.def = f(b.def);
        }
        self.revision = next_revision();
    }

    /// Mutable access; counts as a change for [`BuildingGrid::revision`].
    pub fn get_mut(&mut self, id: BuildingId) -> Option<&mut Building> {
        self.revision = next_revision();
        if self.free.contains(&id) {
            self.free_epoch = next_revision();
        }
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
        let is_free = matches!(b.footprint, crate::model::Footprint::Free { .. });
        let id = self.buildings.insert(b);
        if is_free {
            self.free_epoch = next_revision();
            if self.free_ready {
                self.free.insert(id);
            }
        }
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
        if matches!(b.footprint, crate::model::Footprint::Free { .. }) {
            self.free_epoch = next_revision();
            self.free.remove(&id);
        }
        self.removed.push(id);
        Some(b)
    }

    /// Sets or clears flag bit(s) `bit` of a building; counts as a change only when the flags
    /// really differ. Returns whether they did.
    pub fn set_flag(&mut self, id: BuildingId, bit: u8, on: bool) -> bool {
        let Some(b) = self.buildings.get(id) else {
            return false;
        };
        let new = if on {
            b.flags.0 | bit
        } else {
            b.flags.0 & !bit
        };
        if new == b.flags.0 {
            return false;
        }
        self.revision = next_revision();
        self.buildings.get_mut(id).expect("building").flags.0 = new;
        true
    }

    /// See the field: compare with the value of the last refresh of the zone blockers.
    pub fn free_epoch(&self) -> u64 {
        self.free_epoch
    }

    /// The rectangles of the free-standing buildings in id order, for
    /// `ZoneGrid::set_blockers`. Walks only those buildings once the index is built.
    pub fn free_quads(&mut self) -> Vec<cp_zone::geom::Quad> {
        if !self.free_ready {
            self.free = self
                .buildings
                .iter()
                .filter(|(_, b)| matches!(b.footprint, crate::model::Footprint::Free { .. }))
                .map(|(id, _)| id)
                .collect();
            self.free_ready = true;
        }
        self.free
            .iter()
            .filter_map(|&id| self.buildings.get(id))
            .map(|b| b.footprint_polygon())
            .collect()
    }

    /// The saved parts: the building arena and the pending removed ids.
    pub fn arena(&self) -> &Arena<Building> {
        &self.buildings
    }

    /// Ids removed and not yet drained (see [`BuildingGrid::take_removed`]).
    pub fn removed(&self) -> &[BuildingId] {
        &self.removed
    }

    /// Rebuilds a grid from its saved parts (migrations, tests).
    pub fn from_parts(buildings: Arena<Building>, removed: Vec<BuildingId>) -> Self {
        Self {
            buildings,
            removed,
            ..Self::default()
        }
    }

    pub fn has_removed(&self) -> bool {
        !self.removed.is_empty()
    }

    /// The ids removed since the last call, in removal order (drained by the system that
    /// cleans up what pointed at them).
    pub fn take_removed(&mut self) -> Vec<BuildingId> {
        std::mem::take(&mut self.removed)
    }

    /// Every cell covered by a building, in building order.
    pub fn occupied_cells(&self) -> impl Iterator<Item = ZoneCellRef> + '_ {
        self.buildings
            .iter()
            .flat_map(|(_, b)| b.lot().map(|l| l.cells()).into_iter().flatten())
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

/// The cells of the live zone blocks under the lot rectangle of `b` (`pos`, `rot`, `w` × `d`
/// cells), matched by cell centre. For a building whose `lot.block` is gone (its road was
/// removed and maybe rebuilt) these are the cells it covers now.
pub fn cells_under_lot(zones: &cp_zone::ZoneGrid, b: &Building) -> Vec<ZoneCellRef> {
    use bevy_math::Vec2;
    use cp_zone::CELL_SIZE;
    let (w, d) = (f32::from(b.size().0), f32::from(b.size().1));
    let (u, v) = (
        Vec2::new(b.rot.cos(), b.rot.sin()),
        Vec2::new(-b.rot.sin(), b.rot.cos()),
    );
    let reach = (w.hypot(d) * CELL_SIZE) * 0.5 + CELL_SIZE;
    zones
        .cells_within(b.pos, reach)
        .into_iter()
        .filter(|c| {
            zones.cell_footprint(*c).is_some_and(|o| {
                let off = o.c - b.pos;
                off.dot(u).abs() <= w * CELL_SIZE * 0.5 - 0.5
                    && off.dot(v).abs() <= d * CELL_SIZE * 0.5 - 0.5
            })
        })
        .collect()
}

/// The cells `b` holds: its lot cells, or, when its block is gone, the cells under its lot
/// rectangle in the blocks that replaced it.
pub fn held_cells(zones: &cp_zone::ZoneGrid, b: &Building) -> Vec<ZoneCellRef> {
    // A free-standing building holds no zone cells (§8.1).
    match b.lot() {
        None => Vec::new(),
        Some(lot) if zones.block(lot.block).is_some() => lot.cells().collect(),
        Some(_) => cells_under_lot(zones, b)
            .into_iter()
            .filter(|c| zones.is_occupied(*c))
            .collect(),
    }
}

/// A fire has burned the building down: it becomes `Burned` (its people leave on the
/// citizens' next update) until it is demolished. `false` when it is not `Active` or gone.
pub fn burn(buildings: &mut BuildingGrid, id: BuildingId) -> bool {
    match buildings.get_mut(id) {
        Some(b) if b.state == BuildingState::Active => {
            b.state = BuildingState::Burned;
            true
        }
        _ => false,
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
            let cells = held_cells(zones, &b);
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
            footprint: crate::model::Footprint::Lot(Lot {
                block: Id::new(0, 0),
                col: 0,
                w: 1,
                d: 1,
            }),
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos: bevy_math::Vec2::ZERO,
            rot: 0.0,
            access: Some(AccessPoint {
                segment: Id::new(0, 0),
                t: 0.0,
            }),
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

    /// BUG-033: the zone blockers are refreshed only when a free-standing building changes.
    #[test]
    fn the_free_epoch_moves_only_for_free_standing_buildings() {
        let mut g = BuildingGrid::new();
        assert!(g.free_quads().is_empty());
        let e0 = g.free_epoch();
        let lot = g.insert(building());
        g.get_mut(lot).unwrap().level = 2;
        g.remove(lot);
        assert_eq!(g.free_epoch(), e0, "lot buildings never touch it");

        let free = || Building {
            footprint: crate::model::Footprint::Free { w: 2, d: 2 },
            access: None,
            pos: bevy_math::Vec2::new(500.0, 500.0),
            ..building()
        };
        let id = g.insert(free());
        let e1 = g.free_epoch();
        assert_ne!(e1, e0);
        assert_eq!(g.free_quads().len(), 1);
        g.get_mut(id).unwrap().pos = bevy_math::Vec2::new(600.0, 500.0);
        let e2 = g.free_epoch();
        assert_ne!(e2, e1, "a moved free building");
        g.insert(free());
        assert_eq!(g.free_quads().len(), 2);
        g.remove(id);
        assert_ne!(g.free_epoch(), e2);
        assert_eq!(g.free_quads().len(), 1);
        // A grid loaded from bytes rebuilds the list from its buildings.
        let bytes = postcard::to_stdvec(&g).unwrap();
        let mut loaded: BuildingGrid = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(loaded.free_quads(), g.free_quads());
    }
}
