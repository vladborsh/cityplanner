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
    /// Appended in T-156 (M3): the bus depot.
    Transit = 10,
    /// Appended in T-200 (M4): unique buildings and monuments.
    Landmark = 11,
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

/// A city or district policy (T-190, M4). The order is the bit number in a policy mask
/// (`District.policies`, `CityPolicies.mask`), so it is **append-only** like every enum here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum PolicyKind {
    /// Less garbage from buildings.
    Recycling = 0,
    /// Less fire risk.
    SmokeDetectors = 1,
    /// Less crime growth.
    NeighborhoodWatch = 2,
    /// Less electricity use.
    PowerSaving = 3,
    /// Tall buildings stop at a level.
    HighRiseBan = 4,
    /// Trucks drive around the district.
    HeavyTrafficBan = 5,
    /// Free tickets, transit looks better when choosing a mode.
    FreeTransit = 6,
    /// More tourists (city only).
    TourismPromotion = 7,
}

impl PolicyKind {
    /// Every policy in bit order.
    pub const ALL: [PolicyKind; 8] = [
        PolicyKind::Recycling,
        PolicyKind::SmokeDetectors,
        PolicyKind::NeighborhoodWatch,
        PolicyKind::PowerSaving,
        PolicyKind::HighRiseBan,
        PolicyKind::HeavyTrafficBan,
        PolicyKind::FreeTransit,
        PolicyKind::TourismPromotion,
    ];

    /// The bit of this policy in a policy mask.
    pub const fn bit(self) -> u32 {
        1 << (self as u8)
    }
}

/// Where a policy may be switched on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum PolicyScope {
    City = 0,
    District = 1,
    Both = 2,
}

impl PolicyScope {
    pub fn allows_city(self) -> bool {
        matches!(self, Self::City | Self::Both)
    }

    pub fn allows_district(self) -> bool {
        matches!(self, Self::District | Self::Both)
    }
}
