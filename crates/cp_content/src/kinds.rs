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
    /// Appended in T-208 (M5): the buildings of the industrial chains.
    Industry = 12,
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

/// A district specialisation (T-196, M4). The order is stored in saves (`District.spec`), so
/// it is **append-only**.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum SpecKind {
    /// Commercial: noisier, more tourists.
    Leisure = 0,
    /// Commercial: many tourists, higher tax.
    Tourism = 1,
    /// Commercial: fewer goods used, dearer land.
    Organic = 2,
    /// Office: jobs for the educated, higher tax.
    ItCluster = 3,
    /// Low residential: uses less power and water, lower tax.
    SelfSufficient = 4,
}

impl SpecKind {
    /// Every specialisation in kind order.
    pub const ALL: [SpecKind; 5] = [
        SpecKind::Leisure,
        SpecKind::Tourism,
        SpecKind::Organic,
        SpecKind::ItCluster,
        SpecKind::SelfSufficient,
    ];
}

/// A raw resource of the map (T-205, M5). The order is stored in saves and is the channel order
/// of `resources.png` (R, G, B, A), so it is **append-only**.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ResourceKind {
    /// Fertile land (red channel). Not used up.
    Farming = 0,
    /// Forest (green channel). Not used up; grows back.
    Forestry = 1,
    /// Ore (blue channel). Deposits run out.
    Ore = 2,
    /// Oil (alpha channel). Deposits run out.
    Oil = 3,
}

/// A kind of park area (T-206, M6). Stored in saves (`AreaKind::Park`): append only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ParkKind {
    CityPark = 0,
    Nature = 1,
    Zoo = 2,
    Amusement = 3,
}

impl ParkKind {
    /// Every park kind in kind order.
    pub const ALL: [ParkKind; 4] = [
        ParkKind::CityPark,
        ParkKind::Nature,
        ParkKind::Zoo,
        ParkKind::Amusement,
    ];
}

impl ResourceKind {
    /// Every resource in kind order.
    pub const ALL: [ResourceKind; 4] = [
        ResourceKind::Farming,
        ResourceKind::Forestry,
        ResourceKind::Ore,
        ResourceKind::Oil,
    ];

    /// Channel of `resources.png` (and slot in per-resource tables).
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Ore and oil run out; land and forest do not.
    pub const fn is_finite(self) -> bool {
        matches!(self, Self::Ore | Self::Oil)
    }
}

/// A cargo of the industrial chains (T-208, M5). The order is stored in saves (stock per cargo,
/// trucks): **append-only**. `Goods` is the shop goods of M3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum CargoKind {
    Goods = 0,
    Crops = 1,
    Logs = 2,
    Ore = 3,
    Oil = 4,
    AnimalProducts = 5,
    Flour = 6,
    Planks = 7,
    Paper = 8,
    Metals = 9,
    Glass = 10,
    Petroleum = 11,
    Plastics = 12,
}

impl CargoKind {
    /// Every cargo in kind order.
    pub const ALL: [CargoKind; 13] = [
        CargoKind::Goods,
        CargoKind::Crops,
        CargoKind::Logs,
        CargoKind::Ore,
        CargoKind::Oil,
        CargoKind::AnimalProducts,
        CargoKind::Flour,
        CargoKind::Planks,
        CargoKind::Paper,
        CargoKind::Metals,
        CargoKind::Glass,
        CargoKind::Petroleum,
        CargoKind::Plastics,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }
}

/// What a building of an industrial chain does (T-208). Append-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum IndustryRole {
    /// The main building of an area: one per area, opens the others.
    Main = 0,
    /// Takes the raw resource from the ground.
    Extractor = 1,
    /// Turns raw cargo into products.
    Processor = 2,
    Warehouse = 3,
    /// A unique factory: any industrial area, products from several chains (T-215).
    Factory = 4,
}
