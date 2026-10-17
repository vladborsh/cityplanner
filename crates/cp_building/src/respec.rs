//! Rebuilding to a district's specialisation (T-196, `architecture/system-design.md` §16 M4 B).
//!
//! Once a game day (at its first tick) every `Active` zoned building is compared with the
//! specialisation of its district for the zone type it stands in: a building whose def's
//! `spec` differs is **demolished**, at most `respec_per_day` per district (the others wait for
//! the next day), picked with an RNG forked for the day. The ordinary spawner then builds the
//! right kind on the free cells, so a respec is a demolition and a normal new spawn; the
//! residents and workers leave as they do for any demolition.

use std::collections::BTreeMap;

use cp_content::ContentDb;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{SimRng, Tick};
use cp_district::{DistrictId, DistrictMap, Districts, district_at, spec_at};
use cp_zone::ZoneGrid;

use crate::grid::{BuildingGrid, demolish_building};
use crate::model::{BuildingId, BuildingState};

/// ChaCha stream of the respec picks.
pub const SPEC_STREAM: u64 = 0x5BEC;

/// Demolishes the buildings that no longer match their district's specialisation; see the
/// module docs. Does nothing except at the first tick of a day after tick 0.
pub fn respec(
    db: &ContentDb,
    tick: u64,
    rng: &SimRng,
    zones: &mut ZoneGrid,
    buildings: &mut BuildingGrid,
    map: &DistrictMap,
    districts: &Districts,
) {
    if tick == 0 || !tick.is_multiple_of(TICKS_PER_DAY) {
        return;
    }
    // Mismatched buildings by district (`None`: outside every district), in id order.
    let mut wrong: BTreeMap<Option<DistrictId>, Vec<BuildingId>> = BTreeMap::new();
    for (id, b) in buildings.iter() {
        if b.state != BuildingState::Active {
            continue;
        }
        let def = db.building(b.def);
        let Some(lot) = b.lot() else { continue };
        if def.service.is_some() {
            continue;
        }
        let Some(zone) = lot.cells().next().and_then(|c| zones.zone_of(c)) else {
            continue;
        };
        if spec_at(map, districts, b.pos, zone) != def.spec {
            wrong.entry(district_at(map, b.pos)).or_default().push(id);
        }
    }
    let limit = db.balance().respec_per_day as usize;
    let mut rng = rng.fork(SPEC_STREAM, Tick(tick));
    for (_, mut ids) in wrong {
        // A partial shuffle: the first `limit` of the district go.
        let n = ids.len().min(limit);
        for i in 0..n {
            let j = i + rng.below((ids.len() - i) as u32) as usize;
            ids.swap(i, j);
        }
        for &id in &ids[..n] {
            demolish_building(buildings, zones, id);
        }
    }
}
