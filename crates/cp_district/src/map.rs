//! The district grid and the district records.

use bevy_ecs::prelude::Resource;
use bevy_math::Vec2;
use cp_content::ZoneType;
use cp_terrain::MAP_SIZE;
use serde::{Deserialize, Serialize};

/// Side of one district cell, metres.
pub const CELL_M: f32 = 32.0;
/// Cells per map side.
pub const SIDE: usize = (MAP_SIZE / CELL_M) as usize;
/// Districts a map can hold (ids `1..=255`; `0` is "no district").
pub const MAX_DISTRICTS: usize = 255;
/// Longest district name, in characters.
pub const NAME_MAX: usize = 32;
/// Colours the UI cycles through: a district's `color` is `(id - 1) % PALETTE_LEN`.
pub const PALETTE_LEN: u8 = 12;

/// A cell of the district grid: `(column, row)`, `0..SIDE` each.
pub type Cell = (u16, u16);

/// A district, `1..=255`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DistrictId(pub u8);

/// Tax-table slot of a zone type (`None` pays no tax): the order of the city tax table,
/// `ResLow, ResHigh, ComLow, ComHigh, Industrial, Office`.
pub fn zone_slot(zone: ZoneType) -> Option<usize> {
    (zone as usize).checked_sub(1)
}

/// One district.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct District {
    pub name: String,
    /// Palette index, `0..PALETTE_LEN`.
    pub color: u8,
    /// Tax rate override in percent per zone type (slot as in [`zone_slot`]); `None` = the
    /// city rate.
    pub tax: [Option<u8>; 6],
}

/// The district records. District `n` is `list[n - 1]`.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Districts {
    pub list: Vec<Option<District>>,
}

impl Districts {
    pub fn get(&self, id: DistrictId) -> Option<&District> {
        self.list
            .get(usize::from(id.0).checked_sub(1)?)
            .and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, id: DistrictId) -> Option<&mut District> {
        self.list
            .get_mut(usize::from(id.0).checked_sub(1)?)
            .and_then(Option::as_mut)
    }

    /// Existing districts in id order.
    pub fn iter(&self) -> impl Iterator<Item = (DistrictId, &District)> {
        self.list
            .iter()
            .enumerate()
            .filter_map(|(i, d)| Some((DistrictId(i as u8 + 1), d.as_ref()?)))
    }

    /// The tax override of `zone` in `district`, if the district exists and sets one.
    pub fn tax_override(&self, district: DistrictId, zone: ZoneType) -> Option<u8> {
        self.get(district)?.tax[zone_slot(zone)?]
    }
}

/// One district id per 32 m cell over the whole map, row-major (`row * SIDE + column`).
#[derive(Resource, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistrictMap {
    cells: Vec<u8>,
}

impl Default for DistrictMap {
    fn default() -> Self {
        Self {
            cells: vec![0; SIDE * SIDE],
        }
    }
}

impl DistrictMap {
    fn index((c, r): Cell) -> Option<usize> {
        (usize::from(c) < SIDE && usize::from(r) < SIDE)
            .then(|| usize::from(r) * SIDE + usize::from(c))
    }

    /// The cell holding `pos`, if it is on the map.
    pub fn cell_of(pos: Vec2) -> Option<Cell> {
        if !(0.0..MAP_SIZE).contains(&pos.x) || !(0.0..MAP_SIZE).contains(&pos.y) {
            return None;
        }
        Some(((pos.x / CELL_M) as u16, (pos.y / CELL_M) as u16))
    }

    pub fn get(&self, cell: Cell) -> Option<DistrictId> {
        let v = *self.cells.get(Self::index(cell)?)?;
        (v != 0).then_some(DistrictId(v))
    }

    /// Sets a cell; `None` clears it. Returns false off the map.
    pub fn set(&mut self, cell: Cell, district: Option<DistrictId>) -> bool {
        match Self::index(cell) {
            Some(i) => {
                self.cells[i] = district.map_or(0, |d| d.0);
                true
            }
            None => false,
        }
    }

    /// Clears every cell of `district`.
    pub fn clear_district(&mut self, district: DistrictId) {
        for c in &mut self.cells {
            if *c == district.0 {
                *c = 0;
            }
        }
    }

    /// Cells of `district`, row-major.
    pub fn cells_of(&self, district: DistrictId) -> impl Iterator<Item = Cell> + '_ {
        self.cells
            .iter()
            .enumerate()
            .filter(move |&(_, &v)| v == district.0)
            .map(|(i, _)| ((i % SIDE) as u16, (i / SIDE) as u16))
    }
}

/// The district at world position `pos` (`x`, `z`), if any.
pub fn district_at(map: &DistrictMap, pos: Vec2) -> Option<DistrictId> {
    map.get(DistrictMap::cell_of(pos)?)
}
