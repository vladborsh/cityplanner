//! The water and power overlays (`architecture/system-design.md` §10, §13.4): the network
//! (pipes are underground, so the water overlay is where they are seen), the band each network
//! reaches, and every building that needs or provides the utility coloured by its state.
//!
//! Pure functions of the sim snapshot, like the other layers; the Bevy side lives in the parent
//! module.

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

/// The overlay layer of `layer`: reach band, then the network, then the buildings.
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
    let mut m = MeshData::default();
    let reach = reach_m(&db.balance().networks, net);
    for (_, l) in graph.links().filter(|(_, l)| l.net == net) {
        ribbon(&mut m, l, reach, LIFT, band, terrain);
    }
    for (_, b) in buildings.iter() {
        if let Some(s) = building_status(layer, db, b) {
            lot_quads(&mut m, b, zones, terrain, status_color(layer, s));
        }
    }
    for (_, l) in graph.links().filter(|(_, l)| l.net == net) {
        ribbon(&mut m, l, LINE_WIDTH_M * 0.5, LIFT + 0.3, line, terrain);
    }
    m
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
