//! RON definition schemas. See `system-design.md` §6.1 (roads), §8 (buildings).
//!
//! Each `.ron` file holds exactly one [`ContentFile`], e.g. `Road((id: "basic_2l", ...))`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::kinds::{ServiceKind, ServiceSite, UtilityKind, ZoneType};

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
def_id!(
    /// Dense index of a utility (pipe / power line) type, assigned in sorted-string-id order.
    UtilityDefId
);

/// Contents of one `.ron` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ContentFile {
    Road(RoadDef),
    Building(BuildingDef),
    Service(ServiceDef),
    Vehicle(VehicleDef),
    /// Appended in T-037.
    Utility(UtilityDef),
    /// Global balance knobs (`balance.ron`); at most one file.
    Balance(Balance),
}

/// Global balance knobs the Balance Analyst may tune (`assets/content/balance.ron`). Every
/// field is optional in the file; a missing one keeps its default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Balance {
    /// Trips with a straight line shorter than this (metres) are walked (§9.2). On the
    /// movement clock (§2) 1 km on foot is about 2 game hours.
    pub walk_limit_m: f32,
    /// A spawn attempt on a zone block proceeds with probability `1 / spawn_one_in` (§8).
    pub spawn_one_in: u32,
    /// Share of the gap a demand value closes per recomputation (every 64 ticks), `0..=1` (§11).
    pub demand_ema: f32,
    /// Constant added to the raw demand of residential, commercial, industrial and office
    /// (R, C, I, O): what a city with nothing in it wants.
    pub demand_base: [f32; 4],
    /// Weight of a free job slot of education level 0..=3 in residential demand.
    pub demand_job_weight: [f32; 4],
    /// Adults a vacant home is expected to bring: it counts against residential demand.
    pub demand_adults_per_home: f32,
    /// Commercial job slots one resident supports.
    pub demand_shop_jobs_per_citizen: f32,
    /// Industrial job slots one commercial job slot needs goods from.
    pub demand_industry_jobs_per_shop_job: f32,
}

impl Default for Balance {
    fn default() -> Self {
        Self {
            walk_limit_m: 1000.0,
            spawn_one_in: 4,
            demand_ema: 0.25,
            demand_base: [10.0, 5.0, 5.0, 0.0],
            demand_job_weight: [1.0, 1.0, 1.5, 2.0],
            demand_adults_per_home: 2.0,
            demand_shop_jobs_per_citizen: 0.2,
            demand_industry_jobs_per_shop_job: 0.5,
        }
    }
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
    /// Price of a service building placed by the player, currency units (zoned buildings
    /// spawn for free).
    #[serde(default)]
    pub cost: f32,
    /// Weekly upkeep of a service building at a 100% budget, currency units.
    #[serde(default)]
    pub upkeep_week: f32,
    pub style: BuildingStyle,
    /// Service this building provides (id of a `ServiceDef`), for service buildings.
    #[serde(default)]
    pub service: Option<String>,
    /// Construction time in game hours (§8). `0` (absent in the RON) means
    /// [`default_build_hours`] of the lot.
    #[serde(default)]
    pub build_hours: f32,
}

impl BuildingDef {
    /// Construction time in game hours: `build_hours`, or the lot-area default when unset.
    pub fn build_hours(&self) -> f32 {
        if self.build_hours > 0.0 {
            self.build_hours
        } else {
            default_build_hours(self.lot)
        }
    }
}

/// Default construction time in game hours by lot area in cells: 1 → 2 h, ≤ 4 → 3 h,
/// ≤ 9 → 4 h, larger → 6 h.
pub fn default_build_hours(lot: (u8, u8)) -> f32 {
    match u16::from(lot.0) * u16::from(lot.1) {
        0..=1 => 2.0,
        2..=4 => 3.0,
        5..=9 => 4.0,
        _ => 6.0,
    }
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
    /// Supply per game day for utility services (same unit as `BuildingDef.consumption`).
    pub capacity: u32,
    /// Where it may stand.
    #[serde(default)]
    pub site: ServiceSite,
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
    /// Braking, m/s² (§9.4).
    #[serde(default = "default_decel")]
    pub decel: f32,
    /// Standstill bumper-to-bumper gap to the car ahead, metres.
    #[serde(default = "default_min_gap")]
    pub min_gap: f32,
    pub mesh: String,
}

fn default_decel() -> f32 {
    4.0
}

fn default_min_gap() -> f32 {
    2.0
}

/// A utility link type: pipes (water, sewage) and power lines (`system-design.md` §10).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UtilityDef {
    pub id: String,
    pub name: String,
    /// Which network this link carries. Water and sewage share one pipe network.
    pub kind: UtilityKind,
    pub cost_per_m: f32,
    pub upkeep_per_m_week: f32,
    /// Shortest link, metres.
    pub min_link_m: f32,
    /// Longest link, metres; longer legs of a path are cut into equal pieces.
    pub max_link_m: f32,
    pub mesh: String,
}
