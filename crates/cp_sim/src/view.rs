//! `SimView`: the read-only snapshot that client crates (render, UI, tools) read.
//! See `architecture/system-design.md` §13.1.
//!
//! Render and UI code never touch the sim `World`; the app calls [`SimRunner::view`]
//! after stepping and publishes the result as a resource. Domains extend [`SimView`]
//! with their own fields (agent positions, dirty sets, overlay values, stats) as they land.
//! Everything in it must be cheap to clone (`Arc`s and plain values).

use std::sync::Arc;

use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use cp_building::BuildingGrid;
use cp_content::{ContentDb, SimContent};
use cp_core::{CommandLog, CommandQueue, GameTime, SimSpeed, Tick};
use cp_net::{JunctionGeometry, NetGraph};
pub use cp_terrain::SimTerrain;
use cp_terrain::{Terrain, TileOwnership};
use cp_traffic::AgentFrame;
use cp_utility::UtilityGraph;

use crate::checksum::{Fnv1a, SimChecksum};
use crate::command::SimCommand;
use crate::overlay::{OverlayData, ScalarGrid, zone_snapshot};
use crate::runner::SimRunner;

/// How full one arena is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArenaFill {
    pub name: &'static str,
    pub used: u32,
    pub capacity: u32,
}

/// Registered arena size gauges. Domain plugins add one per arena through
/// `SimBuilder::register_arena_gauge`; `SimView::stats` reports them.
#[derive(Resource, Default)]
pub struct ArenaGauges(pub(crate) Vec<Gauge>);

/// `(name, capacity, live-element count)`.
pub(crate) type Gauge = (&'static str, u32, fn(&World) -> usize);

impl ArenaGauges {
    pub fn register(&mut self, name: &'static str, capacity: u32, len: fn(&World) -> usize) {
        self.0.push((name, capacity, len));
    }
}

/// Counters for the debug window. Cheap to compute; refreshed with every [`SimView`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SimStats {
    /// Most recent periodic checksum `(tick it was taken at, value)`; one every 256 ticks.
    pub checksum: Option<(Tick, u64)>,
    /// One entry per registered arena, in registration order.
    pub arenas: Vec<ArenaFill>,
    /// Commands queued for a future tick.
    pub commands_pending: usize,
    /// Commands applied so far (length of the command log).
    pub commands_applied: usize,
}

/// Read-only snapshot of the sim after a tick.
#[derive(Resource, Clone)]
pub struct SimView {
    pub tick: Tick,
    pub game_time: GameTime,
    pub speed: SimSpeed,
    /// Shared handle to the terrain; `None` for a sim without a map (headless tests).
    pub terrain: Option<Arc<Terrain>>,
    /// Which tiles the player owns (client camera bounds, build-area overlays).
    pub tiles: TileOwnership,
    pub stats: SimStats,
    /// Derived junction geometry (shortened-segment cut points and polygons) for mesh
    /// building. Compare `junctions.version()` with the last built version; junctions with a
    /// greater `revision` need a rebuild, ids that vanished from the map were removed.
    pub junctions: JunctionGeometry,
    /// The road network (nodes, segments, lanes) as of this snapshot, for mesh building.
    /// Shared: it is re-cloned from the sim only when the network changed (detected through
    /// `junctions.version()` and the element counts), so unchanged ticks cost an `Arc` clone.
    pub net: Arc<NetGraph>,
    /// All buildings as of this snapshot, for mesh building. Shared like `net`: re-cloned
    /// only when `BuildingGrid::revision` changed.
    pub buildings: Arc<BuildingGrid>,
    /// The district grid and records (T-133), shared: re-cloned only when they changed.
    pub district_map: Arc<cp_district::DistrictMap>,
    pub districts: Arc<cp_district::Districts>,
    /// The content database (road types, building styles, ...), for tools, the build menu and
    /// mesh building; `None` for a sim without content.
    pub content: Option<Arc<ContentDb>>,
    /// Inputs of the overlays (zones, traffic load, land value, pollution), §13.4.
    pub overlays: OverlayData,
    /// The zone blocks and painted types as of this snapshot (tools, zone overlay). Shared and
    /// re-cloned only when `ZoneGrid::revision` changed.
    pub zones: Arc<cp_zone::ZoneGrid>,
    /// The pipe and power-line networks as of this snapshot (tools, power-line and overlay
    /// meshes). Shared: re-cloned only when a link was added or removed.
    pub utilities: Arc<UtilityGraph>,
    /// Poses of vehicles and pedestrians after this tick and after the previously published
    /// one, for the client to interpolate between. Shared; rebuilt only when the tick advanced.
    pub agents: Arc<AgentSnapshot>,
    /// Smoothed demand (R, C, I, O) for the demand bars (§11), read-only.
    pub demand: cp_building::Demand,
    /// The city's money (cents), read-only, for the top bar and the budget window.
    pub treasury: cp_core::Treasury,
    /// What the last daily settlement booked (T-179): the top bar's trend and pop-up.
    pub flow: cp_economy::MoneyFlow,
    /// Tax rates, budget percentages and the last settled week, read-only (budget window).
    pub economy: cp_economy::EconomyState,
    /// Service coverage of the roads by kind, for the overlays (T-134). Shared: re-cloned
    /// only when it was recomputed.
    pub service: Arc<cp_service::ServiceCoverage>,
    /// Service problems per building (bits of `ServiceProblems`) and the facilities' day
    /// counters, read-only. Shared: re-cloned only when the state changed.
    pub service_state: Arc<cp_service::ServiceState>,
    /// The city summary for the panels, recomputed every 64 ticks (§13.6). Shared: re-cloned
    /// only when it was recomputed.
    pub city: Arc<crate::stats::CityStats>,
    /// The population milestone reached (T-149), for locked tiles and the milestone notice.
    pub progress: crate::Progress,
    /// Stops, lines and the routes of the lines (T-157), shared: rebuilt only when they changed.
    pub transit: Arc<cp_transit::TransitView>,
    /// The metro trains and where they are now (T-164), for the Underground view.
    pub metro_trains: Arc<Vec<cp_transit::MetroTrain>>,
    /// The intercity train on the railway, if one is out (T-167).
    pub intercity_train: Option<Arc<cp_transit::IntercityPose>>,
    /// The transit lines with their figures (T-160), in line id order.
    pub lines: Vec<LineView>,
    /// Fares and bus upkeep of the week in progress.
    pub transit_ledger: cp_transit::TransitLedger,
    /// Goods trucks and tourists, for the metrics window (T-171).
    pub freight: FreightView,
}

/// Goods and tourism figures of the moment (T-171).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FreightView {
    /// Goods trucks under way (deliveries, exports and imports).
    pub trucks: u32,
    /// Of those, trucks carrying a load out of the map / in from it.
    pub trucks_exporting: u32,
    pub trucks_importing: u32,
    /// Tourists on the map now, and the visits that arrived since the week began.
    pub tourists_now: u32,
    pub tourists_week: u32,
}

/// One transit line as the Lines window shows it (T-160).
#[derive(Clone, Debug, PartialEq)]
pub struct LineView {
    pub id: cp_transit::LineId,
    pub name: String,
    pub mode: cp_transit::TransitMode,
    pub color: u8,
    pub stops: usize,
    /// Buses asked for, and buses out on the road now.
    pub vehicles: u8,
    pub vehicles_out: u16,
    pub state: cp_transit::LineState,
    pub riders_today: u32,
    pub riders_week: u32,
    pub waiting: u32,
}

/// A bank offer for the Loans tab: the content row, whether the city may take it now, and the
/// active loan on it, if any (T-129).
#[derive(Clone, Debug, PartialEq)]
pub struct LoanOfferView {
    /// The offer's index, what `TakeLoan`/`RepayLoan` name.
    pub index: u8,
    pub offer: cp_content::LoanOffer,
    /// The population threshold is met (as of the last city summary) and the slot is free.
    pub available: bool,
    pub active: Option<cp_economy::Loan>,
}

/// The cached `SimView` district snapshots.
pub type DistrictCache = (
    Option<Arc<cp_district::DistrictMap>>,
    Option<Arc<cp_district::Districts>>,
);

impl SimView {
    /// The bank's offers with their state. Empty for a sim without content.
    pub fn loan_offers(&self) -> Vec<LoanOfferView> {
        let Some(db) = &self.content else {
            return Vec::new();
        };
        db.balance()
            .loan_offers
            .iter()
            .enumerate()
            .take(255)
            .map(|(i, offer)| {
                let active = self.economy.loan(i as u8).copied();
                LoanOfferView {
                    index: i as u8,
                    available: active.is_none() && self.city.population >= offer.min_population,
                    active,
                    offer: offer.clone(),
                }
            })
            .collect()
    }
}

/// Agent poses of two consecutive snapshots. `prev` is the frame of the previous published
/// view (equal to `cur` the first time), so a client draws `lerp(prev, cur, fraction)` where
/// `fraction` is how far real time is into the tick that follows the snapshot.
#[derive(Debug, Default)]
pub struct AgentSnapshot {
    pub tick: Tick,
    pub prev: Arc<AgentFrame>,
    pub cur: Arc<AgentFrame>,
}

/// Last `SimView::agents`.
pub(crate) type AgentCache = Option<Arc<AgentSnapshot>>;

/// `(generation, snapshot)` of the last `SimView::city`.
/// `(generation, snapshot)` of the last `SimView::service`.
pub(crate) type ServiceCache = Option<(u64, Arc<cp_service::ServiceCoverage>)>;

/// `(revision, snapshot)` of the last `SimView::service_state`.
pub(crate) type ServiceStateCache = Option<(u64, Arc<cp_service::ServiceState>)>;

pub(crate) type CityCache = Option<(u64, Arc<crate::stats::CityStats>)>;

/// `(zone revision, snapshot)` of the last `SimView::zones`.
pub(crate) type ZoneCache = Option<(u64, Arc<cp_zone::ZoneGrid>)>;

/// `(revision, building count, snapshot)` of the last `SimView::buildings`.
pub(crate) type BuildingsCache = Option<(u64, usize, Arc<BuildingGrid>)>;

/// `(fingerprint of the links, snapshot)` of the last `SimView::utilities`.
pub(crate) type UtilitiesCache = Option<(u64, Arc<UtilityGraph>)>;

/// Last `SimView::transit`.
pub(crate) type TransitCache = Option<Arc<cp_transit::TransitView>>;

/// `(junction version, nodes, segments, snapshot)` of the last `SimView::net`.
/// `(junction version, node count, segment count)`: changes when the net does.
pub(crate) type NetKey = (u64, usize, usize);
/// The zone grid synced with the net as of `NetKey` (see `SimRunner::zones_now`).
pub(crate) type ZonePreviewCache = Option<(NetKey, Arc<cp_zone::ZoneGrid>)>;
pub(crate) type NetCache = Option<(u64, usize, usize, Arc<NetGraph>)>;

impl SimRunner {
    /// Puts the loaded map into the sim world. If nothing is owned yet (fresh sim),
    /// ownership starts as `TileOwnership::m1_around(meta.start_tile)`; ownership that
    /// is already set (e.g. restored from a save) is left alone. A save restored
    /// afterwards overwrites it, so call this before `load_into_runner`.
    pub fn set_terrain(&mut self, terrain: Arc<Terrain>) {
        let start = terrain.meta().start_tile;
        let world = self.world_mut();
        world.insert_resource(SimTerrain(terrain));
        if world
            .get_resource::<TileOwnership>()
            .is_none_or(|t| *t == TileOwnership::none())
        {
            world.insert_resource(TileOwnership::m1_around(start));
        }
    }

    /// Puts the content database into the sim world (read by command handlers, e.g.
    /// road defs for `NetCommand`). Call before pushing commands that need content.
    pub fn set_content(&mut self, content: Arc<ContentDb>) {
        self.world_mut().insert_resource(SimContent(content));
    }

    /// Key of the net as it is now (`None` without the resources).
    pub(crate) fn net_key(&self) -> Option<NetKey> {
        let j = self.world().get_resource::<JunctionGeometry>()?;
        let g = self.world().get_resource::<NetGraph>()?;
        Some((j.version(), g.node_count(), g.segment_count()))
    }

    /// The zone grid the clients should draw: the world's, or, when the net changed since the
    /// last tick (a road built while paused; the zone sync runs in the tick), a clone synced
    /// with the net. Derived, never saved or hashed (BUG-038).
    pub(crate) fn zones_now(&self) -> Option<Arc<cp_zone::ZoneGrid>> {
        let now = self.net_key()?;
        if self.zones_synced.is_none_or(|k| k == now) {
            return None;
        }
        let mut cache = self.zone_preview_cache.lock().expect("zone preview");
        if let Some((k, grid)) = cache.as_ref()
            && *k == now
        {
            return Some(Arc::clone(grid));
        }
        let w = self.world();
        let content = w.get_resource::<cp_content::SimContent>()?;
        let mut grid = w.get_resource::<cp_zone::ZoneGrid>()?.clone();
        if let Some(t) = w.get_resource::<cp_terrain::SimTerrain>()
            && !grid.has_terrain(&t.0)
        {
            grid.set_terrain(Arc::clone(&t.0));
        }
        cp_zone::sync_grid(
            &mut grid,
            w.get_resource::<NetGraph>()?,
            &content.0,
            w.get_resource::<JunctionGeometry>()?,
        );
        let grid = Arc::new(grid);
        *cache = Some((now, Arc::clone(&grid)));
        Some(grid)
    }

    /// The network snapshot for `SimView::net`, cached while the network is unchanged.
    fn net_snapshot(&self, junctions: &JunctionGeometry) -> Arc<NetGraph> {
        let graph = self.world().get_resource::<NetGraph>();
        let key = (
            junctions.version(),
            graph.map_or(0, NetGraph::node_count),
            graph.map_or(0, NetGraph::segment_count),
        );
        let mut cache = self.net_cache.lock().expect("net cache");
        if let Some((v, n, s, snap)) = cache.as_ref()
            && (*v, *n, *s) == key
        {
            return Arc::clone(snap);
        }
        let snap = Arc::new(graph.cloned().unwrap_or_default());
        *cache = Some((key.0, key.1, key.2, Arc::clone(&snap)));
        snap
    }

    /// The overlay inputs for `SimView::overlays`. Zones are cached while unchanged; traffic
    /// load comes from the agents' instantaneous segment load; land value and pollution stay
    /// empty until their domain (T-042) lands.
    fn overlay_data(&self) -> OverlayData {
        let mut cache = self.zone_cache.lock().expect("zone cache");
        let mut road_load = std::collections::BTreeMap::new();
        if let (Some(net), Some(agents)) = (
            self.world().get_resource::<cp_net::NetGraph>(),
            self.world().get_resource::<cp_traffic::Agents>(),
        ) {
            for (id, _) in net.segments() {
                road_load.insert(id, agents.load.inst(id));
            }
        }
        let (land_value, pollution) = self.land_overlays();
        OverlayData {
            zones: {
                let preview = self.zones_now();
                zone_snapshot(
                    preview
                        .as_deref()
                        .or(self.world().get_resource::<cp_zone::ZoneGrid>()),
                    &mut cache,
                )
            },
            road_load: Arc::new(road_load),
            land_value,
            pollution,
        }
    }

    /// Land value and pollution as `0..=1` heat-map grids, rebuilt only when the grids were.
    fn land_overlays(&self) -> (Option<Arc<ScalarGrid>>, Option<Arc<ScalarGrid>>) {
        let Some(g) = self.world().get_resource::<cp_economy::LandGrids>() else {
            return (None, None);
        };
        let mut cache = self.land_cache.lock().expect("land cache");
        if let Some((rev, v, p)) = cache.as_ref()
            && *rev == g.revision()
        {
            return (Some(Arc::clone(v)), Some(Arc::clone(p)));
        }
        let norm = |vs: &[f32]| {
            Arc::new(ScalarGrid::new(
                cp_economy::SIDE,
                vs.iter().map(|v| (v / 100.0).clamp(0.0, 1.0)).collect(),
            ))
        };
        let (v, p) = (norm(&g.land_value), norm(&g.pollution));
        *cache = Some((g.revision(), Arc::clone(&v), Arc::clone(&p)));
        (Some(v), Some(p))
    }

    /// The coverage snapshot for `SimView::service`, cached while its generation is unchanged.
    fn service_snapshot(&self) -> Arc<cp_service::ServiceCoverage> {
        let cov = self.world().get_resource::<cp_service::ServiceCoverage>();
        let generation = cov.map_or(0, |c| c.generation);
        let mut cache = self.service_cache.lock().expect("service cache");
        if let Some((g, snap)) = cache.as_ref()
            && *g == generation
        {
            return Arc::clone(snap);
        }
        let snap = Arc::new(cov.cloned().unwrap_or_default());
        *cache = Some((generation, Arc::clone(&snap)));
        snap
    }

    /// The service-state snapshot for `SimView::service_state`, cached by revision.
    fn service_state_snapshot(&self) -> Arc<cp_service::ServiceState> {
        let st = self.world().get_resource::<cp_service::ServiceState>();
        let rev = st.map_or(0, cp_service::ServiceState::revision);
        let mut cache = self
            .service_state_cache
            .lock()
            .expect("service state cache");
        if let Some((r, snap)) = cache.as_ref()
            && *r == rev
        {
            return Arc::clone(snap);
        }
        let snap = Arc::new(st.cloned().unwrap_or_default());
        *cache = Some((rev, Arc::clone(&snap)));
        snap
    }

    /// The zone snapshot for `SimView::zones`, cached while the grid's revision is unchanged.
    fn zone_snapshot(&self) -> Arc<cp_zone::ZoneGrid> {
        let preview = self.zones_now();
        let grid = preview
            .as_deref()
            .or(self.world().get_resource::<cp_zone::ZoneGrid>());
        let rev = grid.map_or(0, cp_zone::ZoneGrid::revision);
        let mut cache = self.zone_grid_cache.lock().expect("zone cache");
        if let Some((r, snap)) = cache.as_ref()
            && *r == rev
        {
            return Arc::clone(snap);
        }
        let snap = Arc::new(grid.cloned().unwrap_or_default());
        *cache = Some((rev, Arc::clone(&snap)));
        snap
    }

    /// `SimView::district_map`: the cached copy while the grid is equal to it (a 100 KB compare
    /// a view, instead of a copy).
    fn district_map_snapshot(&self) -> Arc<cp_district::DistrictMap> {
        let now = self.world().get_resource::<cp_district::DistrictMap>();
        let mut cache = self.district_cache.lock().expect("district cache");
        if let Some(snap) = cache.0.as_ref()
            && now.is_none_or(|n| **snap == *n)
        {
            return Arc::clone(snap);
        }
        let snap = Arc::new(now.cloned().unwrap_or_default());
        cache.0 = Some(Arc::clone(&snap));
        snap
    }

    /// `SimView::districts`, cached like [`SimRunner::district_map_snapshot`].
    fn districts_snapshot(&self) -> Arc<cp_district::Districts> {
        let now = self.world().get_resource::<cp_district::Districts>();
        let mut cache = self.district_cache.lock().expect("district cache");
        if let Some(snap) = cache.1.as_ref()
            && now.is_none_or(|n| **snap == *n)
        {
            return Arc::clone(snap);
        }
        let snap = Arc::new(now.cloned().unwrap_or_default());
        cache.1 = Some(Arc::clone(&snap));
        snap
    }

    /// The building snapshot for `SimView::buildings`, cached while the grid is unchanged.
    fn buildings_snapshot(&self) -> Arc<BuildingGrid> {
        let grid = self.world().get_resource::<BuildingGrid>();
        let key = (
            grid.map_or(0, BuildingGrid::revision),
            grid.map_or(0, BuildingGrid::len),
        );
        let mut cache = self.buildings_cache.lock().expect("buildings cache");
        if let Some((r, n, snap)) = cache.as_ref()
            && (*r, *n) == key
        {
            return Arc::clone(snap);
        }
        let snap = Arc::new(grid.cloned().unwrap_or_default());
        *cache = Some((key.0, key.1, Arc::clone(&snap)));
        snap
    }

    /// The utility snapshot for `SimView::utilities`. The key hashes the id (index and
    /// generation, so a removed-then-reused slot differs) and ends of every link and the node
    /// count: links are immutable once built, so any change shows up in it.
    fn utilities_snapshot(&self) -> Arc<UtilityGraph> {
        let graph = self.world().get_resource::<UtilityGraph>();
        let mut h = Fnv1a::new();
        if let Some(g) = graph {
            h.write_u64(g.node_count() as u64);
            for (id, l) in g.links() {
                for v in [id.index, id.generation, l.a.index, l.b.index] {
                    h.write_u64(u64::from(v));
                }
            }
        }
        let key = h.finish();
        let mut cache = self.utilities_cache.lock().expect("utilities cache");
        if let Some((k, snap)) = cache.as_ref()
            && *k == key
        {
            return Arc::clone(snap);
        }
        let snap = Arc::new(graph.cloned().unwrap_or_default());
        *cache = Some((key, Arc::clone(&snap)));
        snap
    }

    /// The transit snapshot for `SimView::transit`, kept while the stops and lines, the plans
    /// and the lane graph are the same.
    fn transit_snapshot(&self) -> Arc<cp_transit::TransitView> {
        let world = self.world();
        let (Some(net), Some(plans), Some(pnet)) = (
            world.get_resource::<cp_transit::TransitNet>(),
            world.get_resource::<cp_transit::LinePlans>(),
            world.get_resource::<cp_traffic::PathNetwork>(),
        ) else {
            return Arc::default();
        };
        let key = plans.fingerprint();
        let mut cache = self.transit_cache.lock().expect("transit cache");
        if let Some(snap) = cache.as_ref()
            && snap.plans_key == key
            && snap.net == *net
            && Arc::ptr_eq(&snap.graph, pnet.graph())
        {
            return Arc::clone(snap);
        }
        let snap = Arc::new(cp_transit::TransitView::build(net, plans, pnet.graph()));
        *cache = Some(Arc::clone(&snap));
        snap
    }

    /// The metro trains for `SimView::metro_trains` (none without a metro line).
    fn metro_trains(&self, tick: Tick) -> Vec<cp_transit::MetroTrain> {
        let world = self.world();
        let (Some(net), Some(plans), Some(fleet), Some(runs), Some(pnet)) = (
            world.get_resource::<cp_transit::TransitNet>(),
            world.get_resource::<cp_transit::LinePlans>(),
            world.get_resource::<cp_transit::TransitFleet>(),
            world.get_resource::<cp_traffic::ServiceRuns>(),
            world.get_resource::<cp_traffic::PathNetwork>(),
        ) else {
            return Vec::new();
        };
        if fleet.buses.is_empty() {
            return Vec::new();
        }
        cp_transit::metro_trains(net, plans, fleet, runs, pnet.graph(), tick)
    }

    /// The intercity train for `SimView::intercity_train` (none when no train is out).
    fn intercity_train(&self, tick: Tick) -> Option<cp_transit::IntercityPose> {
        let world = self.world();
        let intercity = world.get_resource::<cp_transit::Intercity>()?;
        intercity.train?;
        cp_transit::intercity_pose(
            intercity,
            world.get_resource::<cp_traffic::ServiceRuns>()?,
            world.get_resource::<cp_traffic::PathNetwork>()?.graph(),
            tick,
        )
    }

    /// The agent poses for `SimView::agents`, rebuilt only when the tick advanced.
    fn agent_snapshot(&self, tick: Tick) -> Arc<AgentSnapshot> {
        let mut cache = self.agents_cache.lock().expect("agents cache");
        if let Some(snap) = cache.as_ref()
            && snap.tick == tick
        {
            return Arc::clone(snap);
        }
        let world = self.world();
        let cur = match (
            world.get_resource::<cp_traffic::Agents>(),
            world.get_resource::<cp_traffic::PathNetwork>(),
            world.get_resource::<NetGraph>(),
            world.get_resource::<JunctionGeometry>(),
        ) {
            (Some(agents), Some(pnet), Some(net), Some(geo)) => {
                let mut frame = agents.frame(pnet.graph(), net, geo);
                tint_buses(world, agents, &mut frame);
                Arc::new(frame)
            }
            _ => Arc::default(),
        };
        let prev = cache
            .as_ref()
            .map_or_else(|| Arc::clone(&cur), |s| Arc::clone(&s.cur));
        let snap = Arc::new(AgentSnapshot { tick, prev, cur });
        *cache = Some(Arc::clone(&snap));
        snap
    }

    /// Snapshot of the current state for clients.
    pub fn view(&self) -> SimView {
        let tick = self.tick();
        let junctions = self
            .world()
            .get_resource::<JunctionGeometry>()
            .cloned()
            .unwrap_or_default();
        SimView {
            tick,
            game_time: tick.game_time(),
            speed: self.speed(),
            terrain: self
                .world()
                .get_resource::<SimTerrain>()
                .map(|t| Arc::clone(&t.0)),
            tiles: self
                .world()
                .get_resource::<TileOwnership>()
                .copied()
                .unwrap_or_default(),
            stats: self.stats(),
            net: self.net_snapshot(&junctions),
            buildings: self.buildings_snapshot(),
            district_map: self.district_map_snapshot(),
            districts: self.districts_snapshot(),
            zones: self.zone_snapshot(),
            content: self
                .world()
                .get_resource::<SimContent>()
                .map(|c| Arc::clone(&c.0)),
            overlays: self.overlay_data(),
            agents: self.agent_snapshot(tick),
            demand: *self.world().resource::<cp_building::Demand>(),
            treasury: *self.world().resource::<cp_core::Treasury>(),
            flow: self.world().resource::<cp_economy::MoneyFlow>().clone(),
            progress: *self.world().resource::<crate::Progress>(),
            economy: self.world().resource::<cp_economy::EconomyState>().clone(),
            lines: self.line_views(),
            transit_ledger: *self.world().resource::<cp_transit::TransitLedger>(),
            freight: self.freight_view(),
            utilities: self.utilities_snapshot(),
            transit: self.transit_snapshot(),
            metro_trains: Arc::new(self.metro_trains(tick)),
            intercity_train: self.intercity_train(tick).map(Arc::new),
            service: self.service_snapshot(),
            service_state: self.service_state_snapshot(),
            city: self.city_snapshot(),
            junctions,
        }
    }

    fn freight_view(&self) -> FreightView {
        use cp_transit::Endpoint;
        let w = self.world();
        let mut f = FreightView::default();
        if let Some(g) = w.get_resource::<cp_transit::Goods>() {
            f.trucks = g.hauls.len() as u32;
            for h in g.hauls.values() {
                f.trucks_exporting += u32::from(matches!(h.to, Endpoint::Outside(_)));
                f.trucks_importing += u32::from(matches!(h.from, Endpoint::Outside(_)));
            }
        }
        if let Some(t) = w.get_resource::<cp_transit::Tourists>() {
            f.tourists_now = t.visits.len() as u32;
            f.tourists_week = t.visits_week;
        }
        f
    }

    fn line_views(&self) -> Vec<LineView> {
        let net = self.world().resource::<cp_transit::TransitNet>();
        net.lines
            .iter()
            .filter_map(|(id, _)| self.line_view(id))
            .collect()
    }

    /// One transit line with its figures, or `None` if it is gone.
    pub fn line_view(&self, id: cp_transit::LineId) -> Option<LineView> {
        let net = self.world().resource::<cp_transit::TransitNet>();
        let fleet = self.world().resource::<cp_transit::TransitFleet>();
        let l = net.lines.get(id)?;
        let s = net.stats.get(&id).copied().unwrap_or_default();
        Some(LineView {
            id,
            name: l.name.clone(),
            mode: l.mode,
            color: l.color,
            stops: l.stops.len(),
            vehicles: l.vehicles,
            vehicles_out: fleet.of_line(id).count() as u16,
            state: l.state,
            riders_today: s.riders_today,
            riders_week: s.riders_week,
            waiting: s.waiting,
        })
    }

    fn city_snapshot(&self) -> Arc<crate::stats::CityStats> {
        let Some(c) = self.world().get_resource::<crate::stats::CityStats>() else {
            return Arc::default();
        };
        let mut cache = self.city_cache.lock().expect("city cache");
        if let Some((g, snap)) = cache.as_ref()
            && *g == c.generation
        {
            return Arc::clone(snap);
        }
        let snap = Arc::new(c.clone());
        *cache = Some((c.generation, Arc::clone(&snap)));
        snap
    }

    fn stats(&self) -> SimStats {
        let world = self.world();
        SimStats {
            checksum: world.resource::<SimChecksum>().last,
            arenas: world
                .resource::<ArenaGauges>()
                .0
                .iter()
                .map(|&(name, capacity, len)| ArenaFill {
                    name,
                    used: len(world) as u32,
                    capacity,
                })
                .collect(),
            commands_pending: world.resource::<CommandQueue<SimCommand>>().len(),
            commands_applied: world.resource::<CommandLog<SimCommand>>().len(),
        }
    }
}

/// Gives the buses of lines the colour of their line (T-163): the run of a bus names its
/// vehicle, the bus its line.
fn tint_buses(world: &World, agents: &cp_traffic::Agents, frame: &mut AgentFrame) {
    let (Some(fleet), Some(transit)) = (
        world.get_resource::<cp_transit::TransitFleet>(),
        world.get_resource::<cp_transit::TransitNet>(),
    ) else {
        return;
    };
    let mut by_run: std::collections::BTreeMap<cp_traffic::RunId, [u8; 3]> = Default::default();
    for (_, bus) in fleet.buses.iter() {
        if let Some(line) = transit.lines.get(bus.line) {
            let i = usize::from(line.color) % cp_transit::LINE_PALETTE_RGB.len();
            by_run.insert(bus.run, cp_transit::LINE_PALETTE_RGB[i]);
        }
    }
    if by_run.is_empty() {
        return;
    }
    for (id, v) in agents.vehicles() {
        let Some(color) = cp_traffic::run_of(v).and_then(|r| by_run.get(&r)) else {
            continue;
        };
        let key = cp_traffic::agent_key(id.index, id.generation);
        if let Ok(i) = frame.vehicles.binary_search_by_key(&key, |p| p.key) {
            frame.vehicles[i].tint = Some(*color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_terrain::{GRID, MapMeta};

    fn flat_terrain() -> Arc<Terrain> {
        flat_terrain_at((2, 2))
    }

    fn flat_terrain_at(start_tile: (u8, u8)) -> Arc<Terrain> {
        let meta = MapMeta {
            name: "t".into(),
            sea_level: 1.0,
            start_tile,
            external_connections: vec![],
            entry_road: None,
            entry_rail: None,
        };
        let n = GRID * GRID;
        Arc::new(Terrain::from_samples(meta, &vec![1000; n], &vec![0; n]).unwrap())
    }

    #[test]
    fn stats_report_gauges_commands_and_checksum() {
        use crate::plugin::{SimBuilder, SimPlugin};
        struct P;
        impl SimPlugin for P {
            fn build(&self, sim: &mut SimBuilder) {
                sim.register_arena_gauge("dummy", 10, |_| 3);
            }
        }
        let mut sim = SimRunner::new(1);
        sim.add_plugin(&P);
        sim.push_command(
            SimCommand::SetSpeed {
                speed: cp_core::SimSpeed::X2,
            },
            Tick(1),
        );
        sim.push_command(
            SimCommand::SetSpeed {
                speed: cp_core::SimSpeed::X1,
            },
            Tick(9999),
        );
        let v0 = sim.view();
        assert_eq!(
            v0.stats.arenas,
            vec![
                ArenaFill {
                    name: "buildings",
                    used: 0,
                    capacity: cp_core::SimLimits::desktop().buildings
                },
                ArenaFill {
                    name: "citizens",
                    used: 0,
                    capacity: cp_core::SimLimits::desktop().citizens
                },
                ArenaFill {
                    name: "pedestrians",
                    used: 0,
                    capacity: cp_core::SimLimits::desktop().pedestrians
                },
                ArenaFill {
                    name: "vehicles",
                    used: 0,
                    capacity: cp_core::SimLimits::desktop().vehicles
                },
                ArenaFill {
                    name: "trips",
                    used: 0,
                    capacity: cp_core::SimLimits::desktop().citizens
                },
                ArenaFill {
                    name: "households",
                    used: 0,
                    capacity: cp_core::SimLimits::desktop().households
                },
                ArenaFill {
                    name: "dummy",
                    used: 3,
                    capacity: 10
                }
            ]
        );
        assert_eq!(
            (v0.stats.commands_pending, v0.stats.commands_applied),
            (2, 0)
        );
        assert_eq!(v0.stats.checksum, None);
        for _ in 0..256 {
            sim.tick_once();
        }
        let s = sim.view().stats;
        assert_eq!((s.commands_pending, s.commands_applied), (1, 1));
        assert_eq!(s.checksum.unwrap().0, Tick(255));
    }

    #[test]
    fn view_tracks_tick_and_shares_terrain() {
        let mut sim = SimRunner::new(1);
        assert!(sim.view().terrain.is_none());
        let t = flat_terrain();
        sim.set_terrain(Arc::clone(&t));
        sim.tick_once();
        sim.tick_once();
        let v = sim.view();
        assert_eq!(v.tick, Tick(2));
        assert_eq!(v.game_time.tick_of_day, 2);
        assert!(Arc::ptr_eq(v.terrain.as_ref().unwrap(), &t));
        assert_eq!(v.tiles, TileOwnership::m1_around((2, 2)));
    }

    #[test]
    fn set_terrain_opens_the_start_tile_and_its_neighbours() {
        for (start, open) in [((0, 0), 4), ((2, 2), 9), ((4, 2), 6), ((4, 4), 4)] {
            let mut sim = SimRunner::new(1);
            assert_eq!(sim.view().tiles, TileOwnership::none());
            sim.set_terrain(flat_terrain_at(start));
            let tiles = sim.view().tiles;
            assert_eq!(tiles.open_count(), open, "{start:?}");
            assert!(tiles.is_open(start.0 as usize, start.1 as usize));
        }
    }

    #[test]
    fn set_terrain_keeps_existing_ownership() {
        let mut sim = SimRunner::new(1);
        sim.world_mut().insert_resource(TileOwnership::only((4, 4)));
        sim.set_terrain(flat_terrain_at((0, 0)));
        assert_eq!(sim.view().tiles, TileOwnership::only((4, 4)));
    }

    #[test]
    fn view_carries_junction_geometry_after_the_network_phase() {
        use bevy_math::Vec3;
        use cp_core::curve::CubicBezier3;
        use cp_net::{Elevation, NetGraph};

        let db = cp_content::ContentDb::load_dir(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/content"
        ))
        .unwrap();
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        let mut sim = SimRunner::new(1);
        assert!(sim.view().junctions.is_empty());
        {
            let mut g = sim.world_mut().resource_mut::<NetGraph>();
            for tip in [Vec3::new(80.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 80.0)] {
                g.add_segment(
                    &def,
                    road,
                    CubicBezier3::line(Vec3::ZERO, tip),
                    Elevation::Ground,
                )
                .unwrap();
            }
        }
        sim.tick_once();
        let v = sim.view();
        assert_eq!(v.junctions.len(), 3);
        assert_eq!(v.junctions.version(), 1);
        let corner = v
            .junctions
            .iter()
            .map(|(_, j)| j)
            .find(|j| j.arms.len() == 2)
            .unwrap();
        assert_eq!(corner.polygon.len(), 4);
        // A quiet tick changes nothing; the snapshot shares its data.
        sim.tick_once();
        assert_eq!(sim.view().junctions.version(), 1);
        assert_eq!(v.net.segment_count(), 2);
        assert!(Arc::ptr_eq(&v.net, &sim.view().net));
    }

    #[test]
    fn buildings_snapshot_is_shared_until_the_grid_changes() {
        use cp_building::{AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingState, Lot};
        use cp_core::{Id, Tick};
        let mut sim = SimRunner::new(1);
        let (a, b) = (sim.view().buildings, sim.view().buildings);
        assert!(Arc::ptr_eq(&a, &b) && a.is_empty());
        let building = Building {
            def: cp_content::BuildingDefId(0),
            footprint: cp_building::Footprint::Lot(Lot {
                block: Id::new(0, 0),
                col: 0,
                w: 1,
                d: 1,
            }),
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos: bevy_math::Vec2::ZERO,
            rot: 0.0,
            access: Some(AccessPoint {
                segment: Id::new(0, 0),
                t: 0.0,
            }),
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        };
        let id = sim
            .world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(building);
        let c = sim.view().buildings;
        assert!(!Arc::ptr_eq(&a, &c) && c.len() == 1);
        assert!(Arc::ptr_eq(&c, &sim.view().buildings));
        // A state change without a count change is noticed through the revision.
        sim.world_mut()
            .resource_mut::<BuildingGrid>()
            .get_mut(id)
            .unwrap()
            .state = BuildingState::Burned;
        let d = sim.view().buildings;
        assert!(!Arc::ptr_eq(&c, &d));
        assert_eq!(d.get(id).unwrap().state, BuildingState::Burned);
        assert!(sim.view().content.is_none());
    }
}
