//! `town:N` (T-281): a small town as a player builds one, to time the client where the user
//! felt it stutter (~700 residents). Streets through the road tool's own code
//! ([`grid_shots::Hand`]) on the whole-cell lattice: 4 x 3 blocks of 18 x 8 cells between 2-cell
//! streets, the open frontage round it unzoned (its cell outlines show); the two northern rows
//! of blocks low-density homes, the southern row shops and, at its east end, industry. It grows
//! with the demand held full until `N` residents live in it (or a time limit), then it gets its
//! water, sewage and power ([`scene::supply_town`]) and the demand and decay go back to the
//! game's own. The citizens drive their own cars: no demo traffic.

use bevy::prelude::*;
use cp_sim::SimRunner;

use crate::grid_shots::Hand;
use crate::scene;

/// Parses `town:N`, `N` residents (50..=20000).
pub fn parse_town(spec: &str) -> Option<u32> {
    spec.strip_prefix("town:")?
        .parse()
        .ok()
        .filter(|n| (50..=20_000).contains(n))
}

/// Blocks across and down.
const COLS: usize = 4;
const ROWS: usize = 3;
/// Between street centres: 18 + 2 and 8 + 2 cells.
const DX: f32 = 20.0 * cp_net::build::ZONE_CELL;
const DY: f32 = 10.0 * cp_net::build::ZONE_CELL;
/// Ticks the town may take to grow (about 20 game days).
const MAX_TICKS: u64 = 20 * cp_core::time::TICKS_PER_DAY;

/// The town's first junction (north-west), world.
pub fn origin() -> Vec2 {
    let c = cp_terrain::MAP_SIZE / 2.0;
    Vec2::new(c - COLS as f32 * DX / 2.0, c - ROWS as f32 * DY / 2.0)
}

/// The cameras of the scene, `(position, look_at)`: `town` obliquely from the south over the
/// whole town and the open land round it, as a player looks at it; `town-close` low over a
/// street of homes.
pub fn camera(name: &str) -> Option<(Vec3, Vec3)> {
    let f = origin() + Vec2::new(COLS as f32 * DX, ROWS as f32 * DY) / 2.0;
    match name {
        "town" => Some((Vec3::new(f.x, 380.0, f.y + 420.0), Vec3::new(f.x, 0.0, f.y))),
        "town-close" => {
            let c = origin() + Vec2::new(DX * 1.5, DY);
            Some((Vec3::new(c.x, 70.0, c.y + 110.0), Vec3::new(c.x, 0.0, c.y)))
        }
        _ => None,
    }
}

/// Builds the town (see the module docs). Returns the residents it reached and the commands
/// the sim refused.
pub fn build_town(sim: &mut SimRunner, residents: u32) -> (u32, Vec<String>) {
    let mut rejected = lay_out_town(sim);
    let o = origin();
    let ys: Vec<f32> = (0..=ROWS).map(|j| j as f32 * DY).collect();
    let x1 = COLS as f32 * DX;
    let decay = sim.world().resource::<cp_economy::Decay>().clone();
    let demand = *sim.world().resource::<cp_building::Demand>();
    *sim.world_mut().resource_mut::<cp_economy::Decay>() = cp_economy::Decay::disabled();
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(1.0);
    let population = |sim: &SimRunner| {
        sim.world()
            .get_resource::<cp_sim::stats::CityStats>()
            .map_or(0, |c| c.population)
    };
    let start = sim.tick().0;
    while population(sim) < residents && sim.tick().0 - start < MAX_TICKS {
        sim.tick_once();
    }
    let rows: Vec<f32> = ys.iter().map(|y| o.y + y).collect();
    scene::supply_town(sim, o.x, o.x + x1, &rows);
    *sim.world_mut().resource_mut::<cp_economy::Decay>() = decay;
    *sim.world_mut().resource_mut::<cp_building::Demand>() = demand;
    rejected.extend(
        sim.world_mut()
            .resource_mut::<bevy_ecs::message::Messages<cp_core::CommandOutcome>>()
            .drain()
            .filter_map(|o| o.result.err().map(|e| e.to_string())),
    );
    (population(sim), rejected)
}

/// The town's streets and zones as a player lays them out (the module docs), nothing grown yet;
/// the commands the sim refused. BUG-095's probe grows it with the game's own demand.
pub fn lay_out_town(sim: &mut SimRunner) -> Vec<String> {
    use cp_content::ZoneType;
    use cp_zone::{ZoneCellRef, ZoneGrid};
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let db = sim.world().resource::<cp_content::SimContent>().0.clone();
    let road = db.road_id("basic_2l").expect("basic_2l");
    let o = origin();
    let xs: Vec<f32> = (0..=COLS).map(|i| i as f32 * DX).collect();
    let ys: Vec<f32> = (0..=ROWS).map(|j| j as f32 * DY).collect();
    let (x1, y1) = (xs[COLS], ys[ROWS]);
    let rejected = {
        let mut h = Hand::new(sim, o, road);
        let v = Vec2::new;
        h.street(v(0.0, 0.0), v(x1, 0.0));
        h.street(v(0.0, 0.0), v(0.0, y1));
        for &y in &ys[1..] {
            h.street(v(0.0, y), v(x1, y));
        }
        for &x in &xs[1..] {
            h.street(v(x, 0.0), v(x, y1));
        }
        std::mem::take(&mut h.rejected)
    };
    for _ in 0..4 {
        sim.tick_once();
    }
    // Zone the blocks inside the town; the frontage outside stays open.
    let paint: Vec<(ZoneCellRef, ZoneType)> = {
        let zones = sim.world().resource::<ZoneGrid>();
        zones
            .blocks()
            .flat_map(|(id, _)| zones.block_cells(id))
            .filter_map(|c| {
                let l = zones.cell_footprint(c)?.c - o;
                let inside = l.x > 0.0 && l.y > 0.0 && l.x < x1 && l.y < y1;
                inside.then(|| {
                    let zone = if l.y < ys[ROWS - 1] {
                        ZoneType::ResLow
                    } else if l.x < xs[COLS - 1] {
                        ZoneType::ComLow
                    } else {
                        ZoneType::Industrial
                    };
                    (c, zone)
                })
            })
            .collect()
    };
    {
        let mut zones = sim.world_mut().resource_mut::<ZoneGrid>();
        for (cell, zone) in paint {
            zones.set_zone(cell, zone);
        }
    }
    rejected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_resident_count() {
        assert_eq!(parse_town("town:700"), Some(700));
        assert_eq!(parse_town("town:10"), None);
        assert_eq!(parse_town("city:700"), None);
    }
}
