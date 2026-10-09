//! Lot choice and spawning (`architecture/system-design.md` §8).
//!
//! Every tick the zone blocks of one bucket (`block index % 32 == tick % 32`, §2) get one
//! spawn attempt each, in block order, with an RNG forked from the sim RNG for that tick
//! (`SimRng::fork`), so the result depends only on the state. An attempt:
//!
//! 1. is skipped with probability 3/4 (buildings appear gradually);
//! 2. picks a random column; the row-0 cell there must be valid, painted and have demand;
//! 3. collects the level-1 defs of that zone (service buildings excluded) whose lot — `lot.0`
//!    columns along the road by `lot.1` rows deep, starting at the chosen column and the road
//!    edge — fits: inside the block, every cell valid and painted with that zone;
//! 4. picks one of them at random and builds it: `UnderConstruction`, ready one game day later.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use cp_content::{BuildingDefId, ContentDb, SimContent, ZoneType};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{SimLimits, SimRng, Tick};
use cp_net::NetGraph;
use cp_zone::{BLOCK_DEPTH, CELL_SIZE, ZoneBlock, ZoneBlockId, ZoneCellRef, ZoneGrid};

use crate::grid::BuildingGrid;
use crate::model::{AccessPoint, Building, BuildingFlags, BuildingState, Lot};

/// Zone blocks are visited in 32 buckets (§2).
pub const BLOCK_BUCKETS: u64 = 32;
/// ChaCha stream of the spawn RNG.
pub const SPAWN_STREAM: u64 = 0xB01D;
/// An attempt proceeds with probability `1 / SPAWN_ONE_IN`.
pub const SPAWN_ONE_IN: u32 = 4;

/// TEMPORARY demand per zone type until the demand task lands: a value `> 0` allows
/// spawning. Default: everything allowed. Not saved and not checksummed.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct DemandStub {
    pub res_low: f32,
    pub res_high: f32,
    pub com_low: f32,
    pub com_high: f32,
    pub industrial: f32,
    pub office: f32,
}

impl Default for DemandStub {
    fn default() -> Self {
        Self {
            res_low: 1.0,
            res_high: 1.0,
            com_low: 1.0,
            com_high: 1.0,
            industrial: 1.0,
            office: 1.0,
        }
    }
}

impl DemandStub {
    pub fn get(&self, zone: ZoneType) -> f32 {
        match zone {
            ZoneType::None => 0.0,
            ZoneType::ResLow => self.res_low,
            ZoneType::ResHigh => self.res_high,
            ZoneType::ComLow => self.com_low,
            ZoneType::ComHigh => self.com_high,
            ZoneType::Industrial => self.industrial,
            ZoneType::Office => self.office,
        }
    }
}

/// Whether a `w`×`d` lot starting at `col` fits the block with every cell valid and
/// painted `zone`.
fn lot_fits(block: &ZoneBlock, col: u16, w: u8, d: u8, zone: ZoneType) -> bool {
    if col + u16::from(w) > block.cols || u16::from(d) > BLOCK_DEPTH {
        return false;
    }
    (col..col + u16::from(w)).all(|c| {
        (0..u16::from(d)).all(|r| {
            let i = usize::from(c * BLOCK_DEPTH + r);
            block.valid[i] && block.zones[i] == zone
        })
    })
}

/// Level-1 spawnable defs of `zone`, in id order.
fn spawnable(db: &ContentDb, zone: ZoneType) -> Vec<BuildingDefId> {
    db.buildings()
        .filter(|(_, d)| d.zone == zone && d.level == 1 && d.service.is_none())
        .map(|(id, _)| id)
        .collect()
}

/// World position, yaw and access point of a lot.
fn place(
    grid: &ZoneGrid,
    net: &NetGraph,
    block_id: ZoneBlockId,
    lot: &Lot,
) -> Option<(Vec2, f32, AccessPoint)> {
    let block = grid.block(block_id)?;
    let (mut sum, mut n) = (Vec2::ZERO, 0.0f32);
    for cell in lot.cells() {
        sum += grid.cell_footprint(cell)?.c;
        n += 1.0;
    }
    let pos = sum / n;
    let first = grid.cell_footprint(ZoneCellRef {
        block: block_id,
        cell: lot.col * BLOCK_DEPTH,
    })?;
    let rot = first.u.y.atan2(first.u.x);
    // Front of the lot: the middle of its row-0 edge on the road side.
    let mid_col = lot.col + u16::from(lot.w) / 2;
    let mid = grid.cell_footprint(ZoneCellRef {
        block: block_id,
        cell: mid_col * BLOCK_DEPTH,
    })?;
    let front = mid.c - mid.v * (CELL_SIZE / 2.0);
    let seg = net.segment(block.segment)?;
    let (t, _) = seg.curve.nearest_xz(front);
    Some((
        pos,
        rot,
        AccessPoint {
            segment: block.segment,
            t,
        },
    ))
}

/// `Buildings`-phase system: finishes construction and spawns new buildings.
#[allow(clippy::too_many_arguments)]
pub fn update_buildings(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    rng: Res<SimRng>,
    limits: Res<SimLimits>,
    demand: Res<DemandStub>,
    mut zones: ResMut<ZoneGrid>,
    mut buildings: ResMut<BuildingGrid>,
) {
    buildings.finish_construction(tick.0);
    let Some(content) = content else { return };
    if !zones.index_ready() {
        return;
    }
    let db = &content.0;
    let bucket = tick.0 % BLOCK_BUCKETS;
    let ids: Vec<ZoneBlockId> = zones
        .blocks()
        .map(|(id, _)| id)
        .filter(|id| u64::from(id.index) % BLOCK_BUCKETS == bucket)
        .collect();
    if ids.is_empty() {
        return;
    }
    let mut rng = rng.fork(SPAWN_STREAM, *tick);
    for id in ids {
        // Draw first so the stream consumed per block does not depend on the outcome.
        let gate = rng.below(SPAWN_ONE_IN);
        let pick = rng.next_u32();
        let choose = rng.next_u32();
        if gate != 0 || buildings.len() >= limits.buildings as usize {
            continue;
        }
        let Some(block) = zones.block(id) else {
            continue;
        };
        if block.cols == 0 {
            continue;
        }
        let col = (pick % u32::from(block.cols)) as u16;
        let i = usize::from(col * BLOCK_DEPTH);
        let zone = block.zones[i];
        if zone == ZoneType::None || !block.valid[i] || demand.get(zone) <= 0.0 {
            continue;
        }
        let fitting: Vec<BuildingDefId> = spawnable(db, zone)
            .into_iter()
            .filter(|&d| {
                let def = db.building(d);
                lot_fits(block, col, def.lot.0, def.lot.1, zone)
            })
            .collect();
        if fitting.is_empty() {
            continue;
        }
        let def_id = fitting[choose as usize % fitting.len()];
        let def = db.building(def_id);
        let lot = Lot {
            block: id,
            col,
            w: def.lot.0,
            d: def.lot.1,
        };
        let Some((pos, rot, access)) = place(&zones, &net, id, &lot) else {
            continue;
        };
        let cells: Vec<ZoneCellRef> = lot.cells().collect();
        buildings.insert(Building {
            def: def_id,
            lot,
            level: def.level,
            state: BuildingState::UnderConstruction,
            ready_at: Tick(tick.0 + TICKS_PER_DAY),
            pos,
            rot,
            access,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
        zones.occupy(&cells);
    }
}
