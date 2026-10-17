//! Frozen per-version save bodies and the `migrate_vN_to_vN1` chain.
//!
//! A `SaveDataV<N>` type is a snapshot of `SaveData` as it was at version `N` and must
//! never change (its postcard encoding is what old files contain; `SimCommand` and the
//! domain enums are append-only for the same reason).

use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Demand,
    Footprint, Lot,
};
use cp_citizen::{Citizen, CitizenGrid, Household, Location, TripBook};
use cp_citizen::{CitizenId, TripId};
use cp_core::{Arena, Cost};
use cp_core::{CommandLog, CommandQueue, SimRng, SimSpeed, Tick, Treasury};
use cp_district::{CityPolicies, District, DistrictMap, Districts};
use cp_economy::{Budgets, Decay, DemandAcc, EconomyState, LandGrids, TaxRates, WeekReport};
use cp_net::NetGraph;
use cp_sim::SimCommand;
use cp_terrain::TileOwnership;
use cp_traffic::CarPath;
use cp_traffic::{AgentStats, Agents, Pedestrian, SavedPaths, SegmentLoad, Vehicle};
use cp_utility::UtilityGraph;
use serde::{Deserialize, Serialize};

use crate::{SaveData, SaveError};

/// Version 0: before `SimSpeed` was saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV0 {
    pub tick: Tick,
    pub rng: SimRng,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
}

/// Versions 1 and 2 (they share one body; v2 only added the header CRC): before the road
/// network (`NetGraph`) was saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV2 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
}

/// v0 → v1: speed was not saved; resume at ×1, the game default.
pub fn migrate_v0_to_v1(v0: SaveDataV0) -> SaveDataV2 {
    SaveDataV2 {
        tick: v0.tick,
        rng: v0.rng,
        speed: SimSpeed::X1,
        queue: v0.queue,
        log: v0.log,
        tiles: v0.tiles,
    }
}

/// v2 → v3: no network was saved; start with an empty one.
pub fn migrate_v2_to_v3(v2: SaveDataV2) -> SaveDataV3 {
    SaveDataV3 {
        tick: v2.tick,
        rng: v2.rng,
        speed: v2.speed,
        queue: v2.queue,
        log: v2.log,
        tiles: v2.tiles,
        net: NetGraph::default(),
    }
}

/// Version 3: the body of `SaveData` as written by format 3 (body-only CRC, no map hash).
/// Version 4 changed only the header, so the bodies are identical.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV3 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
}

/// Version 4: the body as written by format 4 (before zone state).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV4 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
}

/// Version 5: the body as written by format 5 (zone state, before buildings).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV5 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
}

/// Version 6: the body as written by format 6 (buildings, before citizens).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV6 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGridV12,
}

/// Version 7: the body as written by format 7 (citizens, before pending path requests).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV7 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGridV12,
    pub citizens: CitizenGridV21,
}

/// Version 8: the body as written by format 8 (pending path requests, before utilities).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV8 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGridV12,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
}

/// Version 9: the body as written by format 9 (utility networks, before trips).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV9 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGridV12,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
}

/// Version 10: the body as written by format 10 (trips, before agents).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV10 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGridV12,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
}

/// Counters of the agent pools as written by format 11.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatsV11 {
    pub abstract_trips: u64,
    pub walked: u64,
    pub driven: u64,
    pub no_path: u64,
    pub invalid_routes: u64,
    pub red_waits: u64,
}

/// A vehicle as written by format 11: placeholder motion (an arrival tick, no lane state).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VehicleV11 {
    pub trip: TripId,
    pub citizen: CitizenId,
    pub path: CarPath,
    pub arrive_at: Tick,
    pub stuck_timer: u32,
}

/// The agent pools as written by format 11.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentsV11 {
    pub pedestrians: Arena<Pedestrian>,
    pub vehicles: Arena<VehicleV11>,
    pub stats: AgentStatsV11,
}

/// Counters of the agent pools as written by formats 12–16 (before the re-route counters).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatsV16 {
    pub abstract_trips: u64,
    pub walked: u64,
    pub driven: u64,
    pub no_path: u64,
    pub invalid_routes: u64,
    pub red_waits: u64,
    pub despawned_stuck: u64,
    pub invalid_vehicles: u64,
}

/// A vehicle as written by formats 12–16 (before the re-route request id).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VehicleV16 {
    pub trip: TripId,
    pub citizen: CitizenId,
    pub path: CarPath,
    pub kind: u8,
    pub hop: u16,
    pub in_conn: bool,
    pub spawned: bool,
    pub s: f32,
    pub speed: f32,
    pub stuck_timer: u32,
}

/// The agent pools as written by formats 12–16.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentsV16 {
    pub pedestrians: Arena<Pedestrian>,
    pub vehicles: Arena<VehicleV16>,
    pub stats: AgentStatsV16,
    pub load: SegmentLoad,
}

impl From<&Agents> for AgentsV16 {
    /// The pools as an old writer would have saved them (tests of the frozen writers).
    fn from(a: &Agents) -> Self {
        let vehicles = a.vehicle_arena().map(|v| VehicleV16 {
            trip: v.trip,
            citizen: v.citizen,
            path: v.path.clone(),
            kind: v.kind,
            hop: v.hop,
            in_conn: v.in_conn,
            spawned: v.spawned,
            s: v.s,
            speed: v.speed,
            stuck_timer: v.stuck_timer,
        });
        let s = &a.stats;
        Self {
            pedestrians: a.pedestrian_arena().clone(),
            vehicles,
            stats: AgentStatsV16 {
                abstract_trips: s.abstract_trips,
                walked: s.walked,
                driven: s.driven,
                no_path: s.no_path,
                invalid_routes: s.invalid_routes,
                red_waits: s.red_waits,
                despawned_stuck: s.despawned_stuck,
                invalid_vehicles: s.invalid_vehicles,
            },
            load: a.load.clone(),
        }
    }
}

/// Version 11: the body as written by format 11 (agents with placeholder vehicle motion).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV11 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGridV12,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
    pub agents: AgentsV11,
}

/// The building arena as written by formats 1–12 (before the removed-ids list).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BuildingGridV12 {
    pub buildings: Arena<BuildingV18>,
}

impl From<&BuildingGrid> for BuildingGridV12 {
    fn from(g: &BuildingGrid) -> Self {
        Self {
            buildings: g.arena().map(|b| BuildingV18::from(b)),
        }
    }
}

/// A building as formats 1–18 wrote it: always on a lot, always with an access point (T-106
/// made both optional: `footprint` and `access`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuildingV18 {
    pub def: cp_content::BuildingDefId,
    pub lot: Lot,
    pub level: u8,
    pub state: BuildingState,
    pub ready_at: Tick,
    pub pos: bevy_math::Vec2,
    pub rot: f32,
    pub access: AccessPoint,
    pub flags: BuildingFlags,
    pub level_progress: f32,
}

impl From<&Building> for BuildingV18 {
    /// For the legacy writers and the tests: a free-standing building has no v18 form.
    fn from(b: &Building) -> Self {
        Self {
            def: b.def,
            lot: *b
                .lot()
                .expect("formats before 19 only know buildings on lots"),
            level: b.level,
            state: b.state,
            ready_at: b.ready_at,
            pos: b.pos,
            rot: b.rot,
            access: b
                .access()
                .expect("formats before 19 only know buildings with an access point"),
            flags: b.flags,
            level_progress: b.level_progress,
        }
    }
}

impl From<&BuildingV18> for Building {
    /// v18 → v19: a lot stays a lot, the access point stays.
    fn from(b: &BuildingV18) -> Self {
        Self {
            def: b.def,
            footprint: Footprint::Lot(b.lot),
            level: b.level,
            state: b.state,
            ready_at: b.ready_at,
            pos: b.pos,
            rot: b.rot,
            access: Some(b.access),
            flags: b.flags,
            level_progress: b.level_progress,
        }
    }
}

/// The building grid as formats 13–18 wrote it: the arena and the removed-ids list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BuildingGridV18 {
    pub buildings: Arena<BuildingV18>,
    pub removed: Vec<BuildingId>,
}

impl From<&BuildingGrid> for BuildingGridV18 {
    fn from(g: &BuildingGrid) -> Self {
        Self {
            buildings: g.arena().map(|b| BuildingV18::from(b)),
            removed: g.removed().to_vec(),
        }
    }
}

/// Version 12: the body as written by format 12 (vehicle lane state; buildings without the
/// removed-ids list).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV12 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGridV12,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
    pub agents: AgentsV16,
}

/// Version 13: the body as written by format 13 (removed-building ids; no treasury).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV13 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGridV18,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
    pub agents: AgentsV16,
}

/// Version 14: the body as written by format 14 (treasury and economy settings; no demand).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV14 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGridV18,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
    pub agents: AgentsV16,
    pub treasury: Treasury,
    pub economy: EconomyStateV19,
}

/// Version 15: the body as written by format 15 (demand; no land grids or decay timers).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV15 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGridV18,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
    pub agents: AgentsV16,
    pub treasury: Treasury,
    pub economy: EconomyStateV19,
    pub demand: Demand,
    pub demand_acc: DemandAcc,
}

/// Version 16: the body as written by format 16 (land grids and decay timers; vehicles without
/// the re-route request id).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV16 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGridV18,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
    pub agents: AgentsV16,
    pub treasury: Treasury,
    pub economy: EconomyStateV19,
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    pub land: LandGrids,
    pub decay: Decay,
}

/// Version 17: the body as written by format 17 (re-route ids; no definition names).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV17 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGridV18,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
    pub agents: Agents,
    pub treasury: Treasury,
    pub economy: EconomyStateV19,
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    pub land: LandGrids,
    pub decay: Decay,
}

/// Version 18: the body as written by format 18 (definition names; buildings always on a lot
/// with an access point).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV18 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGridV18,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
    pub agents: Agents,
    pub treasury: Treasury,
    pub economy: EconomyStateV19,
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    pub land: LandGrids,
    pub decay: Decay,
    pub def_names: crate::DefNames,
}

/// The last weekly report as formats 14 to 19 wrote it (before `loan_payments`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeekReportV19 {
    pub week: u64,
    pub income_by_zone: [Cost; 6],
    pub road_upkeep: Cost,
    pub building_upkeep: Cost,
    pub utility_upkeep: Cost,
}

/// The economy state as formats 14 to 19 wrote it (before the loans).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EconomyStateV19 {
    pub taxes: TaxRates,
    pub budgets: Budgets,
    pub last_week: WeekReportV19,
}

impl From<&EconomyState> for EconomyStateV19 {
    /// Drops the loans: for the legacy writers and the tests only.
    fn from(e: &EconomyState) -> Self {
        let w = &e.last_week;
        Self {
            taxes: e.taxes,
            budgets: e.budgets,
            last_week: WeekReportV19 {
                week: w.week,
                income_by_zone: w.income_by_zone,
                road_upkeep: w.road_upkeep,
                building_upkeep: w.building_upkeep,
                utility_upkeep: w.utility_upkeep,
            },
        }
    }
}

/// Version 20: the body as written by format 20 (loans; no districts).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV20 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGrid,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
    pub agents: Agents,
    pub treasury: Treasury,
    pub economy: EconomyStateV31,
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    pub land: LandGrids,
    pub decay: Decay,
    pub def_names: crate::DefNames,
}

/// A citizen as formats up to 21 wrote it (before `sick_since` and `school`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CitizenV21 {
    pub age_days: u32,
    pub education: u8,
    pub household: cp_citizen::HouseholdId,
    pub home: Option<cp_building::BuildingId>,
    pub work: Option<cp_building::BuildingId>,
    pub work_level: u8,
    pub location: Location,
    pub wellbeing: u8,
    pub has_car: bool,
}

/// The citizen grid as formats up to 21 wrote it (before the `dead` table).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CitizenGridV21 {
    citizens: Arena<CitizenV21>,
    households: Arena<Household>,
}

impl From<&CitizenGrid> for CitizenGridV21 {
    /// Drops the T-135 fields: for the legacy writers and the tests only.
    fn from(g: &CitizenGrid) -> Self {
        let (citizens, households) = g.parts();
        Self {
            citizens: citizens.map(|c| CitizenV21 {
                age_days: c.age_days,
                education: c.education,
                household: c.household,
                home: c.home,
                work: c.work,
                work_level: c.work_level,
                location: c.location,
                wellbeing: c.wellbeing,
                has_car: c.has_car,
            }),
            households: households.clone(),
        }
    }
}

/// Version 22: the body as written by format 22 (service state; no worker-shortage timers).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV22 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::legacy::ZoneGridV34,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV31,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`DefNames`] and [`SaveData::remap_defs`].
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22.
    pub service_state: ServiceStateV23,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
}

/// v22 → v23 (T-145): the `NO_WORKERS` timers were added; an old save starts with none.
pub fn migrate_v22_to_v23(v22: SaveDataV22) -> SaveDataV23 {
    SaveDataV23 {
        tick: v22.tick,
        rng: v22.rng,
        speed: v22.speed,
        queue: v22.queue,
        log: v22.log,
        tiles: v22.tiles,
        net: v22.net,
        zones: v22.zones,
        buildings: v22.buildings,
        citizens: v22.citizens,
        paths: v22.paths,
        utilities: v22.utilities,
        trips: v22.trips,
        agents: v22.agents,
        treasury: v22.treasury,
        economy: v22.economy,
        demand: v22.demand,
        demand_acc: v22.demand_acc,
        land: v22.land,
        decay: v22.decay,
        def_names: v22.def_names,
        district_map: v22.district_map,
        districts: v22.districts,
        service_state: v22.service_state,
        coverage: v22.coverage,
        shortage: Default::default(),
    }
}

/// Version 21: the body as written by format 21 (districts; no service state).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV21 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGrid,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
    pub agents: Agents,
    pub treasury: Treasury,
    pub economy: EconomyStateV31,
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    pub land: LandGrids,
    pub decay: Decay,
    pub def_names: crate::DefNames,
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
}

/// v21 → v22 (T-135): citizens got `sick_since` and `school`, the citizen grid its `dead`
/// table, and the service state and the saved coverage were added; all start empty.
pub fn migrate_v21_to_v22(v21: SaveDataV21) -> SaveDataV22 {
    let (citizens, households) = (v21.citizens.citizens, v21.citizens.households);
    SaveDataV22 {
        tick: v21.tick,
        rng: v21.rng,
        speed: v21.speed,
        queue: v21.queue,
        log: v21.log,
        tiles: v21.tiles,
        net: v21.net,
        zones: v21.zones,
        buildings: v21.buildings,
        citizens: CitizenGrid::from_parts(
            citizens.map(|c| Citizen {
                age_days: c.age_days,
                education: c.education,
                household: c.household,
                home: c.home,
                work: c.work,
                work_level: c.work_level,
                location: c.location,
                wellbeing: c.wellbeing,
                has_car: c.has_car,
                sick_since: None,
                school: None,
            }),
            households,
            Default::default(),
        ),
        paths: v21.paths,
        utilities: v21.utilities,
        trips: v21.trips,
        agents: v21.agents,
        treasury: v21.treasury,
        economy: v21.economy,
        demand: v21.demand,
        demand_acc: v21.demand_acc,
        land: v21.land,
        decay: v21.decay,
        def_names: v21.def_names,
        district_map: v21.district_map,
        districts: v21.districts,
        service_state: Default::default(),
        coverage: Default::default(),
    }
}

/// v20 → v21 (T-131): the district map and records were added; an old save has no districts.
pub fn migrate_v20_to_v21(v20: SaveDataV20) -> SaveDataV21 {
    SaveDataV21 {
        tick: v20.tick,
        rng: v20.rng,
        speed: v20.speed,
        queue: v20.queue,
        log: v20.log,
        tiles: v20.tiles,
        net: v20.net,
        zones: v20.zones,
        buildings: v20.buildings,
        citizens: v20.citizens,
        paths: v20.paths,
        utilities: v20.utilities,
        trips: v20.trips,
        agents: v20.agents,
        treasury: v20.treasury,
        economy: v20.economy,
        demand: v20.demand,
        demand_acc: v20.demand_acc,
        land: v20.land,
        decay: v20.decay,
        def_names: v20.def_names,
        district_map: Default::default(),
        districts: Default::default(),
    }
}

/// Version 19: the body as written by format 19 (buildings on a `Footprint`; no loans).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV19 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: cp_zone::legacy::ZoneGridV34,
    pub buildings: BuildingGrid,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
    pub agents: Agents,
    pub treasury: Treasury,
    pub economy: EconomyStateV19,
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    pub land: LandGrids,
    pub decay: Decay,
    pub def_names: crate::DefNames,
}

/// v19 → v20 (T-129): the economy state got the loans and the report its loan payments; an
/// old city has none of either.
pub fn migrate_v19_to_v20(v19: SaveDataV19) -> SaveDataV20 {
    let w = v19.economy.last_week;
    SaveDataV20 {
        tick: v19.tick,
        rng: v19.rng,
        speed: v19.speed,
        queue: v19.queue,
        log: v19.log,
        tiles: v19.tiles,
        net: v19.net,
        zones: v19.zones,
        buildings: v19.buildings,
        citizens: v19.citizens,
        paths: v19.paths,
        utilities: v19.utilities,
        trips: v19.trips,
        agents: v19.agents,
        treasury: v19.treasury,
        economy: EconomyStateV31 {
            taxes: v19.economy.taxes,
            budgets: v19.economy.budgets,
            last_week: WeekReportV31 {
                week: w.week,
                income_by_zone: w.income_by_zone,
                road_upkeep: w.road_upkeep,
                building_upkeep: w.building_upkeep,
                utility_upkeep: w.utility_upkeep,
                loan_payments: Cost::ZERO,
            },
            loans: Vec::new(),
        },
        demand: v19.demand,
        demand_acc: v19.demand_acc,
        land: v19.land,
        decay: v19.decay,
        def_names: v19.def_names,
    }
}

/// v18 → v19 (T-106): a building's `lot` became `footprint: Footprint::Lot(lot)` and its
/// access point an `Option` (always `Some` in an old save). Ids, generations and the removed
/// list are kept.
pub fn migrate_v18_to_v19(v18: SaveDataV18) -> SaveDataV19 {
    SaveDataV19 {
        tick: v18.tick,
        rng: v18.rng,
        speed: v18.speed,
        queue: v18.queue,
        log: v18.log,
        tiles: v18.tiles,
        net: v18.net,
        zones: v18.zones,
        buildings: BuildingGrid::from_parts(
            v18.buildings.buildings.map(|b| Building::from(b)),
            v18.buildings.removed,
        ),
        citizens: v18.citizens,
        paths: v18.paths,
        utilities: v18.utilities,
        trips: v18.trips,
        agents: v18.agents,
        treasury: v18.treasury,
        economy: v18.economy,
        demand: v18.demand,
        demand_acc: v18.demand_acc,
        land: v18.land,
        decay: v18.decay,
        def_names: v18.def_names,
    }
}

/// v17 → v18: definition names were added. An old save has none, so they come from the frozen
/// tables of the content that wrote it ([`crate::def_tables`]); this step assumes a v17 writer,
/// `decode_and_migrate` replaces the names for files from before T-038 (v11 and older).
pub fn migrate_v17_to_v18(v17: SaveDataV17) -> SaveDataV18 {
    SaveDataV18 {
        tick: v17.tick,
        rng: v17.rng,
        speed: v17.speed,
        queue: v17.queue,
        log: v17.log,
        tiles: v17.tiles,
        net: v17.net,
        zones: v17.zones,
        buildings: v17.buildings,
        citizens: v17.citizens,
        paths: v17.paths,
        utilities: v17.utilities,
        trips: v17.trips,
        agents: v17.agents,
        treasury: v17.treasury,
        economy: v17.economy,
        demand: v17.demand,
        demand_acc: v17.demand_acc,
        land: v17.land,
        decay: v17.decay,
        def_names: crate::def_tables::def_names_before_v18(17),
    }
}

/// v16 → v17: a car may carry the id of its re-route request. Nobody has one in an old save.
pub fn migrate_v16_to_v17(v16: SaveDataV16) -> SaveDataV17 {
    // Slots and generations are kept, so every vehicle id stays what it was.
    let vehicles = v16.agents.vehicles.map(|v| Vehicle {
        trip: v.trip,
        citizen: v.citizen,
        path: v.path.clone(),
        kind: v.kind,
        hop: v.hop,
        in_conn: v.in_conn,
        spawned: v.spawned,
        s: v.s,
        speed: v.speed,
        stuck_timer: v.stuck_timer,
        reroute: None,
    });
    let s = &v16.agents.stats;
    let agents = Agents::from_parts(
        v16.agents.pedestrians,
        vehicles,
        AgentStats {
            abstract_trips: s.abstract_trips,
            walked: s.walked,
            driven: s.driven,
            no_path: s.no_path,
            invalid_routes: s.invalid_routes,
            red_waits: s.red_waits,
            despawned_stuck: s.despawned_stuck,
            invalid_vehicles: s.invalid_vehicles,
            ..AgentStats::default()
        },
        v16.agents.load,
    );
    SaveDataV17 {
        tick: v16.tick,
        rng: v16.rng,
        speed: v16.speed,
        queue: v16.queue,
        log: v16.log,
        tiles: v16.tiles,
        net: v16.net,
        zones: v16.zones,
        buildings: v16.buildings,
        citizens: v16.citizens,
        paths: v16.paths,
        utilities: v16.utilities,
        trips: v16.trips,
        agents,
        treasury: v16.treasury,
        economy: v16.economy,
        demand: v16.demand,
        demand_acc: v16.demand_acc,
        land: v16.land,
        decay: v16.decay,
    }
}

/// v15 → v16: the land grids and the decay timers were added. An old city starts with empty
/// grids (the first 256-tick pass computes the real ones before anything reads them) and no
/// timers: buildings in trouble start their week then.
pub fn migrate_v15_to_v16(v15: SaveDataV15) -> SaveDataV16 {
    SaveDataV16 {
        tick: v15.tick,
        rng: v15.rng,
        speed: v15.speed,
        queue: v15.queue,
        log: v15.log,
        tiles: v15.tiles,
        net: v15.net,
        zones: v15.zones,
        buildings: v15.buildings,
        citizens: v15.citizens,
        paths: v15.paths,
        utilities: v15.utilities,
        trips: v15.trips,
        agents: v15.agents,
        treasury: v15.treasury,
        economy: v15.economy,
        demand: v15.demand,
        demand_acc: v15.demand_acc,
        land: LandGrids::default(),
        decay: Decay::default(),
    }
}

/// v14 → v15: demand and its cycle sums were added. An old city starts from the demand of a
/// new one (housing, shops and industry wanted, no offices) with empty sums, so spawning and
/// move-in are not stalled; the smoothing pulls the values to the city's real ones within a
/// few 64-tick cycles, and the cycle that was in progress is dropped.
pub fn migrate_v14_to_v15(v14: SaveDataV14) -> SaveDataV15 {
    SaveDataV15 {
        tick: v14.tick,
        rng: v14.rng,
        speed: v14.speed,
        queue: v14.queue,
        log: v14.log,
        tiles: v14.tiles,
        net: v14.net,
        zones: v14.zones,
        buildings: v14.buildings,
        citizens: v14.citizens,
        paths: v14.paths,
        utilities: v14.utilities,
        trips: v14.trips,
        agents: v14.agents,
        treasury: v14.treasury,
        economy: v14.economy,
        demand: Demand::default(),
        demand_acc: DemandAcc::default(),
    }
}

/// v13 → v14: the treasury and the economy settings were added. An old city starts with the
/// default treasury (70 000.00), 9% taxes, 100% budgets and no weekly report.
pub fn migrate_v13_to_v14(v13: SaveDataV13) -> SaveDataV14 {
    SaveDataV14 {
        tick: v13.tick,
        rng: v13.rng,
        speed: v13.speed,
        queue: v13.queue,
        log: v13.log,
        tiles: v13.tiles,
        net: v13.net,
        zones: v13.zones,
        buildings: v13.buildings,
        citizens: v13.citizens,
        paths: v13.paths,
        utilities: v13.utilities,
        trips: v13.trips,
        agents: v13.agents,
        treasury: Treasury::default(),
        economy: EconomyStateV19::default(),
    }
}

/// v12 → v13: the building grid also saves the ids removed and not yet drained (a save on
/// pause right after a `Demolish` used to lose them). An old save had none pending.
pub fn migrate_v12_to_v13(v12: SaveDataV12) -> SaveDataV13 {
    SaveDataV13 {
        tick: v12.tick,
        rng: v12.rng,
        speed: v12.speed,
        queue: v12.queue,
        log: v12.log,
        tiles: v12.tiles,
        net: v12.net,
        zones: v12.zones,
        buildings: BuildingGridV18 {
            buildings: v12.buildings.buildings,
            removed: Vec::new(),
        },
        citizens: v12.citizens,
        paths: v12.paths,
        utilities: v12.utilities,
        trips: v12.trips,
        agents: v12.agents,
    }
}

/// v11 → v12: vehicles got lane state and segment load was added. A v11 car had no
/// position, only an arrival tick: it starts over at the beginning of its path (it waits for
/// room there like a new car); if its start lane is gone the drive code completes the trip
/// abstractly. Pedestrians are unchanged; the load starts empty.
pub fn migrate_v11_to_v12(v11: SaveDataV11) -> SaveDataV12 {
    let mut vehicles = Arena::new();
    for (_, v) in v11.agents.vehicles.iter() {
        vehicles.insert(VehicleV16 {
            trip: v.trip,
            citizen: v.citizen,
            path: v.path.clone(),
            kind: 0,
            hop: 0,
            in_conn: false,
            spawned: false,
            s: v.path.start_s,
            speed: 0.0,
            stuck_timer: 0,
        });
    }
    let s = v11.agents.stats;
    SaveDataV12 {
        tick: v11.tick,
        rng: v11.rng,
        speed: v11.speed,
        queue: v11.queue,
        log: v11.log,
        tiles: v11.tiles,
        net: v11.net,
        zones: v11.zones,
        buildings: v11.buildings,
        citizens: v11.citizens,
        paths: v11.paths,
        utilities: v11.utilities,
        trips: v11.trips,
        agents: AgentsV16 {
            pedestrians: v11.agents.pedestrians,
            vehicles,
            stats: AgentStatsV16 {
                abstract_trips: s.abstract_trips,
                walked: s.walked,
                driven: s.driven,
                no_path: s.no_path,
                invalid_routes: s.invalid_routes,
                red_waits: s.red_waits,
                ..AgentStatsV16::default()
            },
            load: SegmentLoad::default(),
        },
    }
}

/// v10 → v11: agents were added. A trip in flight in an old save has no agent: its state
/// is one the old format knows (`Scheduled` or an abstract `InTransit`), which the routine's
/// timer still finishes, so nothing needs rerouting.
pub fn migrate_v10_to_v11(v10: SaveDataV10) -> SaveDataV11 {
    SaveDataV11 {
        tick: v10.tick,
        rng: v10.rng,
        speed: v10.speed,
        queue: v10.queue,
        log: v10.log,
        tiles: v10.tiles,
        net: v10.net,
        zones: v10.zones,
        buildings: v10.buildings,
        citizens: v10.citizens,
        paths: v10.paths,
        utilities: v10.utilities,
        trips: v10.trips,
        agents: AgentsV11::default(),
    }
}

/// v9 → v10: trips were added; an old save has none in flight (citizens simply decide
/// again at their next visit).
pub fn migrate_v9_to_v10(v9: SaveDataV9) -> SaveDataV10 {
    SaveDataV10 {
        tick: v9.tick,
        rng: v9.rng,
        speed: v9.speed,
        queue: v9.queue,
        log: v9.log,
        tiles: v9.tiles,
        net: v9.net,
        zones: v9.zones,
        buildings: v9.buildings,
        citizens: v9.citizens,
        paths: v9.paths,
        utilities: v9.utilities,
        trips: TripBook::default(),
    }
}

/// v8 → v9: the utility networks (pipes, power lines) were added; an old save has none.
pub fn migrate_v8_to_v9(v8: SaveDataV8) -> SaveDataV9 {
    SaveDataV9 {
        tick: v8.tick,
        rng: v8.rng,
        speed: v8.speed,
        queue: v8.queue,
        log: v8.log,
        tiles: v8.tiles,
        net: v8.net,
        zones: v8.zones,
        buildings: v8.buildings,
        citizens: v8.citizens,
        paths: v8.paths,
        utilities: UtilityGraph::default(),
    }
}

/// v7 → v8: pending path requests were added; an old save has none in flight and the next
/// request id restarts at 0.
pub fn migrate_v7_to_v8(v7: SaveDataV7) -> SaveDataV8 {
    SaveDataV8 {
        tick: v7.tick,
        rng: v7.rng,
        speed: v7.speed,
        queue: v7.queue,
        log: v7.log,
        tiles: v7.tiles,
        net: v7.net,
        zones: v7.zones,
        buildings: v7.buildings,
        citizens: v7.citizens,
        paths: SavedPaths::default(),
    }
}

/// v6 → v7: citizens and households were added; an old save has none (its homes are
/// empty and move-ins start again).
pub fn migrate_v6_to_v7(v6: SaveDataV6) -> SaveDataV7 {
    SaveDataV7 {
        tick: v6.tick,
        rng: v6.rng,
        speed: v6.speed,
        queue: v6.queue,
        log: v6.log,
        tiles: v6.tiles,
        net: v6.net,
        zones: v6.zones,
        buildings: v6.buildings,
        citizens: CitizenGridV21::default(),
    }
}

/// v5 → v6: buildings were added; an old save has none (its zones keep their painted
/// types and every valid cell is free).
pub fn migrate_v5_to_v6(v5: SaveDataV5) -> SaveDataV6 {
    SaveDataV6 {
        tick: v5.tick,
        rng: v5.rng,
        speed: v5.speed,
        queue: v5.queue,
        log: v5.log,
        tiles: v5.tiles,
        net: v5.net,
        zones: v5.zones,
        buildings: BuildingGridV12::default(),
    }
}

/// v4 → v5: zone state was added; an old save starts with an empty [`ZoneGrid`] and the
/// `Network` phase builds the blocks from the loaded road network on the first tick.
pub fn migrate_v4_to_v5(v4: SaveDataV4) -> SaveDataV5 {
    SaveDataV5 {
        tick: v4.tick,
        rng: v4.rng,
        speed: v4.speed,
        queue: v4.queue,
        log: v4.log,
        tiles: v4.tiles,
        net: v4.net,
        zones: cp_zone::legacy::ZoneGridV34::default(),
    }
}

/// v3 → v4: header-only change (CRC over the whole file, map hash); the body is carried
/// over unchanged.
pub fn migrate_v3_to_v4(v3: SaveDataV3) -> SaveDataV4 {
    SaveDataV4 {
        tick: v3.tick,
        rng: v3.rng,
        speed: v3.speed,
        queue: v3.queue,
        log: v3.log,
        tiles: v3.tiles,
        net: v3.net,
    }
}

/// The last two chain steps, shared by every arm.
fn up(v: SaveDataV10) -> SaveData {
    let v = migrate_v11_to_v12(migrate_v10_to_v11(v));
    from_v12(v)
}

/// The rest of the chain from a decoded v12 body.
fn from_v12(v: SaveDataV12) -> SaveData {
    let v = migrate_v12_to_v13(v);
    from_v13(v)
}

fn from_v13(v: SaveDataV13) -> SaveData {
    from_v14(migrate_v13_to_v14(v))
}

fn from_v14(v: SaveDataV14) -> SaveData {
    from_v15(migrate_v14_to_v15(v))
}

fn from_v15(v: SaveDataV15) -> SaveData {
    from_v16(migrate_v15_to_v16(v))
}

/// Version 23: the body as written by format 23 (worker-shortage timers; fires without a
/// station).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV23 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::legacy::ZoneGridV34,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV31,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`DefNames`] and [`SaveData::remap_defs`].
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22.
    pub service_state: ServiceStateV23,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
}

/// A fire as formats up to 23 wrote it (no `station`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FireV23 {
    pub since: Tick,
    pub out_at: Option<Tick>,
}

/// A service slot as formats up to 23 wrote it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildingServicesV23 {
    pub generation: u32,
    pub garbage: u16,
    pub crime: u16,
    pub fire: Option<FireV23>,
    pub problems: cp_service::ServiceProblems,
}

/// The service state as formats up to 23 wrote it (same field order as `ServiceState`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceStateV23 {
    buildings: Vec<BuildingServicesV23>,
    facilities: std::collections::BTreeMap<BuildingId, cp_service::Facility>,
}

impl ServiceStateV23 {
    fn upgrade(self) -> cp_service::ServiceState {
        cp_service::ServiceState::from_parts(
            self.buildings
                .into_iter()
                .map(|b| cp_service::BuildingServices {
                    generation: b.generation,
                    garbage: b.garbage,
                    crime: b.crime,
                    fire: b.fire.map(|f| cp_service::Fire {
                        since: f.since,
                        out_at: f.out_at,
                        station: None,
                    }),
                    problems: b.problems,
                })
                .collect(),
            self.facilities,
        )
    }
}

/// v23 → v24 (T-142): a fire names the station whose engine is on it; the saved fires (there
/// are none in practice) have no engine.
pub fn migrate_v23_to_v24(v23: SaveDataV23) -> SaveDataV24 {
    SaveDataV24 {
        tick: v23.tick,
        rng: v23.rng,
        speed: v23.speed,
        queue: v23.queue,
        log: v23.log,
        tiles: v23.tiles,
        net: v23.net,
        zones: v23.zones,
        buildings: v23.buildings,
        citizens: v23.citizens,
        paths: v23.paths,
        utilities: v23.utilities,
        trips: v23.trips,
        agents: v23.agents,
        treasury: v23.treasury,
        economy: v23.economy,
        demand: v23.demand,
        demand_acc: v23.demand_acc,
        land: v23.land,
        decay: v23.decay,
        def_names: v23.def_names,
        district_map: v23.district_map,
        districts: v23.districts,
        service_state: v23.service_state.upgrade(),
        coverage: v23.coverage,
        shortage: v23.shortage,
    }
}

/// Version 24: the body as written by format 24 (fires name their station; no milestone
/// progress).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV24 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::legacy::ZoneGridV34,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV31,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`crate::DefNames`] and `SaveData::remap_defs`.
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
}

/// v24 → v25 (T-149): the milestone progress. A save from before it has every milestone
/// (nothing is locked in a city that already exists).
pub fn migrate_v24_to_v25(v24: SaveDataV24) -> SaveDataV25 {
    SaveDataV25 {
        tick: v24.tick,
        rng: v24.rng,
        speed: v24.speed,
        queue: v24.queue,
        log: v24.log,
        tiles: v24.tiles,
        net: v24.net,
        zones: v24.zones,
        buildings: v24.buildings,
        citizens: v24.citizens,
        paths: v24.paths,
        utilities: v24.utilities,
        trips: v24.trips,
        agents: v24.agents,
        treasury: v24.treasury,
        economy: v24.economy,
        demand: v24.demand,
        demand_acc: v24.demand_acc,
        land: v24.land,
        decay: v24.decay,
        def_names: v24.def_names,
        district_map: v24.district_map,
        districts: v24.districts,
        service_state: v24.service_state,
        coverage: v24.coverage,
        shortage: v24.shortage,
        progress: ProgressV38::ALL,
    }
}

/// Version 25: the body as written by format 25 (milestone progress; move-in households
/// appear at once, no `TripPurpose::MoveIn`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV25 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::legacy::ZoneGridV34,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV31,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`crate::DefNames`] and `SaveData::remap_defs`.
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached. Added in v25.
    pub progress: ProgressV38,
}

/// v25 → v26 (T-118): `TripPurpose::MoveIn` is appended; nothing in a v25 body changes.
pub fn migrate_v25_to_v26(v25: SaveDataV25) -> SaveDataV26 {
    SaveDataV26 {
        tick: v25.tick,
        rng: v25.rng,
        speed: v25.speed,
        queue: v25.queue,
        log: v25.log,
        tiles: v25.tiles,
        net: v25.net,
        zones: v25.zones,
        buildings: v25.buildings,
        citizens: v25.citizens,
        paths: v25.paths,
        utilities: v25.utilities,
        trips: v25.trips,
        agents: v25.agents,
        treasury: v25.treasury,
        economy: v25.economy,
        demand: v25.demand,
        demand_acc: v25.demand_acc,
        land: v25.land,
        decay: v25.decay,
        def_names: v25.def_names,
        district_map: v25.district_map,
        districts: v25.districts,
        service_state: v25.service_state,
        coverage: v25.coverage,
        shortage: v25.shortage,
        progress: v25.progress,
    }
}

/// Version 26: the body as written by format 26 (`MoveIn` trips; no service runs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV26 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::legacy::ZoneGridV34,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV31,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`crate::DefNames`] and `SaveData::remap_defs`.
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached. Added in v25.
    pub progress: ProgressV38,
}

/// v26 → v27 (T-151): service runs. A save from before it has none under way.
pub fn migrate_v26_to_v27(v26: SaveDataV26) -> SaveDataV27 {
    SaveDataV27 {
        tick: v26.tick,
        rng: v26.rng,
        speed: v26.speed,
        queue: v26.queue,
        log: v26.log,
        tiles: v26.tiles,
        net: v26.net,
        zones: v26.zones,
        buildings: v26.buildings,
        citizens: v26.citizens,
        paths: v26.paths,
        utilities: v26.utilities,
        trips: v26.trips,
        agents: v26.agents,
        treasury: v26.treasury,
        economy: v26.economy,
        demand: v26.demand,
        demand_acc: v26.demand_acc,
        land: v26.land,
        decay: v26.decay,
        def_names: v26.def_names,
        district_map: v26.district_map,
        districts: v26.districts,
        service_state: v26.service_state,
        coverage: v26.coverage,
        shortage: v26.shortage,
        progress: v26.progress,
        service_runs: ServiceRunsV28::default(),
    }
}

/// Version 27: the body as written by format 27 (service runs; no transit).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV27 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::legacy::ZoneGridV34,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV31,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`crate::DefNames`] and `SaveData::remap_defs`.
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached. Added in v25.
    pub progress: ProgressV38,
    /// Fire engines and garbage trucks on their way (T-151). Added in v27.
    pub service_runs: ServiceRunsV28,
}

/// Version 28: the body as written by format 28 (bus stops and lines; runs between buildings only).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV28 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::legacy::ZoneGridV34,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV31,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`crate::DefNames`] and `SaveData::remap_defs`.
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached. Added in v25.
    pub progress: ProgressV38,
    /// Fire engines and garbage trucks on their way (T-151). Added in v27.
    pub service_runs: ServiceRunsV28,
    /// Bus stops and lines (T-153). Added in v28.
    pub transit: TransitNetV30,
}

/// v27 → v28 (T-153): stops and lines. A save from before it has none.
pub fn migrate_v27_to_v28(v27: SaveDataV27) -> SaveDataV28 {
    SaveDataV28 {
        tick: v27.tick,
        rng: v27.rng,
        speed: v27.speed,
        queue: v27.queue,
        log: v27.log,
        tiles: v27.tiles,
        net: v27.net,
        zones: v27.zones,
        buildings: v27.buildings,
        citizens: v27.citizens,
        paths: v27.paths,
        utilities: v27.utilities,
        trips: v27.trips,
        agents: v27.agents,
        treasury: v27.treasury,
        economy: v27.economy,
        demand: v27.demand,
        demand_acc: v27.demand_acc,
        land: v27.land,
        decay: v27.decay,
        def_names: v27.def_names,
        district_map: v27.district_map,
        districts: v27.districts,
        service_state: v27.service_state,
        coverage: v27.coverage,
        shortage: v27.shortage,
        progress: v27.progress,
        service_runs: v27.service_runs,
        transit: TransitNetV30::default(),
    }
}

/// A run as v28 wrote it: between two buildings, never kept.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceRunV28 {
    pub kind: u8,
    pub from: BuildingId,
    pub to: BuildingId,
    pub state: cp_traffic::RunState,
}

/// The service runs as v27 and v28 wrote them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceRunsV28 {
    next: u32,
    runs: std::collections::BTreeMap<u32, ServiceRunV28>,
}

impl ServiceRunsV28 {
    fn upgrade(self) -> cp_traffic::ServiceRuns {
        cp_traffic::ServiceRuns::from_parts(
            self.next,
            self.runs
                .into_iter()
                .map(|(id, r)| {
                    (
                        id,
                        cp_traffic::ServiceRun {
                            kind: r.kind,
                            from: cp_traffic::RunPoint::Building(r.from),
                            to: cp_traffic::RunPoint::Building(r.to),
                            keep: false,
                            state: r.state,
                        },
                    )
                })
                .collect(),
        )
    }
}

/// v28 → v29 (T-154): runs go between points and may be kept. A v28 run is between two
/// buildings and not kept.
pub fn migrate_v28_to_v29(v28: SaveDataV28) -> SaveDataV29 {
    SaveDataV29 {
        tick: v28.tick,
        rng: v28.rng,
        speed: v28.speed,
        queue: v28.queue,
        log: v28.log,
        tiles: v28.tiles,
        net: v28.net,
        zones: v28.zones,
        buildings: v28.buildings,
        citizens: v28.citizens,
        paths: v28.paths,
        utilities: v28.utilities,
        trips: v28.trips,
        agents: v28.agents,
        treasury: v28.treasury,
        economy: v28.economy,
        demand: v28.demand,
        demand_acc: v28.demand_acc,
        land: v28.land,
        decay: v28.decay,
        def_names: v28.def_names,
        district_map: v28.district_map,
        districts: v28.districts,
        service_state: v28.service_state,
        coverage: v28.coverage,
        shortage: v28.shortage,
        progress: v28.progress,
        service_runs: v28.service_runs.upgrade(),
        transit: v28.transit,
    }
}

/// Version 29: the body as written by format 29 (no buses).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV29 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::legacy::ZoneGridV34,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV31,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`DefNames`] and [`SaveData::remap_defs`].
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached. Added in v25.
    pub progress: ProgressV38,
    /// Runs of service vehicles, buses and the like (T-151). Added in v27; between points,
    /// with keep, since v29.
    pub service_runs: cp_traffic::ServiceRuns,
    /// Bus stops and lines (T-153). Added in v28.
    pub transit: TransitNetV30,
}

/// The transit state as v28–v30 wrote it: no riders, no line statistics.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TransitNetV30 {
    pub stops: Arena<cp_transit::Stop>,
    pub lines: Arena<cp_transit::Line>,
}

/// The transit ledger as v30 wrote it: no fares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitLedgerV30 {
    pub upkeep: Cost,
}

/// v29 → v30 (T-156): buses and the transit ledger. A save from before it has neither.
pub fn migrate_v29_to_v30(v29: SaveDataV29) -> SaveDataV30 {
    SaveDataV30 {
        tick: v29.tick,
        rng: v29.rng,
        speed: v29.speed,
        queue: v29.queue,
        log: v29.log,
        tiles: v29.tiles,
        net: v29.net,
        zones: v29.zones,
        buildings: v29.buildings,
        citizens: v29.citizens,
        paths: v29.paths,
        utilities: v29.utilities,
        trips: v29.trips,
        agents: v29.agents,
        treasury: v29.treasury,
        economy: v29.economy,
        demand: v29.demand,
        demand_acc: v29.demand_acc,
        land: v29.land,
        decay: v29.decay,
        def_names: v29.def_names,
        district_map: v29.district_map,
        districts: v29.districts,
        service_state: v29.service_state,
        coverage: v29.coverage,
        shortage: v29.shortage,
        progress: v29.progress,
        service_runs: v29.service_runs,
        transit: v29.transit,
        fleet: cp_transit::TransitFleet::default(),
        ledger: TransitLedgerV30::default(),
    }
}

/// Version 30: the body as written by format 30 (buses; no riders).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV30 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::legacy::ZoneGridV34,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV31,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`DefNames`] and [`SaveData::remap_defs`].
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached. Added in v25.
    pub progress: ProgressV38,
    /// Runs of service vehicles, buses and the like (T-151). Added in v27; between points,
    /// with keep, since v29.
    pub service_runs: cp_traffic::ServiceRuns,
    /// Bus stops and lines (T-153). Added in v28.
    pub transit: TransitNetV30,
    /// Buses out on the lines and the week's transit money (T-156). Added in v30.
    pub fleet: cp_transit::TransitFleet,
    pub ledger: TransitLedgerV30,
}

/// The week report as formats 20 to 31 wrote it (before the transit lines).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeekReportV31 {
    pub week: u64,
    pub income_by_zone: [Cost; 6],
    pub road_upkeep: Cost,
    pub building_upkeep: Cost,
    pub utility_upkeep: Cost,
    pub loan_payments: Cost,
}

/// The economy state as formats 20 to 31 wrote it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EconomyStateV31 {
    pub taxes: TaxRates,
    pub budgets: Budgets,
    pub last_week: WeekReportV31,
    pub loans: Vec<cp_economy::Loan>,
}

/// v30 → v31 (T-159): riders, line statistics and fares. A save from before it has none.
pub fn migrate_v30_to_v31(v30: SaveDataV30) -> SaveDataV31 {
    SaveDataV31 {
        tick: v30.tick,
        rng: v30.rng,
        speed: v30.speed,
        queue: v30.queue,
        log: v30.log,
        tiles: v30.tiles,
        net: v30.net,
        zones: v30.zones,
        buildings: v30.buildings,
        citizens: v30.citizens,
        paths: v30.paths,
        utilities: v30.utilities,
        trips: v30.trips,
        agents: v30.agents,
        treasury: v30.treasury,
        economy: v30.economy,
        demand: v30.demand,
        demand_acc: v30.demand_acc,
        land: v30.land,
        decay: v30.decay,
        def_names: v30.def_names,
        district_map: v30.district_map,
        districts: v30.districts,
        service_state: v30.service_state,
        coverage: v30.coverage,
        shortage: v30.shortage,
        progress: v30.progress,
        service_runs: v30.service_runs,
        transit: cp_transit::TransitNet {
            stops: v30.transit.stops,
            lines: v30.transit.lines,
            ..Default::default()
        },
        fleet: v30.fleet,
        ledger: TransitLedgerV32 {
            upkeep: v30.ledger.upkeep,
            fares: Cost::ZERO,
        },
    }
}

/// Version 31: the body as written by format 31 (riders; the week report without transit).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV31 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::legacy::ZoneGridV34,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV31,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`DefNames`] and [`SaveData::remap_defs`].
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached. Added in v25.
    pub progress: ProgressV38,
    /// Runs of service vehicles, buses and the like (T-151). Added in v27; between points,
    /// with keep, since v29.
    pub service_runs: cp_traffic::ServiceRuns,
    /// Bus stops and lines (T-153). Added in v28.
    pub transit: cp_transit::TransitNet,
    /// Buses out on the lines and the week's transit money (T-156). Added in v30.
    pub fleet: cp_transit::TransitFleet,
    pub ledger: TransitLedgerV32,
}

/// The transit ledger as formats 31 and 32 wrote it: no tourism.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitLedgerV32 {
    pub upkeep: Cost,
    pub fares: Cost,
}

/// The week report as format 32 wrote it (transit money, no tourism).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeekReportV32 {
    pub week: u64,
    pub income_by_zone: [Cost; 6],
    pub road_upkeep: Cost,
    pub building_upkeep: Cost,
    pub utility_upkeep: Cost,
    pub loan_payments: Cost,
    pub transit_fares: Cost,
    pub transit_upkeep: Cost,
}

/// The economy state as format 32 wrote it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EconomyStateV32 {
    pub taxes: TaxRates,
    pub budgets: Budgets,
    pub last_week: WeekReportV32,
    pub loans: Vec<cp_economy::Loan>,
}

/// v31 → v32 (T-160): the week report gets the transit fares and upkeep. A save from before it
/// has zeros.
pub fn migrate_v31_to_v32(v31: SaveDataV31) -> SaveDataV32 {
    let w = v31.economy.last_week;
    SaveDataV32 {
        tick: v31.tick,
        rng: v31.rng,
        speed: v31.speed,
        queue: v31.queue,
        log: v31.log,
        tiles: v31.tiles,
        net: v31.net,
        zones: v31.zones,
        buildings: v31.buildings,
        citizens: v31.citizens,
        paths: v31.paths,
        utilities: v31.utilities,
        trips: v31.trips,
        agents: v31.agents,
        treasury: v31.treasury,
        economy: EconomyStateV32 {
            taxes: v31.economy.taxes,
            budgets: v31.economy.budgets,
            last_week: WeekReportV32 {
                week: w.week,
                income_by_zone: w.income_by_zone,
                road_upkeep: w.road_upkeep,
                building_upkeep: w.building_upkeep,
                utility_upkeep: w.utility_upkeep,
                loan_payments: w.loan_payments,
                transit_fares: Cost::ZERO,
                transit_upkeep: Cost::ZERO,
            },
            loans: v31.economy.loans,
        },
        demand: v31.demand,
        demand_acc: v31.demand_acc,
        land: v31.land,
        decay: v31.decay,
        def_names: v31.def_names,
        district_map: v31.district_map,
        districts: v31.districts,
        service_state: v31.service_state,
        coverage: v31.coverage,
        shortage: v31.shortage,
        progress: v31.progress,
        service_runs: v31.service_runs,
        transit: v31.transit,
        fleet: v31.fleet,
        ledger: v31.ledger,
    }
}

/// Version 32: the body as written by format 32 (transit fares in the week report; no tourists).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV32 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::legacy::ZoneGridV34,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV32,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`DefNames`] and [`SaveData::remap_defs`].
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached. Added in v25.
    pub progress: ProgressV38,
    /// Runs of service vehicles, buses and the like (T-151). Added in v27; between points,
    /// with keep, since v29.
    pub service_runs: cp_traffic::ServiceRuns,
    /// Bus stops and lines (T-153). Added in v28.
    pub transit: cp_transit::TransitNet,
    /// Buses out on the lines and the week's transit money (T-156). Added in v30.
    pub fleet: cp_transit::TransitFleet,
    pub ledger: TransitLedgerV32,
}

/// v32 → v33 (T-168): tourists. The week report and the ledger get a tourism line, there are no
/// visits under way and no building has had a customer.
pub fn migrate_v32_to_v33(v32: SaveDataV32) -> SaveDataV33 {
    let w = v32.economy.last_week;
    SaveDataV33 {
        tick: v32.tick,
        rng: v32.rng,
        speed: v32.speed,
        queue: v32.queue,
        log: v32.log,
        tiles: v32.tiles,
        net: v32.net,
        zones: v32.zones,
        buildings: v32.buildings,
        citizens: v32.citizens,
        paths: v32.paths,
        utilities: v32.utilities,
        trips: v32.trips,
        agents: v32.agents,
        treasury: v32.treasury,
        demand: v32.demand,
        demand_acc: v32.demand_acc,
        land: v32.land,
        decay: v32.decay,
        def_names: v32.def_names,
        district_map: v32.district_map,
        districts: v32.districts,
        service_state: v32.service_state,
        coverage: v32.coverage,
        shortage: v32.shortage,
        progress: v32.progress,
        service_runs: v32.service_runs,
        transit: v32.transit,
        fleet: v32.fleet,
        economy: EconomyStateV36 {
            taxes: v32.economy.taxes,
            budgets: v32.economy.budgets,
            last_week: WeekReportV36 {
                week: w.week,
                income_by_zone: w.income_by_zone,
                road_upkeep: w.road_upkeep,
                building_upkeep: w.building_upkeep,
                utility_upkeep: w.utility_upkeep,
                loan_payments: w.loan_payments,
                transit_fares: w.transit_fares,
                transit_upkeep: w.transit_upkeep,
                tourism: Cost::ZERO,
            },
            loans: v32.economy.loans,
        },
        ledger: TransitLedgerV36 {
            upkeep: v32.ledger.upkeep,
            fares: v32.ledger.fares,
            tourism: Cost::ZERO,
        },
        tourists: cp_transit::Tourists::default(),
    }
}

/// Version 33: the body as written by format 33 (tourists; no goods).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV33 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::legacy::ZoneGridV34,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV36,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`DefNames`] and [`SaveData::remap_defs`].
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached. Added in v25.
    pub progress: ProgressV38,
    /// Runs of service vehicles, buses and the like (T-151). Added in v27; between points,
    /// with keep, since v29.
    pub service_runs: cp_traffic::ServiceRuns,
    /// Bus stops and lines (T-153). Added in v28.
    pub transit: cp_transit::TransitNet,
    /// Buses out on the lines and the week's transit money (T-156). Added in v30.
    pub fleet: cp_transit::TransitFleet,
    pub ledger: TransitLedgerV36,
    /// Tourists on the way and the visits each building has had (T-168). Added in v33.
    pub tourists: cp_transit::Tourists,
}

/// v33 → v34 (T-169): goods. No building holds stock and no truck is on the road.
pub fn migrate_v33_to_v34(v33: SaveDataV33) -> SaveDataV34 {
    SaveDataV34 {
        tick: v33.tick,
        rng: v33.rng,
        speed: v33.speed,
        queue: v33.queue,
        log: v33.log,
        tiles: v33.tiles,
        net: v33.net,
        zones: v33.zones,
        buildings: v33.buildings,
        citizens: v33.citizens,
        paths: v33.paths,
        utilities: v33.utilities,
        trips: v33.trips,
        agents: v33.agents,
        treasury: v33.treasury,
        economy: v33.economy,
        demand: v33.demand,
        demand_acc: v33.demand_acc,
        land: v33.land,
        decay: v33.decay,
        def_names: v33.def_names,
        district_map: v33.district_map,
        districts: v33.districts,
        service_state: v33.service_state,
        coverage: v33.coverage,
        shortage: v33.shortage,
        progress: v33.progress,
        service_runs: v33.service_runs,
        transit: v33.transit,
        fleet: v33.fleet,
        ledger: v33.ledger,
        tourists: v33.tourists,
        goods: GoodsV36::default(),
    }
}

/// Version 34: the body as written by format 34 (zone blocks with square cells only).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV34 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::legacy::ZoneGridV34,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV36,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`DefNames`] and [`SaveData::remap_defs`].
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached. Added in v25.
    pub progress: ProgressV38,
    /// Runs of service vehicles, buses and the like (T-151). Added in v27; between points,
    /// with keep, since v29.
    pub service_runs: cp_traffic::ServiceRuns,
    /// Bus stops and lines (T-153). Added in v28.
    pub transit: cp_transit::TransitNet,
    /// Buses out on the lines and the week's transit money (T-156). Added in v30.
    pub fleet: cp_transit::TransitFleet,
    pub ledger: TransitLedgerV36,
    /// Tourists on the way and the visits each building has had (T-168). Added in v33.
    pub tourists: cp_transit::Tourists,
    /// Stock of factories and shops and the trucks on the road (T-169). Added in v34.
    pub goods: GoodsV36,
}

/// v34 → v35 (T-185): zone blocks carry the length and depth of their cells (a block fitted
/// to its surroundings has stretched ones). Every block of an older save has standard cells.
pub fn migrate_v34_to_v35(v34: SaveDataV34) -> SaveDataV35 {
    SaveDataV35 {
        tick: v34.tick,
        rng: v34.rng,
        speed: v34.speed,
        queue: v34.queue,
        log: v34.log,
        tiles: v34.tiles,
        net: v34.net,
        zones: v34.zones.into_current(),
        buildings: v34.buildings,
        citizens: v34.citizens,
        paths: v34.paths,
        utilities: v34.utilities,
        trips: v34.trips,
        agents: v34.agents,
        treasury: v34.treasury,
        economy: v34.economy,
        demand: v34.demand,
        demand_acc: v34.demand_acc,
        land: v34.land,
        decay: v34.decay,
        def_names: v34.def_names,
        district_map: v34.district_map,
        districts: v34.districts,
        service_state: v34.service_state,
        coverage: v34.coverage,
        shortage: v34.shortage,
        progress: v34.progress,
        service_runs: v34.service_runs,
        transit: v34.transit,
        fleet: v34.fleet,
        ledger: v34.ledger,
        tourists: v34.tourists,
        goods: v34.goods,
    }
}

/// Version 35: the body as written by format 35 (zone cells with their own size; no intercity trains).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV35 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::ZoneGrid,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV36,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`DefNames`] and [`SaveData::remap_defs`].
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached. Added in v25.
    pub progress: ProgressV38,
    /// Runs of service vehicles, buses and the like (T-151). Added in v27; between points,
    /// with keep, since v29.
    pub service_runs: cp_traffic::ServiceRuns,
    /// Bus stops and lines (T-153). Added in v28.
    pub transit: cp_transit::TransitNet,
    /// Buses out on the lines and the week's transit money (T-156). Added in v30.
    pub fleet: cp_transit::TransitFleet,
    pub ledger: TransitLedgerV36,
    /// Tourists on the way and the visits each building has had (T-168). Added in v33.
    pub tourists: cp_transit::Tourists,
    /// Stock of factories and shops and the trucks on the road (T-169). Added in v34.
    pub goods: GoodsV36,
}

/// v35 → v36 (T-167): intercity trains. No train is on the line and no household has come by one.
pub fn migrate_v35_to_v36(v35: SaveDataV35) -> SaveDataV36 {
    SaveDataV36 {
        tick: v35.tick,
        rng: v35.rng,
        speed: v35.speed,
        queue: v35.queue,
        log: v35.log,
        tiles: v35.tiles,
        net: v35.net,
        zones: v35.zones,
        buildings: v35.buildings,
        citizens: v35.citizens,
        paths: v35.paths,
        utilities: v35.utilities,
        trips: v35.trips,
        agents: v35.agents,
        treasury: v35.treasury,
        economy: v35.economy,
        demand: v35.demand,
        demand_acc: v35.demand_acc,
        land: v35.land,
        decay: v35.decay,
        def_names: v35.def_names,
        district_map: v35.district_map,
        districts: v35.districts,
        service_state: v35.service_state,
        coverage: v35.coverage,
        shortage: v35.shortage,
        progress: v35.progress,
        service_runs: v35.service_runs,
        transit: v35.transit,
        fleet: v35.fleet,
        ledger: v35.ledger,
        tourists: v35.tourists,
        goods: v35.goods,
        intercity: cp_transit::Intercity::default(),
    }
}

/// The week report as format 36 wrote it: no export line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeekReportV36 {
    pub week: u64,
    pub income_by_zone: [Cost; 6],
    pub road_upkeep: Cost,
    pub building_upkeep: Cost,
    pub utility_upkeep: Cost,
    pub loan_payments: Cost,
    pub transit_fares: Cost,
    pub transit_upkeep: Cost,
    pub tourism: Cost,
}

/// The economy state as formats 33 to 36 wrote it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EconomyStateV36 {
    pub taxes: TaxRates,
    pub budgets: Budgets,
    pub last_week: WeekReportV36,
    pub loans: Vec<cp_economy::Loan>,
}

/// The transit ledger as formats 33 to 36 wrote it: no exports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitLedgerV36 {
    pub upkeep: Cost,
    pub fares: Cost,
    pub tourism: Cost,
}

/// A truck on the road as formats 34 to 36 wrote it: factory to shop, nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HaulV36 {
    pub from: cp_building::BuildingId,
    pub to: cp_building::BuildingId,
    pub load: f32,
}

/// The goods state as formats 34 to 36 wrote it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GoodsV36 {
    pub stock: std::collections::BTreeMap<cp_building::BuildingId, f32>,
    pub empty_since: std::collections::BTreeMap<cp_building::BuildingId, Tick>,
    pub hauls: std::collections::BTreeMap<cp_traffic::RunId, HaulV36>,
}

/// Version 36: the body as written by format 36 (intercity trains; goods trade and the export line are not in it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV36 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::ZoneGrid,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV36,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`DefNames`] and [`SaveData::remap_defs`].
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached. Added in v25.
    pub progress: ProgressV38,
    /// Runs of service vehicles, buses and the like (T-151). Added in v27; between points,
    /// with keep, since v29.
    pub service_runs: cp_traffic::ServiceRuns,
    /// Bus stops and lines (T-153). Added in v28.
    pub transit: cp_transit::TransitNet,
    /// Buses out on the lines and the week's transit money (T-156). Added in v30.
    pub fleet: cp_transit::TransitFleet,
    pub ledger: TransitLedgerV36,
    /// Tourists on the way and the visits each building has had (T-168). Added in v33.
    pub tourists: cp_transit::Tourists,
    /// Stock of factories and shops and the trucks on the road (T-169). Added in v34.
    pub goods: GoodsV36,
    /// The intercity train and what it has brought (T-167). Added in v36.
    pub intercity: cp_transit::Intercity,
}

/// The week report as format 37 wrote it: no policies line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeekReportV37 {
    pub week: u64,
    pub income_by_zone: [Cost; 6],
    pub road_upkeep: Cost,
    pub building_upkeep: Cost,
    pub utility_upkeep: Cost,
    pub loan_payments: Cost,
    pub transit_fares: Cost,
    pub transit_upkeep: Cost,
    pub tourism: Cost,
    pub exports: Cost,
}

/// The economy state as format 37 wrote it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EconomyStateV37 {
    pub taxes: TaxRates,
    pub budgets: Budgets,
    pub last_week: WeekReportV37,
    pub loans: Vec<cp_economy::Loan>,
}

/// A district as format 37 wrote it: no policies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistrictV37 {
    pub name: String,
    pub color: u8,
    pub tax: [Option<u8>; 6],
}

/// The district records as formats 21 to 37 wrote them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistrictsV37 {
    pub list: Vec<Option<DistrictV37>>,
}

/// Version 37: the body as written by format 37 (goods trade; policies are not in it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV37 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::ZoneGrid,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyStateV37,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`DefNames`] and [`SaveData::remap_defs`].
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV37,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached. Added in v25.
    pub progress: ProgressV38,
    /// Runs of service vehicles, buses and the like (T-151). Added in v27; between points,
    /// with keep, since v29.
    pub service_runs: cp_traffic::ServiceRuns,
    /// Bus stops and lines (T-153). Added in v28.
    pub transit: cp_transit::TransitNet,
    /// Buses out on the lines and the week's transit money (T-156). Added in v30.
    pub fleet: cp_transit::TransitFleet,
    pub ledger: cp_transit::TransitLedger,
    /// Tourists on the way and the visits each building has had (T-168). Added in v33.
    pub tourists: cp_transit::Tourists,
    /// Stock of factories and shops and the trucks on the road (T-169). Added in v34.
    pub goods: cp_transit::Goods,
    /// The intercity train and what it has brought (T-167). Added in v36.
    pub intercity: cp_transit::Intercity,
}

/// v36 → v37 (T-170): goods trade. Trucks are factory-to-shop hauls, nothing has been exported
/// or imported, and the week report and the ledger get their export lines at zero.
pub fn migrate_v36_to_v37(v36: SaveDataV36) -> SaveDataV37 {
    let w = v36.economy.last_week;
    let l = v36.ledger;
    let goods = cp_transit::Goods {
        stock: v36.goods.stock,
        empty_since: v36.goods.empty_since,
        hauls: v36
            .goods
            .hauls
            .into_iter()
            .map(|(id, h)| {
                (
                    id,
                    cp_transit::Haul {
                        from: cp_transit::Endpoint::Building(h.from),
                        to: cp_transit::Endpoint::Building(h.to),
                        load: h.load,
                    },
                )
            })
            .collect(),
    };
    SaveDataV37 {
        tick: v36.tick,
        rng: v36.rng,
        speed: v36.speed,
        queue: v36.queue,
        log: v36.log,
        tiles: v36.tiles,
        net: v36.net,
        zones: v36.zones,
        buildings: v36.buildings,
        citizens: v36.citizens,
        paths: v36.paths,
        utilities: v36.utilities,
        trips: v36.trips,
        agents: v36.agents,
        treasury: v36.treasury,
        demand: v36.demand,
        demand_acc: v36.demand_acc,
        land: v36.land,
        decay: v36.decay,
        def_names: v36.def_names,
        district_map: v36.district_map,
        districts: v36.districts,
        service_state: v36.service_state,
        coverage: v36.coverage,
        shortage: v36.shortage,
        progress: v36.progress,
        service_runs: v36.service_runs,
        transit: v36.transit,
        fleet: v36.fleet,
        tourists: v36.tourists,
        economy: EconomyStateV37 {
            taxes: v36.economy.taxes,
            budgets: v36.economy.budgets,
            last_week: WeekReportV37 {
                week: w.week,
                income_by_zone: w.income_by_zone,
                road_upkeep: w.road_upkeep,
                building_upkeep: w.building_upkeep,
                utility_upkeep: w.utility_upkeep,
                loan_payments: w.loan_payments,
                transit_fares: w.transit_fares,
                transit_upkeep: w.transit_upkeep,
                tourism: w.tourism,
                exports: Cost::ZERO,
            },
            loans: v36.economy.loans,
        },
        ledger: cp_transit::TransitLedger {
            upkeep: l.upkeep,
            fares: l.fares,
            tourism: l.tourism,
            exports: Cost::ZERO,
            export_loads: 0,
            import_loads: 0,
        },
        goods,
        intercity: v36.intercity,
    }
}

/// The milestone progress as written by formats 25 to 38 (no unique buildings yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressV38 {
    pub reached: u8,
}

impl ProgressV38 {
    pub const ALL: Self = Self { reached: u8::MAX };
}

/// A district as format 39 wrote it: policies, no specialisations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistrictV39 {
    pub name: String,
    pub color: u8,
    pub tax: [Option<u8>; 6],
    pub policies: u32,
}

/// The district records as formats 38 and 39 wrote them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistrictsV39 {
    pub list: Vec<Option<DistrictV39>>,
}

/// Version 38: the body as written by format 38 (policies).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV38 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::ZoneGrid,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyState,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`DefNames`] and [`SaveData::remap_defs`].
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV39,
    /// The policies switched on for the whole city (T-190). Added in v38; a district's own are
    /// in its record.
    pub city_policies: CityPolicies,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached (v25) and the unique buildings opened (v38).
    pub progress: ProgressV38,
    /// Runs of service vehicles, buses and the like (T-151). Added in v27; between points,
    /// with keep, since v29.
    pub service_runs: cp_traffic::ServiceRuns,
    /// Bus stops and lines (T-153). Added in v28.
    pub transit: cp_transit::TransitNet,
    /// Buses out on the lines and the week's transit money (T-156). Added in v30.
    pub fleet: cp_transit::TransitFleet,
    pub ledger: cp_transit::TransitLedger,
    /// Tourists on the way and the visits each building has had (T-168). Added in v33.
    pub tourists: cp_transit::Tourists,
    /// Stock of factories and shops and the trucks on the road (T-169). Added in v34.
    pub goods: cp_transit::Goods,
    /// The intercity train and what it has brought (T-167). Added in v36.
    pub intercity: cp_transit::Intercity,
}

/// Version 39: the body as written by format 39 (unique buildings; district specialisations are not in it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV39 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    /// Zone blocks, painted types and cell validity. Added in v5.
    pub zones: cp_zone::ZoneGrid,
    /// Buildings (lots, state, access points). Added in v6.
    /// Includes the ids removed and not yet drained since v13.
    pub buildings: BuildingGrid,
    /// Citizens and households. Added in v7.
    pub citizens: CitizenGrid,
    /// Pathfinding requests in flight (id, submit tick, request, congestion read, stale
    /// flag). Added in v8; see `cp_traffic::service` for why this makes load + N ticks equal
    /// the continuous run.
    pub paths: SavedPaths,
    /// Pipe and power-line networks. Added in v9.
    pub utilities: UtilityGraph,
    /// Trips in flight and the routine's counters. Added in v10.
    pub trips: TripBook,
    /// Pedestrians and vehicles in the agent pools (v11); vehicles have lane state and the
    /// agents carry the segment load since v12; vehicles carry their re-route request id
    /// since v17.
    pub agents: cp_traffic::Agents,
    /// The city's money. Added in v14.
    pub treasury: Treasury,
    /// Tax rates, budgets, the last weekly report. Added in v14; the active loans in v20.
    pub economy: EconomyState,
    /// Smoothed demand and the sums of the cycle in progress. Added in v15.
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    /// Pollution, noise and land value grids. Added in v16.
    pub land: LandGrids,
    /// Abandonment timers. Added in v16.
    pub decay: Decay,
    /// Names of the definitions the `*DefId` indices above stand for. Added in v18; see
    /// [`DefNames`] and [`SaveData::remap_defs`].
    pub def_names: crate::DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: DistrictsV39,
    /// The policies switched on for the whole city (T-190). Added in v38; a district's own are
    /// in its record.
    pub city_policies: CityPolicies,
    /// Per-building service state and facility counters. Added in v22; fires name their
    /// station since v24.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
    /// Since when each workplace lacks workers (`NO_WORKERS`). Added in v23.
    pub shortage: cp_economy::WorkerShortage,
    /// The population milestone reached (v25) and the unique buildings opened (v39).
    pub progress: cp_sim::Progress,
    /// Runs of service vehicles, buses and the like (T-151). Added in v27; between points,
    /// with keep, since v29.
    pub service_runs: cp_traffic::ServiceRuns,
    /// Bus stops and lines (T-153). Added in v28.
    pub transit: cp_transit::TransitNet,
    /// Buses out on the lines and the week's transit money (T-156). Added in v30.
    pub fleet: cp_transit::TransitFleet,
    pub ledger: cp_transit::TransitLedger,
    /// Tourists on the way and the visits each building has had (T-168). Added in v33.
    pub tourists: cp_transit::Tourists,
    /// Stock of factories and shops and the trucks on the road (T-169). Added in v34.
    pub goods: cp_transit::Goods,
    /// The intercity train and what it has brought (T-167). Added in v36.
    pub intercity: cp_transit::Intercity,
}

/// v38 → v39 (T-200): unique buildings. None is open; the milestone stays.
pub fn migrate_v38_to_v39(v: SaveDataV38) -> SaveDataV39 {
    SaveDataV39 {
        tick: v.tick,
        rng: v.rng,
        speed: v.speed,
        queue: v.queue,
        log: v.log,
        tiles: v.tiles,
        net: v.net,
        zones: v.zones,
        buildings: v.buildings,
        citizens: v.citizens,
        paths: v.paths,
        utilities: v.utilities,
        trips: v.trips,
        agents: v.agents,
        treasury: v.treasury,
        economy: v.economy,
        demand: v.demand,
        demand_acc: v.demand_acc,
        land: v.land,
        decay: v.decay,
        def_names: v.def_names,
        district_map: v.district_map,
        districts: v.districts,
        city_policies: v.city_policies,
        service_state: v.service_state,
        coverage: v.coverage,
        shortage: v.shortage,
        progress: cp_sim::Progress::at(v.progress.reached),
        service_runs: v.service_runs,
        transit: v.transit,
        fleet: v.fleet,
        ledger: v.ledger,
        tourists: v.tourists,
        goods: v.goods,
        intercity: v.intercity,
    }
}

/// v39 → v40 (T-196): district specialisations. No district has one.
pub fn migrate_v39_to_v40(v: SaveDataV39) -> SaveData {
    SaveData {
        tick: v.tick,
        rng: v.rng,
        speed: v.speed,
        queue: v.queue,
        log: v.log,
        tiles: v.tiles,
        net: v.net,
        zones: v.zones,
        buildings: v.buildings,
        citizens: v.citizens,
        paths: v.paths,
        utilities: v.utilities,
        trips: v.trips,
        agents: v.agents,
        treasury: v.treasury,
        economy: v.economy,
        demand: v.demand,
        demand_acc: v.demand_acc,
        land: v.land,
        decay: v.decay,
        def_names: v.def_names,
        district_map: v.district_map,
        districts: Districts {
            list: v
                .districts
                .list
                .into_iter()
                .map(|d| {
                    d.map(|d| District {
                        name: d.name,
                        color: d.color,
                        tax: d.tax,
                        policies: d.policies,
                        spec: [None; 6],
                    })
                })
                .collect(),
        },
        city_policies: v.city_policies,
        service_state: v.service_state,
        coverage: v.coverage,
        shortage: v.shortage,
        progress: v.progress,
        service_runs: v.service_runs,
        transit: v.transit,
        fleet: v.fleet,
        ledger: v.ledger,
        tourists: v.tourists,
        goods: v.goods,
        intercity: v.intercity,
    }
}

/// v37 → v38 (T-190): policies. No district has one and the city has none; the week report
/// gets its policies line at zero.
pub fn migrate_v37_to_v38(v37: SaveDataV37) -> SaveDataV38 {
    let w = v37.economy.last_week;
    SaveDataV38 {
        tick: v37.tick,
        rng: v37.rng,
        speed: v37.speed,
        queue: v37.queue,
        log: v37.log,
        tiles: v37.tiles,
        net: v37.net,
        zones: v37.zones,
        buildings: v37.buildings,
        citizens: v37.citizens,
        paths: v37.paths,
        utilities: v37.utilities,
        trips: v37.trips,
        agents: v37.agents,
        treasury: v37.treasury,
        demand: v37.demand,
        demand_acc: v37.demand_acc,
        land: v37.land,
        decay: v37.decay,
        def_names: v37.def_names,
        district_map: v37.district_map,
        districts: DistrictsV39 {
            list: v37
                .districts
                .list
                .into_iter()
                .map(|d| {
                    d.map(|d| DistrictV39 {
                        name: d.name,
                        color: d.color,
                        tax: d.tax,
                        policies: 0,
                    })
                })
                .collect(),
        },
        city_policies: CityPolicies::default(),
        service_state: v37.service_state,
        coverage: v37.coverage,
        shortage: v37.shortage,
        progress: v37.progress,
        service_runs: v37.service_runs,
        transit: v37.transit,
        fleet: v37.fleet,
        tourists: v37.tourists,
        economy: EconomyState {
            taxes: v37.economy.taxes,
            budgets: v37.economy.budgets,
            last_week: WeekReport {
                week: w.week,
                income_by_zone: w.income_by_zone,
                road_upkeep: w.road_upkeep,
                building_upkeep: w.building_upkeep,
                utility_upkeep: w.utility_upkeep,
                loan_payments: w.loan_payments,
                transit_fares: w.transit_fares,
                transit_upkeep: w.transit_upkeep,
                tourism: w.tourism,
                exports: w.exports,
                policies: Cost::ZERO,
            },
            loans: v37.economy.loans,
        },
        ledger: v37.ledger,
        goods: v37.goods,
        intercity: v37.intercity,
    }
}

/// Everything from a v22 body to the current format.
fn from_v22(v: SaveDataV22) -> SaveData {
    let v = migrate_v24_to_v25(migrate_v23_to_v24(migrate_v22_to_v23(v)));
    from_v25(v)
}

fn from_v25(v: SaveDataV25) -> SaveData {
    from_v27(migrate_v26_to_v27(migrate_v25_to_v26(v)))
}

fn from_v27(v: SaveDataV27) -> SaveData {
    from_v29(migrate_v28_to_v29(migrate_v27_to_v28(v)))
}

fn from_v36(v: SaveDataV36) -> SaveData {
    migrate_v39_to_v40(migrate_v38_to_v39(migrate_v37_to_v38(migrate_v36_to_v37(
        v,
    ))))
}

fn from_v29(v: SaveDataV29) -> SaveData {
    from_v36(migrate_v35_to_v36(migrate_v34_to_v35(migrate_v33_to_v34(
        migrate_v32_to_v33(migrate_v31_to_v32(migrate_v30_to_v31(migrate_v29_to_v30(
            v,
        )))),
    ))))
}

fn from_v16(v: SaveDataV16) -> SaveData {
    from_v17(migrate_v16_to_v17(v))
}

fn from_v17(v: SaveDataV17) -> SaveData {
    from_v22(migrate_v21_to_v22(migrate_v20_to_v21(migrate_v19_to_v20(
        migrate_v18_to_v19(migrate_v17_to_v18(v)),
    ))))
}

/// Decodes a body written at `version` and runs the migration chain up to
/// [`crate::FORMAT_VERSION`]. Add one `match` arm per version and one chain step per bump.
pub fn decode_and_migrate(version: u32, body: &[u8]) -> Result<SaveData, SaveError> {
    let mut data = decode_and_migrate_ids(version, body)?;
    if version < 18 {
        data.def_names = crate::def_tables::def_names_before_v18(version);
    }
    Ok(data)
}

fn decode_and_migrate_ids(version: u32, body: &[u8]) -> Result<SaveData, SaveError> {
    fn dec<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, SaveError> {
        let (value, rest) =
            postcard::take_from_bytes(body).map_err(|e| SaveError::Decode(e.to_string()))?;
        if rest.is_empty() {
            Ok(value)
        } else {
            Err(SaveError::Decode("trailing bytes after save body".into()))
        }
    }
    // One linear chain: every arm decodes its version's body and then applies each
    // remaining `migrate_vN_to_vN1` step in order.
    match version {
        0 => {
            let v = migrate_v0_to_v1(dec(body)?);
            let v = migrate_v2_to_v3(v);
            let v = migrate_v3_to_v4(v);
            let v = migrate_v4_to_v5(v);
            let v = migrate_v5_to_v6(v);
            let v = migrate_v6_to_v7(v);
            let v = migrate_v7_to_v8(v);
            Ok(up(migrate_v9_to_v10(migrate_v8_to_v9(v))))
        }
        // v1 and v2 share the same body; v2 only added the header CRC.
        1 | 2 => {
            let v = migrate_v2_to_v3(dec(body)?);
            let v = migrate_v3_to_v4(v);
            let v = migrate_v4_to_v5(v);
            let v = migrate_v5_to_v6(v);
            let v = migrate_v6_to_v7(v);
            let v = migrate_v7_to_v8(v);
            Ok(up(migrate_v9_to_v10(migrate_v8_to_v9(v))))
        }
        3 => {
            let v = migrate_v3_to_v4(dec(body)?);
            let v = migrate_v4_to_v5(v);
            let v = migrate_v5_to_v6(v);
            let v = migrate_v6_to_v7(v);
            let v = migrate_v7_to_v8(v);
            Ok(up(migrate_v9_to_v10(migrate_v8_to_v9(v))))
        }
        4 => {
            let v = migrate_v4_to_v5(dec(body)?);
            let v = migrate_v5_to_v6(v);
            let v = migrate_v6_to_v7(v);
            let v = migrate_v7_to_v8(v);
            Ok(up(migrate_v9_to_v10(migrate_v8_to_v9(v))))
        }
        5 => {
            let v = migrate_v5_to_v6(dec(body)?);
            let v = migrate_v6_to_v7(v);
            let v = migrate_v7_to_v8(v);
            Ok(up(migrate_v9_to_v10(migrate_v8_to_v9(v))))
        }
        6 => {
            let v = migrate_v6_to_v7(dec(body)?);
            let v = migrate_v7_to_v8(v);
            Ok(up(migrate_v9_to_v10(migrate_v8_to_v9(v))))
        }
        7 => Ok(up(migrate_v9_to_v10(migrate_v8_to_v9(migrate_v7_to_v8(
            dec(body)?,
        ))))),
        8 => Ok(up(migrate_v9_to_v10(migrate_v8_to_v9(dec(body)?)))),
        9 => Ok(up(migrate_v9_to_v10(dec(body)?))),
        10 => Ok(up(dec(body)?)),
        11 => Ok(from_v12(migrate_v11_to_v12(dec(body)?))),
        12 => Ok(from_v12(dec(body)?)),
        13 => Ok(from_v13(dec(body)?)),
        14 => Ok(from_v14(dec(body)?)),
        15 => Ok(from_v15(dec(body)?)),
        16 => Ok(from_v16(dec(body)?)),
        17 => Ok(from_v17(dec(body)?)),
        18 => Ok(from_v22(migrate_v21_to_v22(migrate_v20_to_v21(
            migrate_v19_to_v20(migrate_v18_to_v19(dec(body)?)),
        )))),
        19 => Ok(from_v22(migrate_v21_to_v22(migrate_v20_to_v21(
            migrate_v19_to_v20(dec(body)?),
        )))),
        20 => Ok(from_v22(migrate_v21_to_v22(migrate_v20_to_v21(dec(body)?)))),
        21 => Ok(from_v22(migrate_v21_to_v22(dec(body)?))),
        22 => Ok(from_v22(dec(body)?)),
        23 => Ok(from_v25(migrate_v24_to_v25(migrate_v23_to_v24(dec(body)?)))),
        24 => Ok(from_v25(migrate_v24_to_v25(dec(body)?))),
        25 => Ok(from_v25(dec(body)?)),
        26 => Ok(from_v27(migrate_v26_to_v27(dec(body)?))),
        27 => Ok(from_v27(dec(body)?)),
        28 => Ok(from_v29(migrate_v28_to_v29(dec(body)?))),
        29 => Ok(from_v29(dec(body)?)),
        30 => Ok(from_v36(migrate_v35_to_v36(migrate_v34_to_v35(
            migrate_v33_to_v34(migrate_v32_to_v33(migrate_v31_to_v32(migrate_v30_to_v31(
                dec(body)?,
            )))),
        )))),
        31 => Ok(from_v36(migrate_v35_to_v36(migrate_v34_to_v35(
            migrate_v33_to_v34(migrate_v32_to_v33(migrate_v31_to_v32(dec(body)?))),
        )))),
        32 => Ok(from_v36(migrate_v35_to_v36(migrate_v34_to_v35(
            migrate_v33_to_v34(migrate_v32_to_v33(dec(body)?)),
        )))),
        33 => Ok(from_v36(migrate_v35_to_v36(migrate_v34_to_v35(
            migrate_v33_to_v34(dec(body)?),
        )))),
        34 => Ok(from_v36(migrate_v35_to_v36(migrate_v34_to_v35(dec(body)?)))),
        35 => Ok(from_v36(migrate_v35_to_v36(dec(body)?))),
        36 => Ok(from_v36(dec(body)?)),
        37 => Ok(migrate_v39_to_v40(migrate_v38_to_v39(migrate_v37_to_v38(
            dec(body)?,
        )))),
        38 => Ok(migrate_v39_to_v40(migrate_v38_to_v39(dec(body)?))),
        39 => Ok(migrate_v39_to_v40(dec(body)?)),
        40 => dec(body),
        v => Err(SaveError::UnsupportedVersion(v)),
    }
}

/// Writes a **version 0** file. Legacy writer kept only to (re)generate and test the v0
/// fixture; real saves always use [`crate::save`].
pub fn encode_v0(v0: &SaveDataV0, content_hash: u64, map_id: &str) -> Result<Vec<u8>, SaveError> {
    let mut out = Vec::new();
    crate::write_header(
        &mut out,
        &crate::Header {
            format_version: 0,
            content_hash,
            tick: v0.tick.0,
            map_id: map_id.to_owned(),
            map_hash: None,
            body_crc: None,
        },
    )?;
    let body = postcard::to_stdvec(v0).expect("serialization failed");
    out.extend_from_slice(&crate::compress(&body));
    Ok(out)
}

/// Writes a **version 1** file (no body CRC). Legacy writer kept only to (re)generate and
/// test the v1 fixture; real saves always use [`crate::save`].
pub fn encode_v1(v1: &SaveDataV2, content_hash: u64, map_id: &str) -> Result<Vec<u8>, SaveError> {
    let mut out = Vec::new();
    crate::write_header(
        &mut out,
        &crate::Header {
            format_version: 1,
            content_hash,
            tick: v1.tick.0,
            map_id: map_id.to_owned(),
            map_hash: None,
            body_crc: None,
        },
    )?;
    let body = postcard::to_stdvec(v1).expect("serialization failed");
    out.extend_from_slice(&crate::compress(&body));
    Ok(out)
}

/// Writes a **version 2** file (body CRC, no network). Legacy writer kept only to
/// (re)generate and test the v2 fixture; real saves always use [`crate::save`].
pub fn encode_v2(v2: &SaveDataV2, content_hash: u64, map_id: &str) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v2).expect("serialization failed");
    let frame = crate::compress(&body);
    let mut out = Vec::new();
    crate::write_header(
        &mut out,
        &crate::Header {
            format_version: 2,
            content_hash,
            tick: v2.tick.0,
            map_id: map_id.to_owned(),
            map_hash: None,
            body_crc: Some(crate::crc32(&frame)),
        },
    )?;
    out.extend_from_slice(&frame);
    Ok(out)
}

/// Writes a **version 3** file (body-only CRC, no map hash). Legacy writer kept only to
/// (re)generate and test the v3 fixture; real saves always use [`crate::save`].
pub fn encode_v3(v3: &SaveDataV3, content_hash: u64, map_id: &str) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v3).expect("serialization failed");
    let frame = crate::compress(&body);
    let mut out = Vec::new();
    crate::write_header(
        &mut out,
        &crate::Header {
            format_version: 3,
            content_hash,
            tick: v3.tick.0,
            map_id: map_id.to_owned(),
            map_hash: None,
            body_crc: Some(crate::crc32(&frame)),
        },
    )?;
    out.extend_from_slice(&frame);
    Ok(out)
}

/// Writes a **version 4** file (whole-file CRC and map hash, no zone state). Legacy writer
/// kept only to regenerate and test the v4 fixture; real saves always use [`crate::save`].
pub fn encode_v4(
    v4: &SaveDataV4,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v4).expect("serialization failed");
    crate::assemble(4, v4.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 5** file (zone state, no buildings). Legacy writer kept only to
/// regenerate and test the v5 fixture; real saves always use [`crate::save`].
pub fn encode_v5(
    v5: &SaveDataV5,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v5).expect("serialization failed");
    crate::assemble(5, v5.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 6** file (buildings, no citizens). Legacy writer kept only to
/// regenerate and test the v6 fixture; real saves always use [`crate::save`].
pub fn encode_v6(
    v6: &SaveDataV6,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v6).expect("serialization failed");
    crate::assemble(6, v6.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 7** file (citizens, no pending path requests). Legacy writer kept only
/// to regenerate and test the v7 fixture; real saves always use [`crate::save`].
pub fn encode_v7(
    v7: &SaveDataV7,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v7).expect("serialization failed");
    crate::assemble(7, v7.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 8** file (pending path requests, no utilities). Legacy writer kept
/// only to (re)generate and test the v8 fixture; real saves always use [`crate::save`].
pub fn encode_v8(
    v8: &SaveDataV8,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v8).expect("serialization failed");
    crate::assemble(8, v8.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 9** file (utility networks, no trips). Legacy writer kept only to
/// regenerate and test the v9 fixture; real saves always use [`crate::save`].
pub fn encode_v9(
    v9: &SaveDataV9,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v9).expect("serialization failed");
    crate::assemble(9, v9.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 10** file (trips, no agents). Legacy writer kept only to regenerate
/// and test the v10 fixture; real saves always use [`crate::save`].
pub fn encode_v10(
    v10: &SaveDataV10,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v10).expect("serialization failed");
    crate::assemble(10, v10.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 11** file (agents, placeholder vehicle motion). Legacy writer kept only
/// to regenerate and test the v11 fixture; real saves always use [`crate::save`].
pub fn encode_v11(
    v11: &SaveDataV11,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v11).expect("serialization failed");
    crate::assemble(11, v11.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 12** file (vehicle lane state, no removed-ids list). Legacy writer kept
/// only to test the v12 fixture; real saves always use [`crate::save`].
pub fn encode_v12(
    v12: &SaveDataV12,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v12).expect("serialization failed");
    crate::assemble(12, v12.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 13** file (removed-building ids, no treasury). Legacy writer kept only
/// to test the v13 fixture; real saves always use [`crate::save`].
pub fn encode_v13(
    v13: &SaveDataV13,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v13).expect("serialization failed");
    crate::assemble(13, v13.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 14** file (treasury and economy settings, no demand). Legacy writer kept
/// only to test the v14 fixture; real saves always use [`crate::save`].
pub fn encode_v14(
    v14: &SaveDataV14,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v14).expect("serialization failed");
    crate::assemble(14, v14.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 15** file (demand, no land grids or decay timers). Legacy writer kept
/// only to test the v15 fixture; real saves always use [`crate::save`].
pub fn encode_v15(
    v15: &SaveDataV15,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v15).expect("serialization failed");
    crate::assemble(15, v15.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 16** file (land grids and decay timers; vehicles without a re-route
/// id). Legacy writer kept only to test the v16 fixture; real saves always use [`crate::save`].
pub fn encode_v16(
    v16: &SaveDataV16,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v16).expect("serialization failed");
    crate::assemble(16, v16.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 17** file (re-route ids; no definition names). Legacy writer kept only
/// to test the v17 fixture; real saves always use [`crate::save`].
pub fn encode_v17(
    v17: &SaveDataV17,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v17).expect("serialization failed");
    crate::assemble(17, v17.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 18** file (definition names; buildings on lots with an access point).
/// Legacy writer kept only to test the v18 fixture; real saves always use [`crate::save`].
pub fn encode_v18(
    v18: &SaveDataV18,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v18).expect("serialization failed");
    crate::assemble(18, v18.tick.0, &body, content_hash, map_id, map_hash)
}
