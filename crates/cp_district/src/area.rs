//! Industry and park areas (T-206, `architecture/system-design.md` §16 M5 (B)): a second grid
//! of the same shape as the district map, and the area records. A cell belongs to at most one
//! area; areas and districts overlap freely. The systems that make an area do something live
//! in `cp_industry` (M5) and `cp_park` (M6).

use bevy_ecs::prelude::Resource;
use bevy_math::Vec2;
use cp_content::{ParkKind, ResourceKind};
use cp_terrain::MAP_SIZE;
use serde::{Deserialize, Serialize};

use crate::map::{CELL_M, Cell, SIDE};

/// Areas a map can hold (ids `1..=255`; `0` is "no area").
pub const MAX_AREAS: usize = 255;

/// An area, `1..=255`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AreaId(pub u8);

/// What an area is for. The order of the variants is stored in saves: append only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AreaKind {
    /// An industrial area of one resource (M5).
    Industry(ResourceKind),
    /// A park area (M6).
    Park(ParkKind),
}

/// One area.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Area {
    pub name: String,
    pub kind: AreaKind,
    /// Level, from 1.
    pub level: u8,
    /// Experience towards the next level.
    pub xp: u32,
}

/// The area records. Area `n` is `list[n - 1]`.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Areas {
    pub list: Vec<Option<Area>>,
}

impl Areas {
    pub fn get(&self, id: AreaId) -> Option<&Area> {
        self.list
            .get(usize::from(id.0).checked_sub(1)?)
            .and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, id: AreaId) -> Option<&mut Area> {
        self.list
            .get_mut(usize::from(id.0).checked_sub(1)?)
            .and_then(Option::as_mut)
    }

    /// Existing areas in id order.
    pub fn iter(&self) -> impl Iterator<Item = (AreaId, &Area)> {
        self.list
            .iter()
            .enumerate()
            .filter_map(|(i, a)| Some((AreaId(i as u8 + 1), a.as_ref()?)))
    }
}

/// One area id per 32 m cell over the whole map, row-major (`row * SIDE + column`).
#[derive(Resource, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AreaMap {
    cells: Vec<u8>,
}

impl Default for AreaMap {
    fn default() -> Self {
        Self {
            cells: vec![0; SIDE * SIDE],
        }
    }
}

impl AreaMap {
    fn index((c, r): Cell) -> Option<usize> {
        (usize::from(c) < SIDE && usize::from(r) < SIDE)
            .then(|| usize::from(r) * SIDE + usize::from(c))
    }

    /// The cell holding `pos`, if it is on the map (same grid as the district map).
    pub fn cell_of(pos: Vec2) -> Option<Cell> {
        if !(0.0..MAP_SIZE).contains(&pos.x) || !(0.0..MAP_SIZE).contains(&pos.y) {
            return None;
        }
        Some(((pos.x / CELL_M) as u16, (pos.y / CELL_M) as u16))
    }

    pub fn get(&self, cell: Cell) -> Option<AreaId> {
        let v = *self.cells.get(Self::index(cell)?)?;
        (v != 0).then_some(AreaId(v))
    }

    /// Sets a cell; `None` clears it. Returns false off the map.
    pub fn set(&mut self, cell: Cell, area: Option<AreaId>) -> bool {
        match Self::index(cell) {
            Some(i) => {
                self.cells[i] = area.map_or(0, |a| a.0);
                true
            }
            None => false,
        }
    }

    /// Clears every cell of `area`.
    pub fn clear_area(&mut self, area: AreaId) {
        for c in &mut self.cells {
            if *c == area.0 {
                *c = 0;
            }
        }
    }

    /// Cells of `area`, row-major.
    pub fn cells_of(&self, area: AreaId) -> impl Iterator<Item = Cell> + '_ {
        self.cells
            .iter()
            .enumerate()
            .filter(move |&(_, &v)| v == area.0)
            .map(|(i, _)| ((i % SIDE) as u16, (i / SIDE) as u16))
    }

    /// Number of cells of `area`.
    pub fn cell_count(&self, area: AreaId) -> usize {
        self.cells.iter().filter(|&&v| v == area.0).count()
    }
}

/// The area at world position `pos` (`x`, `z`), if any.
pub fn area_at(map: &AreaMap, pos: Vec2) -> Option<AreaId> {
    map.get(AreaMap::cell_of(pos)?)
}
