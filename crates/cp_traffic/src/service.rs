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
//! Pending requests are **not saved**: after a load the requester (citizens) re-requests.

use std::sync::Arc;

use bevy_ecs::change_detection::DetectChanges;
use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_tasks::{ComputeTaskPool, Task, TaskPool, block_on};
use cp_core::Tick;
use cp_net::{JunctionGeometry, NetGraph};

use crate::astar::{CarPath, NoPath, PedPath, find_car_path, find_ped_path};
use crate::graph::{Access, CongestionMap, PathGraph};

/// Ticks between submitting a request and applying its result.
pub const PATH_DELAY_TICKS: u64 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Car,
    Pedestrian,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathRequest {
    pub mode: Mode,
    pub from: Access,
    pub to: Access,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PathResult {
    Car(Result<CarPath, NoPath>),
    Pedestrian(Result<PedPath, NoPath>),
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
    /// Bumped on every rebuild; outcomes computed on an older generation are stale.
    generation: u64,
    geo_version: u64,
    built: bool,
}

impl Default for PathNetwork {
    fn default() -> Self {
        Self {
            graph: Arc::new(PathGraph::default()),
            congestion: Arc::new(CongestionMap::default()),
            generation: 0,
            geo_version: 0,
            built: false,
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

    /// Rebuilds the snapshot from the road network.
    pub fn rebuild(&mut self, net: &NetGraph, geo: &JunctionGeometry) {
        self.graph = Arc::new(PathGraph::build(net, geo));
        self.geo_version = geo.version();
        self.generation += 1;
        self.built = true;
    }
}

fn compute(graph: &PathGraph, cong: &CongestionMap, req: &PathRequest) -> PathResult {
    match req.mode {
        Mode::Car => PathResult::Car(find_car_path(graph, cong, &req.from, &req.to)),
        Mode::Pedestrian => PathResult::Pedestrian(find_ped_path(graph, &req.from, &req.to)),
    }
}

enum Job {
    Spawned(Task<PathResult>),
    /// Run at delivery time (synchronous mode).
    Deferred {
        graph: Arc<PathGraph>,
        congestion: Arc<CongestionMap>,
        request: PathRequest,
    },
}

struct Pending {
    id: RequestId,
    submitted: Tick,
    due: Tick,
    generation: u64,
    job: Job,
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

    /// Drops everything in flight (after loading a save).
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// Submits `req` in tick `now`; the outcome is applied at `now + 8`.
    pub fn submit(&mut self, net: &PathNetwork, now: Tick, req: PathRequest) -> RequestId {
        let id = RequestId(self.next_id);
        self.next_id += 1;
        let graph = Arc::clone(&net.graph);
        let congestion = Arc::clone(&net.congestion);
        let job = if self.is_sync() {
            Job::Deferred {
                graph,
                congestion,
                request: req,
            }
        } else {
            let fut = async move { compute(&graph, &congestion, &req) };
            let task = match &self.pool {
                Some(p) => p.spawn(fut),
                None => ComputeTaskPool::get_or_init(TaskPool::default).spawn(fut),
            };
            Job::Spawned(task)
        };
        self.pending.push(Pending {
            id,
            submitted: now,
            due: Tick(now.0 + PATH_DELAY_TICKS),
            generation: net.generation,
            job,
        });
        id
    }

    /// Collects the outcomes due at `now`, in request-id order (`pending` is in
    /// submission order). Blocks until a spawned search finishes.
    fn deliver(&mut self, now: Tick, current_generation: u64, out: &mut Vec<PathOutcome>) {
        let mut keep = Vec::new();
        for p in std::mem::take(&mut self.pending) {
            if p.due > now {
                keep.push(p);
                continue;
            }
            let result = match p.job {
                Job::Spawned(task) => block_on(task),
                Job::Deferred {
                    graph,
                    congestion,
                    request,
                } => compute(&graph, &congestion, &request),
            };
            let result = if p.generation == current_generation {
                result
            } else {
                PathResult::Stale
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

/// `Network` phase (after `update_junctions`): rebuild the snapshot when roads changed.
pub fn update_path_network(
    net: Res<NetGraph>,
    geo: Res<JunctionGeometry>,
    mut pn: ResMut<PathNetwork>,
) {
    if !pn.built || net.is_changed() || geo.version() != pn.geo_version {
        pn.rebuild(&net, &geo);
    }
}

/// `Paths` phase: publish the outcomes that are due this tick.
pub fn deliver_paths(
    tick: Res<Tick>,
    pn: Res<PathNetwork>,
    mut service: ResMut<PathService>,
    mut results: ResMut<PathResults>,
) {
    results.0.clear();
    service.deliver(*tick, pn.generation, &mut results.0);
}
