//! RON definition schemas. See `system-design.md` §6.1 (roads), §8 (buildings).
//!
//! Each `.ron` file holds exactly one [`ContentFile`], e.g. `Road((id: "basic_2l", ...))`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::kinds::{
    CargoKind, IndustryRole, ParkKind, ParkRole, PolicyKind, PolicyScope, ResourceKind,
    ServiceKind, ServiceSite, SpecKind, UtilityKind, ZoneType,
};

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
    /// Dense index of a building style, assigned in sorted-string-id order.
    StyleDefId
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
    /// Appended in T-111.
    Style(StyleDef),
    /// Global balance knobs (`balance.ron`); at most one file.
    Balance(Box<Balance>),
    /// Population milestones (`milestones.ron`); at most one file. Appended in T-149.
    Milestones(MilestonesDef),
    /// Appended in T-190.
    Policy(PolicyDef),
    /// Appended in T-196.
    Spec(SpecDef),
    /// Appended in T-208.
    Cargo(CargoDef),
}

/// A cargo of the industrial chains (T-208): how it is shown and what it is worth outside the
/// city. Prices are placeholders // tune later.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CargoDef {
    pub kind: CargoKind,
    pub name: String,
    /// Icon name (a placeholder until the cargo icons are drawn).
    pub icon: String,
    /// Colour of the cargo in overlays and tooltips, sRGB.
    pub color: [u8; 3],
    /// Money per unit when the city sells it to the outside.
    pub export_price: f32,
    /// Money per unit when the city buys it from the outside.
    pub import_price: f32,
}

/// What a building does in an industrial chain (T-208, design §16 M5 B). Amounts are per game
/// day at full staffing. Numbers: tune later.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndustrySpec {
    /// The resource of the area it stands in; `None` for a factory (any industrial area).
    pub resource: Option<ResourceKind>,
    pub role: IndustryRole,
    #[serde(default)]
    pub inputs: Vec<(CargoKind, f32)>,
    #[serde(default)]
    pub outputs: Vec<(CargoKind, f32)>,
    /// What the building can hold.
    #[serde(default)]
    pub storage: f32,
    /// The area level it needs (1 = any).
    #[serde(default = "one_level")]
    pub min_level: u8,
}

fn one_level() -> u8 {
    1
}

/// A district specialisation (T-196): the zone types it applies to and the numbers of its
/// effects. The buildings that belong to it are the `BuildingDef`s with `spec: Some(kind)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecDef {
    pub kind: SpecKind,
    pub name: String,
    /// Icon name (a placeholder until T-197 draws one per specialisation).
    pub icon: String,
    /// Zone types the district may specialise (`ZoneType::None` is not one).
    pub zones: Vec<ZoneType>,
    /// Multiplier of the tax of its buildings.
    pub tax_mul: f32,
    /// Weight of its buildings as a tourist's goal (an ordinary shop is 1).
    pub tourist_weight: f32,
    /// Noise its buildings add around them.
    pub noise_add: f32,
    /// Land value its buildings add around them.
    pub land_value_add: f32,
    /// Multiplier of the goods its buildings use.
    pub goods_mul: f32,
}

/// A policy (T-190): which one, where it may be on, what it costs and its one number.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDef {
    pub id: String,
    pub name: String,
    pub kind: PolicyKind,
    /// Icon name (a placeholder until T-193 draws one per policy).
    pub icon: String,
    pub scope: PolicyScope,
    /// Currency units per building under the policy per week; charged daily as a seventh.
    pub cost_per_building_week: f32,
    /// The one number of the effect; what it means depends on `kind`.
    pub value: f32,
}

/// The population milestones in ascending order. The first has population 0 and is where a new
/// game starts; the buildings a milestone `unlocks` cannot be placed before it is reached, and
/// `tiles` is how many more tiles the player may buy once it is (T-150).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MilestonesDef {
    pub milestones: Vec<MilestoneDef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MilestoneDef {
    /// Residents needed to reach it.
    pub population: u32,
    pub name: String,
    /// Ids of the building defs that become placeable.
    #[serde(default)]
    pub unlocks: Vec<String>,
    /// Zone types that can be painted from this milestone on (T-231).
    #[serde(default)]
    pub zones: Vec<ZoneType>,
    /// Tiles this milestone lets the player buy on top of the start tile.
    #[serde(default)]
    pub tiles: u8,
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
    /// Unemployed adults up to this share of the adults do not count against residential
    /// demand and move-in: a small town's commuters (BUG-057).
    pub demand_commuter_share: f32,
    /// ... but at least this many unemployed are always allowed.
    pub demand_commuter_min: f32,
    /// Weight of an unemployed manual worker in industrial demand.
    pub demand_unemployed_industry_weight: f32,
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
    /// Problem flags (`BuildingFlags` bits) that count towards abandonment; the others only
    /// show. Default: all but `NO_WORKERS` (T-145).
    pub decay_mask: u8,
    /// A workplace with fewer filled jobs than this share of its slots for `no_workers_days`
    /// game days gets `NO_WORKERS` (T-145).
    pub no_workers_ratio: f32,
    pub no_workers_days: f32,
    /// Weekly tax of one household living in a residential building of level 1..=5 at a 100%
    /// rate (T-128).
    pub tax_household: [f32; 5],
    /// Weekly tax of one filled job of education 0..=3 at a 100% rate (T-128).
    pub tax_job: [f32; 4],
    /// Multiplier of `tax_job` by building level 1..=3 (commercial, industrial, office).
    pub level_mul: [f32; 3],
    /// Loans the bank offers, by index (the index is what a save and a command name); at most
    /// 255, one active loan per offer (§16 M2).
    pub loan_offers: Vec<LoanOffer>,
    /// Price of a tile by its ring: the Chebyshev distance from the start tile, 1..=4 (T-150).
    /// // tune later
    pub tile_price: [f32; 4],
    /// The treasury of a new game (BUG-061). // tune later
    pub start_money: f32,
    /// Garbage a household makes per game day (T-136). // tune later
    pub garbage_household: f32,
    /// Garbage a filled job makes per game day. // tune later
    pub garbage_job: f32,
    /// Garbage piled up in a building from which it has the GARBAGE problem. // tune later
    pub garbage_problem: f32,
    /// Garbage a building holds before a garbage truck is sent for it (T-151). // tune later
    pub garbage_truck_min: f32,
    /// Chance per building and game day to catch fire with no fire cover (T-142). // tune later
    pub fire_base: f32,
    /// Share of `fire_base` a full fire coverage takes away.
    pub fire_cover: f32,
    /// Game hours an engine needs at the fire (after it has arrived) to put it out.
    pub fire_extinguish_hours: f32,
    /// Game hours an unattended fire takes to burn the building down (`Burned`).
    pub fire_burn_hours: f32,
    /// Game days a burned building stays before it is demolished.
    pub burned_days: f32,
    /// Chance per game hour that a burning building sets a neighbour on fire.
    pub fire_spread: f32,
    /// Reach of the spreading, metres between lot centres.
    pub fire_spread_m: f32,
    /// Speed of a fire engine on the road, km/h.
    pub fire_engine_kmh: f32,
    /// Crime a building makes per game day with no police cover on land worth 100 (T-144). // tune later
    pub crime_base: f32,
    /// Share of the crime a full police coverage takes away.
    pub police_cover: f32,
    /// Extra crime on cheap land: the rate is multiplied by `1 + crime_poor × (1 − land / 100)`.
    pub crime_poor: f32,
    /// Crime piled up in a building from which it has the CRIME problem.
    pub crime_problem: f32,
    /// Chance per game day to die by age: `(age in years, chance)` points, linear between them,
    /// 0 before the first and flat after the last (T-138). // tune later
    pub death_curve: Vec<(f32, f32)>,
    /// Game days of sickness after which the sick can die (T-138). // tune later
    pub sick_death_days: f32,
    /// Chance per game day to die once sick for `sick_death_days`.
    pub sick_death_chance: f32,
    /// Chance per game day that a household of two adults aged 20-40 with a free place has a
    /// child.
    pub birth_chance: f32,
    /// Chance per game day to fall ill at home with no pollution (T-139). // tune later
    pub sick_base: f32,
    /// Extra chance per game day per point (0..=100) of pollution at the home.
    pub sick_pollution: f32,
    /// Land value a full coverage of Health, Police, Fire and Education adds (T-146). // tune later
    pub land_service: [f32; 4],
    /// Land value a building with the GARBAGE / CRIME problem takes from its cell.
    pub land_garbage: f32,
    pub land_crime: f32,
    /// Services (of Health, Police, Fire, Education) covered at least `res_service_cover` a home
    /// of level 1..=5 needs to stand at that level.
    pub res_level_services: [u8; 5],
    pub res_service_cover: f32,
    /// `ServiceProblems` bits that count towards abandonment next to `decay_mask`: GARBAGE and
    /// DEAD by default (1 | 4).
    pub decay_service_mask: u16,
    /// Price of one bus stop, currency units (T-153). // tune later
    pub bus_stop_cost: f32,
    /// Seconds (movement clock) a bus stands at a stop (T-156). // tune later
    pub dwell_s: f32,
    /// Weekly upkeep of one bus that is out on a line, currency units. // tune later
    pub bus_upkeep_week: f32,
    /// Straight-line metres from a trip's ends within which a stop can be used (T-159).
    /// // tune later
    pub transit_walk_m: f32,
    /// Seconds added to the estimate of a ride for the bother of it (T-159). // tune later
    pub transit_penalty_s: f32,
    /// A car owner rides when `cost_transit < car_bias × cost_car` (T-159). // tune later
    pub car_bias: f32,
    /// Metres per second a car is assumed to make, for the choice of mode only (T-159).
    /// // tune later
    pub car_speed_est: f32,
    /// Seconds (movement clock) a rider waits at a stop before the trip goes on abstractly
    /// (T-159). // tune later
    pub max_wait_s: f32,
    /// Price of one boarding, currency units (T-159). // tune later
    pub transit_fare: f32,
    /// Weekly upkeep of one metro train that is out on a line, currency units (T-163).
    /// // tune later
    pub metro_upkeep_week: f32,
    /// Price of one boarding of a metro train, currency units (T-163). // tune later
    pub metro_fare: f32,
    /// Seconds (movement clock) a train needs to turn round at the end of its line (T-163).
    /// // tune later
    pub metro_turn_s: f32,
    /// Metres between a stop of one line and a stop of another within which a rider changes on
    /// foot (T-165). // tune later
    pub transfer_walk_m: f32,
    /// Seconds added to the estimate of a trip with a change, on top of `transit_penalty_s`
    /// (T-165). // tune later
    pub transfer_penalty_s: f32,
    /// Most lines considered at either end of a trip when looking for a change (T-165).
    /// // tune later
    pub transit_lines_cap: u8,
    /// Tourists (T-168): visits a day with no commercial building at all, and the visits a day
    /// each commercial building adds. // tune later
    pub tourism_base: f32,
    pub tourism_per_com: f32,
    /// Hours a tourist stays at the building (T-168). // tune later
    pub tourism_visit_h: f32,
    /// Money one visit brings, currency units (T-168). // tune later
    pub tourism_income: f32,
    /// Largest share of the vehicle pool tourists may hold at once (T-168). // tune later
    pub tourism_pool_share: f32,
    /// Metres of distance from the entry at which a building is half as likely a goal as one at
    /// the entry (T-168). // tune later
    pub tourism_reach_m: f32,
    /// HeavyTrafficBan (T-195): a truck's A* cost of a lane in a banned district is multiplied by
    /// this unless its start or goal is in that district. // tune later
    pub heavy_ban_cost_mul: f32,
    /// Most buildings a district rebuilds a day when its specialisation changes (T-196). // tune later
    pub respec_per_day: u32,
    /// Goods (T-169): units an industrial building makes a day per filled job, units a
    /// commercial building sells a day per filled job, the most a building stores, and the load
    /// of one truck. // tune later
    pub goods_per_worker: f32,
    pub goods_per_job: f32,
    pub goods_stock_cap: f32,
    pub goods_truck_load: f32,
    /// Metres within which a factory looks for a shop to supply (T-169). // tune later
    pub freight_radius_m: f32,
    /// Money one unit of exported goods brings, currency units, and the stock below which a shop
    /// with no factory in reach gets an import truck (T-170). // tune later
    pub export_price: f32,
    pub goods_import_threshold: f32,
    /// Days a shop may stand without stock before `NO_GOODS` (T-169). // tune later
    pub goods_empty_days: f32,
    /// Whether `NO_GOODS` counts towards abandonment (T-169); off by default.
    pub goods_decay: bool,
    /// Metres by which a block that is nearly whole cells is fitted to them: the cells of a new
    /// street are stretched or shrunk so a block 64 m deep (8 rows) or a side that is a whole
    /// number of cells long comes out as one clean grid (T-185). // tune later
    pub zoning_block_fit_m: f32,
    /// Intercity trains (T-167): hours between two trains from the outside, hours a train
    /// stands at the station, the share (percent) of arriving households that come by train when
    /// a station is joined to the railway, and the tourists a train brings. // tune later
    pub intercity_interval_h: f32,
    pub intercity_dwell_h: f32,
    pub move_in_by_train: u8,
    pub tourism_by_train: u8,
    /// Ore and oil in a 64 m resource cell at richness 1 (T-205); the start is
    /// `richness × deposit_per_cell`, rounded. // tune later
    pub deposit_per_cell: f32,
    /// Experience an area needs for levels 2 to 5 (T-209), and the share of storage each level
    /// above the first adds. // tune later
    pub industry_levels: [u32; 4],
    pub industry_storage_per_level: f32,
    /// A resource extractor needs this average richness (`0..=1`) under its lot (T-208). // tune later
    pub min_richness: f32,
    /// A park area's attractiveness grows by this share per level above the first (T-223).
    /// // tune later
    pub park_attract_per_level: f32,
    /// How far water, sewage and power reach (T-181).
    pub networks: Networks,
}

/// Distances of the pipe and power networks, metres (`balance.ron` `networks`, T-181). Tune later.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Networks {
    /// The largest gap between the footprint edges of two buildings over which one passes power
    /// on to the other.
    pub power_gap_m: f32,
    /// How far a building's footprint edge may be from a power line to be attached to it.
    pub line_reach_m: f32,
    /// The same for a pipe (water and sewage).
    pub pipe_reach_m: f32,
}

impl Networks {
    /// Upper bound of every distance: keeps the lookup grids small.
    pub const MAX_M: f32 = 400.0;

    /// The distances in declaration order, for validation and the content hash.
    pub fn knobs(&self) -> [f32; 3] {
        [self.power_gap_m, self.line_reach_m, self.pipe_reach_m]
    }
}

impl Default for Networks {
    fn default() -> Self {
        Self {
            power_gap_m: 64.0,
            line_reach_m: 80.0,
            pipe_reach_m: 64.0,
        }
    }
}

/// One loan the bank offers (`balance.ron` `loan_offers`). Interest is simple, for the whole
/// term: `amount × interest_pct / 100`, rounded once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoanOffer {
    /// Money paid out, in currency units.
    pub amount: f32,
    /// Interest for the whole term, percent of `amount`.
    pub interest_pct: f32,
    /// Term in weeks, paid in equal weekly parts.
    pub weeks: u16,
    /// Residents the city needs for the bank to offer it.
    pub min_population: u32,
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
            self.no_workers_ratio,
            self.no_workers_days,
        ];
        v.extend(self.level_res_land);
        v.extend(self.level_com_land);
        v.extend(self.level_ind_edu);
        v.extend(self.level_office_edu);
        v.extend(self.tax_household);
        v.extend(self.tax_job);
        v.extend(self.level_mul);
        v.extend(self.tile_price);
        v.push(self.start_money);
        v.extend([
            self.garbage_household,
            self.garbage_job,
            self.garbage_problem,
            self.garbage_truck_min,
            self.fire_base,
            self.fire_cover,
            self.fire_extinguish_hours,
            self.fire_burn_hours,
            self.burned_days,
            self.fire_spread,
            self.fire_spread_m,
            self.fire_engine_kmh,
            self.crime_base,
            self.police_cover,
            self.crime_poor,
            self.crime_problem,
            self.sick_death_days,
            self.sick_death_chance,
            self.birth_chance,
            self.sick_base,
            self.sick_pollution,
            self.land_garbage,
            self.land_crime,
            self.res_service_cover,
            self.bus_stop_cost,
            self.dwell_s,
            self.bus_upkeep_week,
            self.transit_walk_m,
            self.transit_penalty_s,
            self.car_bias,
            self.car_speed_est,
            self.max_wait_s,
            self.transit_fare,
            self.metro_upkeep_week,
            self.metro_fare,
            self.metro_turn_s,
            self.transfer_walk_m,
            self.transfer_penalty_s,
            f32::from(self.transit_lines_cap),
            self.tourism_base,
            self.tourism_per_com,
            self.tourism_visit_h,
            self.tourism_income,
            self.tourism_pool_share,
            self.tourism_reach_m,
            self.heavy_ban_cost_mul,
            self.respec_per_day as f32,
            self.goods_per_worker,
            self.goods_per_job,
            self.goods_stock_cap,
            self.goods_truck_load,
            self.freight_radius_m,
            self.export_price,
            self.goods_import_threshold,
            self.goods_empty_days,
            f32::from(u8::from(self.goods_decay)),
            self.zoning_block_fit_m,
            self.intercity_interval_h,
            self.intercity_dwell_h,
            f32::from(self.move_in_by_train),
            f32::from(self.tourism_by_train),
            self.deposit_per_cell,
            self.min_richness,
            self.park_attract_per_level,
            self.industry_storage_per_level,
        ]);
        v.extend(self.industry_levels.map(|l| l as f32));
        v.extend(self.land_service);
        v.extend(self.death_curve.iter().flat_map(|&(a, c)| [a, c]));
        v
    }
}

impl Default for Balance {
    fn default() -> Self {
        Self {
            walk_limit_m: 400.0,
            spawn_one_in: 4,
            demand_ema: 0.25,
            demand_base: [10.0, 5.0, 5.0, 0.0],
            demand_job_weight: [1.0, 1.0, 1.5, 2.0],
            demand_adults_per_home: 2.0,
            demand_shop_jobs_per_citizen: 0.2,
            demand_industry_jobs_per_shop_job: 0.5,
            demand_commuter_share: 0.25,
            demand_commuter_min: 20.0,
            demand_unemployed_industry_weight: 2.0,
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
            decay_mask: 0xFB,
            no_workers_ratio: 0.5,
            no_workers_days: 3.0,
            tax_household: [264.0, 336.0, 420.0, 516.0, 624.0],
            tax_job: [54.0, 81.0, 117.0, 162.0],
            level_mul: [1.0, 1.2, 1.45],
            loan_offers: default_loan_offers(),
            tile_price: [5000.0, 10000.0, 20000.0, 40000.0],
            start_money: 70000.0,
            garbage_household: 4.0,
            garbage_job: 1.0,
            garbage_problem: 40.0,
            garbage_truck_min: 20.0,
            fire_base: 0.002,
            fire_cover: 0.8,
            fire_extinguish_hours: 2.0,
            fire_burn_hours: 6.0,
            burned_days: 2.0,
            fire_spread: 0.15,
            fire_spread_m: 12.0,
            fire_engine_kmh: 50.0,
            crime_base: 20.0,
            police_cover: 0.9,
            crime_poor: 1.0,
            crime_problem: 100.0,
            death_curve: default_death_curve(),
            sick_death_days: 7.0,
            sick_death_chance: 0.1,
            birth_chance: 0.02,
            sick_base: 0.003,
            sick_pollution: 0.0002,
            land_service: [8.0, 6.0, 4.0, 8.0],
            land_garbage: 8.0,
            land_crime: 10.0,
            res_level_services: [0, 0, 1, 2, 4],
            res_service_cover: 0.3,
            decay_service_mask: 5,
            bus_stop_cost: 500.0,
            dwell_s: 10.0,
            bus_upkeep_week: 120.0,
            transit_walk_m: 600.0,
            transit_penalty_s: 120.0,
            car_bias: 1.0,
            car_speed_est: 9.0,
            max_wait_s: 900.0,
            transit_fare: 2.0,
            metro_upkeep_week: 400.0,
            metro_fare: 3.0,
            metro_turn_s: 30.0,
            transfer_walk_m: 150.0,
            transfer_penalty_s: 60.0,
            transit_lines_cap: 8,
            tourism_base: 4.0,
            tourism_per_com: 1.5,
            tourism_visit_h: 2.0,
            tourism_income: 12.0,
            tourism_pool_share: 0.1,
            tourism_reach_m: 1500.0,
            heavy_ban_cost_mul: 8.0,
            respec_per_day: 4,
            zoning_block_fit_m: 6.0,
            goods_per_worker: 0.5,
            goods_per_job: 0.4,
            goods_stock_cap: 100.0,
            goods_truck_load: 20.0,
            freight_radius_m: 1500.0,
            export_price: 10.0,
            goods_import_threshold: 5.0,
            goods_empty_days: 2.0,
            goods_decay: false,
            intercity_interval_h: 6.0,
            intercity_dwell_h: 0.5,
            move_in_by_train: 40,
            tourism_by_train: 8,
            deposit_per_cell: 20_000.0,
            min_richness: 0.3,
            park_attract_per_level: 0.1,
            industry_levels: [2_000, 8_000, 25_000, 60_000],
            industry_storage_per_level: 0.1,
            networks: Networks::default(),
        }
    }
}

/// The death curve of a content set without a `death_curve` entry. // tune later
pub fn default_death_curve() -> Vec<(f32, f32)> {
    vec![
        (60.0, 0.0),
        (70.0, 0.002),
        (80.0, 0.01),
        (90.0, 0.04),
        (100.0, 0.2),
    ]
}

/// The offers of a content set without a `loan_offers` entry. // tune later
pub fn default_loan_offers() -> Vec<LoanOffer> {
    [
        (25_000.0, 3.0, 26, 0),
        (60_000.0, 4.0, 52, 1000),
        (150_000.0, 5.0, 104, 5000),
    ]
    .into_iter()
    .map(|(amount, interest_pct, weeks, min_population)| LoanOffer {
        amount,
        interest_pct,
        weeks,
        min_population,
    })
    .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum LaneKind {
    Car = 0,
    Pedestrian = 1,
    Bike = 2,
    Tram = 3,
    Bus = 4,
    /// Metro and rail track (T-161): only trains use it.
    Rail = 5,
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
    /// Track for trains (T-161): Rail lanes only, a sub-network of its own.
    Rail,
    /// A pedestrian path (T-222): `Pedestrian` lanes only, no zoning, ground only; it may
    /// share a node with a road but never with a railway. Appended last.
    Path,
}

/// Underground version of a road type (T-161): its own price and mesh, and the depth below the
/// ground the player gets by default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelVariant {
    pub cost_per_m: f32,
    pub upkeep_per_m_week: f32,
    pub mesh: String,
    /// Default depth of the tunnel axis below the terrain, metres.
    pub depth: f32,
}

fn default_true() -> bool {
    true
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
    /// Street lamps on both sidewalks every this many metres (T-233, render only); 0 = none.
    #[serde(default)]
    pub lamp_spacing_m: f32,
    /// Decorative wooden utility poles on one side every this many metres (T-233, render
    /// only); 0 = none.
    #[serde(default)]
    pub pole_spacing_m: f32,
    /// Zebra crossings on this road's arms of a junction (T-240, render only).
    #[serde(default)]
    pub crosswalks: bool,
    /// Priority rank at junctions: higher wins.
    pub rank: u8,
    #[serde(default)]
    pub elevated: Option<RoadVariant>,
    #[serde(default)]
    pub bridge: Option<RoadVariant>,
    /// Underground version (T-161); `None` for a road that cannot be tunnelled.
    #[serde(default)]
    pub tunnel: Option<TunnelVariant>,
    /// Whether the plain (on the ground) version exists; `false` for a tunnel-only type.
    #[serde(default = "default_true")]
    pub ground: bool,
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
    Shed = 3,
    Mansard = 4,
}

/// Shape of the volume of a building inside its lot (`system-design.md` §8.2). Variants are
/// only ever appended. The renderer draws the ones it does not know yet as `Box`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Massing {
    Box = 0,
    LShape = 1,
    Stepped = 2,
    TowerOnPodium = 3,
    TwinBlocks = 4,
    Sawtooth = 5,
    PumpHouse = 6,
    Outflow = 7,
    CoalPlant = 8,
    WindTurbine = 9,
    WaterTower = 10,
    TreatmentPlant = 11,
    /// The M2 service silhouettes (T-147).
    Landfill = 12,
    Incinerator = 13,
    Clinic = 14,
    Hospital = 15,
    Cemetery = 16,
    Crematorium = 17,
    School = 18,
    HighSchool = 19,
    University = 20,
    FireStation = 21,
    PoliceStation = 22,
    /// The bus depot (T-158): a hangar with gates and a yard of parked buses.
    BusDepot = 23,
    /// The metro station (T-162).
    MetroStation = 24,
    /// The unique buildings and monuments (T-202, T-203).
    PlazaFountain = 25,
    ClockTower = 26,
    GrandLibrary = 27,
    OperaHouse = 28,
    ScienceMuseum = 29,
    Stadium = 30,
    TvTower = 31,
    Cathedral = 32,
    GoldenArch = 33,
    SkylineSpire = 34,
    /// The district specialisation buildings (T-198, T-199).
    SpecLeisure = 35,
    SpecTourism = 36,
    SpecOrganic = 37,
    SpecItCluster = 38,
    SpecSelfSufficient = 39,
    /// The M5 chain buildings (T-212, T-213): main, extractors, processors, warehouse.
    FarmMain = 40,
    FarmExtractorS = 41,
    FarmExtractorL = 42,
    FarmProcessorA = 43,
    FarmProcessorB = 44,
    FarmWarehouse = 45,
    ForestMain = 46,
    ForestExtractorS = 47,
    ForestExtractorL = 48,
    ForestProcessorA = 49,
    ForestProcessorB = 50,
    ForestWarehouse = 51,
    OreMain = 52,
    OreExtractorS = 53,
    OreExtractorL = 54,
    OreProcessorA = 55,
    OreProcessorB = 56,
    OreWarehouse = 57,
    OilMain = 58,
    OilExtractorS = 59,
    OilExtractorL = 60,
    OilProcessorA = 61,
    OilProcessorB = 62,
    OilWarehouse = 63,
    /// The city parks (T-221).
    ParkSmall = 64,
    ParkPlayground = 65,
    ParkPlaza = 66,
    ParkSports = 67,
}

/// Facade pattern family of a style.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Facade {
    Punched = 0,
    Ribbon = 1,
    Curtain = 2,
    Shopfront = 3,
    Industrial = 4,
}

/// Extra parts a style may carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Detail {
    Chimney = 0,
    Balconies = 1,
    Awning = 2,
    RooftopUnits = 3,
    Sign = 4,
    LoadingDock = 5,
    Tanks = 6,
    Fence = 7,
}

/// Linear RGB colour, `0..=1` per channel.
pub type Rgb = [f32; 3];

/// Colours of a style. `walls` and `roof` and `trim` are alternatives picked per building.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StylePalette {
    pub walls: Vec<Rgb>,
    pub roof: Vec<Rgb>,
    pub trim: Vec<Rgb>,
    pub glass: Rgb,
}

/// Look of a building type (`assets/content/styles/*.ron`, `system-design.md` §8.2). A
/// `BuildingDef` lists the styles it may use; which one a building gets is a hash of its
/// `BuildingId` (not sim state).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StyleDef {
    pub id: String,
    pub massing: Massing,
    pub roof: RoofKind,
    /// Floors, inclusive range.
    pub floors: (u8, u8),
    /// Share of the lot area the volume covers, inclusive range within `0..=1`.
    pub footprint_fill: (f32, f32),
    pub palette: StylePalette,
    pub facade: Facade,
    #[serde(default)]
    pub details: Vec<Detail>,
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
    /// Optional extra tax base on top of the household / job tax (T-128); 0 when omitted.
    #[serde(default)]
    pub tax_base: f32,
    /// Price of a service building placed by the player, currency units (zoned buildings
    /// spawn for free).
    #[serde(default)]
    pub cost: f32,
    /// Weekly upkeep of a service building at a 100% budget, currency units.
    #[serde(default)]
    pub upkeep_week: f32,
    /// Styles (ids of `StyleDef`) the look of a building of this type is chosen from; at
    /// least one.
    pub styles: Vec<String>,
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
    /// The platform a station lays under itself (T-162); `None` for every other building.
    #[serde(default)]
    pub track: Option<TrackSpec>,
    /// Set for a unique building or monument (T-200): one per city, unlocked by conditions.
    #[serde(default)]
    pub unique: Option<UniqueSpec>,
    /// Land value this building adds around it (T-200), shared by M4 to M6 buildings.
    #[serde(default)]
    pub land_effect: Option<LandEffect>,
    /// The district specialisation this building belongs to (T-196); `None` for every ordinary
    /// building. A specialised building is level 1 and never rises.
    #[serde(default)]
    pub spec: Option<SpecKind>,
    /// The role in an industrial chain (T-208); `None` for every other building.
    #[serde(default)]
    pub industry: Option<IndustrySpec>,
    /// The role in a park (T-223); `None` for every other building.
    #[serde(default)]
    pub park: Option<ParkSpec>,
}

/// What a building does in a park (T-223, design §16 M6 C). Numbers: tune later.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParkSpec {
    /// The kind of park area it belongs to; `None` for a city park (stands anywhere) and a prop
    /// (any park area).
    #[serde(default)]
    pub park: Option<ParkKind>,
    pub role: ParkRole,
    /// What it adds to the area's attractiveness.
    #[serde(default)]
    pub attract: f32,
    /// Visitors it lets in at once.
    #[serde(default)]
    pub capacity: u16,
    /// The area level it needs (0 = any).
    #[serde(default)]
    pub min_level: u8,
}

/// What makes a building unique (T-200, design §16 M4 C). Numbers: tune later.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UniqueSpec {
    /// A monument (its own section of the build menu) rather than a unique building.
    #[serde(default)]
    pub monument: bool,
    /// All of these must hold for the building to unlock; it stays unlocked after.
    #[serde(default)]
    pub unlock: Vec<UnlockCond>,
    /// Tourists a day it adds to the city's flow, and the weight it has as a goal.
    #[serde(default)]
    pub tourists_per_day: f32,
}

/// One condition of an unlock. Append-only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum UnlockCond {
    Population(u32),
    /// `n` buildings of a service kind stand (any state but demolished).
    ServiceBuildings {
        kind: ServiceKind,
        n: u16,
    },
    /// Tourist visits in the week in progress.
    TouristsWeek(u32),
    /// `n` buildings of a district specialisation (by its id, e.g. `it_cluster`). Counts 0
    /// until specialisations exist (T-196).
    SpecBuildings {
        spec: String,
        n: u16,
    },
    /// This many unique buildings (monuments included) have been built.
    UniquesBuilt(u8),
    /// This many districts exist.
    Districts(u8),
}

/// Land value around a building: `add` at its centre, falling off linearly to 0 at `radius_m`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandEffect {
    pub add: f32,
    pub radius_m: f32,
    /// Wellbeing of the residents inside the radius, `0..=1` at the centre, falling off linearly
    /// (T-237): fewer fall ill and more households move in. Parks only. // tune later
    #[serde(default)]
    pub green: f32,
    /// The district specialisation this building belongs to (T-196); `None` for every ordinary
    /// building. A specialised building is level 1 and never rises.
    #[serde(default)]
    pub spec: Option<SpecKind>,
}

/// The track under a station: `road` along the building's axis, centred on it, in a tunnel
/// `depth` metres below the ground (T-162); `depth: 0` lays it on the ground (the train
/// station, T-166).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackSpec {
    /// Id of a `RoadDef` of class `Rail` (with a tunnel variant when `depth` is above 0).
    pub road: String,
    pub length_m: f32,
    pub depth: f32,
    /// A platform on the ground stands behind the building, not under it (BUG-046): its axis
    /// is this many metres behind the building's back edge, away from the street. `None`: the
    /// platform runs through the building's centre (the metro's, which is underground).
    #[serde(default)]
    pub behind_m: Option<f32>,
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

fn road_access_default() -> bool {
    true
}

/// A service building's definition; M2 fields live in `cp_service`.
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
    /// The building needs a road (a lot in a zone block and an access point). `false` makes it
    /// free-standing (§8.1): placed anywhere on land, no jobs or households.
    #[serde(default = "road_access_default")]
    pub road_access: bool,
    /// Vehicle ids (`VehicleDef`) the service dispatches.
    #[serde(default)]
    pub vehicles: Vec<String>,
    /// What the building can hold (garbage, bodies); 0 = not a store (T-134). // tune later
    #[serde(default)]
    pub storage: u32,
    /// Education tier 1..=3 (elementary, high school, university); 0 for every other kind.
    #[serde(default)]
    pub tier: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum VehicleKind {
    Car = 0,
    Truck = 1,
    Bus = 2,
    Emergency = 3,
    /// A metro train (T-163).
    Metro = 4,
    /// A goods truck (T-169b): its own spec and mesh, apart from the garbage `Truck`.
    Cargo = 5,
    /// An intercity train (T-167): rides the railway from the map edge to a station.
    Train = 6,
    /// Farm cargo truck (T-213b): crops, animal products, flour.
    CargoFarm = 7,
    /// Log trailer: logs, planks, paper.
    CargoLogs = 8,
    /// Ore tipper: ore, metals, glass.
    CargoOre = 9,
    /// Tanker: oil, petroleum, plastics.
    CargoTanker = 10,
}

impl VehicleKind {
    /// Every kind in number order: the tables indexed by `kind as usize` (`cp_traffic`'s specs,
    /// the render's looks) have this many entries. The match in [`Self::number`] has no
    /// wildcard, so a new variant does not compile until it is added here.
    pub const ALL: [VehicleKind; 11] = [
        Self::Car,
        Self::Truck,
        Self::Bus,
        Self::Emergency,
        Self::Metro,
        Self::Cargo,
        Self::Train,
        Self::CargoFarm,
        Self::CargoLogs,
        Self::CargoOre,
        Self::CargoTanker,
    ];
    pub const COUNT: usize = Self::ALL.len();

    /// The truck that hauls a cargo (T-213b): the shop goods ride the plain `Cargo` truck, each
    /// chain its own.
    pub const fn for_cargo(cargo: crate::CargoKind) -> VehicleKind {
        use crate::CargoKind as C;
        match cargo {
            C::Goods => Self::Cargo,
            C::Crops | C::AnimalProducts | C::Flour => Self::CargoFarm,
            C::Logs | C::Planks | C::Paper => Self::CargoLogs,
            C::Ore | C::Metals | C::Glass => Self::CargoOre,
            C::Oil | C::Petroleum | C::Plastics => Self::CargoTanker,
        }
    }

    const fn number(self) -> usize {
        match self {
            Self::Car => 0,
            Self::Truck => 1,
            Self::Bus => 2,
            Self::Emergency => 3,
            Self::Metro => 4,
            Self::Cargo => 5,
            Self::Train => 6,
            Self::CargoFarm => 7,
            Self::CargoLogs => 8,
            Self::CargoOre => 9,
            Self::CargoTanker => 10,
        }
    }
}

// `ALL` lists the kinds in number order, and `kind as usize` is that number.
const _: () = {
    let mut i = 0;
    while i < VehicleKind::COUNT {
        assert!(VehicleKind::ALL[i].number() == i);
        assert!(VehicleKind::ALL[i] as usize == i);
        i += 1;
    }
};

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
    /// Passengers it carries (buses, trains); 0 for the rest (T-156).
    #[serde(default)]
    pub capacity: u16,
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
