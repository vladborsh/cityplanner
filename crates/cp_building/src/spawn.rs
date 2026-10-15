//! Lot choice and spawning (`architecture/system-design.md` §8).
//!
//! Every tick the zone blocks of one bucket (`block index % 32 == tick % 32`, §2) get one
//! spawn attempt each, in block order, with an RNG forked from the sim RNG for that tick
//! (`SimRng::fork`), so the result depends only on the state. An attempt:
//!
//! 1. is skipped with probability `1 - 1/spawn_one_in` (`balance.ron`; buildings appear gradually);
//! 2. picks a random column; the row-0 cell there must be valid, painted and have demand;
//! 3. collects the level-1 defs of that zone (service buildings excluded) whose lot — `lot.0`
//!    columns along the road by `lot.1` rows deep, starting at the chosen column and the road
//!    edge — fits: inside the block, every cell valid and painted with that zone;
//! 4. picks one of them at random and builds it: `UnderConstruction`, ready `build_hours` of game time later (`BuildingDef::build_hours`).

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use cp_content::{BuildingDefId, ContentDb, SimContent, ZoneType};
use cp_core::{SimLimits, SimRng, Tick};
use cp_net::NetGraph;
use cp_zone::{BLOCK_DEPTH, CELL_SIZE, ZoneBlock, ZoneBlockId, ZoneCellRef, ZoneGrid};
use serde::{Deserialize, Serialize};

use crate::grid::{BuildingGrid, cells_under_lot};
use crate::model::build_ticks;
use crate::model::{AccessPoint, Building, BuildingFlags, BuildingState, Lot};

/// Zone blocks are visited in 32 buckets (§2).
pub const BLOCK_BUCKETS: u64 = 32;
/// ChaCha stream of the spawn RNG.
pub const SPAWN_STREAM: u64 = 0xB01D;

/// Smoothed demand per zone family, the economy's output and the gate of spawning and move-in
/// (`architecture/system-design.md` §11): a value `> 0` allows it. `cp_economy` recomputes it
/// every [`DEMAND_PERIOD`] ticks; it is serialized sim state.
///
/// The default is the state of a new city (the `Balance` defaults of `demand_base`): housing,
/// shops and industry are wanted, offices are not until there are educated jobless.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Demand {
    /// Residential (`ResLow`, `ResHigh`): wanted new homes. Vacant homes count against it.
    pub r: f32,
    /// Commercial (`ComLow`, `ComHigh`).
    pub c: f32,
    /// Industrial.
    pub i: f32,
    /// Office.
    pub o: f32,
    /// Households move into the homes that exist while this is `> 0`: like `r` without the
    /// vacant-homes term (vacant homes are what moving in fills; counting them would stall it).
    pub move_in: f32,
    /// Scenario/test hook: while set, the economy leaves the values alone.
    pub frozen: bool,
}

/// Ticks between two recomputations of [`Demand`] (the 64 buckets of §11, one per tick).
pub const DEMAND_PERIOD: u64 = 64;

impl Default for Demand {
    fn default() -> Self {
        Self {
            r: 10.0,
            c: 5.0,
            i: 5.0,
            o: 0.0,
            move_in: 10.0,
            frozen: false,
        }
    }
}

impl Demand {
    /// The same value for every family, frozen: for scenarios and tests.
    pub fn fixed(v: f32) -> Self {
        Self {
            r: v,
            c: v,
            i: v,
            o: v,
            move_in: v,
            frozen: true,
        }
    }

    pub fn get(&self, zone: ZoneType) -> f32 {
        match zone {
            ZoneType::None => 0.0,
            ZoneType::ResLow | ZoneType::ResHigh => self.r,
            ZoneType::ComLow | ZoneType::ComHigh => self.c,
            ZoneType::Industrial => self.i,
            ZoneType::Office => self.o,
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
pub fn place(
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

/// Buildings stay when their road is removed (§8 v1.1): the access point is checked once per
/// bucket (`index % 32 == tick % 32`). A building whose segment is gone gets `NO_ROAD`, unless
/// a road now runs along its lot (rebuilt along the old one): then its access point moves
/// there and the flag clears.
fn revalidate_access(net: &NetGraph, buildings: &mut BuildingGrid, tick: u64) {
    let mut changes: Vec<(crate::BuildingId, Option<AccessPoint>)> = Vec::new();
    for (id, b) in buildings.iter_bucket(BLOCK_BUCKETS as u32, (tick % BLOCK_BUCKETS) as u32) {
        // A free-standing building has no road to lose (§8.1): never NO_ROAD.
        let Some(current) = b.access() else {
            continue;
        };
        let alive = net.segment(current.segment).is_some();
        if alive && !b.flags.has(BuildingFlags::NO_ROAD) {
            continue;
        }
        if alive {
            changes.push((id, Some(current)));
            continue;
        }
        // Nearest road within reach of the lot's front; lowest segment id wins a tie.
        let reach = f32::from(b.size().1) * CELL_SIZE + ACCESS_SLACK;
        let mut best: Option<(f32, AccessPoint)> = None;
        for (sid, seg) in net.segments() {
            let (t, d) = seg.curve.nearest_xz(b.pos);
            if d <= reach && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, AccessPoint { segment: sid, t }));
            }
        }
        changes.push((id, best.map(|(_, a)| a)));
    }
    for (id, access) in changes {
        if let Some(a) = access
            && buildings.get(id).is_some_and(|b| b.access() != Some(a))
        {
            buildings.get_mut(id).expect("building").set_access(Some(a));
        }
        buildings.set_flag(id, BuildingFlags::NO_ROAD, access.is_none());
    }
}

/// Buildings that outlived their zone block (the road was removed; §8 v1.1) take their ground
/// back when a road is rebuilt along it: the cells of the new blocks under the lot are occupied
/// (only the ones not yet occupied, so a settled survivor leaves the zone revision alone), and
/// when they form the lot's rectangle in one block the lot is re-bound to it. Without this the
/// spawner would build on top of survivors (BUG-019).
fn rebind_lots(zones: &mut ZoneGrid, buildings: &mut BuildingGrid) {
    let stale: Vec<(crate::BuildingId, Building)> = buildings
        .iter()
        .filter(|(_, b)| b.lot().is_some_and(|l| zones.block(l.block).is_none()))
        .map(|(id, b)| (id, b.clone()))
        .collect();
    for (id, b) in stale {
        let Some(old_lot) = b.lot().copied() else {
            continue;
        };
        let cells = cells_under_lot(zones, &b);
        let Some(first) = cells.first().copied() else {
            continue;
        };
        let fresh: Vec<ZoneCellRef> = cells
            .iter()
            .copied()
            .filter(|c| !zones.is_occupied(*c))
            .collect();
        if !fresh.is_empty() {
            zones.occupy(&fresh);
        }
        // A whole rectangle in one block, rows starting at the road edge: re-bind.
        let col = cells
            .iter()
            .map(|c| c.cell / BLOCK_DEPTH)
            .min()
            .unwrap_or(0);
        let one_block = cells.iter().all(|c| c.block == first.block);
        let rows_ok = cells
            .iter()
            .all(|c| c.cell % BLOCK_DEPTH < u16::from(old_lot.d));
        if cells.len() == usize::from(old_lot.w) * usize::from(old_lot.d) && one_block && rows_ok {
            let lot = Lot {
                block: first.block,
                col,
                w: old_lot.w,
                d: old_lot.d,
            };
            if let Some(bm) = buildings.get_mut(id) {
                bm.set_lot(lot);
            }
        }
    }
}

/// Slack beyond the lot depth when looking for a road again, metres (half a wide road).
const ACCESS_SLACK: f32 = 12.0;

/// `Buildings`-phase system: finishes construction and spawns new buildings.
#[allow(clippy::too_many_arguments)]
pub fn update_buildings(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    rng: Res<SimRng>,
    limits: Res<SimLimits>,
    demand: Res<Demand>,
    mut zones: ResMut<ZoneGrid>,
    mut buildings: ResMut<BuildingGrid>,
    mut synced: Local<u64>,
) {
    buildings.finish_construction(tick.0);
    if *synced != buildings.revision() {
        *synced = buildings.revision();
        crate::free::sync_blockers(&mut zones, &buildings);
    }
    revalidate_access(&net, &mut buildings, tick.0);
    let Some(content) = content else { return };
    if !zones.index_ready() {
        return;
    }
    rebind_lots(&mut zones, &mut buildings);
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
    let one_in = db.balance().spawn_one_in.max(1);
    let mut rng = rng.fork(SPAWN_STREAM, *tick);
    for id in ids {
        // Draw first so the stream consumed per block does not depend on the outcome.
        let gate = rng.below(one_in);
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
            footprint: crate::model::Footprint::Lot(lot),
            level: def.level,
            state: BuildingState::UnderConstruction,
            ready_at: Tick(tick.0 + build_ticks(def)),
            pos,
            rot,
            access: Some(access),
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
        zones.occupy(&cells);
    }
}

#[cfg(test)]
mod free_standing_tests {
    use super::*;
    use crate::model::{FOOTPRINT_CELL_M, Footprint};
    use bevy_math::Vec2;
    use cp_core::{Id, Tick};

    fn free(w: u8, d: u8, pos: Vec2, rot: f32) -> Building {
        Building {
            def: cp_content::BuildingDefId(0),
            footprint: Footprint::Free { w, d },
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos,
            rot,
            access: None,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        }
    }

    #[test]
    fn the_helpers_of_a_free_standing_building() {
        let b = free(2, 3, Vec2::new(100.0, 50.0), 0.0);
        assert!(b.lot().is_none() && b.access().is_none());
        assert_eq!(b.size(), (2, 3));
        let q = b.footprint_polygon();
        // 16 m by 24 m around the centre, counter-clockwise.
        assert_eq!(q[0], Vec2::new(100.0 - 8.0, 50.0 - 12.0));
        assert_eq!(q[2], Vec2::new(100.0 + 8.0, 50.0 + 12.0));
        assert!((q[0].distance(q[1]) - 2.0 * FOOTPRINT_CELL_M).abs() < 1e-4);
        assert!(b.footprint_contains(Vec2::new(105.0, 55.0)));
        assert!(!b.footprint_contains(Vec2::new(120.0, 50.0)));
        // Turned a quarter: the long side runs along x.
        let t = free(2, 3, Vec2::ZERO, std::f32::consts::FRAC_PI_2);
        assert!(
            t.footprint_contains(Vec2::new(11.0, 0.0))
                && !t.footprint_contains(Vec2::new(0.0, 11.0))
        );
    }

    /// §8.1: a free-standing building never gets NO_ROAD and keeps no cells in the zone grid.
    #[test]
    fn a_free_standing_building_never_loses_its_road_and_holds_no_cells() {
        let mut grid = BuildingGrid::default();
        let id = grid.insert(free(2, 2, Vec2::new(10.0, 10.0), 0.0));
        let net = NetGraph::default();
        for tick in 0..BLOCK_BUCKETS * 2 {
            revalidate_access(&net, &mut grid, tick);
        }
        let b = grid.get(id).unwrap();
        assert!(!b.flags.has(BuildingFlags::NO_ROAD));
        assert_eq!(b.access(), None);
        assert_eq!(grid.occupied_cells().count(), 0);
        let zones = ZoneGrid::default();
        assert!(crate::grid::held_cells(&zones, b).is_empty());
        let _ = Id::<Building>::new(0, 0);
    }
}
