//! `cp_save` — save format, versions and migrations (`architecture/system-design.md` §12).
//!
//! **File layout** (all integers little-endian):
//!
//! | bytes | field |
//! |---|---|
//! | 4 | magic `"CPSV"` |
//! | 4 | `format_version: u32` |
//! | 8 | `content_hash: u64` (the `ContentDb` hash the save was made with) |
//! | 8 | `tick: u64` |
//! | 8 | `map_hash: u64` — `Terrain::map_hash()` of the map the save was made on, `0` = unknown; **version 4 and later only** |
//! | 2 + n | `map_id`: `u16` length, UTF-8 bytes (at most 65535 bytes) |
//! | 4 | `crc: u32` — CRC-32 (IEEE); **version 2 and later only** (see below) |
//! | rest | zstd frame (pure-Rust `ruzstd`) of the postcard-encoded body, nothing after it |
//!
//! **Integrity.** Version 4 and later: the CRC covers the whole file except the CRC field
//! itself (every header byte before it, then the compressed body), so a flipped bit in
//! `content_hash`, `tick`, `map_hash` or `map_id` is [`SaveError::CrcMismatch`], not a
//! silent load of another map or a bogus warning. Versions 2 and 3 only covered the
//! compressed body; version 0/1 have no CRC.
//!
//! **Map identity.** `map_hash` is supplied by the caller like `content_hash`
//! ([`save_with_map`]/[`load_with_map`]); [`save_runner`]/[`load_into_runner`] take it from the
//! runner's `SimTerrain` when one is set. A different map yields
//! [`LoadWarning::MapMismatch`], never an error. Saves before version 4, or made/loaded
//! without a terrain, have an *unknown* map hash and never warn.
//!
//! The header is read before the body so a version is known before decoding. The header
//! `tick` is only a preview: loading fails if it disagrees with the body. Trailing bytes
//! after the zstd frame, a CRC mismatch and a map id that does not fit are errors. Output is
//! deterministic: saving the same state twice gives identical bytes.
//!
//! **Versions.** During active development old saves are not migrated (the user's decision,
//! AGENTS.md, T-275): a file of another [`FORMAT_VERSION`] is refused with
//! [`SaveError::OlderVersion`] (or [`SaveError::UnsupportedVersion`] when it is newer). The header
//! of any version is still read, so the refusal names the version.
//!
//! **What is saved, what is rebuilt (audit, T-046).** Every resource `SimRunner::new` creates:
//!
//! | resource | in the save? |
//! |---|---|
//! | `Tick`, `SimRng`, `SimSpeed`, `CommandQueue`, `CommandLog` | yes |
//! | `TileOwnership`, `NetGraph`, `ZoneGrid` (occupancy is rebuilt from the buildings), `BuildingGrid` (incl. the removed ids), `UtilityGraph`, `CitizenGrid` (occupancy tables rebuilt), `TripBook`, `Agents` (pools, lane state, segment load; drive tables rebuilt) | yes |
//! | `PathService` (requests in flight) | yes, as `SavedPaths` |
//! | `TransitNet`, `TransitFleet`, `TransitLedger`, `Tourists`, `Goods`, `Intercity` (stops, lines, riders, line figures, buses, week's money, tourist visits, goods stock and trucks; the routes, `LinePlans`, are derived) | yes |
//! | `ResourceDeposits` (ore and oil left per 64 m cell; a save from before v41 is filled from the map by `load_into_runner`) | yes |
//! | `AreaMap`, `Areas`, `IndustryState` (stock and problems of the chain buildings), `IndustryLedger`, `WarehouseSettings`, `ParkState` (attractiveness, capacity, visitors and fee per park area), `LeisureTargets`, `ParkVisits` (who is inside, the walkers), `ParkLedger`, `CargoHubs` (cargo station stock and orders) | yes |
//! | `Treasury`, `EconomyState`, `Demand`, `DemandAcc`, `LandGrids`, `Decay`, `WorkerShortage`, `Progress`, `DistrictMap`, `Districts`, `CityPolicies` | yes |
//! | `JunctionGeometry`, `PathNetwork` (snapshot + congestion map; the published congestion is restored from the agents) | derived, rebuilt by the first ticks |
//! | `SimChecksum` (log hash cache), `ArenaGauges`, `PathResults` (cleared every tick), `Messages<CommandOutcome>` | transient |
//! | `SimLimits`, `RoutineSettings`, `AgentSettings`, `SimContent`, `SimTerrain` | configuration of the runner, not of the game |
//!
//! Nothing is missing: the fixtures `m1-small` and `m1-traffic` (cars and pedestrians in
//! flight, removed ids pending) load and continue to a recorded checksum, and the mid-run
//! save tests compare a loaded run with the continuous one.
//!
//! **Adding state** (a domain's arenas, etc.): add a field to [`SaveData`], bump
//! [`FORMAT_VERSION`] and extend `capture`/`restore`; the fixtures the tests load
//! (`fixtures/saves/m1-*.sav`, the interchange, the benches) are made again by their generators
//! with the new writer.

use std::io::Read;

use bevy_ecs::world::World;
use cp_building::{BuildingGrid, Demand};
use cp_citizen::{CitizenGrid, TripBook};
use cp_content::{BuildingDefId, ContentDb, DefKind, RoadDefId};
use cp_core::{CommandLog, CommandQueue, SimRng, SimSpeed, Tick, Treasury};
use cp_district::{AreaMap, Areas, CityPolicies, DistrictMap, Districts};
use cp_economy::{Decay, DemandAcc, EconomyState, LandGrids, WorkerShortage};
use cp_net::NetGraph;
use cp_sim::{SimCommand, SimRunner, SimTerrain};
use cp_terrain::TileOwnership;
use cp_traffic::SavedPaths;
use cp_utility::UtilityGraph;
use cp_zone::ZoneGrid;
use serde::{Deserialize, Serialize};

pub const MAGIC: [u8; 4] = *b"CPSV";
/// Current save format version.
pub const FORMAT_VERSION: u32 = 58;

/// The string ids behind the dense `*DefId` indices a save holds: entry `i` of a list names the
/// definition that index `i` meant when the file was written (`system-design.md` §12).
/// Empty lists (saves older than v18, or saved without content) mean "the indices already match
/// the content that loads them".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefNames {
    pub roads: Vec<String>,
    pub buildings: Vec<String>,
    pub services: Vec<String>,
    pub vehicles: Vec<String>,
    pub utilities: Vec<String>,
}

impl DefNames {
    /// The names of `db`'s tables, in id order.
    pub fn from_content(db: &ContentDb) -> Self {
        Self {
            roads: db.def_names(DefKind::Road),
            buildings: db.def_names(DefKind::Building),
            services: db.def_names(DefKind::Service),
            vehicles: db.def_names(DefKind::Vehicle),
            utilities: db.def_names(DefKind::Utility),
        }
    }
}

/// Saved index -> index in the loading content, for one table.
struct IdMap {
    kind: DefKind,
    /// `None`: the saved table is the current one (or absent); ids stay.
    table: Option<Vec<u32>>,
    used: std::cell::RefCell<std::collections::BTreeSet<u32>>,
}

impl IdMap {
    fn new(kind: DefKind, saved: &[String], db: &ContentDb) -> Self {
        let table: Vec<u32> = saved
            .iter()
            .map(|n| {
                db.def_index(kind, n)
                    .unwrap_or_else(|| ContentDb::placeholder(kind))
            })
            .collect();
        let identity = table.iter().enumerate().all(|(i, t)| *t == i as u32);
        Self {
            kind,
            table: (!saved.is_empty() && !identity).then_some(table),
            used: Default::default(),
        }
    }

    fn map(&self, old: u32) -> u32 {
        let Some(table) = &self.table else {
            return old;
        };
        self.used.borrow_mut().insert(old);
        table
            .get(old as usize)
            .copied()
            .unwrap_or_else(|| ContentDb::placeholder(self.kind))
    }

    /// One warning per saved name that was used and is not in the current content.
    fn warnings(&self, saved: &[String], db: &ContentDb) -> Vec<LoadWarning> {
        self.used
            .borrow()
            .iter()
            .filter_map(|&i| saved.get(i as usize))
            .filter(|n| db.def_index(self.kind, n).is_none())
            .map(|n| LoadWarning::UnknownDef {
                kind: self.kind,
                name: n.clone(),
            })
            .collect()
    }
}

/// Everything the sim needs to resume, at [`FORMAT_VERSION`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveData {
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
    pub def_names: DefNames,
    /// The district grid and the district records. Added in v21.
    pub district_map: DistrictMap,
    pub districts: Districts,
    /// The industry and park areas (T-206). Added in v42.
    pub area_map: AreaMap,
    pub areas: Areas,
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
    pub shortage: WorkerShortage,
    /// The population milestone reached (v25) and the unique buildings opened (v38).
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
    /// Ore and oil left per resource cell (T-205). Added in v41; older saves are filled from
    /// the map by [`load_into_runner`] (empty here: a migration has no map).
    pub deposits: cp_terrain::ResourceDeposits,
    /// Stock and problems of the chain buildings (T-209). Added in v43.
    pub industry: cp_industry::IndustryState,
    /// The week's cargo trade money (T-210). Added in v44; shop-goods trucks carry a cargo
    /// since v44.
    pub industry_ledger: cp_industry::IndustryLedger,
    /// The modes and cargo of the warehouses that were set (T-214). Added in v45.
    pub warehouses: cp_industry::WarehouseSettings,
    /// The park areas: attractiveness, capacity, visitors, fee (T-223). Added in v47.
    pub parks: cp_park::ParkState,
    /// Where residents may go in their free hours (T-224). Added in v48.
    pub leisure: cp_citizen::LeisureTargets,
    /// Who is inside the parks and the walkers (T-224). Added in v48.
    pub park_visits: cp_park::ParkVisits,
    /// The entrance fees of the week (T-224). Added in v48.
    pub park_ledger: cp_park::ParkLedger,
    /// The cargo stations' stock and orders (T-216). Added in v50; the ships
    /// in v51 (T-217), the trains and the goods premium in v52 (T-216b).
    pub hubs: cp_transit::CargoHubs,
    /// The mail waiting in the buildings and the sorting backlog (T-247). Added in v53.
    pub mail: cp_service::MailState,
}

impl SaveData {
    /// Re-assigns every road and building definition id in the data (segments, zone
    /// fingerprints, buildings, the command log and queue) from the saved names to the indices
    /// of `db`. A name `db` does not have becomes the built-in placeholder and is returned as
    /// a [`LoadWarning::UnknownDef`]. Afterwards `def_names` is `db`'s own table. Saves
    /// without names keep their ids. The order of everything is by saved index, so the result
    /// does not depend on anything but the save and `db`.
    pub fn remap_defs(&mut self, db: &ContentDb) -> Vec<LoadWarning> {
        let roads = IdMap::new(DefKind::Road, &self.def_names.roads, db);
        let buildings = IdMap::new(DefKind::Building, &self.def_names.buildings, db);
        if roads.table.is_some() {
            let f = |r: RoadDefId| RoadDefId(roads.map(r.0));
            self.net.remap_roads(f);
            self.zones.remap_roads(f);
        }
        if buildings.table.is_some() {
            self.buildings
                .remap_defs(|b| BuildingDefId(buildings.map(b.0)));
        }
        if roads.table.is_some() || buildings.table.is_some() {
            let rf = |r: RoadDefId| RoadDefId(roads.map(r.0));
            let bf = |b: BuildingDefId| BuildingDefId(buildings.map(b.0));
            self.log.map_commands(|c| c.remap_defs(&rf, &bf));
            self.queue.map_commands(|c| c.remap_defs(&rf, &bf));
        }
        let mut warnings = roads.warnings(&self.def_names.roads, db);
        warnings.extend(buildings.warnings(&self.def_names.buildings, db));
        self.def_names = DefNames::from_content(db);
        warnings
    }

    /// Snapshots a runner's sim state.
    pub fn capture(world: &World) -> Self {
        Self {
            tick: *world.resource::<Tick>(),
            rng: world.resource::<SimRng>().clone(),
            speed: *world.resource::<SimSpeed>(),
            queue: world.resource::<CommandQueue<SimCommand>>().clone(),
            log: world.resource::<CommandLog<SimCommand>>().clone(),
            tiles: *world.resource::<TileOwnership>(),
            net: world.resource::<NetGraph>().clone(),
            zones: world.resource::<ZoneGrid>().clone(),
            buildings: world.resource::<BuildingGrid>().clone(),
            citizens: world.resource::<CitizenGrid>().clone(),
            paths: world.resource::<cp_traffic::PathService>().export(),
            utilities: world.resource::<UtilityGraph>().clone(),
            trips: world.resource::<TripBook>().clone(),
            agents: world.resource::<cp_traffic::Agents>().clone(),
            treasury: *world.resource::<Treasury>(),
            economy: world.resource::<EconomyState>().clone(),
            demand: *world.resource::<Demand>(),
            demand_acc: *world.resource::<DemandAcc>(),
            land: world.resource::<LandGrids>().clone(),
            decay: world.resource::<Decay>().clone(),
            def_names: world
                .get_resource::<cp_content::SimContent>()
                .map_or_else(DefNames::default, |c| DefNames::from_content(&c.0)),
            district_map: world.resource::<DistrictMap>().clone(),
            districts: world.resource::<Districts>().clone(),
            area_map: world.resource::<AreaMap>().clone(),
            areas: world.resource::<Areas>().clone(),
            city_policies: *world.resource::<CityPolicies>(),
            service_state: world.resource::<cp_service::ServiceState>().clone(),
            coverage: world.resource::<cp_service::ServiceCoverage>().clone(),
            shortage: world.resource::<WorkerShortage>().clone(),
            progress: world.resource::<cp_sim::Progress>().clone(),
            service_runs: world.resource::<cp_traffic::ServiceRuns>().clone(),
            transit: world.resource::<cp_transit::TransitNet>().clone(),
            fleet: world.resource::<cp_transit::TransitFleet>().clone(),
            ledger: *world.resource::<cp_transit::TransitLedger>(),
            tourists: world.resource::<cp_transit::Tourists>().clone(),
            goods: world.resource::<cp_transit::Goods>().clone(),
            intercity: world.resource::<cp_transit::Intercity>().clone(),
            deposits: world.resource::<cp_terrain::ResourceDeposits>().clone(),
            industry: world.resource::<cp_industry::IndustryState>().clone(),
            industry_ledger: *world.resource::<cp_industry::IndustryLedger>(),
            warehouses: world.resource::<cp_industry::WarehouseSettings>().clone(),
            parks: world.resource::<cp_park::ParkState>().clone(),
            leisure: world.resource::<cp_citizen::LeisureTargets>().clone(),
            park_visits: world.resource::<cp_park::ParkVisits>().clone(),
            park_ledger: *world.resource::<cp_park::ParkLedger>(),
            hubs: world.resource::<cp_transit::CargoHubs>().clone(),
            mail: world.resource::<cp_service::MailState>().clone(),
        }
    }

    /// Overwrites the runner's sim state. Add plugins to the runner separately.
    pub fn restore(self, world: &mut World) {
        world.insert_resource(self.tick);
        world.insert_resource(self.rng);
        world.insert_resource(self.speed);
        world.insert_resource(self.queue);
        world.insert_resource(self.log);
        // The checksum caches a hash of the old log; it must not outlive the replacement.
        if let Some(mut cs) = world.get_resource_mut::<cp_sim::SimChecksum>() {
            cs.reset_log_cache();
        }
        world.insert_resource(self.tiles);
        let restored_net = self.net.clone();
        world.insert_resource(self.net);
        // Derived from the net; rebuilt by the Network phase.
        world.insert_resource(cp_net::JunctionGeometry::default());
        // The saved `valid` masks already account for buildings; the occupancy set they
        // derive from is rebuilt from the buildings themselves.
        let mut zones = self.zones;
        zones.set_occupied(self.buildings.occupied_cells());
        world.insert_resource(zones);
        world.insert_resource(self.buildings);
        // Occupancy tables are derived; the citizen system rebuilds them on its next run.
        world.insert_resource(self.citizens);
        world.insert_resource(self.utilities);
        world.insert_resource(self.trips);
        world.insert_resource(self.treasury);
        world.insert_resource(self.economy);
        world.insert_resource(self.demand);
        world.insert_resource(self.demand_acc);
        world.insert_resource(self.land);
        world.insert_resource(self.decay);
        world.insert_resource(self.district_map);
        world.insert_resource(self.districts);
        world.insert_resource(self.area_map);
        world.insert_resource(self.areas);
        world.insert_resource(self.city_policies);
        world.insert_resource(self.service_state);
        world.insert_resource(self.shortage);
        world.insert_resource(self.progress);
        // The service unlocks are derived from the progress.
        cp_sim::progress::refresh_unlocks(world);
        world.insert_resource(self.service_runs);
        world.insert_resource(self.transit);
        world.insert_resource(self.fleet);
        world.insert_resource(self.ledger);
        world.insert_resource(self.tourists);
        world.insert_resource(self.goods);
        world.insert_resource(self.intercity);
        world.insert_resource(self.deposits);
        world.insert_resource(self.industry);
        world.insert_resource(self.industry_ledger);
        world.insert_resource(self.warehouses);
        world.insert_resource(self.parks);
        world.insert_resource(self.leisure);
        world.insert_resource(self.park_visits);
        world.insert_resource(self.park_ledger);
        world.insert_resource(self.hubs);
        world.insert_resource(self.mail);
        // The routes are derived: the first Network phase after the load attaches the stops
        // and plans every line again.
        world.insert_resource(cp_transit::LinePlans::default());
        // The snapshot cache is keyed by the generation, which a load must change.
        let mut coverage = self.coverage;
        coverage.generation = world
            .get_resource::<cp_service::ServiceCoverage>()
            .map_or(0, |c| c.generation)
            + 1;
        world.insert_resource(coverage);
        // The pathfinder reads the segment load last published to it (saved with the agents).
        let published = cp_traffic::CongestionMap(self.agents.load.published.clone());
        world.insert_resource(self.agents);
        // Requests in flight continue after the load: re-launched on the snapshot rebuilt from
        // the restored net, which only counts as a network change if the net differs.
        world
            .resource_mut::<cp_traffic::PathService>()
            .import(self.paths);
        world
            .resource_mut::<cp_traffic::PathNetwork>()
            .mark_restored(&restored_net);
        world
            .resource_mut::<cp_traffic::PathNetwork>()
            .set_congestion(published);
    }
}

/// Parsed file header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub format_version: u32,
    pub content_hash: u64,
    pub tick: u64,
    pub map_id: String,
    /// `Terrain::map_hash()` at save time; `None` = unknown (version < 4, or saved without
    /// a terrain).
    pub map_hash: Option<u64>,
    /// The stored CRC-32 field; `None` for versions before 2. Covers the compressed body
    /// (versions 2–3) or header + body (version 4 and later).
    pub body_crc: Option<u32>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SaveError {
    BadMagic,
    Truncated,
    /// File is from a newer build than this one.
    UnsupportedVersion(u32),
    /// File is from an older build: old saves are not migrated during development (T-275).
    OlderVersion(u32),
    Compression(String),
    Decode(String),
    /// The file does not match its CRC (bit rot or tampering).
    CrcMismatch,
    /// Bytes after the end of the compressed body.
    TrailingBytes,
    /// The header's preview tick disagrees with the body.
    TickMismatch {
        header: u64,
        body: u64,
    },
    /// `map_id` is longer than 65535 bytes (save only).
    MapIdTooLong(usize),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "not a CPSV save file"),
            Self::Truncated => write!(f, "save file is truncated"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported save format version {v}"),
            Self::OlderVersion(v) => write!(
                f,
                "save from an older version (format {v}; this build reads format {FORMAT_VERSION})"
            ),
            Self::Compression(e) => write!(f, "decompression failed: {e}"),
            Self::Decode(e) => write!(f, "save body is corrupt: {e}"),
            Self::CrcMismatch => write!(f, "save file failed its integrity check"),
            Self::TrailingBytes => write!(f, "unexpected data after the save body"),
            Self::TickMismatch { header, body } => {
                write!(f, "header tick {header} does not match body tick {body}")
            }
            Self::MapIdTooLong(n) => write!(f, "map id is {n} bytes, the limit is 65535"),
        }
    }
}

impl std::error::Error for SaveError {}

/// Non-fatal findings from [`load`], returned to the caller to show to the player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadWarning {
    /// The save was made with different content (defs) than what is loaded now.
    ContentHashMismatch { saved: u64, current: u64 },
    /// The save was made on a different map (terrain) than the one loaded now.
    MapMismatch { saved: u64, current: u64 },
    /// The save uses a definition (by name) the loaded content does not have; its things
    /// were given the built-in placeholder.
    UnknownDef { kind: DefKind, name: String },
}

#[derive(Debug)]
pub struct Loaded {
    pub header: Header,
    pub data: SaveData,
    pub warnings: Vec<LoadWarning>,
}

/// Header bytes up to (not including) the CRC field.
pub(crate) fn write_prefix(out: &mut Vec<u8>, h: &Header) -> Result<(), SaveError> {
    let id = h.map_id.as_bytes();
    let len = u16::try_from(id.len()).map_err(|_| SaveError::MapIdTooLong(id.len()))?;
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&h.format_version.to_le_bytes());
    out.extend_from_slice(&h.content_hash.to_le_bytes());
    out.extend_from_slice(&h.tick.to_le_bytes());
    if h.format_version >= 4 {
        out.extend_from_slice(&h.map_hash.unwrap_or(0).to_le_bytes());
    }
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(id);
    Ok(())
}

/// CRC-32 (IEEE 802.3, reflected, as in zlib/PNG) of `bytes`.
pub fn crc32(bytes: &[u8]) -> u32 {
    crc32_parts(&[bytes])
}

/// CRC-32 of the concatenation of `parts`.
pub fn crc32_parts(parts: &[&[u8]]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    let mut c = !0u32;
    for part in parts {
        for &b in *part {
            c = TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    !c
}

fn take<'a>(bytes: &mut &'a [u8], n: usize) -> Result<&'a [u8], SaveError> {
    if bytes.len() < n {
        return Err(SaveError::Truncated);
    }
    let (head, tail) = bytes.split_at(n);
    *bytes = tail;
    Ok(head)
}

/// A parsed header with the map id still raw (UTF-8 is checked after the CRC).
struct Parsed<'a> {
    format_version: u32,
    content_hash: u64,
    tick: u64,
    map_hash: Option<u64>,
    map_id: &'a [u8],
    crc: Option<u32>,
    /// Every byte before the CRC field.
    prefix: &'a [u8],
    frame: &'a [u8],
}

impl Parsed<'_> {
    /// Checks the stored CRC (if the version has one).
    fn check_crc(&self) -> Result<(), SaveError> {
        let Some(stored) = self.crc else {
            return Ok(());
        };
        let actual = if self.format_version >= 4 {
            crc32_parts(&[self.prefix, self.frame])
        } else {
            crc32(self.frame)
        };
        if stored == actual {
            Ok(())
        } else {
            Err(SaveError::CrcMismatch)
        }
    }

    fn header(&self) -> Result<Header, SaveError> {
        let map_id = String::from_utf8(self.map_id.to_vec())
            .map_err(|e| SaveError::Decode(e.to_string()))?;
        Ok(Header {
            format_version: self.format_version,
            content_hash: self.content_hash,
            tick: self.tick,
            map_id,
            map_hash: self.map_hash,
            body_crc: self.crc,
        })
    }
}

fn parse(all: &[u8]) -> Result<Parsed<'_>, SaveError> {
    let mut bytes = all;
    if take(&mut bytes, 4)? != MAGIC {
        return Err(SaveError::BadMagic);
    }
    let format_version = u32::from_le_bytes(take(&mut bytes, 4)?.try_into().unwrap());
    if format_version > FORMAT_VERSION {
        return Err(SaveError::UnsupportedVersion(format_version));
    }
    let content_hash = u64::from_le_bytes(take(&mut bytes, 8)?.try_into().unwrap());
    let tick = u64::from_le_bytes(take(&mut bytes, 8)?.try_into().unwrap());
    let map_hash = if format_version >= 4 {
        match u64::from_le_bytes(take(&mut bytes, 8)?.try_into().unwrap()) {
            0 => None,
            h => Some(h),
        }
    } else {
        None
    };
    let len = u16::from_le_bytes(take(&mut bytes, 2)?.try_into().unwrap());
    let map_id = take(&mut bytes, usize::from(len))?;
    let prefix = &all[..all.len() - bytes.len()];
    let crc = if format_version >= 2 {
        Some(u32::from_le_bytes(take(&mut bytes, 4)?.try_into().unwrap()))
    } else {
        None
    };
    Ok(Parsed {
        format_version,
        content_hash,
        tick,
        map_hash,
        map_id,
        crc,
        prefix,
        frame: bytes,
    })
}

/// Parses the header, returning it and the compressed body. Does **not** verify the CRC
/// (that is [`load`]'s job), so tools can inspect damaged files.
pub fn read_header(bytes: &[u8]) -> Result<(Header, &[u8]), SaveError> {
    let p = parse(bytes)?;
    Ok((p.header()?, p.frame))
}

pub(crate) fn compress(body: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(body, ruzstd::encoding::CompressionLevel::Fastest)
}

fn decompress(frame: &[u8]) -> Result<Vec<u8>, SaveError> {
    let mut src = frame;
    let mut out = Vec::new();
    {
        let mut dec = ruzstd::decoding::StreamingDecoder::new(&mut src)
            .map_err(|e| SaveError::Compression(e.to_string()))?;
        dec.read_to_end(&mut out)
            .map_err(|e| SaveError::Compression(e.to_string()))?;
    }
    if !src.is_empty() {
        return Err(SaveError::TrailingBytes);
    }
    Ok(out)
}

/// Encodes `data` as a save file at [`FORMAT_VERSION`] with an unknown map hash. Fails
/// only if `map_id` is longer than 65535 bytes.
pub fn save(data: &SaveData, content_hash: u64, map_id: &str) -> Result<Vec<u8>, SaveError> {
    save_with_map(data, content_hash, map_id, None)
}

/// Like [`save`], recording `map_hash` (`Terrain::map_hash()`) in the header.
pub fn save_with_map(
    data: &SaveData,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(data).expect("save serialization failed");
    assemble(
        FORMAT_VERSION,
        data.tick.0,
        &body,
        content_hash,
        map_id,
        map_hash,
    )
}

/// Builds a version >= 4 file (whole-file CRC) around an encoded body.
pub(crate) fn assemble(
    format_version: u32,
    tick: u64,
    body: &[u8],
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    debug_assert!(format_version >= 4);
    let frame = compress(body);
    let mut out = Vec::new();
    write_prefix(
        &mut out,
        &Header {
            format_version,
            content_hash,
            tick,
            map_id: map_id.to_owned(),
            map_hash,
            body_crc: None,
        },
    )?;
    let crc = crc32_parts(&[&out, &frame]);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&frame);
    Ok(out)
}

/// Decodes a save file of the current version (an older one is refused, T-275).
/// `current_content_hash` is the hash of the content now loaded; a difference yields a
/// [`LoadWarning`], not an error. The map is not checked (see [`load_with_map`]).
pub fn load(bytes: &[u8], current_content_hash: u64) -> Result<Loaded, SaveError> {
    load_with_map(bytes, current_content_hash, None)
}

/// Like [`load`]; `current_map_hash` is the loaded terrain's `map_hash()` (`None` = no
/// terrain, nothing to compare). A save whose recorded map hash differs gets
/// [`LoadWarning::MapMismatch`].
pub fn load_with_map(
    bytes: &[u8],
    current_content_hash: u64,
    current_map_hash: Option<u64>,
) -> Result<Loaded, SaveError> {
    let parsed = parse(bytes)?;
    parsed.check_crc()?;
    let header = parsed.header()?;
    if header.format_version < FORMAT_VERSION {
        return Err(SaveError::OlderVersion(header.format_version));
    }
    let body = decompress(parsed.frame)?;
    let (data, rest): (SaveData, _) =
        postcard::take_from_bytes(&body).map_err(|e| SaveError::Decode(e.to_string()))?;
    if !rest.is_empty() {
        return Err(SaveError::Decode("trailing bytes after save body".into()));
    }
    if header.tick != data.tick.0 {
        return Err(SaveError::TickMismatch {
            header: header.tick,
            body: data.tick.0,
        });
    }
    let mut warnings = Vec::new();
    if header.content_hash != current_content_hash {
        warnings.push(LoadWarning::ContentHashMismatch {
            saved: header.content_hash,
            current: current_content_hash,
        });
    }
    if let (Some(saved), Some(current)) = (header.map_hash, current_map_hash)
        && saved != current
    {
        warnings.push(LoadWarning::MapMismatch { saved, current });
    }
    Ok(Loaded {
        header,
        data,
        warnings,
    })
}

fn runner_map_hash(sim: &SimRunner) -> Option<u64> {
    sim.world()
        .get_resource::<SimTerrain>()
        .map(|t| t.0.map_hash())
}

/// Saves a runner's state; the map hash comes from its `SimTerrain` if one is set.
pub fn save_runner(sim: &SimRunner, content_hash: u64, map_id: &str) -> Result<Vec<u8>, SaveError> {
    save_with_map(
        &SaveData::capture(sim.world()),
        content_hash,
        map_id,
        runner_map_hash(sim),
    )
}

/// Loads `bytes` into `sim` (overwriting its sim state) and returns the header and
/// warnings. The map is compared against the runner's `SimTerrain` if one is set.
pub fn load_into_runner(
    sim: &mut SimRunner,
    bytes: &[u8],
    current_content_hash: u64,
) -> Result<(Header, Vec<LoadWarning>), SaveError> {
    let map_hash = runner_map_hash(sim);
    let mut loaded = load_with_map(bytes, current_content_hash, map_hash)?;
    // Ids are re-assigned by name against the content this runner has.
    if let Some(db) = sim
        .world()
        .get_resource::<cp_content::SimContent>()
        .map(|c| std::sync::Arc::clone(&c.0))
    {
        let more = loaded.data.remap_defs(&db);
        loaded.warnings.extend(more);
    }
    loaded.data.restore(sim.world_mut());
    Ok((loaded.header, loaded.warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_core::SimSpeed;
    use cp_sim::run_headless;

    const HASH: u64 = 0xABCD;

    /// A sim that has run a while with queued and applied commands.
    fn busy_runner(seed: u64, ticks: u64) -> SimRunner {
        let mut sim = SimRunner::new(seed);
        sim.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::X3,
            },
            Tick(5),
        );
        sim.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::X2,
            },
            Tick(10_000),
        );
        for _ in 0..ticks {
            sim.tick_once();
        }
        sim
    }

    #[test]
    fn save_load_save_is_byte_identical() {
        let sim = busy_runner(3, 50);
        let a = save_runner(&sim, HASH, "flatlands").unwrap();
        assert_eq!(a, save_runner(&sim, HASH, "flatlands").unwrap());

        let loaded = load(&a, HASH).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.header.format_version, FORMAT_VERSION);
        assert_eq!(loaded.header.map_id, "flatlands");
        assert_eq!(loaded.header.tick, 50);
        let b = save(&loaded.data, HASH, "flatlands").unwrap();
        assert_eq!(a, b);
        assert_eq!(loaded.data, SaveData::capture(sim.world()));
        assert_eq!(loaded.data.speed, SimSpeed::X3);
        assert_eq!(loaded.data.queue.len(), 1);
        assert_eq!(loaded.data.log.len(), 1);
    }

    #[test]
    fn load_then_run_matches_a_fresh_run() {
        let (a, n) = (100, 150);
        let mut first = busy_runner(9, a);
        let bytes = save_runner(&first, HASH, "m").unwrap();

        let mut resumed = SimRunner::new(0); // seed is overwritten by the save
        load_into_runner(&mut resumed, &bytes, HASH).unwrap();
        for _ in 0..n {
            resumed.tick_once();
            first.tick_once();
        }
        assert_eq!(resumed.checksum(), first.checksum());

        // And against run_headless doing the same commands over a + n ticks.
        let cmds = [
            (
                Tick(5),
                SimCommand::SetSpeed {
                    speed: SimSpeed::X3,
                },
            ),
            (
                Tick(10_000),
                SimCommand::SetSpeed {
                    speed: SimSpeed::X2,
                },
            ),
        ];
        assert_eq!(resumed.checksum(), run_headless(9, &cmds, a + n));
    }

    #[test]
    fn content_hash_mismatch_warns_but_loads() {
        let bytes = save_runner(&busy_runner(1, 3), HASH, "m").unwrap();
        let loaded = load(&bytes, HASH + 1).unwrap();
        assert_eq!(
            loaded.warnings,
            vec![LoadWarning::ContentHashMismatch {
                saved: HASH,
                current: HASH + 1
            }]
        );
        assert_eq!(loaded.data.tick, Tick(3));
    }

    /// T-275: old saves are not migrated during development; one of an older format is refused
    /// with the version it has.
    #[test]
    fn a_save_of_an_older_format_is_refused() {
        let body = postcard::to_stdvec(&SaveData::capture(SimRunner::new(1).world())).unwrap();
        let old = assemble(FORMAT_VERSION - 1, 0, &body, HASH, "map", None).unwrap();
        assert_eq!(
            load(&old, HASH).map(|_| ()),
            Err(SaveError::OlderVersion(FORMAT_VERSION - 1))
        );
        assert!(
            SaveError::OlderVersion(1)
                .to_string()
                .starts_with("save from an older version")
        );
    }

    #[test]
    fn network_survives_save_and_load() {
        use bevy_math::Vec3;
        use cp_content::ContentDb;
        use cp_core::curve::CubicBezier3;
        use cp_net::Elevation;

        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let road = db.road_id("medium_4l").unwrap();
        let def = db.road(road).clone();
        let mut sim = busy_runner(2, 20);
        {
            let mut g = sim.world_mut().resource_mut::<NetGraph>();
            let s = g
                .add_segment(
                    &def,
                    road,
                    CubicBezier3::new(
                        Vec3::ZERO,
                        Vec3::new(30.0, 0.0, 8.0),
                        Vec3::new(60.0, 0.0, -8.0),
                        Vec3::new(90.0, 0.0, 0.0),
                    ),
                    Elevation::Ground,
                )
                .unwrap();
            g.split_segment(&def, s, 0.4).unwrap();
        }
        let before = sim.checksum();
        let bytes = save_runner(&sim, HASH, "m").unwrap();

        let mut fresh = SimRunner::new(0);
        load_into_runner(&mut fresh, &bytes, HASH).unwrap();
        assert_eq!(
            fresh.world().resource::<NetGraph>(),
            sim.world().resource::<NetGraph>()
        );
        assert_eq!(fresh.world().resource::<NetGraph>().segment_count(), 2);
        assert_eq!(fresh.checksum(), before);
        assert_eq!(save_runner(&fresh, HASH, "m").unwrap(), bytes);
    }

    #[test]
    fn restore_discards_stale_junction_geometry_and_it_is_rebuilt() {
        use bevy_math::Vec3;
        use cp_content::ContentDb;
        use cp_core::curve::CubicBezier3;
        use cp_net::{Elevation, JunctionGeometry};

        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        let mut a = SimRunner::new(1);
        {
            let mut g = a.world_mut().resource_mut::<NetGraph>();
            g.add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::ZERO, Vec3::new(60.0, 0.0, 0.0)),
                Elevation::Ground,
            )
            .unwrap();
        }
        a.tick_once();
        assert_eq!(a.world().resource::<JunctionGeometry>().len(), 2);
        let bytes = save_runner(&a, HASH, "m").unwrap();

        // A runner with unrelated geometry: loading must replace it, then rebuild from the net.
        let mut b = SimRunner::new(2);
        {
            let mut g = b.world_mut().resource_mut::<NetGraph>();
            for tip in [Vec3::new(0.0, 0.0, 50.0), Vec3::new(50.0, 0.0, 0.0)] {
                g.add_segment(
                    &def,
                    road,
                    CubicBezier3::line(Vec3::new(9.0, 0.0, 9.0), tip + Vec3::new(9.0, 0.0, 9.0)),
                    Elevation::Ground,
                )
                .unwrap();
            }
        }
        b.tick_once();
        assert_eq!(b.world().resource::<JunctionGeometry>().len(), 3);
        load_into_runner(&mut b, &bytes, HASH).unwrap();
        assert!(b.world().resource::<JunctionGeometry>().is_empty());
        b.tick_once();
        let geo = b.world().resource::<JunctionGeometry>();
        assert_eq!(geo.len(), 2);
        // Same geometry as the original runner computes for the same net.
        let ga = a.world().resource::<JunctionGeometry>();
        for (id, j) in geo.iter() {
            assert_eq!(j.arms.len(), ga.get(*id).unwrap().arms.len());
            assert_eq!(j.polygon, ga.get(*id).unwrap().polygon);
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(load(b"NOPE", 0).unwrap_err(), SaveError::BadMagic);
        assert_eq!(load(b"CPS", 0).unwrap_err(), SaveError::Truncated);
        let mut bytes = save_runner(&busy_runner(1, 1), HASH, "m").unwrap();
        bytes[4..8].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert_eq!(
            load(&bytes, HASH).unwrap_err(),
            SaveError::UnsupportedVersion(FORMAT_VERSION + 1)
        );
        let mut bytes = save_runner(&busy_runner(1, 1), HASH, "m").unwrap();
        let n = bytes.len();
        bytes.truncate(n - 5);
        assert!(load(&bytes, HASH).is_err());
    }
}
