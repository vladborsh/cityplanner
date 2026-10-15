//! Changing a building's level (`architecture/system-design.md` §8): the building is rebuilt
//! as another definition of the same zone one level up or down, **in the same lot**: same
//! block, same first column, same road edge. A larger definition grows the lot into free,
//! valid cells of the same painted zone beside it (further along the road and deeper); a
//! smaller one gives the surplus cells back. State, flags, `ready_at` and the people living
//! and working there stay as they are.

use cp_content::{BuildingDefId, ContentDb};
use cp_net::NetGraph;
use cp_zone::{BLOCK_DEPTH, ZoneCellRef, ZoneGrid};

use crate::grid::BuildingGrid;
use crate::model::{Building, BuildingId, Lot};
use crate::spawn::place;

/// The definition one level `up` (or down) from `b` and the lot it takes, if there is one.
/// Up needs a definition at least as wide and as deep as the current lot whose extra cells
/// are free; the one needing the fewest extra cells wins, ties go to the lowest id. Down needs
/// a definition that fits inside the lot; the largest wins.
pub fn rebuild_candidate(
    db: &ContentDb,
    zones: &ZoneGrid,
    b: &Building,
    up: bool,
) -> Option<(BuildingDefId, Lot)> {
    let zone = db.building(b.def).zone;
    let target = if up {
        b.level.checked_add(1)?
    } else {
        b.level.checked_sub(1).filter(|l| *l >= 1)?
    };
    let cur = *b.lot()?;
    let block = zones.block(cur.block)?;
    let area = |w: u8, d: u8| u32::from(w) * u32::from(d);
    let mut best: Option<(u32, BuildingDefId, Lot)> = None;
    for (id, def) in db.buildings() {
        if def.zone != zone || def.level != target || def.service.is_some() {
            continue;
        }
        let (w, d) = def.lot;
        let lot = Lot { w, d, ..cur };
        let fits = if up {
            w >= cur.w
                && d >= cur.d
                && lot.col + u16::from(w) <= block.cols
                && u16::from(d) <= BLOCK_DEPTH
                && lot
                    .cells()
                    .filter(|c| !cur.cells().any(|o| o == *c))
                    .all(|c| {
                        zones.is_valid(c) && !zones.is_occupied(c) && zones.zone_of(c) == Some(zone)
                    })
        } else {
            w <= cur.w && d <= cur.d
        };
        if !fits {
            continue;
        }
        // Up: fewest extra cells first; down: most cells kept first.
        let key = if up {
            area(w, d)
        } else {
            u32::MAX - area(w, d)
        };
        if best.is_none_or(|(k, _, _)| key < k) {
            best = Some((key, id, lot));
        }
    }
    best.map(|(_, id, lot)| (id, lot))
}

/// Rebuilds building `id` as `def` in `lot` (from [`rebuild_candidate`]): occupies the cells
/// it grows into, releases the ones it gives up, moves its position and access point to the
/// new lot and resets `level_progress`. Returns false (nothing changed) if the building is
/// gone.
pub fn rebuild(
    buildings: &mut BuildingGrid,
    zones: &mut ZoneGrid,
    net: &NetGraph,
    db: &ContentDb,
    id: BuildingId,
    def: BuildingDefId,
    lot: Lot,
) -> bool {
    let Some(old) = buildings.get(id).and_then(|b| b.lot().copied()) else {
        return false;
    };
    let old_cells: Vec<ZoneCellRef> = old.cells().collect();
    let new_cells: Vec<ZoneCellRef> = lot.cells().collect();
    let grow: Vec<ZoneCellRef> = new_cells
        .iter()
        .copied()
        .filter(|c| !old_cells.contains(c))
        .collect();
    let shrink: Vec<ZoneCellRef> = old_cells
        .iter()
        .copied()
        .filter(|c| !new_cells.contains(c))
        .collect();
    if !grow.is_empty() {
        zones.occupy(&grow);
    }
    if !shrink.is_empty() {
        zones.release(&shrink);
    }
    let placed = place(zones, net, lot.block, &lot);
    let b = buildings.get_mut(id).expect("checked above");
    b.def = def;
    b.level = db.building(def).level;
    b.set_lot(lot);
    b.level_progress = 0.0;
    if let Some((pos, rot, access)) = placed {
        b.pos = pos;
        b.rot = rot;
        b.set_access(Some(access));
    }
    true
}
