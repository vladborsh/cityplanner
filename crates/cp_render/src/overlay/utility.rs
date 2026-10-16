//! The water and power overlays (`architecture/system-design.md` §10, §13.4): the network
//! (pipes are underground, so the water overlay is where they are seen), the band each network
//! reaches, and every building that needs or provides the utility coloured by its state.
//!
//! Pure functions of the sim snapshot, like the other layers; the Bevy side lives in the parent
//! module.

use std::collections::BTreeMap;

use bevy::math::Vec2;
use cp_building::{Building, BuildingFlags, BuildingGrid, BuildingState};
use cp_content::{ContentDb, ServiceKind, UtilityKind};
use cp_core::curve::to_xz;
use cp_terrain::Terrain;
use cp_utility::{Coverage, UtilityGraph, UtilityNet, reach_m};
use cp_zone::ZoneGrid;

use super::{LIFT, flat};
use crate::chunk::MeshData;

/// Which utility an overlay shows. Water also covers sewage (one pipe network).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UtilityLayer {
    Water,
    Power,
}

impl UtilityLayer {
    pub fn net(self) -> UtilityNet {
        match self {
            Self::Water => UtilityNet::Pipe,
            Self::Power => UtilityNet::Power,
        }
    }
}

/// What a building looks like to a utility overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildingStatus {
    /// A pump / outflow / power plant.
    Source,
    /// Needs the utility and has it.
    Supplied,
    NoWater,
    NoSewage,
    NoPower,
}

/// Pipe centre line.
pub const PIPE: [f32; 4] = [0.15, 0.45, 0.95, 0.95];
/// Power-line centre line.
pub const LINE: [f32; 4] = [0.95, 0.80, 0.10, 0.95];
/// The band of the pipe reach (`balance.ron`) each side of a pipe.
pub const WATER_BAND: [f32; 4] = [0.20, 0.60, 0.90, 0.14];
/// The band of the line reach (`balance.ron`) each side of a power line.
pub const POWER_BAND: [f32; 4] = [0.95, 0.85, 0.20, 0.14];

const LINE_WIDTH_M: f32 = 3.0;
/// Side of one cell of the connected-area fill, metres.
const FILL_CELL_M: f32 = 16.0;
const FILL_ALPHA: f32 = 0.22;
const SAMPLE_M: f32 = 16.0;

pub fn status_color(layer: UtilityLayer, s: BuildingStatus) -> [f32; 4] {
    match (s, layer) {
        (BuildingStatus::Source, UtilityLayer::Water) => [0.25, 0.55, 1.0, 0.9],
        (BuildingStatus::Source, UtilityLayer::Power) => [1.0, 0.60, 0.10, 0.9],
        (BuildingStatus::Supplied, _) => [0.20, 0.80, 0.30, 0.7],
        (BuildingStatus::NoWater | BuildingStatus::NoPower, _) => [0.90, 0.15, 0.10, 0.85],
        (BuildingStatus::NoSewage, _) => [0.55, 0.35, 0.10, 0.85],
    }
}

/// How `b` stands for `layer`: `None` for buildings the overlay does not concern (not
/// `Active`, or neither consuming nor providing this utility). The deficit comes from the flags
/// `cp_utility::update_supply` writes.
pub fn building_status(
    layer: UtilityLayer,
    db: &ContentDb,
    b: &Building,
) -> Option<BuildingStatus> {
    if b.state != BuildingState::Active {
        return None;
    }
    let def = db.building(b.def);
    let source = def
        .service
        .as_deref()
        .and_then(|s| db.service_id(s))
        .map(|s| db.service(s).kind);
    let is_source = match layer {
        UtilityLayer::Water => matches!(source, Some(ServiceKind::Water | ServiceKind::Sewage)),
        UtilityLayer::Power => source == Some(ServiceKind::Power),
    };
    if is_source {
        return Some(BuildingStatus::Source);
    }
    let needs = |k: UtilityKind| def.consumption.get(&k).is_some_and(|v| *v > 0.0);
    match layer {
        UtilityLayer::Water => {
            if !(needs(UtilityKind::Water) || needs(UtilityKind::Sewage)) {
                None
            } else if b.flags.has(BuildingFlags::NO_WATER) {
                Some(BuildingStatus::NoWater)
            } else if b.flags.has(BuildingFlags::NO_SEWAGE) {
                Some(BuildingStatus::NoSewage)
            } else {
                Some(BuildingStatus::Supplied)
            }
        }
        UtilityLayer::Power => {
            if !needs(UtilityKind::Power) {
                None
            } else if b.flags.has(BuildingFlags::NO_POWER) {
                Some(BuildingStatus::NoPower)
            } else {
                Some(BuildingStatus::Supplied)
            }
        }
    }
}

/// A ribbon of `half` metres either side along the link's curve, draped on the terrain
/// `lift` metres above it.
fn ribbon(
    m: &mut MeshData,
    link: &cp_utility::UtilityLink,
    half: f32,
    lift: f32,
    color: [f32; 4],
    terrain: &Terrain,
) {
    let n = ((link.length / SAMPLE_M).ceil() as usize).clamp(1, 64);
    let edge = |t: f32| {
        let p = to_xz(link.curve.point(t));
        let tg = link.curve.tangent(t);
        let side = Vec2::new(tg.z, -tg.x).try_normalize().unwrap_or(Vec2::X) * half;
        let at = |q: Vec2| [q.x, terrain.height(q) + lift, q.y];
        (at(p + side), at(p - side))
    };
    let mut prev = edge(0.0);
    for i in 1..=n {
        let cur = edge(i as f32 / n as f32);
        flat(m, [prev.0, cur.0, cur.1, prev.1], color);
        prev = cur;
    }
}

/// Lot cells of `b` as flat quads.
pub(super) fn lot_quads(
    m: &mut MeshData,
    b: &Building,
    zones: &ZoneGrid,
    terrain: &Terrain,
    c: [f32; 4],
) {
    let Some(lot) = b.lot() else {
        // A free-standing building (a wind turbine, a plant): its whole rectangle.
        let k = b
            .footprint_polygon()
            .map(|q| [q.x, terrain.height(q) + LIFT + 0.15, q.y]);
        flat(m, k, c);
        return;
    };
    for cell in lot.cells() {
        let Some(o) = zones.cell_footprint(cell) else {
            continue;
        };
        let k = o
            .corners()
            .map(|q| [q.x, terrain.height(q) + LIFT + 0.15, q.y]);
        flat(m, k, c);
    }
}

/// Fill colour of a building that stands in a network's reach but gets too little (T-183).
pub const DEFICIT: [f32; 4] = [1.0, 0.55, 0.05, 0.85];

/// Tints of the networks of a layer, by network number: yellow shades for power, blue shades
/// for water, so two unconnected networks look different.
pub fn network_color(layer: UtilityLayer, index: usize, alpha: f32) -> [f32; 4] {
    const POWER: [[f32; 3]; 4] = [
        [0.98, 0.85, 0.15],
        [0.80, 0.95, 0.20],
        [1.00, 0.70, 0.35],
        [0.90, 0.90, 0.60],
    ];
    const WATER: [[f32; 3]; 4] = [
        [0.20, 0.55, 0.95],
        [0.15, 0.85, 0.85],
        [0.45, 0.45, 0.98],
        [0.55, 0.80, 0.95],
    ];
    let t = match layer {
        UtilityLayer::Power => POWER,
        UtilityLayer::Water => WATER,
    }[index % 4];
    [t[0], t[1], t[2], alpha]
}

/// One connected network of a layer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetworkStats {
    /// What the sources of the network give and what its buildings ask for (water: m3 per
    /// week as the content states it; power: its power unit).
    pub capacity: f32,
    pub demand: f32,
    pub buildings: u32,
    /// Buildings of the network the flags say lack the utility.
    pub deficit: u32,
    /// `(building, position)` of its sources.
    pub sources: Vec<(cp_building::BuildingId, Vec2)>,
}

/// The networks of a layer and where each building stands (T-183). Pure function of the
/// snapshot; the legend and the source captions read it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UtilityReport {
    pub networks: Vec<NetworkStats>,
    /// Buildings the overlay concerns that no network reaches (they show red).
    pub unconnected: u32,
    /// Network index of each concerned building (`None`: unconnected).
    pub net_of: BTreeMap<cp_building::BuildingId, Option<usize>>,
    /// Network index of each link component (by component label).
    pub comp_net: BTreeMap<u32, usize>,
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (a, b) = (find(parent, a), find(parent, b));
    // The smaller index stays the root: deterministic.
    parent[a.max(b)] = a.min(b);
}

fn half_of(b: &Building) -> f32 {
    let (w, d) = b.size();
    f32::from(w.max(d)) * cp_building::FOOTPRINT_CELL_M * 0.5
}

/// Which network every building of `layer` belongs to, and the sums per network. Buildings that
/// are supplied or are sources are the nodes of the power chain (a deficit building passes
/// nothing on, as in `cp_utility`); a building the flags call short but that stands in reach of
/// a line, or within the power gap of a node, still belongs to that network (it shows orange).
pub fn utility_report(
    layer: UtilityLayer,
    graph: &UtilityGraph,
    buildings: &BuildingGrid,
    db: &ContentDb,
) -> UtilityReport {
    let nets = &db.balance().networks;
    let cov = Coverage::new(graph, layer.net(), nets);
    struct Item {
        id: cp_building::BuildingId,
        pos: Vec2,
        half: f32,
        status: BuildingStatus,
        comp: Option<u32>,
        /// Every component in reach of a source: it joins them (BUG-042).
        reach: Vec<u32>,
    }
    let mut items: Vec<Item> = Vec::new();
    for (id, b) in buildings.iter() {
        let Some(status) = building_status(layer, db, b) else {
            continue;
        };
        let half = half_of(b);
        items.push(Item {
            id,
            pos: b.pos,
            half,
            status,
            comp: cov.component(graph, b.pos, half),
            reach: if status == BuildingStatus::Source {
                cov.components_in_reach(graph, b.pos, half)
            } else {
                Vec::new()
            },
        });
    }
    // Union-find over the items, then the line components (nodes after the items).
    let mut comp_node: BTreeMap<u32, usize> = BTreeMap::new();
    for it in &items {
        for c in it.comp.iter().chain(&it.reach) {
            let next = items.len() + comp_node.len();
            comp_node.entry(*c).or_insert(next);
        }
    }
    let mut parent: Vec<usize> = (0..items.len() + comp_node.len()).collect();
    for (i, it) in items.iter().enumerate() {
        for c in it.comp.iter().chain(&it.reach) {
            union(&mut parent, i, comp_node[c]);
        }
    }
    if layer == UtilityLayer::Power {
        let gap = nets.power_gap_m;
        let max_half = items.iter().fold(0.0f32, |m, it| m.max(it.half));
        let cell = gap + 2.0 * max_half;
        let key = |p: Vec2| ((p.x / cell).floor() as i32, (p.y / cell).floor() as i32);
        let live =
            |s: BuildingStatus| matches!(s, BuildingStatus::Source | BuildingStatus::Supplied);
        let mut grid: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
        for (i, it) in items.iter().enumerate().filter(|(_, it)| live(it.status)) {
            grid.entry(key(it.pos)).or_default().push(i);
        }
        for (i, it) in items.iter().enumerate() {
            let (cx, cz) = key(it.pos);
            for dx in -1..=1 {
                for dz in -1..=1 {
                    for &j in grid.get(&(cx + dx, cz + dz)).into_iter().flatten() {
                        // Only live buildings pass current on; the item itself may be short.
                        if j != i
                            && (live(it.status) || it.comp.is_none())
                            && it.pos.distance(items[j].pos) - it.half - items[j].half <= gap
                        {
                            union(&mut parent, i, j);
                        }
                    }
                }
            }
        }
    }
    // A building is connected when it shares a root with a source, a supplied building or a
    // line component; a short one alone in its set is unconnected.
    let roots: Vec<usize> = (0..parent.len()).map(|i| find(&mut parent, i)).collect();
    let mut members: BTreeMap<usize, u32> = BTreeMap::new();
    for r in &roots {
        *members.entry(*r).or_default() += 1;
    }
    let mut index_of: BTreeMap<usize, usize> = BTreeMap::new();
    let mut report = UtilityReport::default();
    for (i, it) in items.iter().enumerate() {
        let short = matches!(
            it.status,
            BuildingStatus::NoWater | BuildingStatus::NoPower | BuildingStatus::NoSewage
        );
        let connected = !short || it.comp.is_some() || members[&roots[i]] > 1;
        if !connected {
            report.unconnected += 1;
            report.net_of.insert(it.id, None);
            continue;
        }
        let next = index_of.len();
        let k = *index_of.entry(roots[i]).or_insert_with(|| {
            report.networks.push(NetworkStats::default());
            next
        });
        let n = &mut report.networks[k];
        n.buildings += 1;
        if short {
            n.deficit += 1;
        }
        report.net_of.insert(it.id, Some(k));
        let b = buildings.get(it.id);
        if let Some(b) = b {
            let def = db.building(b.def);
            let (want_service, want_use) = match layer {
                UtilityLayer::Water => (ServiceKind::Water, UtilityKind::Water),
                UtilityLayer::Power => (ServiceKind::Power, UtilityKind::Power),
            };
            n.demand += def.consumption.get(&want_use).copied().unwrap_or(0.0);
            if it.status == BuildingStatus::Source
                && let Some(s) = def.service.as_deref().and_then(|s| db.service_id(s))
                && db.service(s).kind == want_service
            {
                n.capacity += db.service(s).capacity as f32;
                n.sources.push((it.id, it.pos));
            } else if it.status == BuildingStatus::Source {
                // A sewage source: listed for the marker, no water capacity.
                n.sources.push((it.id, it.pos));
            }
        }
    }
    for (c, node) in &comp_node {
        if let Some(k) = index_of.get(&roots[*node]) {
            report.comp_net.insert(*c, *k);
        }
    }
    report
}

/// The overlay layer of `layer`: the fill of every connected area (a tint per network), the
/// reach band along the lines, the lots in their state colour, the network, and a marker
/// (glow and ring) at every source.
pub fn utility_overlay_mesh(
    layer: UtilityLayer,
    graph: &UtilityGraph,
    buildings: &BuildingGrid,
    zones: &ZoneGrid,
    db: &ContentDb,
    terrain: &Terrain,
) -> MeshData {
    let net = layer.net();
    let (line, band) = match layer {
        UtilityLayer::Water => (PIPE, WATER_BAND),
        UtilityLayer::Power => (LINE, POWER_BAND),
    };
    let report = utility_report(layer, graph, buildings, db);
    let nets = &db.balance().networks;
    let mut m = MeshData::default();
    // The fill: cells of FILL_CELL_M under the transmission radius of every connected
    // building, one quad per cell and network (no overlap, so no darker seams).
    let radius = match layer {
        UtilityLayer::Power => nets.power_gap_m * 0.5,
        UtilityLayer::Water => 12.0,
    };
    let mut cells: BTreeMap<(i32, i32), usize> = BTreeMap::new();
    for (id, k) in &report.net_of {
        let (Some(k), Some(b)) = (k, buildings.get(*id)) else {
            continue;
        };
        if matches!(
            building_status(layer, db, b),
            Some(BuildingStatus::NoWater | BuildingStatus::NoPower | BuildingStatus::NoSewage)
        ) {
            continue;
        }
        let r = half_of(b) + radius;
        let (x0, x1) = (
            ((b.pos.x - r) / FILL_CELL_M).floor() as i32,
            ((b.pos.x + r) / FILL_CELL_M).floor() as i32,
        );
        let (z0, z1) = (
            ((b.pos.y - r) / FILL_CELL_M).floor() as i32,
            ((b.pos.y + r) / FILL_CELL_M).floor() as i32,
        );
        for cx in x0..=x1 {
            for cz in z0..=z1 {
                let c = Vec2::new(
                    (cx as f32 + 0.5) * FILL_CELL_M,
                    (cz as f32 + 0.5) * FILL_CELL_M,
                );
                if c.distance(b.pos) <= r {
                    cells.entry((cx, cz)).or_insert(*k);
                }
            }
        }
    }
    for ((cx, cz), k) in &cells {
        let (x, z) = (*cx as f32 * FILL_CELL_M, *cz as f32 * FILL_CELL_M);
        let at = |px: f32, pz: f32| [px, terrain.height(Vec2::new(px, pz)) + LIFT, pz];
        flat(
            &mut m,
            [
                at(x, z),
                at(x + FILL_CELL_M, z),
                at(x + FILL_CELL_M, z + FILL_CELL_M),
                at(x, z + FILL_CELL_M),
            ],
            network_color(layer, *k, FILL_ALPHA),
        );
    }
    let reach = reach_m(nets, net);
    let labels: BTreeMap<_, _> = graph.components(net).labels.iter().copied().collect();
    for (_, l) in graph.links().filter(|(_, l)| l.net == net) {
        let tint = labels
            .get(&l.a)
            .and_then(|c| report.comp_net.get(c))
            .map_or(band, |k| network_color(layer, *k, band[3]));
        ribbon(&mut m, l, reach, LIFT + 0.05, tint, terrain);
    }
    for (id, b) in buildings.iter() {
        let Some(s) = building_status(layer, db, b) else {
            continue;
        };
        let connected = report.net_of.get(&id).is_some_and(Option::is_some);
        let color = match s {
            BuildingStatus::NoWater | BuildingStatus::NoPower | BuildingStatus::NoSewage
                if connected =>
            {
                DEFICIT
            }
            _ => status_color(layer, s),
        };
        lot_quads(&mut m, b, zones, terrain, color);
    }
    for (_, l) in graph.links().filter(|(_, l)| l.net == net) {
        ribbon(&mut m, l, LINE_WIDTH_M * 0.5, LIFT + 0.3, line, terrain);
    }
    // Source markers: a glow disc and a bright ring round every source.
    for (k, n) in report.networks.iter().enumerate() {
        for (id, pos) in &n.sources {
            let half = buildings.get(*id).map_or(8.0, half_of);
            disc(
                &mut m,
                terrain,
                *pos,
                half + 10.0,
                0.0,
                network_color(layer, k, 0.35),
                LIFT + 0.4,
            );
            disc(
                &mut m,
                terrain,
                *pos,
                half + 14.0,
                half + 11.0,
                [1.0, 1.0, 1.0, 0.95],
                LIFT + 0.5,
            );
        }
    }
    m
}

/// A disc (or, with `inner > 0`, a ring) of 24 segments draped `lift` above the terrain.
fn disc(
    m: &mut MeshData,
    terrain: &Terrain,
    c: Vec2,
    outer: f32,
    inner: f32,
    color: [f32; 4],
    lift: f32,
) {
    const N: usize = 24;
    // A disc is a ring with a tiny hole, so no triangle is degenerate.
    let inner = inner.max(0.2);
    let at = |r: f32, i: usize| {
        let a = i as f32 / N as f32 * std::f32::consts::TAU;
        let p = c + Vec2::new(a.cos(), a.sin()) * r;
        [p.x, terrain.height(p) + lift, p.y]
    };
    for i in 0..N {
        // A ring segment, or a triangle of the disc as a degenerate quad.
        flat(
            m,
            [
                at(inner, i),
                at(outer, i),
                at(outer, i + 1),
                at(inner, i + 1),
            ],
            color,
        );
    }
}

/// Buildings of `layer` that stand inside the network's reach, for tests and the debug
/// readout: `(covered, uncovered)` counts of the buildings the overlay concerns.
pub fn coverage_counts(
    layer: UtilityLayer,
    graph: &UtilityGraph,
    buildings: &BuildingGrid,
    db: &ContentDb,
) -> (usize, usize) {
    let cov = Coverage::new(graph, layer.net(), &db.balance().networks);
    let (mut yes, mut no) = (0, 0);
    for (_, b) in buildings.iter() {
        if building_status(layer, db, b).is_none() {
            continue;
        }
        let (w, d) = b.size();
        let half = f32::from(w.max(d)) * cp_building::FOOTPRINT_CELL_M * 0.5;
        if cov.covers(graph, b.pos, half) {
            yes += 1;
        } else {
            no += 1;
        }
    }
    (yes, no)
}
