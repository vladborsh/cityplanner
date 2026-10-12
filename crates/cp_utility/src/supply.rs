//! Supply of water, sewage and power (`architecture/system-design.md` §10).
//!
//! Every [`SUPPLY_PERIOD`] = 64 ticks [`update_supply`] recomputes, from scratch, which
//! `Active` buildings lack water, sewage or power and writes the `NO_WATER` / `NO_SEWAGE` /
//! `NO_POWER` bits of `BuildingFlags`. Nothing new is saved: the flags are derived (the
//! next recompute after a load rewrites them). The computation is [`compute_supply`], a pure
//! function of the content, the buildings and the utility graph.
//!
//! * **Reach.** A building is *attached* to a network when its position is within
//!   [`LINE_REACH_M`] (40 m) of one of its links (the lot is within 40 m of the pipe); it then
//!   belongs to the connected component of the nearest link.
//! * **Water.** Pumps (service kind `Water`, `Active`) add their `capacity` to the pipe
//!   component they are attached to. Consumers in that component, in ascending `BuildingId`,
//!   subtract their `consumption[Water]`; the first one that does not fit and every later one
//!   get `NO_WATER` (the deficit cuts the highest ids first).
//! * **Sewage.** The same with outflows (`Sewage`), but an outflow only works when its pipe
//!   component also holds a working pump ("an outflow with no connection to water does not
//!   work").
//! * **Power.** Plants (`Power`) add capacity to the line component they are attached to (a
//!   plant with no line is a pool of its own). From the pool a BFS runs in layers: layer 0
//!   is the buildings attached to the pool's lines (and the plants), each following layer is
//!   the buildings within [`POWER_HOP_M`] (30 m) of a building *supplied* in the previous
//!   one. Within a layer buildings are served in ascending id; the first that does not fit
//!   and everything after it (this layer and later ones) get `NO_POWER`. Pools are
//!   processed in order of their line component, then unattached plants by id; a building
//!   belongs to the first pool that reaches it.
//!
//! Plant output does not depend on the plants' own power; service buildings are ordinary
//! consumers for their flags.

use std::collections::{BTreeMap, BTreeSet};

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use cp_building::{BuildingFlags, BuildingGrid, BuildingId, BuildingState};
use cp_content::{ContentDb, ServiceKind, SimContent, UtilityKind};
use cp_core::Tick;
use cp_core::curve::to_xz;

use crate::graph::{UtilityGraph, UtilityLinkId, UtilityNet};

/// Recompute period, ticks (§2: utilities run every 64 ticks).
pub const SUPPLY_PERIOD: u64 = 64;
/// How far a building may be from a pipe or power line, metres.
pub const LINE_REACH_M: f32 = 40.0;
/// How far a supplied building passes power on to other buildings, metres.
pub const POWER_HOP_M: f32 = 30.0;

const LINK_SAMPLE_M: f32 = 16.0;
const BUCKET_M: f32 = 64.0;

struct Site {
    id: BuildingId,
    pos: Vec2,
    needs: [f32; 3],
    /// `(kind, capacity)` of a utility source.
    source: Option<(ServiceKind, f32)>,
}

fn bucket(p: Vec2, size: f32) -> (i32, i32) {
    ((p.x / size).floor() as i32, (p.y / size).floor() as i32)
}

/// Link lookup of one network: which component a position is attached to.
pub(crate) struct Attach {
    buckets: BTreeMap<(i32, i32), Vec<UtilityLinkId>>,
    comp_of_link: BTreeMap<UtilityLinkId, u32>,
    /// XZ bounding box of each link's control points: a Bézier curve lies inside it, so a
    /// position farther than [`LINE_REACH_M`] from the box cannot be within reach of the curve
    /// and the (expensive) exact nearest-point search is skipped for that link.
    bbox: BTreeMap<UtilityLinkId, (Vec2, Vec2)>,
}

impl Attach {
    pub(crate) fn build(graph: &UtilityGraph, net: UtilityNet) -> Self {
        let comps = graph.components(net);
        let label: BTreeMap<_, _> = comps.labels.iter().copied().collect();
        let mut buckets: BTreeMap<(i32, i32), Vec<UtilityLinkId>> = BTreeMap::new();
        let mut comp_of_link = BTreeMap::new();
        let mut bbox = BTreeMap::new();
        for (id, l) in graph.links().filter(|(_, l)| l.net == net) {
            comp_of_link.insert(id, label[&l.a]);
            let c = [l.curve.p0, l.curve.p1, l.curve.p2, l.curve.p3].map(to_xz);
            bbox.insert(
                id,
                (
                    c.iter().fold(Vec2::splat(f32::MAX), |a, p| a.min(*p)),
                    c.iter().fold(Vec2::splat(f32::MIN), |a, p| a.max(*p)),
                ),
            );
            let n = (l.length / LINK_SAMPLE_M).ceil().max(1.0) as usize;
            let mut seen = BTreeSet::new();
            for i in 0..=n {
                let p = to_xz(l.curve.point(i as f32 / n as f32));
                if seen.insert(bucket(p, BUCKET_M)) {
                    buckets.entry(bucket(p, BUCKET_M)).or_default().push(id);
                }
            }
        }
        Self {
            buckets,
            comp_of_link,
            bbox,
        }
    }

    /// The component of the nearest link within [`LINE_REACH_M`] of `p`.
    pub(crate) fn comp_at(&self, graph: &UtilityGraph, p: Vec2) -> Option<u32> {
        let (bx, bz) = bucket(p, BUCKET_M);
        let mut cand = BTreeSet::new();
        for dx in -1..=1 {
            for dz in -1..=1 {
                if let Some(v) = self.buckets.get(&(bx + dx, bz + dz)) {
                    cand.extend(v.iter().copied());
                }
            }
        }
        let mut best: Option<(f32, UtilityLinkId)> = None;
        for id in cand {
            let (lo, hi) = self.bbox[&id];
            let off = (lo - p).max(p - hi).max(Vec2::ZERO);
            if off.length() > LINE_REACH_M {
                continue;
            }
            let (_, d) = graph.link(id)?.curve.nearest_xz(p);
            if d <= LINE_REACH_M && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, id));
            }
        }
        best.map(|(_, id)| self.comp_of_link[&id])
    }
}

/// The sites binned by [`POWER_HOP_M`] cells in a dense grid over their bounding box (cell
/// contents in ascending site index): the neighbour lookup of the power BFS is an array index.
struct HopGrid {
    x0: i32,
    z0: i32,
    w: usize,
    h: usize,
    start: Vec<u32>,
    items: Vec<usize>,
}

impl HopGrid {
    fn build(sites: &[Site]) -> Self {
        let cells: Vec<(i32, i32)> = sites.iter().map(|s| bucket(s.pos, POWER_HOP_M)).collect();
        let (x0, x1) = cells
            .iter()
            .fold((i32::MAX, i32::MIN), |a, c| (a.0.min(c.0), a.1.max(c.0)));
        let (z0, z1) = cells
            .iter()
            .fold((i32::MAX, i32::MIN), |a, c| (a.0.min(c.1), a.1.max(c.1)));
        if cells.is_empty() {
            return Self {
                x0: 0,
                z0: 0,
                w: 0,
                h: 0,
                start: vec![0],
                items: Vec::new(),
            };
        }
        let (w, h) = ((x1 - x0 + 1) as usize, (z1 - z0 + 1) as usize);
        let at = |c: (i32, i32)| (c.1 - z0) as usize * w + (c.0 - x0) as usize;
        let mut start = vec![0u32; w * h + 1];
        for &c in &cells {
            start[at(c) + 1] += 1;
        }
        for i in 0..w * h {
            start[i + 1] += start[i];
        }
        let mut fill = start.clone();
        let mut items = vec![0usize; cells.len()];
        for (i, &c) in cells.iter().enumerate() {
            let a = at(c);
            items[fill[a] as usize] = i;
            fill[a] += 1;
        }
        Self {
            x0,
            z0,
            w,
            h,
            start,
            items,
        }
    }

    fn get(&self, bx: i32, bz: i32) -> &[usize] {
        let (x, z) = (bx - self.x0, bz - self.z0);
        if x < 0 || z < 0 || x as usize >= self.w || z as usize >= self.h {
            return &[];
        }
        let c = z as usize * self.w + x as usize;
        &self.items[self.start[c] as usize..self.start[c + 1] as usize]
    }
}

fn kind_index(k: UtilityKind) -> usize {
    match k {
        UtilityKind::Water => 0,
        UtilityKind::Sewage => 1,
        UtilityKind::Power => 2,
    }
}

/// Which pipe and power-line component each building is attached to, remembered between
/// recomputes: positions and links rarely change, and the nearest-link search is the expensive
/// part of [`compute_supply`]. An entry is valid for the graph it was computed for (a
/// fingerprint of every link) and for the position it was computed at.
/// `(generation, position, pipe component, line component)` of one building.
type CachedAttach = (u32, Vec2, Option<u32>, Option<u32>);

#[derive(Default)]
pub struct AttachCache {
    graph: u64,
    /// The link lookups of that graph, built when the first building needs one.
    attach: Option<(Attach, Attach)>,
    /// By arena index of the building: `(generation, position, pipe component, line component)`.
    by_building: Vec<Option<CachedAttach>>,
}

/// Consumption per utility and the source `(kind, capacity)` of one building def.
type DefSupply = ([f32; 3], Option<(ServiceKind, f32)>);

/// FNV-1a over every link: network, endpoints and curve.
fn graph_fingerprint(graph: &UtilityGraph) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |v: u32| {
        for b in v.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for (id, l) in graph.links() {
        mix(id.index);
        mix(id.generation);
        mix(l.net as u32);
        for n in [l.a, l.b] {
            mix(n.index);
            mix(n.generation);
        }
        for p in [l.curve.p0, l.curve.p1, l.curve.p2, l.curve.p3] {
            mix(p.x.to_bits());
            mix(p.y.to_bits());
            mix(p.z.to_bits());
        }
    }
    h
}

/// Buildings `(id -> missing-supply bits)` for every `Active` building that lacks something;
/// buildings that have everything are absent.
pub fn compute_supply(
    db: &ContentDb,
    buildings: &BuildingGrid,
    graph: &UtilityGraph,
) -> BTreeMap<BuildingId, u8> {
    let missing = compute_supply_cached(db, buildings, graph, &mut AttachCache::default());
    buildings
        .iter()
        .filter_map(|(id, _)| {
            let bits = missing.get(id.index as usize).copied().unwrap_or(0);
            (bits != 0).then_some((id, bits))
        })
        .collect()
}

/// [`compute_supply`] reusing (and refreshing) `cache`; the result is the same as without it,
/// but dense: the missing-supply bits by arena index of the building (`0` = has everything).
pub fn compute_supply_cached(
    db: &ContentDb,
    buildings: &BuildingGrid,
    graph: &UtilityGraph,
    cache: &mut AttachCache,
) -> Vec<u8> {
    // Needs and source of each building def, once.
    let defs: Vec<DefSupply> = db
        .buildings()
        .map(|(_, def)| {
            let mut needs = [0.0; 3];
            for (k, v) in &def.consumption {
                needs[kind_index(*k)] = *v;
            }
            let source = def
                .service
                .as_deref()
                .and_then(|s| db.service_id(s))
                .map(|s| db.service(s))
                .map(|s| (s.kind, s.capacity as f32));
            (needs, source)
        })
        .collect();
    let sites: Vec<Site> = buildings
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .map(|(id, b)| {
            let (needs, source) = defs[b.def.index()];
            Site {
                id,
                pos: b.pos,
                needs,
                source,
            }
        })
        .collect();
    let mut missing: Vec<u8> = vec![
        0;
        sites
            .iter()
            .map(|s| s.id.index as usize + 1)
            .max()
            .unwrap_or(0)
    ];
    let mut flag = |id: BuildingId, bit: u8| {
        missing[id.index as usize] |= bit;
    };

    // The component of each building in the pipe and in the power network.
    let fingerprint = graph_fingerprint(graph);
    if cache.graph != fingerprint {
        cache.graph = fingerprint;
        cache.attach = None;
        cache.by_building.clear();
    }
    let mut comps: Vec<(Option<u32>, Option<u32>)> = Vec::with_capacity(sites.len());
    for s in &sites {
        let i = s.id.index as usize;
        let hit = match cache.by_building.get(i) {
            Some(Some((g, pos, pipe, line))) if *g == s.id.generation && *pos == s.pos => {
                Some((*pipe, *line))
            }
            _ => None,
        };
        let pair = hit.unwrap_or_else(|| {
            let (pipes, lines) = cache.attach.get_or_insert_with(|| {
                (
                    Attach::build(graph, UtilityNet::Pipe),
                    Attach::build(graph, UtilityNet::Power),
                )
            });
            let pair = (pipes.comp_at(graph, s.pos), lines.comp_at(graph, s.pos));
            if cache.by_building.len() <= i {
                cache.by_building.resize(i + 1, None);
            }
            cache.by_building[i] = Some((s.id.generation, s.pos, pair.0, pair.1));
            pair
        });
        comps.push(pair);
    }

    // --- pipes: water, then sewage ---
    let comp: Vec<Option<u32>> = comps.iter().map(|c| c.0).collect();
    let mut water_cap: BTreeMap<u32, f32> = BTreeMap::new();
    for (s, c) in sites.iter().zip(&comp) {
        if let (Some((ServiceKind::Water, cap)), Some(c)) = (s.source, c) {
            *water_cap.entry(*c).or_default() += cap;
        }
    }
    let mut sewage_cap: BTreeMap<u32, f32> = BTreeMap::new();
    for (s, c) in sites.iter().zip(&comp) {
        if let (Some((ServiceKind::Sewage, cap)), Some(c)) = (s.source, c)
            && water_cap.get(c).is_some_and(|w| *w > 0.0)
        {
            *sewage_cap.entry(*c).or_default() += cap;
        }
    }
    for (kind, bit, caps) in [
        (0usize, BuildingFlags::NO_WATER, &mut water_cap),
        (1usize, BuildingFlags::NO_SEWAGE, &mut sewage_cap),
    ] {
        let mut exhausted: BTreeSet<u32> = BTreeSet::new();
        // `sites` are in ascending id order.
        for (s, c) in sites.iter().zip(&comp) {
            let need = s.needs[kind];
            if need <= 0.0 {
                continue;
            }
            let Some(c) = c else {
                flag(s.id, bit);
                continue;
            };
            let left = caps.get_mut(c);
            match left {
                Some(left) if !exhausted.contains(c) && *left >= need => *left -= need,
                _ => {
                    exhausted.insert(*c);
                    flag(s.id, bit);
                }
            }
        }
    }

    // --- power ---
    let attached: Vec<Option<u32>> = comps.iter().map(|c| c.1).collect();
    let hop = HopGrid::build(&sites);
    // Pools: line components (ascending) first, then unattached plants by id.
    enum Pool {
        Comp(u32),
        Plant(usize),
    }
    let mut cap: BTreeMap<u32, f32> = BTreeMap::new();
    let mut pools: Vec<(Pool, f32)> = Vec::new();
    let mut loose: Vec<(usize, f32)> = Vec::new();
    for (i, s) in sites.iter().enumerate() {
        if let Some((ServiceKind::Power, c)) = s.source {
            match attached[i] {
                Some(comp) => *cap.entry(comp).or_default() += c,
                None => loose.push((i, c)),
            }
        }
    }
    pools.extend(cap.into_iter().map(|(c, v)| (Pool::Comp(c), v)));
    pools.extend(loose.into_iter().map(|(i, v)| (Pool::Plant(i), v)));
    let mut visited = vec![false; sites.len()];
    let mut served = vec![false; sites.len()];
    for (pool, mut left) in pools {
        let mut layer: Vec<usize> = match pool {
            Pool::Comp(c) => (0..sites.len())
                .filter(|&i| attached[i] == Some(c) && !visited[i])
                .collect(),
            Pool::Plant(i) => vec![i],
        };
        let mut exhausted = false;
        while !layer.is_empty() {
            layer.sort_unstable();
            let mut next: BTreeSet<usize> = BTreeSet::new();
            for &i in &layer {
                visited[i] = true;
                let need = sites[i].needs[2];
                if need <= 0.0 || (!exhausted && left >= need) {
                    left -= need;
                    served[i] = true;
                    let (bx, bz) = bucket(sites[i].pos, POWER_HOP_M);
                    for dx in -1..=1 {
                        for dz in -1..=1 {
                            for &j in hop.get(bx + dx, bz + dz) {
                                if !visited[j] && sites[j].pos.distance(sites[i].pos) <= POWER_HOP_M
                                {
                                    next.insert(j);
                                }
                            }
                        }
                    }
                } else {
                    exhausted = true;
                }
            }
            // Every member of `layer` is visited by now, so this also drops those.
            next.retain(|j| !visited[*j]);
            layer = next.into_iter().collect();
        }
    }
    for (i, s) in sites.iter().enumerate() {
        if s.needs[2] > 0.0 && !served[i] {
            flag(s.id, BuildingFlags::NO_POWER);
        }
    }
    missing
}

/// `Utilities`-phase system: every [`SUPPLY_PERIOD`] ticks, rewrites the utility bits of the
/// buildings' flags.
pub fn update_supply(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    graph: Res<UtilityGraph>,
    mut buildings: ResMut<BuildingGrid>,
    mut cache: Local<AttachCache>,
) {
    if !tick.0.is_multiple_of(SUPPLY_PERIOD) {
        return;
    }
    let Some(content) = content else { return };
    let missing = compute_supply_cached(&content.0, &buildings, &graph, &mut cache);
    let changes: Vec<(BuildingId, u8)> = buildings
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .filter_map(|(id, b)| {
            let want = missing.get(id.index as usize).copied().unwrap_or(0);
            ((b.flags.0 & BuildingFlags::UTILITY_MASK) != want).then_some((id, want))
        })
        .collect();
    for (id, want) in changes {
        if let Some(b) = buildings.get_mut(id) {
            b.flags.0 = (b.flags.0 & !BuildingFlags::UTILITY_MASK) | want;
        }
    }
}

/// Which positions a network reaches (within [`LINE_REACH_M`] of one of its links), for the
/// coverage overlay. Built once per graph, then asked per position.
pub struct Coverage {
    attach: Attach,
}

impl Coverage {
    pub fn new(graph: &UtilityGraph, net: UtilityNet) -> Self {
        Self {
            attach: Attach::build(graph, net),
        }
    }

    /// Whether `p` (XZ) is within reach of the network, the rule buildings use to attach.
    pub fn covers(&self, graph: &UtilityGraph, p: Vec2) -> bool {
        self.attach.comp_at(graph, p).is_some()
    }
}
