//! Supply of water, sewage and power (`architecture/system-design.md` §10).
//!
//! Every [`SUPPLY_PERIOD`] = 64 ticks [`update_supply`] recomputes, from scratch, which
//! `Active` buildings lack water, sewage or power and writes the `NO_WATER` / `NO_SEWAGE` /
//! `NO_POWER` bits of `BuildingFlags`. Nothing new is saved: the flags are derived (the
//! next recompute after a load rewrites them). The computation is [`compute_supply`], a pure
//! function of the content, the buildings and the utility graph.
//!
//! * **Reach.** A building is *attached* to a network when its footprint edge is within
//!   `line_reach_m` (power, 80 m) or `pipe_reach_m` (pipes, 64 m) of `balance.ron` `networks` of
//!   one of its links (its centre is within that plus the building's half size, [`Site::half`]);
//!   it then belongs to the connected component of the nearest link.
//! * **Water.** Pumps (service kind `Water`, `Active`) add their `capacity` to the pipe
//!   component they are attached to. Consumers in that component, in ascending `BuildingId`,
//!   subtract their `consumption[Water]`; the first one that does not fit and every later one
//!   get `NO_WATER` (the deficit cuts the highest ids first).
//! * **Sewage.** The same with the sewage sources (service kind `Sewage`: outflows and
//!   treatment plants), but such a source only works when its pipe component also holds a
//!   working water source (service kind `Water`: a pump or a water tower; "a sewage source
//!   with no connection to water does not work").
//! * **Power.** Buildings pass power on to each other: two whose footprint edges are within
//!   `power_gap_m` (64 m) are joined (BUG-036), and so are the buildings attached to one power
//!   line component (`line_reach_m`, 80 m, of a link). A connected group of buildings is one
//!   pool with the summed capacity of every plant (`Power`) in it (BUG-087: a plant without a
//!   line was a pool of its own, and the first one claimed the town). Each pool serves from all
//!   its plants and the buildings on their lines at once, in layers outwards over the gap and
//!   the lines, ascending id within a layer; the first building that does not fit and every
//!   later one (this layer and the farther ones) get `NO_POWER`. A group with no plant has no
//!   power.
//!
//! Plant output does not depend on the plants' own power; service buildings are ordinary
//! consumers for their flags.

use std::collections::{BTreeMap, BTreeSet};

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use cp_building::{BuildingFlags, BuildingGrid, BuildingId, BuildingState};
use cp_content::{ContentDb, Networks, PolicyKind, ServiceKind, SimContent, UtilityKind};
use cp_core::Tick;
use cp_core::curve::to_xz;
use cp_district::{Policies, PolicyParams};

use crate::graph::{UtilityGraph, UtilityLinkId, UtilityNet};

/// Recompute period, ticks (§2: utilities run every 64 ticks).
pub const SUPPLY_PERIOD: u64 = 64;

/// How far a building may be from a link of `net`, metres (`balance.ron` `networks`).
pub fn reach_m(n: &Networks, net: UtilityNet) -> f32 {
    match net {
        UtilityNet::Pipe => n.pipe_reach_m,
        UtilityNet::Power => n.line_reach_m,
    }
}

const LINK_SAMPLE_M: f32 = 16.0;
const BUCKET_M: f32 = 64.0;

struct Site {
    id: BuildingId,
    pos: Vec2,
    /// Half the longer side of the footprint, metres.
    half: f32,
    needs: [f32; 3],
    /// `(kind, capacity)` of a utility source.
    source: Option<(ServiceKind, f32)>,
}

fn bucket(p: Vec2, size: f32) -> (i32, i32) {
    ((p.x / size).floor() as i32, (p.y / size).floor() as i32)
}

/// Link lookup of one network: which component a position is attached to.
pub(crate) struct Attach {
    /// Reach of this network, metres.
    reach: f32,
    buckets: BTreeMap<(i32, i32), Vec<UtilityLinkId>>,
    comp_of_link: BTreeMap<UtilityLinkId, u32>,
    /// XZ bounding box of each link's control points: a Bézier curve lies inside it, so a
    /// position farther than `reach` from the box cannot be within reach of the curve
    /// and the (expensive) exact nearest-point search is skipped for that link.
    bbox: BTreeMap<UtilityLinkId, (Vec2, Vec2)>,
}

impl Attach {
    pub(crate) fn build(graph: &UtilityGraph, net: UtilityNet, reach: f32) -> Self {
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
            reach,
            buckets,
            comp_of_link,
            bbox,
        }
    }

    /// The nearest link within the reach of `p` (ties: the lowest id).
    pub(crate) fn link_at(
        &self,
        graph: &UtilityGraph,
        p: Vec2,
        half: f32,
    ) -> Option<UtilityLinkId> {
        let mut best: Option<(f32, UtilityLinkId)> = None;
        self.within(graph, p, half, |d, id, _| {
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, id));
            }
        });
        best.map(|(_, id)| id)
    }

    /// Every link within the reach of `p`, ascending.
    pub(crate) fn links_at(&self, graph: &UtilityGraph, p: Vec2, half: f32) -> Vec<UtilityLinkId> {
        let mut all = Vec::new();
        self.within(graph, p, half, |_, id, _| all.push(id));
        all
    }

    /// The component of a link of this network (`None` for a link that is gone).
    fn comp_of(&self, link: UtilityLinkId) -> Option<u32> {
        self.comp_of_link.get(&link).copied()
    }

    /// The component of the nearest link within the reach of `p`.
    pub(crate) fn comp_at(&self, graph: &UtilityGraph, p: Vec2, half: f32) -> Option<u32> {
        self.nearest(graph, p, half).map(|(_, c)| c)
    }

    /// Every component with a link within the reach of `p`, ascending.
    pub(crate) fn comps_at(&self, graph: &UtilityGraph, p: Vec2, half: f32) -> Vec<u32> {
        let mut all = BTreeSet::new();
        self.within(graph, p, half, |_, _, c| {
            all.insert(c);
        });
        all.into_iter().collect()
    }

    fn nearest(&self, graph: &UtilityGraph, p: Vec2, half: f32) -> Option<(f32, u32)> {
        let mut best: Option<(f32, UtilityLinkId, u32)> = None;
        self.within(graph, p, half, |d, id, c| {
            if best.is_none_or(|(bd, _, _)| d < bd) {
                best = Some((d, id, c));
            }
        });
        best.map(|(d, _, c)| (d, c))
    }

    /// Calls `f(distance, link, component)` for every link of the network within the reach of
    /// `p`, in ascending link id.
    fn within(
        &self,
        graph: &UtilityGraph,
        p: Vec2,
        half: f32,
        mut f: impl FnMut(f32, UtilityLinkId, u32),
    ) {
        let reach = self.reach + half;
        let (bx, bz) = bucket(p, BUCKET_M);
        let span = (reach / BUCKET_M).ceil() as i32;
        let mut cand = BTreeSet::new();
        for dx in -span..=span {
            for dz in -span..=span {
                if let Some(v) = self.buckets.get(&(bx + dx, bz + dz)) {
                    cand.extend(v.iter().copied());
                }
            }
        }
        for id in cand {
            let (lo, hi) = self.bbox[&id];
            let off = (lo - p).max(p - hi).max(Vec2::ZERO);
            if off.length() > reach {
                continue;
            }
            let Some(link) = graph.link(id) else { continue };
            let (_, d) = link.curve.nearest_xz(p);
            if d <= reach {
                f(d, id, self.comp_of_link[&id]);
            }
        }
    }
}

/// The sites binned by cells in a dense grid over their bounding box (cell
/// contents in ascending site index): the neighbour lookup of the power BFS is an array index.
struct HopGrid {
    /// Cell side: the longest centre distance of two buildings within the power gap.
    cell: f32,
    x0: i32,
    z0: i32,
    w: usize,
    h: usize,
    start: Vec<u32>,
    /// Live items of each cell (a prefix of its range): visited sites drop out on a scan.
    len: Vec<u32>,
    items: Vec<usize>,
}

impl HopGrid {
    fn build(sites: &[Site], gap: f32) -> Self {
        let cell = gap + 2.0 * sites.iter().fold(0.0f32, |m, s| m.max(s.half));
        let cells: Vec<(i32, i32)> = sites.iter().map(|s| bucket(s.pos, cell)).collect();
        let (x0, x1) = cells
            .iter()
            .fold((i32::MAX, i32::MIN), |a, c| (a.0.min(c.0), a.1.max(c.0)));
        let (z0, z1) = cells
            .iter()
            .fold((i32::MAX, i32::MIN), |a, c| (a.0.min(c.1), a.1.max(c.1)));
        if cells.is_empty() {
            return Self {
                cell,
                x0: 0,
                z0: 0,
                w: 0,
                h: 0,
                start: vec![0],
                len: Vec::new(),
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
        let len: Vec<u32> = (0..w * h).map(|c| start[c + 1] - start[c]).collect();
        let mut fill = start.clone();
        let mut items = vec![0usize; cells.len()];
        for (i, &c) in cells.iter().enumerate() {
            let a = at(c);
            items[fill[a] as usize] = i;
            fill[a] += 1;
        }
        Self {
            cell,
            x0,
            z0,
            w,
            h,
            start,
            len,
            items,
        }
    }

    /// The sites of a cell that are not `visited` (ascending index); the visited ones are
    /// dropped from the cell for good.
    fn unvisited(&mut self, bx: i32, bz: i32, visited: &[bool]) -> &[usize] {
        let (x, z) = (bx - self.x0, bz - self.z0);
        if x < 0 || z < 0 || x as usize >= self.w || z as usize >= self.h {
            return &[];
        }
        let c = z as usize * self.w + x as usize;
        let from = self.start[c] as usize;
        let mut keep = from;
        for k in from..from + self.len[c] as usize {
            let j = self.items[k];
            if !visited[j] {
                self.items[keep] = j;
                keep += 1;
            }
        }
        self.len[c] = (keep - from) as u32;
        &self.items[from..keep]
    }
}

/// Component -> the smallest component it is joined with by a source of `wanted` kinds that has
/// links of both within its reach. Components that join nothing are absent.
fn source_joins(
    sites: &[Site],
    graph: &UtilityGraph,
    attach: &Attach,
    near_cache: &mut Vec<Option<CachedNear>>,
    wanted: impl Fn(ServiceKind) -> bool,
) -> BTreeMap<u32, u32> {
    let mut parent: BTreeMap<u32, u32> = BTreeMap::new();
    fn root(parent: &BTreeMap<u32, u32>, mut x: u32) -> u32 {
        while let Some(&p) = parent.get(&x) {
            if p == x {
                break;
            }
            x = p;
        }
        x
    }
    for s in sites {
        let Some((kind, _)) = s.source else { continue };
        if !wanted(kind) {
            continue;
        }
        let i = s.id.index as usize;
        if near_cache.len() <= i {
            near_cache.resize(i + 1, None);
        }
        let hit =
            matches!(&near_cache[i], Some((g, pos, _)) if *g == s.id.generation && *pos == s.pos);
        if !hit {
            near_cache[i] = Some((
                s.id.generation,
                s.pos,
                attach.links_at(graph, s.pos, s.half),
            ));
        }
        let mut near: Vec<u32> = near_cache[i]
            .as_ref()
            .map_or(&[][..], |c| &c.2[..])
            .iter()
            .filter_map(|&l| attach.comp_of(l))
            .collect();
        near.sort_unstable();
        near.dedup();
        for pair in near.windows(2) {
            let (a, b) = (root(&parent, pair[0]), root(&parent, pair[1]));
            if a != b {
                let (lo, hi) = (a.min(b), a.max(b));
                parent.insert(hi, lo);
                parent.entry(lo).or_insert(lo);
            }
        }
    }
    parent.keys().map(|&c| (c, root(&parent, c))).collect()
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
type CachedAttach = (u32, Vec2, Option<UtilityLinkId>, Option<UtilityLinkId>);

/// `(generation, position, links in reach)` of a source building, for one network.
type CachedNear = (u32, Vec2, Vec<UtilityLinkId>);

#[derive(Default)]
pub struct AttachCache {
    graph: u64,
    /// Geometry of every link at the last pass: what changed since says which entries to drop.
    prints: BTreeMap<UtilityLinkId, LinkPrint>,
    /// The reach of the two networks (bits) the entries were made with.
    reach: (u32, u32),
    /// The link lookups of that graph, built when the first building needs one.
    attach: Option<(Attach, Attach)>,
    /// By arena index of the building: `(generation, position, nearest pipe, nearest line)`.
    /// Links, not components: a link keeps its id when an edit renames the components.
    by_building: Vec<Option<CachedAttach>>,
    /// By arena index of a source: the pipes / the lines within its reach.
    pipe_near: Vec<Option<CachedNear>>,
    line_near: Vec<Option<CachedNear>>,
}

/// Consumption per utility and the source `(kind, capacity)` of one building def.
type DefSupply = ([f32; 3], Option<(ServiceKind, f32)>);

/// Hash and XZ box of the geometry of one link (network and curve): all a building's nearest
/// link depends on.
#[derive(Clone, Copy, PartialEq)]
struct LinkPrint {
    hash: u64,
    lo: Vec2,
    hi: Vec2,
}

fn link_prints(graph: &UtilityGraph) -> BTreeMap<UtilityLinkId, LinkPrint> {
    graph
        .links()
        .map(|(id, l)| {
            let mut h = 0xcbf2_9ce4_8422_2325u64;
            let mut mix = |v: u32| {
                for b in v.to_le_bytes() {
                    h ^= u64::from(b);
                    h = h.wrapping_mul(0x0000_0100_0000_01b3);
                }
            };
            mix(l.net as u32);
            let pts = [l.curve.p0, l.curve.p1, l.curve.p2, l.curve.p3];
            for p in pts {
                mix(p.x.to_bits());
                mix(p.y.to_bits());
                mix(p.z.to_bits());
            }
            let c = pts.map(to_xz);
            let lo = c.iter().fold(Vec2::splat(f32::MAX), |a, p| a.min(*p));
            let hi = c.iter().fold(Vec2::splat(f32::MIN), |a, p| a.max(*p));
            (id, LinkPrint { hash: h, lo, hi })
        })
        .collect()
}

/// FNV-1a over every link: network, endpoints and curve.
fn graph_fingerprint(graph: &UtilityGraph, nets: &Networks) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |v: u32| {
        for b in v.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    mix(nets.line_reach_m.to_bits());
    mix(nets.pipe_reach_m.to_bits());
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

/// Forgets the entries whose position is within `margin` of a box.
fn drop_near<T>(
    entries: &mut [Option<T>],
    pos: impl Fn(&T) -> Vec2,
    boxes: &[(Vec2, Vec2)],
    margin: f32,
) {
    for e in entries.iter_mut() {
        let Some(t) = e else { continue };
        let p = pos(t);
        if boxes
            .iter()
            .any(|&(lo, hi)| (lo - p).max(p - hi).max(Vec2::ZERO).length() <= margin)
        {
            *e = None;
        }
    }
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
    compute_supply_with(db, buildings, graph, cache, None)
}

/// [`compute_supply_cached`] with the policies in force: PowerSaving (T-192) cuts the power
/// a building needs by its `value` where it is on.
pub fn compute_supply_with(
    db: &ContentDb,
    buildings: &BuildingGrid,
    graph: &UtilityGraph,
    cache: &mut AttachCache,
    policies: Option<&Policies>,
) -> Vec<u8> {
    compute_supply_pools(db, buildings, graph, cache, policies).0
}

/// One power pool (BUG-087): the plants' output and what its buildings need.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PowerPool {
    pub capacity: f32,
    pub demand: f32,
}

/// The power pools of the last supply pass (BUG-087), so a building without power can say why:
/// derived like the flags (rewritten every [`SUPPLY_PERIOD`] ticks, never saved; empty until
/// the first pass after a load).
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct PowerPools {
    /// By building arena index: its generation and the index of its pool in `pools`.
    pub pool_of: Vec<Option<(u32, u32)>>,
    pub pools: Vec<PowerPool>,
}

impl PowerPools {
    /// The pool of building `id`, if the last pass saw it.
    pub fn of(&self, id: BuildingId) -> Option<PowerPool> {
        match self.pool_of.get(id.index as usize) {
            Some(Some((generation, pool))) if *generation == id.generation => {
                self.pools.get(*pool as usize).copied()
            }
            _ => None,
        }
    }
}

/// [`compute_supply_with`] and the power pools it found.
pub fn compute_supply_pools(
    db: &ContentDb,
    buildings: &BuildingGrid,
    graph: &UtilityGraph,
    cache: &mut AttachCache,
    policies: Option<&Policies>,
) -> (Vec<u8>, PowerPools) {
    let nets = &db.balance().networks;
    let power_gap = nets.power_gap_m;
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
            // A placeholder (a def the save named and the content lacks) needs and gives nothing.
            let (mut needs, source) = defs.get(b.def.index()).copied().unwrap_or(([0.0; 3], None));
            if let Some(p) = policies {
                needs[kind_index(UtilityKind::Power)] *=
                    p.factor_at(db, b.pos, PolicyKind::PowerSaving);
            }
            let (w, d) = b.size();
            Site {
                id,
                pos: b.pos,
                half: f32::from(w.max(d)) * cp_building::FOOTPRINT_CELL_M * 0.5,
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
    let slots = missing.len();
    let mut flag = |id: BuildingId, bit: u8| {
        missing[id.index as usize] |= bit;
    };

    // The component of each building in the pipe and in the power network. What a building
    // remembers is its nearest link, which an edit changes only for the buildings within reach
    // of the links it added, removed or moved: the others keep theirs, and the components are
    // read off the fresh lookup (T-259: every edit used to search all of them again).
    let fingerprint = graph_fingerprint(graph, nets);
    if cache.graph != fingerprint {
        cache.graph = fingerprint;
        cache.attach = None;
        let reach = (nets.pipe_reach_m.to_bits(), nets.line_reach_m.to_bits());
        let prints = link_prints(graph);
        let mut boxes: Vec<(Vec2, Vec2)> = Vec::new();
        for (id, new) in &prints {
            if cache.prints.get(id) != Some(new) {
                boxes.push((new.lo, new.hi));
            }
        }
        for (id, old) in &cache.prints {
            if prints.get(id) != Some(old) {
                boxes.push((old.lo, old.hi));
            }
        }
        if reach != cache.reach || boxes.len() > 256 {
            cache.by_building.clear();
            cache.pipe_near.clear();
            cache.line_near.clear();
        } else {
            let half = sites.iter().fold(0.0f32, |m, s| m.max(s.half));
            let margin = nets.pipe_reach_m.max(nets.line_reach_m) + half + 1.0;
            drop_near(&mut cache.by_building, |e| e.1, &boxes, margin);
            drop_near(&mut cache.pipe_near, |e| e.1, &boxes, margin);
            drop_near(&mut cache.line_near, |e| e.1, &boxes, margin);
        }
        cache.reach = reach;
        cache.prints = prints;
    }
    let (pipes, lines) = cache.attach.get_or_insert_with(|| {
        (
            Attach::build(graph, UtilityNet::Pipe, nets.pipe_reach_m),
            Attach::build(graph, UtilityNet::Power, nets.line_reach_m),
        )
    });
    let mut comps: Vec<(Option<u32>, Option<u32>)> = Vec::with_capacity(sites.len());
    for s in &sites {
        let i = s.id.index as usize;
        let live = |l: Option<UtilityLinkId>, a: &Attach| l.is_none_or(|l| a.comp_of(l).is_some());
        let hit = match cache.by_building.get(i) {
            Some(Some((g, pos, pipe, line)))
                if *g == s.id.generation
                    && *pos == s.pos
                    && live(*pipe, pipes)
                    && live(*line, lines) =>
            {
                Some((*pipe, *line))
            }
            _ => None,
        };
        let (pipe, line) = hit.unwrap_or_else(|| {
            let pair = (
                pipes.link_at(graph, s.pos, s.half),
                lines.link_at(graph, s.pos, s.half),
            );
            if cache.by_building.len() <= i {
                cache.by_building.resize(i + 1, None);
            }
            cache.by_building[i] = Some((s.id.generation, s.pos, pair.0, pair.1));
            pair
        });
        comps.push((
            pipe.and_then(|l| pipes.comp_of(l)),
            line.and_then(|l| lines.comp_of(l)),
        ));
    }

    // A source stands on every network of its net within reach, and so joins them (BUG-042: a
    // tower beside a spur and a street pipe that are not joined by a node served only the
    // nearer one).
    if sites.iter().any(|s| s.source.is_some()) {
        let pipe_join = source_joins(&sites, graph, pipes, &mut cache.pipe_near, |k| {
            matches!(k, ServiceKind::Water | ServiceKind::Sewage)
        });
        let line_join = source_joins(&sites, graph, lines, &mut cache.line_near, |k| {
            k == ServiceKind::Power
        });
        for c in &mut comps {
            c.0 = c.0.map(|x| pipe_join.get(&x).copied().unwrap_or(x));
            c.1 = c.1.map(|x| line_join.get(&x).copied().unwrap_or(x));
        }
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
    // Pools (BUG-087): the buildings joined by the power gap (and by the lines they stand on)
    // are one pool with the output of every plant in it. A plant without a line used to be a
    // pool of its own whose search claimed the buildings it could not serve, so the town got
    // the output of one plant only.
    let attached: Vec<Option<u32>> = comps.iter().map(|c| c.1).collect();
    // The sites on each line component, ascending.
    let mut on_line: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, c) in attached.iter().enumerate() {
        if let Some(c) = c {
            on_line.entry(*c).or_default().push(i);
        }
    }
    let gap_ok = |i: usize, j: usize| {
        sites[j].pos.distance(sites[i].pos) - sites[i].half - sites[j].half <= power_gap
    };
    // Visits the neighbours of `i` not yet `seen`: within the power gap, and on its line.
    let neighbours = |hop: &mut HopGrid, i: usize, seen: &mut [bool], out: &mut Vec<usize>| {
        let (bx, bz) = bucket(sites[i].pos, hop.cell);
        for dx in -1..=1 {
            for dz in -1..=1 {
                for &j in hop.unvisited(bx + dx, bz + dz, seen) {
                    // Cheap reject (a few ulps of slack) before the exact test.
                    let reach = power_gap + sites[i].half + sites[j].half;
                    if sites[j].pos.distance_squared(sites[i].pos) > reach * reach * 1.001 {
                        continue;
                    }
                    if gap_ok(i, j) {
                        out.push(j);
                    }
                }
            }
        }
        // Marked as they are found: a building beside `i` that is also on its line comes once.
        for &j in out.iter() {
            seen[j] = true;
        }
        if let Some(line) = attached[i].and_then(|c| on_line.get(&c)) {
            for &j in line {
                if !seen[j] {
                    seen[j] = true;
                    out.push(j);
                }
            }
        }
    };
    // 1. The groups: connected over the gap and the lines, numbered by their smallest site.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    {
        let mut hop = HopGrid::build(&sites, power_gap);
        let mut seen = vec![false; sites.len()];
        let mut found = Vec::new();
        for start in 0..sites.len() {
            if seen[start] {
                continue;
            }
            seen[start] = true;
            let mut members = vec![start];
            let mut k = 0;
            while k < members.len() {
                found.clear();
                neighbours(&mut hop, members[k], &mut seen, &mut found);
                members.extend_from_slice(&found);
                k += 1;
            }
            groups.push(members);
        }
    }
    // 2. Each group serves from all its plants at once, layer by layer outwards (ascending
    // site within a layer): the first building that does not fit and every later one go dark.
    let mut hop = HopGrid::build(&sites, power_gap);
    let mut seen = vec![false; sites.len()];
    let mut served = vec![false; sites.len()];
    let mut found = Vec::new();
    let mut pools = PowerPools {
        pool_of: vec![None; slots],
        pools: Vec::with_capacity(groups.len()),
    };
    for members in &groups {
        let pool = pools.pools.len() as u32;
        let mut report = PowerPool::default();
        for &i in members {
            pools.pool_of[sites[i].id.index as usize] = Some((sites[i].id.generation, pool));
            report.demand += sites[i].needs[2];
            if let Some((ServiceKind::Power, c)) = sites[i].source {
                report.capacity += c;
            }
        }
        pools.pools.push(report);
        let mut left = 0.0f32;
        let mut powered_lines: BTreeSet<u32> = BTreeSet::new();
        for &i in members {
            if let Some((ServiceKind::Power, c)) = sites[i].source {
                left += c;
                powered_lines.extend(attached[i]);
            }
        }
        if left <= 0.0 {
            // No plant: only the buildings that need nothing are fine.
            for &i in members {
                served[i] = sites[i].needs[2] <= 0.0;
            }
            continue;
        }
        let mut layer: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&i| {
                matches!(sites[i].source, Some((ServiceKind::Power, _)))
                    || attached[i].is_some_and(|c| powered_lines.contains(&c))
            })
            .collect();
        for &i in &layer {
            seen[i] = true;
        }
        let mut exhausted = false;
        while !layer.is_empty() {
            layer.sort_unstable();
            let mut next = Vec::new();
            for &i in &layer {
                let need = sites[i].needs[2];
                if need <= 0.0 || (!exhausted && left >= need) {
                    left -= need;
                    served[i] = true;
                } else {
                    exhausted = true;
                }
                found.clear();
                neighbours(&mut hop, i, &mut seen, &mut found);
                next.extend_from_slice(&found);
            }
            layer = next;
        }
        debug_assert!(members.iter().all(|&i| seen[i]), "a group is connected");
    }
    for (i, s) in sites.iter().enumerate() {
        if s.needs[2] > 0.0 && !served[i] {
            flag(s.id, BuildingFlags::NO_POWER);
        }
    }
    (missing, pools)
}

/// `Utilities`-phase system: every [`SUPPLY_PERIOD`] ticks, rewrites the utility bits of the
/// buildings' flags.
pub fn update_supply(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    graph: Res<UtilityGraph>,
    mut buildings: ResMut<BuildingGrid>,
    mut cache: Local<AttachCache>,
    mut pools: ResMut<PowerPools>,
    policies: PolicyParams,
) {
    if !tick.0.is_multiple_of(SUPPLY_PERIOD) {
        return;
    }
    let Some(content) = content else { return };
    let policies = policies.get();
    let (missing, found) = compute_supply_pools(
        &content.0,
        &buildings,
        &graph,
        &mut cache,
        policies.as_ref(),
    );
    if *pools != found {
        *pools = found;
    }
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

/// Which positions a network reaches (within its reach of one of its links), for the
/// coverage overlay. Built once per graph, then asked per position.
pub struct Coverage {
    attach: Attach,
}

impl Coverage {
    pub fn new(graph: &UtilityGraph, net: UtilityNet, nets: &Networks) -> Self {
        Self {
            attach: Attach::build(graph, net, reach_m(nets, net)),
        }
    }

    /// Whether a building centred on `p` (XZ) with half size `half` is within reach of the
    /// network, the rule buildings use to attach.
    pub fn covers(&self, graph: &UtilityGraph, p: Vec2, half: f32) -> bool {
        self.attach.comp_at(graph, p, half).is_some()
    }

    /// Every component of the network within reach of `p`: the ones a source there joins.
    pub fn components_in_reach(&self, graph: &UtilityGraph, p: Vec2, half: f32) -> Vec<u32> {
        self.attach.comps_at(graph, p, half)
    }

    /// The connected component of the network a building at `p` attaches to, if in reach.
    pub fn component(&self, graph: &UtilityGraph, p: Vec2, half: f32) -> Option<u32> {
        self.attach.comp_at(graph, p, half)
    }
}
