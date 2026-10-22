//! Coverage of a service along the roads (§16 M2 D "Покрытие").
//!
//! Per service kind, every segment gets `max over Active buildings of the kind of
//! 1 − d / radius_m`, clamped at 0, where `d` is the road **reach** from the building's access
//! point to the nearest point of the segment: 0 on the access segment itself, otherwise the
//! shorter way to one of its ends. Roads are walked without regard to direction (Dijkstra over
//! the segment graph); the access point sits `t × length` along its segment.
//!
//! **Reach is free-flow travel time, in metres of a [`REACH_BASE_KMH`] street** (T-305): a
//! segment counts `length × (REACH_BASE_KMH / its car speed limit)`, so a 2-lane street at
//! 40 km/h counts its metres exactly (every radius in content keeps its meaning there), a
//! 6-lane arterial at 90 km/h counts 4/9 of them and a service reaches farther along it. No
//! congestion: the reach is a property of the roads, not of today's traffic. A segment without a
//! car lane (a park path, rails) counts its metres.
//!
//! The values are recomputed when `tick % 64 == 0`. Since save v22 (T-135) they are **saved
//! and hashed**: between two boundaries the sim reads the values of the last boundary, and a
//! load in the middle of a cycle must see those same values even if a road or a building
//! changed since. [`rebuild_coverage`] is only for saves that carry none (formats before 22).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Local, Res, ResMut};
use bevy_ecs::world::World;
use cp_building::{AccessPoint, Building, BuildingGrid, BuildingState};
use cp_content::{ContentDb, LaneKind, ServiceKind, SimContent};
use cp_core::Tick;
use cp_net::{NetGraph, SegmentId};
use serde::{Deserialize, Serialize};

/// The road graph of the current network for the service searches (T-256: every service system
/// built its own from the whole network on every run). Filled lazily by the first search after
/// the network changed (keyed by the network's change tick), so no system of its own is needed.
/// Derived from the network: not saved, not hashed.
#[derive(Resource, Default)]
pub struct ServiceRoads {
    cache: std::sync::Mutex<Option<(bevy_ecs::change_detection::Tick, std::sync::Arc<RoadGraph>)>>,
}

/// The network and its cached road graph, for the systems that search along the roads.
#[derive(bevy_ecs::system::SystemParam)]
pub struct ServiceNet<'w> {
    pub net: Res<'w, NetGraph>,
    roads: Option<Res<'w, ServiceRoads>>,
}

impl ServiceNet<'_> {
    /// The road graph of the network as it is now: the cached one while the network has not
    /// changed since it was built, else built (and cached when the world has [`ServiceRoads`]).
    pub fn graph(&self) -> std::sync::Arc<RoadGraph> {
        let changed = bevy_ecs::change_detection::DetectChanges::last_changed(&self.net);
        let Some(roads) = self.roads.as_deref() else {
            return std::sync::Arc::new(RoadGraph::new(&self.net));
        };
        let mut cache = roads.cache.lock().unwrap_or_else(|e| e.into_inner());
        match cache.as_ref() {
            Some((at, g)) if *at == changed => std::sync::Arc::clone(g),
            _ => {
                let g = std::sync::Arc::new(RoadGraph::new(&self.net));
                *cache = Some((changed, std::sync::Arc::clone(&g)));
                g
            }
        }
    }
}

/// Ticks between coverage recomputations.
pub const COVERAGE_PERIOD: u64 = 64;

/// Coverage `0..=1` per service kind and segment, dense by segment index.
#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct ServiceCoverage {
    by_kind: BTreeMap<ServiceKind, Vec<f32>>,
    /// Changes at every recomputation and load (cache key of the published snapshot). Not
    /// sim state: neither saved nor compared nor hashed.
    #[serde(skip)]
    pub generation: u64,
}

impl PartialEq for ServiceCoverage {
    fn eq(&self, other: &Self) -> bool {
        self.by_kind == other.by_kind
    }
}

impl ServiceCoverage {
    /// Coverage of `kind` on `segment`; 0 when nothing of that kind reaches it.
    pub fn on_segment(&self, kind: ServiceKind, segment: SegmentId) -> f32 {
        self.by_kind
            .get(&kind)
            .and_then(|v| v.get(segment.index as usize))
            .copied()
            .unwrap_or(0.0)
    }

    /// Sets the coverage of `kind` per segment index (tests and scenarios; the coverage pass
    /// computes it from the buildings).
    pub fn set_kind(&mut self, kind: ServiceKind, by_segment: Vec<f32>) {
        self.by_kind.insert(kind, by_segment);
    }

    /// [`Self::on_segment`] by the segment's arena index.
    pub fn on_segment_index(&self, kind: ServiceKind, index: u32) -> f32 {
        self.by_kind
            .get(&kind)
            .and_then(|v| v.get(index as usize))
            .copied()
            .unwrap_or(0.0)
    }

    /// Coverage of `kind` at a building: the value on its access segment (0 without one).
    pub fn coverage_of(&self, kind: ServiceKind, building: &Building) -> f32 {
        building
            .access()
            .map_or(0.0, |a| self.on_segment(kind, a.segment))
    }

    /// The kinds that have any coverage computed, ascending.
    pub fn kinds(&self) -> impl Iterator<Item = ServiceKind> + '_ {
        self.by_kind.keys().copied()
    }
}

/// The street speed the reach is measured against, km/h: a road at this speed counts its metres.
pub const REACH_BASE_KMH: f32 = 40.0;

/// The reach of a segment (see the module doc): its metres scaled by `REACH_BASE_KMH / the
/// fastest car lane`, or its metres when it has no car lane. Exactly the metres at the base
/// speed (the factor is `1.0`: both speeds come from the same `kmh / 3.6`).
fn segment_reach(net: &NetGraph, s: &cp_net::Segment) -> f32 {
    let car = s
        .lanes
        .iter()
        .filter_map(|&l| net.lane(l))
        .filter(|l| l.kind == LaneKind::Car)
        .map(|l| l.speed_limit)
        .fold(0.0f32, f32::max);
    if car > 0.0 {
        s.length * (REACH_BASE_KMH / 3.6 / car)
    } else {
        s.length
    }
}

/// The road network as a graph of nodes joined by segments, for reaches along the roads
/// (free-flow travel time in metres of a [`REACH_BASE_KMH`] street, see the module doc).
/// Owns its data (no borrow of the [`NetGraph`]), so a client can keep one per net revision.
#[derive(Clone, Debug, PartialEq)]
pub struct RoadGraph {
    adj: Vec<Vec<(usize, f32)>>,
    /// Segments in id order with their end node indices.
    segs: Vec<(SegmentId, usize, usize)>,
    /// Per segment index: the live id, its end node indices and its reach.
    ends: Vec<Option<(SegmentId, usize, usize, f32)>>,
    /// Per node: the arena indices of the segments that end there.
    inc: Vec<Vec<u32>>,
    seg_slots: usize,
}

impl RoadGraph {
    pub fn new(net: &NetGraph) -> Self {
        let mut nodes = 0usize;
        let mut seg_slots = 0usize;
        for (id, s) in net.segments() {
            seg_slots = seg_slots.max(id.index as usize + 1);
            nodes = nodes
                .max(s.start.index as usize + 1)
                .max(s.end.index as usize + 1);
        }
        let mut adj: Vec<Vec<(usize, f32)>> = vec![Vec::new(); nodes];
        let mut segs = Vec::new();
        let mut ends = vec![None; seg_slots];
        let mut inc: Vec<Vec<u32>> = vec![Vec::new(); nodes];
        for (id, s) in net.segments() {
            let (a, b) = (s.start.index as usize, s.end.index as usize);
            let reach = segment_reach(net, s);
            adj[a].push((b, reach));
            adj[b].push((a, reach));
            inc[a].push(id.index);
            inc[b].push(id.index);
            segs.push((id, a, b));
            ends[id.index as usize] = Some((id, a, b, reach));
        }
        Self {
            adj,
            segs,
            ends,
            inc,
            seg_slots,
        }
    }

    /// Segment slots (`max index + 1`): the length of a per-segment coverage vector.
    pub fn seg_slots(&self) -> usize {
        self.seg_slots
    }

    /// End node indices and reach of a live segment.
    fn seg(&self, id: SegmentId) -> Option<(usize, usize, f32)> {
        match self.ends.get(id.index as usize) {
            Some(&Some((live, a, b, len))) if live == id => Some((a, b, len)),
            _ => None,
        }
    }

    /// Reach along segment `seg` at curve parameter `t`.
    fn offset(&self, seg: SegmentId, t: f32) -> Option<f32> {
        let (_, _, len) = self.seg(seg)?;
        Some((t * len).clamp(0.0, len))
    }

    /// Road reach from the access point to every node; unreached nodes and nodes beyond
    /// `limit` stay infinite. Roads are walked without regard to direction.
    pub fn node_distances(&self, from: AccessPoint, limit: f32) -> Vec<f32> {
        self.search(from, limit, |_, _| f32::INFINITY)
    }

    /// Heap Dijkstra behind [`Self::node_distances`]; see [`Self::search_scratch`].
    pub(crate) fn search(
        &self,
        from: AccessPoint,
        limit: f32,
        settled: impl FnMut(usize, f32) -> f32,
    ) -> Vec<f32> {
        let mut sc = Scratch::new(self);
        self.search_scratch(&mut sc, from, limit, settled);
        sc.dist
    }

    /// Heap Dijkstra into `sc.dist`. `settled(node, d)` is called as each node gets its final
    /// distance and returns a bound: the search stops at the first node farther than the bound,
    /// so every node at or below it is final. Ties pop by node index.
    pub(crate) fn search_scratch(
        &self,
        sc: &mut Scratch,
        from: AccessPoint,
        limit: f32,
        mut settled: impl FnMut(usize, f32) -> f32,
    ) {
        sc.reset();
        let (Some((a, b, len)), Some(s)) =
            (self.seg(from.segment), self.offset(from.segment, from.t))
        else {
            return;
        };
        sc.set(a, sc.dist[a].min(s));
        sc.set(b, sc.dist[b].min(len - s));
        // Non-negative f32 order as u32 bits.
        let mut heap = BinaryHeap::new();
        for n in [a, b] {
            heap.push(Reverse((sc.dist[n].to_bits(), n)));
        }
        let mut bound = f32::INFINITY;
        while let Some(Reverse((bits, u))) = heap.pop() {
            let du = f32::from_bits(bits);
            if sc.done[u] || du > sc.dist[u] {
                continue;
            }
            if du > bound {
                break;
            }
            sc.done[u] = true;
            bound = bound.min(settled(u, du));
            if du >= limit {
                continue;
            }
            for &(v, len) in &self.adj[u] {
                if du + len < sc.dist[v] {
                    sc.set(v, du + len);
                    heap.push(Reverse((sc.dist[v].to_bits(), v)));
                }
            }
        }
    }

    /// Lowers `tail[n]` for the ends of the segment of `to` to the reach from `to` to them.
    pub(crate) fn add_tail(&self, tail: &mut [f32], touched: &mut Vec<usize>, to: AccessPoint) {
        let (Some((a, b, len)), Some(s2)) = (self.seg(to.segment), self.offset(to.segment, to.t))
        else {
            return;
        };
        for (n, m) in [(a, s2), (b, len - s2)] {
            if tail[n].is_infinite() {
                touched.push(n);
            }
            tail[n] = tail[n].min(m);
        }
    }

    /// Reach along one segment between two access points on it.
    pub(crate) fn same_segment(&self, a: AccessPoint, b: AccessPoint) -> f32 {
        match (self.offset(a.segment, a.t), self.offset(b.segment, b.t)) {
            (Some(x), Some(y)) => (x - y).abs(),
            _ => f32::INFINITY,
        }
    }

    /// Road reach from `from` (whose [`Self::node_distances`] are `dist`) to the access
    /// point `to`; infinite when unreachable.
    pub fn distance_to(&self, dist: &[f32], from: AccessPoint, to: AccessPoint) -> f32 {
        let (Some((a, b, len)), Some(s2)) = (self.seg(to.segment), self.offset(to.segment, to.t))
        else {
            return f32::INFINITY;
        };
        let via = (dist[a] + s2).min(dist[b] + (len - s2));
        if from.segment == to.segment
            && let Some(s1) = self.offset(from.segment, from.t)
        {
            return via.min((s1 - s2).abs());
        }
        via
    }
}

/// The coverage one building of radius `radius_m` with its access point at `access` gives:
/// `(segment index, 1 − d / radius_m)` for every segment it reaches (value > 0), by ascending
/// index. 0 on the access segment itself is `d`; otherwise the nearer end of the segment. The
/// sim folds these by max ([`kind_coverage`]); the client draws them for a ghost (§13.5).
pub fn site_coverage(g: &RoadGraph, access: AccessPoint, radius_m: f32) -> Vec<(u32, f32)> {
    site_coverage_with(g, &mut Scratch::new(g), access, radius_m)
}

/// [`site_coverage`] reusing `sc`, which must belong to `g`. Only the segments that touch a node
/// the search reached are looked at: the others are out of reach (coverage <= 0).
fn site_coverage_with(
    g: &RoadGraph,
    sc: &mut Scratch,
    access: AccessPoint,
    radius_m: f32,
) -> Vec<(u32, f32)> {
    let mut out = Vec::new();
    if radius_m <= 0.0 || g.seg(access.segment).is_none() {
        return out;
    }
    g.search_scratch(sc, access, radius_m, |_, _| f32::INFINITY);
    let mut segs: Vec<u32> = sc
        .touched
        .iter()
        .flat_map(|&n| g.inc[n].iter().copied())
        .collect();
    segs.sort_unstable();
    segs.dedup();
    for si in segs {
        let Some((id, na, nb, _)) = g.ends[si as usize] else {
            continue;
        };
        let d = if id == access.segment {
            0.0
        } else {
            sc.dist[na].min(sc.dist[nb])
        };
        let c = 1.0 - d / radius_m;
        if c > 0.0 {
            out.push((id.index, c));
        }
    }
    out
}

/// The service of `b` when it is an Active building with an access point and a radius.
fn served_site(db: &ContentDb, b: &Building) -> Option<(ServiceKind, AccessPoint, f32)> {
    if b.state != BuildingState::Active {
        return None;
    }
    let access = b.access()?;
    let service = db.building_service(b.def)?;
    let def = db.service(service);
    (def.radius_m > 0.0).then_some((def.kind, access, def.radius_m))
}

/// The coverage of `kind` per segment index that the next recomputation gives for this
/// network and these buildings: Active buildings of the kind in id order, max of their
/// [`site_coverage`]. The same function backs the sim and the client's live overlay.
pub fn kind_coverage(
    g: &RoadGraph,
    buildings: &BuildingGrid,
    db: &ContentDb,
    kind: ServiceKind,
) -> Vec<f32> {
    let mut out = vec![0.0; g.seg_slots];
    let mut sc = Scratch::new(g);
    for (_, b) in buildings.iter() {
        if let Some((k, access, radius)) = served_site(db, b)
            && k == kind
        {
            fold_max(&mut out, &site_coverage_with(g, &mut sc, access, radius));
        }
    }
    out
}

fn fold_max(out: &mut [f32], site: &[(u32, f32)]) {
    for &(i, c) in site {
        let slot = &mut out[i as usize];
        *slot = slot.max(c);
    }
}

/// Recomputes every kind's coverage from the world.
fn compute(
    net: &NetGraph,
    buildings: &BuildingGrid,
    content: &SimContent,
) -> BTreeMap<ServiceKind, Vec<f32>> {
    compute_on(
        &std::sync::Arc::new(RoadGraph::new(net)),
        buildings,
        content,
        &mut CoverageMemo::default(),
    )
}

/// What one site gave, remembered for the next recomputation: valid while the building, its
/// access point and radius and the road graph are the same.
struct SiteMemo {
    generation: u32,
    access: (SegmentId, u32),
    radius: u32,
    cover: Vec<(u32, f32)>,
}

/// The per-site results of the last [`update_coverage`] (T-258: a pass re-searched the roads of
/// every service building, ~30 ms at 100k, although nothing it reads had changed). A cache
/// only: the values are what a search would give, so it is neither saved nor hashed.
#[derive(Default)]
pub struct CoverageMemo {
    graph: Option<std::sync::Arc<RoadGraph>>,
    scratch: Option<Scratch>,
    /// By arena index of the building.
    sites: Vec<Option<SiteMemo>>,
}

/// [`compute`] on a road graph already built.
fn compute_on(
    g: &std::sync::Arc<RoadGraph>,
    buildings: &BuildingGrid,
    content: &SimContent,
    memo: &mut CoverageMemo,
) -> BTreeMap<ServiceKind, Vec<f32>> {
    let same = memo
        .graph
        .as_ref()
        .is_some_and(|old| std::sync::Arc::ptr_eq(old, g) || **old == **g);
    if !same {
        memo.graph = Some(std::sync::Arc::clone(g));
        memo.scratch = Some(Scratch::new(g));
        memo.sites.clear();
    }
    let CoverageMemo { scratch, sites, .. } = memo;
    let Some(sc) = scratch.as_mut() else {
        return BTreeMap::new();
    };
    let mut out: BTreeMap<ServiceKind, Vec<f32>> = BTreeMap::new();
    // Buildings in id order: the max is order-free, but the iteration is deterministic anyway.
    for (id, b) in buildings.iter() {
        let Some((kind, access, radius)) = served_site(&content.0, b) else {
            continue;
        };
        let i = id.index as usize;
        if sites.len() <= i {
            sites.resize_with(i + 1, || None);
        }
        let key = (access.segment, access.t.to_bits());
        let fresh = matches!(&sites[i], Some(m)
            if m.generation == id.generation && m.access == key && m.radius == radius.to_bits());
        if !fresh {
            sites[i] = Some(SiteMemo {
                generation: id.generation,
                access: key,
                radius: radius.to_bits(),
                cover: site_coverage_with(g, sc, access, radius),
            });
        }
        let v = out.entry(kind).or_insert_with(|| vec![0.0; g.seg_slots]);
        if let Some(m) = &sites[i] {
            fold_max(v, &m.cover);
        }
    }
    out
}

/// Services phase: recompute the coverage on every [`COVERAGE_PERIOD`]th tick.
pub fn update_coverage(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: ServiceNet,
    buildings: Res<BuildingGrid>,
    mut coverage: ResMut<ServiceCoverage>,
    mut memo: Local<CoverageMemo>,
) {
    if !tick.0.is_multiple_of(COVERAGE_PERIOD) {
        return;
    }
    let Some(content) = content else { return };
    coverage.by_kind = compute_on(&net.graph(), &buildings, &content, &mut memo);
    coverage.generation += 1;
}

/// Recomputes the coverage now (after a load: the values are not saved).
pub fn rebuild_coverage(world: &mut World) {
    let Some(content) = world.get_resource::<SimContent>() else {
        return;
    };
    let (Some(net), Some(buildings)) = (
        world.get_resource::<NetGraph>(),
        world.get_resource::<BuildingGrid>(),
    ) else {
        return;
    };
    let by_kind = compute(net, buildings, content);
    let mut cov = world.get_resource_or_insert_with(ServiceCoverage::default);
    cov.by_kind = by_kind;
    cov.generation += 1;
}

/// What a sim runner installs for this crate.
pub struct ServicePlugin;

impl ServicePlugin {
    /// Inserts the resources.
    pub fn init(world: &mut World) {
        world.init_resource::<ServiceCoverage>();
        world.init_resource::<ServiceRoads>();
        world.init_resource::<crate::ServiceState>();
        world.init_resource::<crate::MailState>();
    }
}

/// Reusable arrays of a search over one [`RoadGraph`]: only the nodes touched are reset.
pub(crate) struct Scratch {
    pub dist: Vec<f32>,
    done: Vec<bool>,
    touched: Vec<usize>,
    pub tail: Vec<f32>,
    pub tail_touched: Vec<usize>,
}

impl Scratch {
    pub fn new(g: &RoadGraph) -> Self {
        let n = g.adj.len();
        Self {
            dist: vec![f32::INFINITY; n],
            done: vec![false; n],
            touched: Vec::new(),
            tail: vec![f32::INFINITY; n],
            tail_touched: Vec::new(),
        }
    }

    fn reset(&mut self) {
        for &n in &self.touched {
            self.dist[n] = f32::INFINITY;
            self.done[n] = false;
        }
        self.touched.clear();
    }

    fn set(&mut self, n: usize, d: f32) {
        if self.dist[n].is_infinite() {
            self.touched.push(n);
        }
        self.dist[n] = d;
    }

    pub fn clear_tails(&mut self) {
        for &n in &self.tail_touched {
            self.tail[n] = f32::INFINITY;
        }
        self.tail_touched.clear();
    }
}
