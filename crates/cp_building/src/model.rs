//! Building data (`architecture/system-design.md` §8).
//!
//! Save stability: serialised with postcard; changing fields is a save format change and
//! enums are append-only.

use bevy_math::Vec2;
use cp_content::{BuildingDef, BuildingDefId};
use cp_core::time::TICKS_PER_HOUR;
use cp_core::{Id, Tick};
use cp_net::SegmentId;
use cp_zone::{BLOCK_DEPTH, ZoneBlockId, ZoneCellRef};
use serde::{Deserialize, Serialize};

pub type BuildingId = Id<Building>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuildingState {
    /// Being built; becomes `Active` after one game day.
    UnderConstruction,
    Active,
    /// Left empty for too long (abandonment rules land with the services/economy tasks).
    Abandoned,
    Burned,
}

/// A lot: `w` columns along the road by `d` rows deep, in one zone block. The first row
/// touches the road, so the lot is adjacent to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lot {
    pub block: ZoneBlockId,
    /// First column.
    pub col: u16,
    pub w: u8,
    pub d: u8,
}

impl Lot {
    /// The covered cells, column by column.
    pub fn cells(&self) -> impl Iterator<Item = ZoneCellRef> + use<> {
        let (block, col, w, d) = (self.block, self.col, self.w, self.d);
        (col..col + u16::from(w)).flat_map(move |c| {
            (0..u16::from(d)).map(move |r| ZoneCellRef {
                block,
                cell: c * BLOCK_DEPTH + r,
            })
        })
    }
}

/// What a building stands on (`architecture/system-design.md` §8.1). Enum variants are
/// append-only (save format).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Footprint {
    /// A lot in one zone block.
    Lot(Lot),
    /// A free-standing rectangle of `w` × `d` cells (8 m) centred on the building's `pos`,
    /// turned by its `rot`. Not tied to a zone block or a road.
    Free { w: u8, d: u8 },
}

/// Side of a zone cell, metres (the free-standing rectangle is counted in these too).
pub const FOOTPRINT_CELL_M: f32 = 8.0;

impl Building {
    /// The lot, for a building on a lot (`None` when free-standing).
    pub fn lot(&self) -> Option<&Lot> {
        match &self.footprint {
            Footprint::Lot(lot) => Some(lot),
            Footprint::Free { .. } => None,
        }
    }

    /// The lot, mutably (`None` when free-standing).
    pub fn lot_mut(&mut self) -> Option<&mut Lot> {
        match &mut self.footprint {
            Footprint::Lot(lot) => Some(lot),
            Footprint::Free { .. } => None,
        }
    }

    /// The access point, `None` for a free-standing building.
    pub fn access(&self) -> Option<AccessPoint> {
        self.access
    }

    /// Puts the building on `lot`.
    pub fn set_lot(&mut self, lot: Lot) {
        self.footprint = Footprint::Lot(lot);
    }

    /// Moves the access point (`None` for a free-standing building).
    pub fn set_access(&mut self, access: Option<AccessPoint>) {
        self.access = access;
    }

    /// Size in cells: `(w, d)`.
    pub fn size(&self) -> (u8, u8) {
        match self.footprint {
            Footprint::Lot(l) => (l.w, l.d),
            Footprint::Free { w, d } => (w, d),
        }
    }

    /// Whether `p` (world XZ) is inside the building's rectangle.
    pub fn footprint_contains(&self, p: Vec2) -> bool {
        let q = self.footprint_polygon();
        (0..4).all(|i| {
            let (a, b) = (q[i], q[(i + 1) % 4]);
            (b - a).perp_dot(p - a) >= 0.0
        })
    }

    /// The rectangle the building covers, in world XZ, counter-clockwise: `pos` is its centre
    /// and `rot` turns the `w` side (along `x` at `rot = 0`) and the `d` side. Lots use the
    /// same rectangle, centred on `pos` and turned by the road direction.
    pub fn footprint_polygon(&self) -> [Vec2; 4] {
        let (w, d) = self.size();
        footprint_quad(self.pos, self.rot, w, d)
    }
}

/// The rectangle of `w` × `d` cells centred on `pos` and turned by `rot`, counter-clockwise.
pub fn footprint_quad(pos: Vec2, rot: f32, w: u8, d: u8) -> [Vec2; 4] {
    let (hw, hd) = (
        f32::from(w) * FOOTPRINT_CELL_M * 0.5,
        f32::from(d) * FOOTPRINT_CELL_M * 0.5,
    );
    let (s, c) = rot.sin_cos();
    let (ax, ay) = (Vec2::new(c, s), Vec2::new(-s, c));
    [
        pos - ax * hw - ay * hd,
        pos + ax * hw - ay * hd,
        pos + ax * hw + ay * hd,
        pos - ax * hw + ay * hd,
    ]
}

/// Where the building meets the road network: a point on a segment, for pathfinding.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccessPoint {
    pub segment: SegmentId,
    /// Curve parameter of the segment's curve.
    pub t: f32,
}

/// Problem flags (`§8`): the abandonment counter reads them. Bits are append-only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildingFlags(pub u8);

impl BuildingFlags {
    pub const NO_WATER: u8 = 1;
    pub const NO_POWER: u8 = 2;
    pub const NO_WORKERS: u8 = 4;
    pub const NO_CUSTOMERS: u8 = 8;
    pub const NO_GOODS: u8 = 16;
    /// Appended in T-038.
    pub const NO_SEWAGE: u8 = 32;
    /// Appended in T-061: the road the access point stood on is gone and no other is near.
    pub const NO_ROAD: u8 = 64;
    /// Appended in T-061: `cp_traffic` found no route from/to the building (last trip result).
    pub const NO_PATH: u8 = 128;
    /// The utility bits (water, power, sewage), which `cp_utility` recomputes every 64 ticks.
    pub const UTILITY_MASK: u8 = Self::NO_WATER | Self::NO_POWER | Self::NO_SEWAGE;

    pub fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }
}

/// Ticks a building of `def` stays under construction: `round(build_hours × TICKS_PER_HOUR)`.
pub fn build_ticks(def: &BuildingDef) -> u64 {
    (def.build_hours() * TICKS_PER_HOUR).round() as u64
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Building {
    pub def: BuildingDefId,
    /// What the building stands on: a lot in a zone block, or (free-standing, §8.1) a
    /// rectangle at `pos` / `rot`. Read it through [`Building::lot`] and
    /// [`Building::footprint_polygon`].
    pub footprint: Footprint,
    pub level: u8,
    pub state: BuildingState,
    /// Tick at which construction finishes (spawn tick + one game day).
    pub ready_at: Tick,
    /// Lot centre (world XZ) and the yaw of the road direction in the XZ plane, radians.
    pub pos: Vec2,
    pub rot: f32,
    /// Where the building meets the road network; `None` for a free-standing building.
    /// Read it through [`Building::access`].
    pub access: Option<AccessPoint>,
    pub flags: BuildingFlags,
    /// Progress of the level checks: towards `+1` (a level up) or `-1` (a level down), `0`
    /// when the building is where its surroundings put it (`cp_economy::growth`).
    pub level_progress: f32,
}
