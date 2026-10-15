//! Coverage of a service along the roads (§16 M2 D "Покрытие").
//!
//! Per service kind, every segment gets `max over Active buildings of the kind of
//! 1 − d / radius_m`, clamped at 0, where `d` is the road distance from the building's access
//! point to the nearest point of the segment: 0 on the access segment itself, otherwise the
//! shorter way to one of its ends. Roads are walked without regard to direction (Dijkstra over
//! the segment graph by length); the access point sits `t × length` along its segment.
//!
//! The values are *derived*: recomputed when `tick % 64 == 0` and rebuilt by
//! [`rebuild_coverage`] after a load, never saved or hashed. A load in the middle of a cycle
//! rebuilds from the loaded state, so it matches the continuous run as long as the buildings
//! and roads did not change since the last boundary.

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_ecs::world::World;
use cp_building::{Building, BuildingGrid, BuildingState};
use cp_content::{ServiceKind, SimContent};
use cp_core::Tick;
use cp_net::{NetGraph, SegmentId};

/// Ticks between coverage recomputations.
pub const COVERAGE_PERIOD: u64 = 64;

/// Coverage `0..=1` per service kind and segment, dense by segment index.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct ServiceCoverage {
    by_kind: BTreeMap<ServiceKind, Vec<f32>>,
    /// Changes at every recomputation (cache key of the published snapshot).
    pub generation: u64,
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

/// Road distance from `(segment, s)` to every node, then coverage of one building folded into
/// `out` by max.
fn add_building(
    net: &NetGraph,
    adj: &[Vec<(usize, f32)>],
    seg_ends: &[(SegmentId, usize, usize, f32)],
    access: (SegmentId, f32),
    radius: f32,
    out: &mut [f32],
) {
    let Some(start) = net.segment(access.0) else {
        return;
    };
    let s = (access.1 * start.length).clamp(0.0, start.length);
    let mut dist = vec![f32::INFINITY; adj.len()];
    // Dijkstra with a linear scan: the graphs are small and this runs every 64 ticks.
    let (a, b) = (start.start.index as usize, start.end.index as usize);
    dist[a] = dist[a].min(s);
    dist[b] = dist[b].min(start.length - s);
    let mut done = vec![false; adj.len()];
    loop {
        let mut best = None;
        for (i, d) in dist.iter().enumerate() {
            if !done[i] && d.is_finite() && best.is_none_or(|(_, bd)| *d < bd) {
                best = Some((i, *d));
            }
        }
        let Some((u, du)) = best else { break };
        done[u] = true;
        if du >= radius {
            continue;
        }
        for &(v, len) in &adj[u] {
            if du + len < dist[v] {
                dist[v] = du + len;
            }
        }
    }
    for &(id, na, nb, _) in seg_ends {
        let d = if id == access.0 {
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
    let mut nodes = 0usize;
    let mut segs = 0usize;
    for (id, s) in net.segments() {
        segs = segs.max(id.index as usize + 1);
        nodes = nodes
            .max(s.start.index as usize + 1)
            .max(s.end.index as usize + 1);
    }
    let mut adj: Vec<Vec<(usize, f32)>> = vec![Vec::new(); nodes];
    let mut seg_ends = Vec::new();
    for (id, s) in net.segments() {
        let (a, b) = (s.start.index as usize, s.end.index as usize);
        adj[a].push((b, s.length));
        adj[b].push((a, s.length));
        seg_ends.push((id, a, b, s.length));
    }
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
        let v = out.entry(def.kind).or_insert_with(|| vec![0.0; segs]);
        add_building(
            net,
            &adj,
            &seg_ends,
            (access.segment, access.t),
            def.radius_m,
            v,
        );
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
    }
}
