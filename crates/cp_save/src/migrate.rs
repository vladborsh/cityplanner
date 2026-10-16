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
use cp_district::{DistrictMap, Districts};
use cp_economy::{Budgets, Decay, DemandAcc, EconomyState, LandGrids, TaxRates, WeekReport};
use cp_net::NetGraph;
use cp_sim::SimCommand;
use cp_terrain::TileOwnership;
use cp_traffic::CarPath;
use cp_traffic::{AgentStats, Agents, Pedestrian, SavedPaths, SegmentLoad, Vehicle};
use cp_utility::UtilityGraph;
use cp_zone::ZoneGrid;
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
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
    pub zones: ZoneGrid,
    pub buildings: BuildingGrid,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
    pub agents: Agents,
    pub treasury: Treasury,
    pub economy: EconomyState,
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
    pub zones: ZoneGrid,
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
    pub districts: Districts,
    /// Per-building service state and facility counters. Added in v22.
    pub service_state: cp_service::ServiceState,
    /// Service coverage along the roads as of the last 64-tick boundary. Saved because the
    /// sim reads the values of that boundary until the next one. Added in v22.
    pub coverage: cp_service::ServiceCoverage,
}

/// v22 → v23 (T-145): the `NO_WORKERS` timers were added; an old save starts with none.
pub fn migrate_v22_to_v23(v22: SaveDataV22) -> SaveData {
    SaveData {
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
    pub zones: ZoneGrid,
    pub buildings: BuildingGrid,
    pub citizens: CitizenGridV21,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
    pub trips: TripBook,
    pub agents: Agents,
    pub treasury: Treasury,
    pub economy: EconomyState,
    pub demand: Demand,
    pub demand_acc: DemandAcc,
    pub land: LandGrids,
    pub decay: Decay,
    pub def_names: crate::DefNames,
    pub district_map: DistrictMap,
    pub districts: Districts,
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
    pub zones: ZoneGrid,
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
        economy: EconomyState {
            taxes: v19.economy.taxes,
            budgets: v19.economy.budgets,
            last_week: WeekReport {
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
        zones: ZoneGrid::default(),
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

fn from_v16(v: SaveDataV16) -> SaveData {
    from_v17(migrate_v16_to_v17(v))
}

fn from_v17(v: SaveDataV17) -> SaveData {
    migrate_v22_to_v23(migrate_v21_to_v22(migrate_v20_to_v21(migrate_v19_to_v20(
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
        18 => Ok(migrate_v22_to_v23(migrate_v21_to_v22(migrate_v20_to_v21(
            migrate_v19_to_v20(migrate_v18_to_v19(dec(body)?)),
        )))),
        19 => Ok(migrate_v22_to_v23(migrate_v21_to_v22(migrate_v20_to_v21(
            migrate_v19_to_v20(dec(body)?),
        )))),
        20 => Ok(migrate_v22_to_v23(migrate_v21_to_v22(migrate_v20_to_v21(
            dec(body)?,
        )))),
        21 => Ok(migrate_v22_to_v23(migrate_v21_to_v22(dec(body)?))),
        22 => Ok(migrate_v22_to_v23(dec(body)?)),
        23 => dec(body),
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
