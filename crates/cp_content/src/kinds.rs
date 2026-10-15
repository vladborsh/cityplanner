//! Content vocabulary shared by RON definitions and commands (`system-design.md` §4).
//!
//! These enums are serialised by variant number (postcard) in saves: `#[repr(u8)]`,
//! explicit discriminants, **append-only** — never reorder or remove a variant.

use serde::{Deserialize, Serialize};

/// Zoning category of a cell or building. `None` erases zoning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ZoneType {
    None = 0,
    ResLow = 1,
    ResHigh = 2,
    ComLow = 3,
    ComHigh = 4,
    Industrial = 5,
    Office = 6,
}

/// Utility network kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum UtilityKind {
    Water = 0,
    Sewage = 1,
    Power = 2,
}

/// Service kind: the budgeted utilities and roads of M1, the city services of M2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ServiceKind {
    Water = 0,
    Power = 1,
    Roads = 2,
    /// Appended in T-038: sewage outflow.
    Sewage = 3,
    /// Appended in T-134 (M2 city services).
    Garbage = 4,
    Health = 5,
    Deathcare = 6,
    Fire = 7,
    Police = 8,
    Education = 9,
}

/// Where a service building may stand (checked by `cp_building::validate_placement`).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[repr(u8)]
pub enum ServiceSite {
    /// On dry land.
    #[default]
    Land = 0,
    /// At least one footprint cell in water (a water pump).
    OnWater = 1,
    /// On dry land, with water within [`ServiceSite::SHORE_REACH_M`] (a sewage outflow).
    Shore = 2,
}

impl ServiceSite {
    /// How close water must be for [`ServiceSite::Shore`], metres.
    pub const SHORE_REACH_M: f32 = 16.0;
}
