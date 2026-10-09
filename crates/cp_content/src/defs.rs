//! RON definition schemas. See `system-design.md` §6.1 (roads), §8 (buildings).
//!
//! Each `.ron` file holds exactly one [`ContentFile`], e.g. `Road((id: "basic_2l", ...))`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::kinds::{ServiceKind, UtilityKind, ZoneType};

macro_rules! def_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(pub u32);
        impl $name {
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

def_id!(
    /// Dense index of a road type, assigned in sorted-string-id order.
    RoadDefId
);
def_id!(
    /// Dense index of a building type, assigned in sorted-string-id order.
    BuildingDefId
);
def_id!(
    /// Dense index of a service type, assigned in sorted-string-id order.
    ServiceDefId
);
def_id!(
    /// Dense index of a vehicle type, assigned in sorted-string-id order.
    VehicleDefId
);

/// Contents of one `.ron` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ContentFile {
    Road(RoadDef),
    Building(BuildingDef),
    Service(ServiceDef),
    Vehicle(VehicleDef),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum LaneKind {
    Car = 0,
    Pedestrian = 1,
    Bike = 2,
    Tram = 3,
    Bus = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum LaneDir {
    Forward = 0,
    Backward = 1,
}

/// One lane of a road profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneDef {
    pub kind: LaneKind,
    pub dir: LaneDir,
    pub width: f32,
    /// Lateral centre offset from the road axis in metres; positive is left of travel.
    pub offset: f32,
}

/// Elevated or bridge version of a road: extra cost and its own mesh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoadVariant {
    pub cost_per_m: f32,
    pub upkeep_per_m_week: f32,
    pub mesh: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoadDef {
    pub id: String,
    pub name: String,
    /// Total profile width, metres.
    pub width: f32,
    pub lanes: Vec<LaneDef>,
    /// Width of a median between directions (0 when none), metres.
    #[serde(default)]
    pub median_width: f32,
    pub speed_kmh: f32,
    /// Steepest allowed grade (rise / run), e.g. `0.12` for streets, `0.06` for highways.
    pub max_slope: f32,
    pub cost_per_m: f32,
    pub upkeep_per_m_week: f32,
    pub allows_zoning: bool,
    /// Priority rank at junctions: higher wins.
    pub rank: u8,
    #[serde(default)]
    pub elevated: Option<RoadVariant>,
    #[serde(default)]
    pub bridge: Option<RoadVariant>,
    pub mesh: String,
    pub material: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum RoofKind {
    Flat = 0,
    Gabled = 1,
    Hipped = 2,
}

/// Procedural look of a building.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingStyle {
    pub floors: u8,
    pub roof: RoofKind,
    pub palette: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingDef {
    pub id: String,
    pub name: String,
    pub zone: ZoneType,
    pub level: u8,
    /// Lot size in cells (x, z).
    pub lot: (u8, u8),
    #[serde(default)]
    pub households: u16,
    /// Jobs per education level 0..=3.
    #[serde(default)]
    pub jobs: [u16; 4],
    /// Consumption per game day by utility.
    #[serde(default)]
    pub consumption: BTreeMap<UtilityKind, f32>,
    pub tax_base: f32,
    pub style: BuildingStyle,
    /// Service this building provides (id of a `ServiceDef`), for service buildings.
    #[serde(default)]
    pub service: Option<String>,
}

/// M1 stub; fields grow in M2 (`cp_service`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceDef {
    pub id: String,
    pub name: String,
    pub kind: ServiceKind,
    /// Coverage radius along roads, metres.
    pub radius_m: f32,
    pub capacity: u32,
    /// Vehicle ids (`VehicleDef`) the service dispatches.
    #[serde(default)]
    pub vehicles: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum VehicleKind {
    Car = 0,
    Truck = 1,
    Bus = 2,
    Emergency = 3,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VehicleDef {
    pub id: String,
    pub name: String,
    pub kind: VehicleKind,
    pub length: f32,
    pub width: f32,
    pub max_speed_kmh: f32,
    /// Acceleration, m/s².
    pub accel: f32,
    pub mesh: String,
}
