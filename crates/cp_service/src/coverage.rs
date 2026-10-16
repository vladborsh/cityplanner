//! Coverage of a service along the roads (§16 M2 D "Покрытие").
//!
//! Per service kind, every segment gets `max over Active buildings of the kind of
//! 1 − d / radius_m`, clamped at 0, where `d` is the road distance from the building's access
//! point to the nearest point of the segment: 0 on the access segment itself, otherwise the
//! shorter way to one of its ends. Roads are walked without regard to direction (Dijkstra over
//! the segment graph by length); the access point sits `t × length` along its segment.
//!
//! The values are recomputed when `tick % 64 == 0`. Since save v22 (T-135) they are **saved
//! and hashed**: between two boundaries the sim reads the values of the last boundary, and a
//! load in the middle of a cycle must see those same values even if a road or a building
//! changed since. [`rebuild_coverage`] is only for saves that carry none (formats before 22).

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_ecs::world::World;
use cp_building::{AccessPoint, Building, BuildingGrid, BuildingState};
use cp_content::{ServiceKind, SimContent};
use cp_core::Tick;
use cp_net::{NetGraph, SegmentId};
use serde::{Deserialize, Serialize};

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

/// The road network as a graph of nodes joined by segments, for distances along the roads.
pub struct RoadGraph<'a> {
    net: &'a NetGraph,
    adj: Vec<Vec<(usize, f32)>>,
    /// Segments in id order with their end node indices.
    segs: Vec<(SegmentId, usize, usize)>,
    seg_slots: usize,
}

impl<'a> RoadGraph<'a> {
    pub fn new(net: &'a NetGraph) -> Self {
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
        for (id, s) in net.segments() {
            let (a, b) = (s.start.index as usize, s.end.index as usize);
            adj[a].push((b, s.length));
            adj[b].push((a, s.length));
            segs.push((id, a, b));
        }
        Self {
            net,
            adj,
            segs,
            seg_slots,
        }
    }

    /// Metres along segment `seg` at curve parameter `t`.
    fn offset(&self, seg: SegmentId, t: f32) -> Option<f32> {
        let s = self.net.segment(seg)?;
        Some((t * s.length).clamp(0.0, s.length))
    }

    /// Road distance from the access point to every node; unreached nodes and nodes beyond
    /// `limit` stay infinite. Roads are walked without regard to direction.
    pub fn node_distances(&self, from: AccessPoint, limit: f32) -> Vec<f32> {
        let mut dist = vec![f32::INFINITY; self.adj.len()];
        let (Some(start), Some(s)) = (
            self.net.segment(from.segment),
            self.offset(from.segment, from.t),
        ) else {
            return dist;
        };
        let (a, b) = (start.start.index as usize, start.end.index as usize);
        dist[a] = dist[a].min(s);
        dist[b] = dist[b].min(start.length - s);
        let mut done = vec![false; dist.len()];
        // Dijkstra with a linear scan: the graphs are small and this runs every 64 ticks.
        loop {
            let mut best = None;
            for (i, d) in dist.iter().enumerate() {
                if !done[i] && d.is_finite() && best.is_none_or(|(_, bd)| *d < bd) {
                    best = Some((i, *d));
                }
            }
            let Some((u, du)) = best else { break };
            done[u] = true;
            if du >= limit {
                continue;
            }
            for &(v, len) in &self.adj[u] {
                if du + len < dist[v] {
                    dist[v] = du + len;
                }
            }
        }
        dist
    }

    /// Road distance from `from` (whose [`Self::node_distances`] are `dist`) to the access
    /// point `to`; infinite when unreachable.
    pub fn distance_to(&self, dist: &[f32], from: AccessPoint, to: AccessPoint) -> f32 {
        let (Some(seg), Some(s2)) = (self.net.segment(to.segment), self.offset(to.segment, to.t))
        else {
            return f32::INFINITY;
        };
        let via = dist[seg.start.index as usize] + s2;
        let via = via.min(dist[seg.end.index as usize] + (seg.length - s2));
        if from.segment == to.segment
            && let Some(s1) = self.offset(from.segment, from.t)
        {
            return via.min((s1 - s2).abs());
        }
        via
    }
}

/// Folds the coverage of one building (`radius` along the roads from `access`) into `out` by max.
fn add_building(g: &RoadGraph, access: AccessPoint, radius: f32, out: &mut [f32]) {
    let dist = g.node_distances(access, radius);
    for &(id, na, nb) in &g.segs {
        let d = if id == access.segment {
            0.0
        } else {
            dist[na].min(dist[nb])
        };
        let c = 1.0 - d / radius;
        if c > 0.0 {
            let slot = &mut out[id.index as usize];
            *slot = slot.max(c);
        }
    }
}

/// Recomputes every kind's coverage from the world.
fn compute(
    net: &NetGraph,
    buildings: &BuildingGrid,
    content: &SimContent,
) -> BTreeMap<ServiceKind, Vec<f32>> {
    let g = RoadGraph::new(net);
    let mut out: BTreeMap<ServiceKind, Vec<f32>> = BTreeMap::new();
    let db = &content.0;
    // Buildings in id order: the max is order-free, but the iteration is deterministic anyway.
    for (_, b) in buildings.iter() {
        if b.state != BuildingState::Active {
            continue;
        }
        let Some(access) = b.access() else { continue };
        let Some(service) = db
            .building(b.def)
            .service
            .as_deref()
            .and_then(|s| db.service_id(s))
        else {
            continue;
        };
        let def = db.service(service);
        if def.radius_m <= 0.0 {
            continue;
        }
        let v = out
            .entry(def.kind)
            .or_insert_with(|| vec![0.0; g.seg_slots]);
        add_building(&g, access, def.radius_m, v);
    }
    out
}

/// Services phase: recompute the coverage on every [`COVERAGE_PERIOD`]th tick.
pub fn update_coverage(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    buildings: Res<BuildingGrid>,
    mut coverage: ResMut<ServiceCoverage>,
) {
    if !tick.0.is_multiple_of(COVERAGE_PERIOD) {
        return;
    }
    let Some(content) = content else { return };
    coverage.by_kind = compute(&net, &buildings, &content);
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
        world.init_resource::<crate::ServiceState>();
    }
}
