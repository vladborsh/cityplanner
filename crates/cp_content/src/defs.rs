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
    /// Pollution an active industrial building makes per level, at its own cell (`0..=100`
    /// scale), fading linearly to nothing at `pollution_radius_m` (§11).
    pub pollution_per_level: f32,
    pub pollution_radius_m: f32,
    /// Noise of a road at full load, fading linearly to nothing at `noise_radius_m`.
    pub noise_per_load: f32,
    pub noise_radius_m: f32,
    /// Land value (`0..=100`): a base, plus up to `land_access` for a road within reach, plus
    /// `land_water` / `land_power` for a pipe / power line within `land_utility_reach_m`,
    /// minus pollution and noise times their weights.
    pub land_base: f32,
    pub land_access: f32,
    pub land_water: f32,
    pub land_power: f32,
    pub land_utility_reach_m: f32,
    pub land_pollution_weight: f32,
    pub land_noise_weight: f32,
    /// Land value a residential building of level 1..=5 needs.
    pub level_res_land: [f32; 5],
    /// Land value a commercial building of level 1..=3 needs.
    pub level_com_land: [f32; 3],
    /// Average education of the workers an industrial / office building of level 1..=3 needs.
    pub level_ind_edu: [f32; 3],
    pub level_office_edu: [f32; 3],
    /// Level checks (one per 256 ticks) in a row that move a building one level.
    pub level_checks: u32,
    /// Game days of an unbroken problem (no water, power, sewage, road or path) until a
    /// building is abandoned, and until it is demolished (counted from the first problem).
    pub abandon_days: f32,
    pub demolish_days: f32,
}

impl Balance {
    /// The land value, pollution, noise, level and decay knobs (T-042) that are plain floats,
    /// in declaration order: for validation and the content hash.
    pub fn land_knobs(&self) -> Vec<f32> {
        let mut v = vec![
            self.pollution_per_level,
            self.pollution_radius_m,
            self.noise_per_load,
            self.noise_radius_m,
            self.land_base,
            self.land_access,
            self.land_water,
            self.land_power,
            self.land_utility_reach_m,
            self.land_pollution_weight,
            self.land_noise_weight,
            self.abandon_days,
            self.demolish_days,
        ];
        v.extend(self.level_res_land);
        v.extend(self.level_com_land);
        v.extend(self.level_ind_edu);
        v.extend(self.level_office_edu);
        v
    }
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
            pollution_per_level: 40.0,
            pollution_radius_m: 150.0,
            noise_per_load: 60.0,
            noise_radius_m: 96.0,
            land_base: 20.0,
            land_access: 30.0,
            land_water: 10.0,
            land_power: 10.0,
            land_utility_reach_m: 64.0,
            land_pollution_weight: 0.6,
            land_noise_weight: 0.4,
            level_res_land: [0.0, 30.0, 45.0, 55.0, 65.0],
            level_com_land: [0.0, 40.0, 55.0],
            level_ind_edu: [0.0, 0.7, 1.5],
            level_office_edu: [0.0, 1.5, 2.3],
            level_checks: 3,
            abandon_days: 7.0,
            demolish_days: 14.0,
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

/// The role of a road type in the network (§6.2): how the player and the UI group it.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum RoadClass {
    #[default]
    Local,
    Collector,
    Arterial,
    Highway,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoadDef {
    pub id: String,
    pub name: String,
    /// Role of the road type; `Local` when absent from the RON.
    #[serde(default)]
    pub class: RoadClass,
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

impl RoadDef {
    /// Every car lane is `Forward`: traffic runs one way, along the segment. A road without
    /// car lanes is not one-way.
    pub fn is_one_way(&self) -> bool {
        let mut cars = self
            .lanes
            .iter()
            .filter(|l| l.kind == LaneKind::Car)
            .peekable();
        cars.peek().is_some() && cars.all(|l| l.dir == LaneDir::Forward)
    }
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
    /// Name of the UI icon (`assets/ui/icons/<icon>.png`); empty in the RON means the def's
    /// `id` (filled in when the content is loaded).
    #[serde(default)]
    pub icon: String,
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
    /// Name of the UI icon (`assets/ui/icons/<icon>.png`); empty in the RON means the def's
    /// `id` (filled in when the content is loaded).
    #[serde(default)]
    pub icon: String,
}
