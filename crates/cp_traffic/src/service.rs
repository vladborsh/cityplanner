//! The pathfinding service (`architecture/system-design.md` §2, §9.3).
//!
//! A request submitted in tick `t` reads the network **as of `t`** (an immutable
//! [`PathGraph`] snapshot plus the congestion map, both shared by `Arc`), is executed by the
//! compute pool, and its outcome is **applied exactly at tick `t + 8`**: the `Paths` phase
//! joins the task (blocking, no timer) and publishes the outcomes in request-id order in
//! [`PathResults`]. A request is a pure function of `(snapshot, request)`, so results do
//! not depend on thread count or timing. If the network changed between `t` and `t + 8`
//! the outcome is [`PathResult::Stale`] and the requester must ask again.
//!
//! On `wasm32` (no worker threads) — or when [`PathService::set_sync`] is set — the search
//! runs synchronously inside the `Paths` phase at `t + 8` instead; results are identical.
//!
//! **Save/load.** Pending requests are sim state: [`PathService::export`] gives
//! [`SavedPaths`] (next request id, and per pending request its id, submit tick, the request,
//! the congestion map it read, and whether it is already stale). Results and the graph
//! snapshot are not saved. After a load the net is restored, the snapshot rebuilt, and the
//! first [`update_path_network`] re-launches every non-stale request on it, keeping the
//! original id and due tick; it is delivered at `t + 8` like in a continuous run.
//! "Stale in the continuous run ⇔ stale after the load" holds because staleness is an
//! explicit per-request flag, not a generation comparison: a network change in a rebuild
//! marks every pending request stale (so a change *before* the save is stored in the flag,
//! a change *after* the load marks the restored requests exactly as it would have marked
//! them live), and the one rebuild forced by the load itself ([`PathNetwork::mark_restored`])
//! invalidates nothing unless the net differs from the restored one by then (a command
//! applied in the first tick after the load is a change, as it is in a continuous run).
//! A non-stale request ran (or will run) on a graph
//! equal to the restored one, and on the congestion map stored with it, so its result is
//! identical. The pending state is part of the checksum.

use std::sync::Arc;

use bevy_ecs::change_detection::DetectChanges;
use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_tasks::{ComputeTaskPool, Task, TaskPool, block_on};
use cp_core::Tick;
use cp_net::{JunctionGeometry, NetGraph};
use serde::{Deserialize, Serialize};

use crate::astar::{
    CarPath, NoPath, PedPath, find_car_path, find_car_path_lanes, find_class_path,
    find_class_path_lanes, find_ped_path, find_rail_path, find_truck_path, find_truck_path_lanes,
};
use crate::graph::{Access, CongestionMap, PathGraph};
use crate::heavy::HeavyBan;

/// Ticks between submitting a request and applying its result.
pub const PATH_DELAY_TICKS: u64 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RequestId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Car,
    Pedestrian,
    /// A car already on the road asks for a new route from lane `from` to lane `to`
    /// (appended in T-066). `PathRequest::from.s` / `to.s` are lane-metres; the other `Access`
    /// fields are unused.
    CarLanes {
        from: cp_net::LaneId,
        to: cp_net::LaneId,
    },
    /// A train's route over the track graph (T-161). Appended last.
    Rail,
    /// A truck's route between two access points (T-195): like `Car`, but lanes in districts
    /// with the heavy traffic ban cost more. Appended; no earlier save holds it.
    Truck,
    /// A truck already on the road, like `CarLanes` (T-195). Appended.
    TruckLanes {
        from: cp_net::LaneId,
        to: cp_net::LaneId,
    },
    /// A citizen with a car takes the quicker of walking and driving (T-242), decided on the
    /// worker (T-256: it ran two searches on the main thread): the result is the cheaper of the
    /// walking route and the driving route plus the start penalty (`f32` bits; walking wins a
    /// tie), a `Pedestrian` or `Car` result. Neither route: the error of `fallback_car`'s mode.
    /// Appended; no earlier save holds it.
    ByTime {
        car_start_penalty_bits: u32,
        fallback_car: bool,
    },
    /// A route for a vehicle of `class` between two access points (T-302: a bus, tram or
    /// trolleybus keeps to the lanes it may use). Appended.
    Class {
        class: cp_content::VehicleClass,
    },
    /// A vehicle of `class` already on the road, like `CarLanes` (T-302). Appended.
    ClassLanes {
        class: cp_content::VehicleClass,
        from: cp_net::LaneId,
        to: cp_net::LaneId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathRequest {
    pub mode: Mode,
    pub from: Access,
    pub to: Access,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PathResult {
    Car(Result<CarPath, NoPath>),
    Pedestrian(Result<PedPath, NoPath>),
    /// A route over the track graph; the lanes are `Rail` lanes.
    Rail(Result<CarPath, NoPath>),
    /// The network changed while the request was in flight; ask again.
    Stale,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PathOutcome {
    pub id: RequestId,
    /// Tick the request was submitted in.
    pub submitted: Tick,
    pub result: PathResult,
}

/// Outcomes applied this tick, in request-id order. Cleared at the start of the `Paths`
/// phase every tick.
#[derive(Resource, Default, Debug)]
pub struct PathResults(pub Vec<PathOutcome>);

/// The current network snapshot; rebuilt in the `Network` phase when roads change.
#[derive(Resource, Clone)]
pub struct PathNetwork {
    graph: Arc<PathGraph>,
    congestion: Arc<CongestionMap>,
    /// The heavy traffic ban in force (T-195); derived from the districts and policies.
    heavy: Arc<HeavyBan>,
    /// Bumped on every rebuild; outcomes computed on an older generation are stale.
    generation: u64,
    geo_version: u64,
    built: bool,
    /// Set by a load: fingerprint of the restored net. The next rebuild is a network change
    /// only if the net no longer matches it (a command applied in the first tick, say).
    restored: Option<u64>,
}

impl Default for PathNetwork {
    fn default() -> Self {
        Self {
            graph: Arc::new(PathGraph::default()),
            congestion: Arc::new(CongestionMap::default()),
            heavy: Arc::new(HeavyBan::default()),
            generation: 0,
            geo_version: 0,
            built: false,
            restored: None,
        }
    }
}

impl PathNetwork {
    pub fn graph(&self) -> &Arc<PathGraph> {
        &self.graph
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Replaces the congestion map (the traffic task does this once it measures load). Read
    /// by requests submitted from now on.
    pub fn set_congestion(&mut self, map: CongestionMap) {
        self.congestion = Arc::new(map);
    }

    /// Called after a load with the restored net: forces a rebuild on the next
    /// [`update_path_network`], which does not invalidate the restored requests unless the
    /// net differs from `net` by then.
    pub fn mark_restored(&mut self, net: &NetGraph) {
        self.built = false;
        self.restored = Some(net_fingerprint(net));
    }

    /// The heavy traffic ban trucks' requests submitted from now on read.
    pub fn heavy(&self) -> &Arc<HeavyBan> {
        &self.heavy
    }

    /// Replaces the heavy traffic ban (only when it differs).
    pub fn set_heavy(&mut self, heavy: HeavyBan) {
        if *self.heavy != heavy {
            self.heavy = Arc::new(heavy);
        }
    }

    pub fn congestion(&self) -> &Arc<CongestionMap> {
        &self.congestion
    }

    /// Rebuilds the snapshot from the road network.
    pub fn rebuild(&mut self, net: &NetGraph, geo: &JunctionGeometry) {
        self.graph = Arc::new(PathGraph::build(net, geo));
        self.geo_version = geo.version();
        self.generation += 1;
        self.built = true;
    }
}

fn compute(
    graph: &PathGraph,
    cong: &CongestionMap,
    heavy: &HeavyBan,
    req: &PathRequest,
) -> PathResult {
    match req.mode {
        Mode::Car => PathResult::Car(find_car_path(graph, cong, &req.from, &req.to)),
        Mode::Pedestrian => PathResult::Pedestrian(find_ped_path(graph, &req.from, &req.to)),
        Mode::Rail => PathResult::Rail(find_rail_path(graph, &req.from, &req.to)),
        Mode::Truck => PathResult::Car(find_truck_path(graph, cong, heavy, &req.from, &req.to)),
        Mode::TruckLanes { from, to } => PathResult::Car(find_truck_path_lanes(
            graph,
            cong,
            heavy,
            (from, req.from.s),
            (to, req.to.s),
        )),
        Mode::ByTime {
            car_start_penalty_bits,
            fallback_car,
        } => {
            let walk = find_ped_path(graph, &req.from, &req.to).ok();
            let drive = find_car_path(graph, cong, &req.from, &req.to).ok();
            let penalty = f32::from_bits(car_start_penalty_bits);
            match (walk, drive) {
                (Some(w), Some(d)) if w.cost <= d.cost + penalty => PathResult::Pedestrian(Ok(w)),
                (_, Some(d)) => PathResult::Car(Ok(d)),
                (Some(w), None) => PathResult::Pedestrian(Ok(w)),
                (None, None) if fallback_car => PathResult::Car(Err(NoPath)),
                (None, None) => PathResult::Pedestrian(Err(NoPath)),
            }
        }
        Mode::CarLanes { from, to } => PathResult::Car(find_car_path_lanes(
            graph,
            cong,
            (from, req.from.s),
            (to, req.to.s),
        )),
        Mode::Class { class } => {
            PathResult::Car(find_class_path(graph, cong, class, &req.from, &req.to))
        }
        Mode::ClassLanes { class, from, to } => PathResult::Car(find_class_path_lanes(
            graph,
            cong,
            class,
            (from, req.from.s),
            (to, req.to.s),
        )),
    }
}

enum Job {
    Spawned(Task<PathResult>),
    /// Run at delivery time (synchronous mode).
    Deferred(Arc<PathGraph>),
    /// Restored from a save: waits for the first snapshot.
    Unstarted,
    /// The network changed under it: delivers `Stale`, nothing to compute.
    Stale,
}

struct Pending {
    id: RequestId,
    submitted: Tick,
    due: Tick,
    request: PathRequest,
    congestion: Arc<CongestionMap>,
    heavy: Arc<HeavyBan>,
    job: Job,
}

/// One pending request as saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedRequest {
    pub id: RequestId,
    pub submitted: Tick,
    pub request: PathRequest,
    /// Index into [`SavedPaths::congestion`].
    pub congestion: u32,
    pub stale: bool,
}

/// The saved state of the service: what is needed to continue a run bit-identically.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedPaths {
    pub next_id: u64,
    /// Distinct congestion maps read by the pending requests.
    pub congestion: Vec<CongestionMap>,
    /// Pending requests in submission (= id) order.
    pub pending: Vec<SavedRequest>,
}

#[derive(Resource, Default)]
pub struct PathService {
    next_id: u64,
    pending: Vec<Pending>,
    /// Forces synchronous mode (always on for `wasm32`).
    force_sync: bool,
    pool: Option<TaskPool>,
}

impl PathService {
    pub fn new() -> Self {
        Self::default()
    }

    /// A service that runs requests on `pool` instead of the global compute pool (tests,
    /// bench).
    pub fn with_pool(pool: TaskPool) -> Self {
        Self {
            pool: Some(pool),
            ..Self::default()
        }
    }

    /// Synchronous mode: search in the `Paths` phase at `t + 8`, no worker threads.
    pub fn set_sync(&mut self, sync: bool) {
        self.force_sync = sync;
    }

    pub fn is_sync(&self) -> bool {
        self.force_sync || cfg!(target_arch = "wasm32")
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    fn launch(
        &self,
        graph: &Arc<PathGraph>,
        congestion: &Arc<CongestionMap>,
        heavy: &Arc<HeavyBan>,
        req: PathRequest,
    ) -> Job {
        if self.is_sync() {
            Job::Deferred(Arc::clone(graph))
        } else {
            let (graph, congestion, heavy) =
                (Arc::clone(graph), Arc::clone(congestion), Arc::clone(heavy));
            let fut = async move { compute(&graph, &congestion, &heavy, &req) };
            Job::Spawned(match &self.pool {
                Some(p) => p.spawn(fut),
                None => ComputeTaskPool::get_or_init(TaskPool::default).spawn(fut),
            })
        }
    }

    /// Submits `req` in tick `now`; the outcome is applied at `now + 8`.
    pub fn submit(&mut self, net: &PathNetwork, now: Tick, req: PathRequest) -> RequestId {
        let id = RequestId(self.next_id);
        self.next_id += 1;
        let job = self.launch(&net.graph, &net.congestion, &net.heavy, req);
        self.pending.push(Pending {
            id,
            submitted: now,
            due: Tick(now.0 + PATH_DELAY_TICKS),
            request: req,
            congestion: Arc::clone(&net.congestion),
            heavy: Arc::clone(&net.heavy),
            job,
        });
        id
    }

    /// The network changed: everything in flight is stale.
    pub fn invalidate_pending(&mut self) {
        for p in &mut self.pending {
            p.job = Job::Stale;
        }
    }

    /// Launches restored requests on the (new) snapshot.
    fn start_unstarted(&mut self, graph: &Arc<PathGraph>, heavy: &Arc<HeavyBan>) {
        let mut pending = std::mem::take(&mut self.pending);
        for p in &mut pending {
            if matches!(p.job, Job::Unstarted) {
                p.heavy = Arc::clone(heavy);
                p.job = self.launch(graph, &p.congestion, heavy, p.request);
            }
        }
        self.pending = pending;
    }

    /// The state to save (see the module docs).
    pub fn export(&self) -> SavedPaths {
        let mut table: Vec<Arc<CongestionMap>> = Vec::new();
        let pending = self
            .pending
            .iter()
            .map(|p| {
                let idx = match table.iter().position(|c| Arc::ptr_eq(c, &p.congestion)) {
                    Some(i) => i,
                    None => {
                        table.push(Arc::clone(&p.congestion));
                        table.len() - 1
                    }
                };
                SavedRequest {
                    id: p.id,
                    submitted: p.submitted,
                    request: p.request,
                    congestion: idx as u32,
                    stale: matches!(p.job, Job::Stale),
                }
            })
            .collect();
        SavedPaths {
            next_id: self.next_id,
            congestion: table.iter().map(|c| (**c).clone()).collect(),
            pending,
        }
    }

    /// Replaces the service state with a saved one. Requests wait for the first snapshot
    /// ([`update_path_network`] launches them); pair with [`PathNetwork::mark_restored`].
    pub fn import(&mut self, saved: SavedPaths) {
        let table: Vec<Arc<CongestionMap>> = saved.congestion.into_iter().map(Arc::new).collect();
        self.next_id = saved.next_id;
        self.pending = saved
            .pending
            .into_iter()
            .map(|r| Pending {
                id: r.id,
                submitted: r.submitted,
                due: Tick(r.submitted.0 + PATH_DELAY_TICKS),
                request: r.request,
                congestion: Arc::clone(&table[r.congestion as usize]),
                // Replaced by the current ban when the request is launched.
                heavy: Arc::new(HeavyBan::default()),
                job: if r.stale { Job::Stale } else { Job::Unstarted },
            })
            .collect();
    }

    /// Collects the outcomes due at `now`, in request-id order (`pending` is in
    /// submission order). Blocks until a spawned search finishes.
    fn deliver(&mut self, now: Tick, out: &mut Vec<PathOutcome>) {
        let mut keep = Vec::new();
        for p in std::mem::take(&mut self.pending) {
            if p.due > now {
                keep.push(p);
                continue;
            }
            let result = match p.job {
                Job::Spawned(task) => block_on(task),
                Job::Deferred(graph) => compute(&graph, &p.congestion, &p.heavy, &p.request),
                Job::Stale => PathResult::Stale,
                Job::Unstarted => unreachable!("restored request not launched before delivery"),
            };
            out.push(PathOutcome {
                id: p.id,
                submitted: p.submitted,
                result,
            });
        }
        self.pending = keep;
    }
}

/// FNV-1a hash of the net's nodes, segments and lanes (not its dirty set, which the junction
/// update drains), used to tell a load's own rebuild from a real change.
fn net_fingerprint(net: &NetGraph) -> u64 {
    use std::fmt::Write as _;
    struct Fnv(u64);
    impl std::fmt::Write for Fnv {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for b in s.bytes() {
                self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
            }
            Ok(())
        }
    }
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    for (id, n) in net.nodes() {
        let _ = write!(h, "{id:?}{n:?};");
    }
    for (id, s) in net.segments() {
        let _ = write!(h, "{id:?}{s:?};");
    }
    for (id, l) in net.lanes() {
        let _ = write!(h, "{id:?}{l:?};");
    }
    h.0
}

/// `Network` phase (after `update_junctions`): rebuild the snapshot when roads changed. A
/// real change makes everything in flight stale; the rebuild forced by a load does not.
/// Restored requests are launched on the fresh snapshot.
pub fn update_path_network(
    net: Res<NetGraph>,
    geo: Res<JunctionGeometry>,
    mut pn: ResMut<PathNetwork>,
    mut service: ResMut<PathService>,
    policies: cp_district::PolicyParams,
    content: Option<Res<cp_content::SimContent>>,
) {
    // The heavy traffic ban follows the districts and policies, not only the network (T-195).
    if !pn.built || net.is_changed() || policies.changed() {
        let heavy = match (policies.get(), &content) {
            (Some(p), Some(c)) => HeavyBan::build(
                &net,
                p.map,
                p.districts,
                p.city,
                if c.0
                    .policy(cp_content::PolicyKind::HeavyTrafficBan)
                    .is_some()
                {
                    c.0.balance().heavy_ban_cost_mul
                } else {
                    1.0
                },
            ),
            _ => HeavyBan::default(),
        };
        pn.set_heavy(heavy);
    }
    if !pn.built || net.is_changed() || geo.version() != pn.geo_version {
        let unchanged_since_load = pn
            .restored
            .take()
            .is_some_and(|fp| fp == net_fingerprint(&net));
        pn.rebuild(&net, &geo);
        if !unchanged_since_load {
            service.invalidate_pending();
        }
        let (graph, heavy) = (Arc::clone(&pn.graph), Arc::clone(&pn.heavy));
        service.start_unstarted(&graph, &heavy);
    }
}

/// `Paths` phase: publish the outcomes that are due this tick.
pub fn deliver_paths(
    tick: Res<Tick>,
    mut service: ResMut<PathService>,
    mut results: ResMut<PathResults>,
) {
    results.0.clear();
    service.deliver(*tick, &mut results.0);
}
