//! Scripted cities for benchmarks and fixtures (`architecture/system-design.md` §14).
//!
//! [`grid_city`] builds a street lattice, paints every block, lays the utility network and
//! lets time run until the target population has moved in. **Everything is done through
//! commands** (roads, zones, buildings, pipes, power lines), pushed at ticks and applied by
//! the normal dispatcher, so the result is an ordinary save-able state reproducible from
//! `(target_pop, seed, terrain, content)`.
//!
//! **Funds.** A scripted city is not paid for with the player's money: the scenario sets the
//! starting balance to [`GridCityConfig::funds`] before the first tick (a documented setting,
//! not a mid-run write) and leaves whatever is left.
//! **Demand.** While the city grows, [`Demand`] is held at a fixed high value (the documented
//! scenario hook `Demand::frozen`) so the spawner and move-in do not wait for the economy's
//! smoothing; it is released at the end and the economy takes over from there.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_building::{BuildingCommand, BuildingGrid, BuildingState, Demand};
use cp_content::{ContentDb, UtilityKind, ZoneType};
use cp_core::{CommandOutcome, Cost, SimLimits, Tick, Treasury};
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_terrain::{MAP_SIZE, Terrain};
use cp_zone::{BLOCK_DEPTH, ZoneCellRef, ZoneCommand, ZoneGrid};

use crate::{SimCommand, SimRunner};

/// Distance between the streets of the lattice, metres.
pub const STREET_SPACING: f32 = 100.0;
/// Blocks per `BuildRoad` command: longer runs are cut into equal pieces that cross the
/// lattice at awkward places and produce pieces shorter than the sim's minimum.
const BLOCKS_PER_RUN: usize = 2;

/// Knobs of [`grid_city`]; the defaults build the bench cities.
#[derive(Clone, Debug)]
pub struct GridCityConfig {
    /// Starting balance of the scenario (see the module docs).
    pub funds: Cost,
    /// Give up after this many ticks without reaching the target.
    pub max_ticks: u64,
    /// Citizens per lattice crossing the lattice is sized for.
    pub people_per_crossing: usize,
    /// After the target is met the clock runs on to this hour of the day (at most one more
    /// day), so a bench or a save starts in the morning rush, not in the dead of night.
    pub settle_to_hour: f32,
}

impl Default for GridCityConfig {
    fn default() -> Self {
        Self {
            funds: Cost(1_000_000_000_000),
            max_ticks: 200_000,
            people_per_crossing: 40,
            settle_to_hour: 7.0,
        }
    }
}

/// What a [`grid_city`] run did.
#[derive(Clone, Debug, PartialEq)]
pub struct GridCityReport {
    /// Streets each way.
    pub streets: usize,
    pub target_pop: usize,
    pub population: usize,
    pub buildings: usize,
    /// Ticks the sim ran (the final tick).
    pub ticks: u64,
    pub reached: bool,
    /// Commands the dispatcher rejected (should be 0).
    pub rejected: Vec<String>,
}

/// Number of streets each way for `target_pop`.
pub fn streets_for(target_pop: usize, cfg: &GridCityConfig) -> usize {
    let crossings = target_pop.div_ceil(cfg.people_per_crossing.max(1));
    ((crossings as f32).sqrt().ceil() as usize).clamp(4, 58)
}

/// Zone of a block of the lattice: dense and tall in the middle, low and industrial at the
/// edge, mixed in between; deterministic from `(block index, seed)`.
fn zone_for(block_index: u32, seed: u64, dist: f32) -> ZoneType {
    use ZoneType::*;
    let h = (block_index ^ (seed as u32)).wrapping_mul(2_654_435_761) >> 16;
    match dist {
        d if d < 500.0 => [ComHigh, Office, ResHigh, ResHigh][(h % 4) as usize],
        d if d < 1200.0 => [ResHigh, ResHigh, ComLow, ComHigh, ResLow][(h % 5) as usize],
        d if d < 2000.0 => [ResLow, ResLow, ComLow, ResHigh, Industrial][(h % 5) as usize],
        _ => [ResLow, ResLow, Industrial, ResLow][(h % 4) as usize],
    }
}

/// Pushes `cmd` at the next tick, runs it and returns the rejection text, if any.
struct Script<'a> {
    sim: &'a mut SimRunner,
    rejected: Vec<String>,
}

impl Script<'_> {
    fn run(&mut self, cmds: Vec<SimCommand>) {
        let start = self.sim.tick().0;
        let n = cmds.len() as u64;
        for (k, c) in cmds.into_iter().enumerate() {
            self.sim.push_command(c, Tick(start + k as u64));
        }
        for _ in 0..=n {
            self.sim.tick_once();
            let out: Vec<CommandOutcome> = self
                .sim
                .world_mut()
                .resource_mut::<Messages<CommandOutcome>>()
                .drain()
                .collect();
            self.rejected.extend(
                out.into_iter()
                    .filter_map(|o| o.result.err().map(|e| e.to_string())),
            );
        }
    }
}

/// `Place` commands for the wind turbines on lots of the outermost blocks, and, where the
/// owned tiles have water, a pump on it and an outflow on the shore with a pipe from each
/// to the lattice. Every placement is validated first, so nothing here is rejected.
fn service_sites(
    sim: &SimRunner,
    content: &ContentDb,
    target_pop: usize,
    runs: &[(Vec2, Vec2)],
) -> Vec<SimCommand> {
    let world = sim.world();
    let zones = world.resource::<ZoneGrid>();
    let net = world.resource::<cp_net::NetGraph>();
    let terrain = world.get_resource::<crate::SimTerrain>().map(|t| &*t.0);
    let tiles = world.get_resource::<cp_terrain::TileOwnership>();
    let valid = |def, pos| {
        cp_building::validate_placement(content, zones, net, terrain, tiles, def, pos, 0.0).is_ok()
    };
    let place = |def, pos| SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 });
    let mut out = Vec::new();

    let centre = Vec2::splat(MAP_SIZE / 2.0);
    let mut blocks: Vec<(f32, u32, cp_zone::ZoneBlockId, u16)> = zones
        .blocks()
        .filter(|(_, b)| b.cols >= 6)
        .map(|(id, b)| {
            let p = b.curve.point(0.5);
            (Vec2::new(p.x, p.z).distance(centre), id.index, id, b.cols)
        })
        .collect();
    // Farthest first; the index breaks ties deterministically.
    blocks.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    if let Some(turbine) = content.building_id("wind_turbine") {
        let count = (target_pop / 1500 + 1).clamp(1, 40);
        // Every `step`-th outer block, so the turbines are spread along the edge.
        let step = (blocks.len() / (count * 4)).max(1);
        out.extend(
            blocks
                .iter()
                .step_by(step)
                .filter_map(|&(_, _, id, cols)| {
                    zones
                        .cell_footprint(ZoneCellRef {
                            block: id,
                            cell: (cols / 2) * BLOCK_DEPTH + 1,
                        })
                        .map(|o| o.c)
                })
                .filter(|&pos| valid(turbine, pos))
                .take(count)
                .map(|pos| place(turbine, pos)),
        );
    }

    // Water: the wet sample nearest to the lattice that takes a pump, then a shore spot
    // nearby for the outflow.
    let (Some(terrain), Some(pump), Some(outflow)) = (
        terrain,
        content.building_id("water_pump_station"),
        content.building_id("sewage_outflow_station"),
    ) else {
        return out;
    };
    let nearest_street = |p: Vec2| {
        runs.iter()
            .flat_map(|&(a, b)| [a, b])
            .min_by(|a, b| a.distance(p).total_cmp(&b.distance(p)))
    };
    let mut wet: Vec<(f32, Vec2)> = Vec::new();
    let mut z = 0.0;
    while z < MAP_SIZE {
        let mut x = 0.0;
        while x < MAP_SIZE {
            let p = Vec2::new(x, z);
            if terrain.water_depth(p) >= 2.0 {
                wet.push((p.distance(centre), p));
            }
            x += 20.0;
        }
        z += 20.0;
    }
    wet.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.x.total_cmp(&b.1.x)));
    let Some(pump_at) = wet.iter().map(|w| w.1).find(|&p| valid(pump, p)) else {
        return out;
    };
    let shore = (0..64).find_map(|k| {
        let ring = 40.0 + (k / 8) as f32 * 20.0;
        let a = (k % 8) as f32 * std::f32::consts::FRAC_PI_4;
        let p = pump_at + Vec2::new(a.cos(), a.sin()) * ring;
        valid(outflow, p).then_some(p)
    });
    out.push(place(pump, pump_at));
    if let Some(street) = nearest_street(pump_at) {
        out.push(SimCommand::Utility(cp_utility::UtilityCommand::BuildLink {
            kind: UtilityKind::Water,
            path: vec![pump_at, street],
        }));
    }
    if let Some(shore) = shore {
        out.push(place(outflow, shore));
        if let Some(street) = nearest_street(shore) {
            out.push(SimCommand::Utility(cp_utility::UtilityCommand::BuildLink {
                kind: UtilityKind::Sewage,
                path: vec![shore, street],
            }));
        }
    }
    out
}

/// Builds the city and runs it until `target_pop` citizens live in it (or `cfg.max_ticks`).
/// Same arguments, same state: the result is reproducible from the seed.
pub fn grid_city(
    target_pop: usize,
    seed: u64,
    terrain: Arc<Terrain>,
    content: Arc<ContentDb>,
    cfg: &GridCityConfig,
) -> (SimRunner, GridCityReport) {
    let mut sim = SimRunner::with_limits(seed, SimLimits::desktop());
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(&content));
    sim.world_mut().resource_mut::<Treasury>().balance = cfg.funds;
    let n = streets_for(target_pop, cfg);
    let mut script = Script {
        sim: &mut sim,
        rejected: Vec::new(),
    };

    // Roads: every fifth street is a 4-lane avenue.
    let basic = content.road_id("basic_2l").expect("basic_2l");
    let medium = content.road_id("medium_4l").expect("medium_4l");
    let c = MAP_SIZE / 2.0;
    let span = (n - 1) as f32 * STREET_SPACING;
    let (x0, z0) = (c - span / 2.0, c - span / 2.0);
    let mut roads = Vec::new();
    let mut runs: Vec<(Vec2, Vec2)> = Vec::new();
    for horizontal in [true, false] {
        for i in 0..n {
            let mut b = 0;
            while b < n - 1 {
                let o = i as f32 * STREET_SPACING;
                let a = b as f32 * STREET_SPACING;
                let e = ((b + BLOCKS_PER_RUN).min(n - 1)) as f32 * STREET_SPACING;
                let (from, to) = if horizontal {
                    (Vec2::new(x0 + a, z0 + o), Vec2::new(x0 + e, z0 + o))
                } else {
                    (Vec2::new(x0 + o, z0 + a), Vec2::new(x0 + o, z0 + e))
                };
                runs.push((from, to));
                roads.push(SimCommand::Net(NetCommand::BuildRoad {
                    road: if i % 5 == 0 { medium } else { basic },
                    path: RoadPath::Straight { from, to },
                    elevation: ElevationMode::Ground,
                }));
                b += BLOCKS_PER_RUN;
            }
        }
    }
    script.run(roads);
    // Zone blocks exist after a few quiet ticks.
    for _ in 0..4 {
        script.sim.tick_once();
    }

    // Utilities: a power line, a water pipe and a sewage pipe along every street, wind
    // turbines on lots at the edge of the lattice (they stay unpainted), and, where the map
    // has water in the owned tiles, a pump on it and an outflow on the shore.
    let mut utilities = Vec::new();
    for kind in [UtilityKind::Power, UtilityKind::Water, UtilityKind::Sewage] {
        for &(from, to) in &runs {
            utilities.push(SimCommand::Utility(cp_utility::UtilityCommand::BuildLink {
                kind,
                path: vec![from, to],
            }));
        }
    }
    script.run(utilities);
    let sites = service_sites(script.sim, &content, target_pop, &runs);
    script.run(sites);

    // Zones: one `PaintZones` command per block.
    let centre = Vec2::splat(c);
    let paint: Vec<SimCommand> = {
        let zones = script.sim.world().resource::<ZoneGrid>();
        zones
            .blocks()
            .filter_map(|(id, b)| {
                // Blocks squeezed out by their neighbours have nothing to paint.
                let cells: Vec<_> = zones
                    .block_cells(id)
                    .into_iter()
                    .filter(|&c| zones.is_valid(c))
                    .collect();
                let p = b.curve.point(0.5);
                let zone = zone_for(id.index, seed, Vec2::new(p.x, p.z).distance(centre));
                (!cells.is_empty())
                    .then_some(SimCommand::Zone(ZoneCommand::PaintZones { cells, zone }))
            })
            .collect()
    };
    script.run(paint);

    // Growth: demand is held high until the target is met.
    *script.sim.world_mut().resource_mut::<Demand>() = Demand::fixed(1.0);
    let mut reached = false;
    let limit = script.sim.tick().0 + cfg.max_ticks;
    while script.sim.tick().0 < limit {
        script.sim.tick_once();
        if script.sim.tick().0.is_multiple_of(64) && population(script.sim) >= target_pop {
            reached = true;
            break;
        }
    }
    script.sim.world_mut().resource_mut::<Demand>().frozen = false;
    // Let the day come round to the settle hour (the next time it is that hour).
    let day = cp_core::time::TICKS_PER_DAY;
    let want = (cfg.settle_to_hour / 24.0 * day as f32) as u64;
    let now = script.sim.tick().0;
    let mut until = now - now % day + want;
    if until <= now {
        until += day;
    }
    while script.sim.tick().0 < until {
        script.sim.tick_once();
    }
    let report = GridCityReport {
        streets: n,
        target_pop,
        population: population(script.sim),
        buildings: script
            .sim
            .world()
            .resource::<BuildingGrid>()
            .iter()
            .filter(|(_, b)| b.state == BuildingState::Active)
            .count(),
        ticks: script.sim.tick().0,
        reached,
        rejected: script.rejected,
    };
    (sim, report)
}

/// Citizens living in the sim.
pub fn population(sim: &SimRunner) -> usize {
    sim.world()
        .resource::<cp_citizen::CitizenGrid>()
        .citizen_count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn env() -> (Arc<Terrain>, Arc<ContentDb>) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        (
            Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap()),
            Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap()),
        )
    }

    fn run(pop: usize, seed: u64) -> (u64, GridCityReport) {
        let (terrain, content) = env();
        let (mut sim, report) = grid_city(pop, seed, terrain, content, &GridCityConfig::default());
        (sim.checksum(), report)
    }

    #[test]
    fn lattice_size_follows_the_target() {
        let cfg = GridCityConfig::default();
        assert_eq!(streets_for(10_000, &cfg), 16);
        assert_eq!(streets_for(100_000, &cfg), 50);
        assert_eq!(streets_for(1, &cfg), 4);
        assert_eq!(streets_for(10_000_000, &cfg), 58);
    }

    #[test]
    fn a_small_grid_city_is_built_by_commands_and_reproducible_by_seed() {
        let ((a, ra), (b, rb)) = (run(1500, 3), run(1500, 3));
        assert!(ra.reached && ra.population >= 1500, "{ra:?}");
        assert!(ra.rejected.is_empty(), "{:?}", ra.rejected);
        assert_eq!((a, &ra), (b, &rb), "same seed, same city");
        // The seed matters: another one builds another city.
        let (c, _) = run(1500, 4);
        assert_ne!(a, c);
    }

    /// The 10k bench city, as `fixtures/saves/bench-10k.sav` was made. Slow in debug.
    #[test]
    #[ignore = "grid_city 10k: run in release (cargo test -p cp_sim --release grid_city_10k -- --ignored)"]
    fn grid_city_10k_is_reproducible_by_seed() {
        let ((a, ra), (b, _)) = (run(10_000, 1), run(10_000, 1));
        assert!(ra.reached && ra.rejected.is_empty(), "{ra:?}");
        assert_eq!(a, b);
    }
}
