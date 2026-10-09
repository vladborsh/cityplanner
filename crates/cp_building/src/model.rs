//! Building data (`architecture/system-design.md` §8).
//!
//! Save stability: serialised with postcard; changing fields is a save format change and
//! enums are append-only.

use bevy_math::Vec2;
use cp_content::BuildingDefId;
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

    pub fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Building {
    pub def: BuildingDefId,
    pub lot: Lot,
    pub level: u8,
    pub state: BuildingState,
    /// Tick at which construction finishes (spawn tick + one game day).
    pub ready_at: Tick,
    /// Lot centre (world XZ) and the yaw of the road direction in the XZ plane, radians.
    pub pos: Vec2,
    pub rot: f32,
    pub access: AccessPoint,
    pub flags: BuildingFlags,
    /// Progress towards the next level, 0..1 (used by the levelling task).
    pub level_progress: f32,
}
