//! Test scenes for measuring and screenshots: `--scene grid:N` (desktop) or `?scene=grid:N`
//! (web) builds an N×N street lattice through the normal `BuildRoad` commands.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_ecs::resource::Resource;
use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::{CommandOutcome, Tick};
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_sim::{SimCommand, SimRunner};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/content_files.rs"));
}

/// All of `assets/content`, compiled in (the browser build has no file system).
pub fn embedded_content() -> Arc<ContentDb> {
    let sources = embedded::FILES
        .iter()
        .map(|(name, text)| (format!("content/{name}"), (*text).to_string()))
        .collect();
    Arc::new(ContentDb::from_sources(sources).expect("embedded content is valid"))
}

/// Kept for callers that only need roads.
pub fn road_content() -> Arc<ContentDb> {
    embedded_content()
}

/// Distance between streets, metres (below the 96 m segment limit).
pub const SPACING: f32 = 100.0;

/// Parses `city:N` (a grid of N streets with zoning and buildings).
pub fn parse_city(spec: &str) -> Option<usize> {
    spec.strip_prefix("city:")?
        .parse()
        .ok()
        .filter(|n| (2..=60).contains(n))
}

/// Showcase scene: every building def in rows by zone, then a row of states.
pub const SHOWCASE: &str = "showcase";

/// Parses `grid:N`.
pub fn parse_grid(spec: &str) -> Option<usize> {
    spec.strip_prefix("grid:")?
        .parse()
        .ok()
        .filter(|n| (1..=60).contains(n))
}

/// Parses `traffic:N[:CARS:PEDS]`: an `N`×`N` grid with synthetic traffic (default 16 000 cars
/// and 20 000 pedestrians, the §14 budget scene).
pub fn parse_traffic(spec: &str) -> Option<(usize, usize, usize)> {
    let mut parts = spec.strip_prefix("traffic:")?.split(':');
    let n = parts
        .next()?
        .parse()
        .ok()
        .filter(|n| (2..=60).contains(n))?;
    let cars = parts.next().map_or(Some(16_000), |v| v.parse().ok())?;
    let peds = parts.next().map_or(Some(20_000), |v| v.parse().ok())?;
    Some((n, cars, peds))
}

/// Synthetic traffic kept topped up by the app (`traffic:` scene): the app sends
/// `DebugCommand::TopUpTraffic` through the sim's own command queue whenever the pools run below
/// these numbers.
#[derive(Resource, Clone, Copy, Debug)]
pub struct DemoTraffic {
    pub cars: usize,
    pub peds: usize,
}

/// Adds the missing demo agents. Cheap when the pools are full.
pub fn top_up_traffic(sim: &mut SimRunner) {
    let Some(demo) = sim.world().get_resource::<DemoTraffic>().copied() else {
        return;
    };
    let (cars, peds) = sim.agent_counts();
    if cars < demo.cars || peds < demo.peds {
        sim.top_up_traffic(demo.cars as u32, demo.peds as u32);
    }
}

/// Builds an `n`×`n` lattice (n streets each way, n² crossings) centred on the map, and
/// runs the ticks that apply it. Every fifth street is a 4-lane road. Returns the reasons
/// of rejected commands (empty when everything was built).
pub fn build_grid(sim: &mut SimRunner, n: usize) -> Vec<String> {
    build_grid_with(sim, n, 5)
}

/// Like [`build_grid`]; every `arterial_every`-th street is 4-lane (0 = none).
pub fn build_grid_with(sim: &mut SimRunner, n: usize, arterial_every: usize) -> Vec<String> {
    let content = road_content();
    let basic = content.road_id("basic_2l").expect("basic_2l");
    let medium = content.road_id("medium_4l").expect("medium_4l");
    let c = cp_terrain::MAP_SIZE / 2.0;
    let span = (n - 1) as f32 * SPACING;
    let (x0, z0) = (c - span / 2.0, c - span / 2.0);
    // A scripted scene is not paid for: fund it, and put the treasury back afterwards.
    let treasury = sim.world().resource::<cp_core::Treasury>().balance;
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance = cp_core::Cost(i64::MAX / 2);
    let start = sim.tick().0;
    let mut k = 0;
    // Each street is built in runs of two blocks: one long command would be cut into equal
    // pieces that cross the 100 m crossing grid at awkward places and produce pieces
    // shorter than the sim's minimum.
    let blocks_per_run = 2;
    let mut runs: Vec<(bool, usize, usize)> = Vec::new();
    for horizontal in [true, false] {
        for i in 0..n {
            let mut b = 0;
            while b < n - 1 {
                runs.push((horizontal, i, b));
                b += blocks_per_run;
            }
        }
    }
    for (horizontal, i, b) in runs {
        let o = i as f32 * SPACING;
        let a = b as f32 * SPACING;
        let e = ((b + blocks_per_run).min(n - 1)) as f32 * SPACING;
        let (from, to) = if horizontal {
            (Vec2::new(x0 + a, z0 + o), Vec2::new(x0 + e, z0 + o))
        } else {
            (Vec2::new(x0 + o, z0 + a), Vec2::new(x0 + o, z0 + e))
        };
        let road = if arterial_every > 0 && i % arterial_every == 0 {
            medium
        } else {
            basic
        };
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road,
                path: RoadPath::Straight { from, to },
                elevation: ElevationMode::Ground,
            }),
            Tick(start + k),
        );
        k += 1;
    }
    let mut rejected = Vec::new();
    for _ in 0..=k {
        sim.tick_once();
        let outcomes = sim
            .world_mut()
            .resource_mut::<Messages<CommandOutcome>>()
            .drain()
            .collect::<Vec<_>>();
        rejected.extend(
            outcomes
                .into_iter()
                .filter_map(|o| o.result.err().map(|e| e.to_string())),
        );
    }
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance = treasury;
    rejected
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_terrain::Terrain;

    fn sim(n: usize, every: usize) -> SimRunner {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands");
        let mut sim = SimRunner::new(1);
        sim.set_terrain(Arc::new(Terrain::load(&dir).unwrap()));
        sim.set_content(road_content());
        let rejected = build_grid_with(&mut sim, n, every);
        assert!(rejected.is_empty(), "{rejected:#?}");
        sim
    }

    #[test]
    fn grid_scene_builds_every_street() {
        for (n, every) in [(3, 0), (10, 0), (10, 5)] {
            let v = sim(n, every).view();
            // The interior crossings are four-way junctions.
            let crossings = v
                .junctions
                .iter()
                .filter(|(_, j)| j.arms.len() == 4)
                .count();
            assert!(
                n < 4 || crossings >= (n - 2) * (n - 2) * 3 / 4,
                "n={n} every={every}"
            );
            assert!(
                v.net.segment_count() >= 2 * n * (n - 1),
                "n={n}: {}",
                v.net.segment_count()
            );
        }
    }

    #[test]
    fn grid_spec_parsing() {
        assert_eq!(parse_grid("grid:10"), Some(10));
        assert_eq!(parse_grid("grid:0"), None);
        assert_eq!(parse_grid("grid:99"), None);
        assert_eq!(parse_grid("nope"), None);
    }
}

/// Zone painted on a block for a city scene: dense and tall in the middle, low and
/// industrial at the edge, mixed in between (deterministic from the block).
fn zone_for(block_index: u32, dist: f32) -> cp_content::ZoneType {
    use cp_content::ZoneType::*;
    let h = block_index.wrapping_mul(2_654_435_761) >> 16;
    match dist {
        d if d < 400.0 => [ComHigh, Office, ResHigh, Office][(h % 4) as usize],
        d if d < 900.0 => [ResHigh, ComLow, ResHigh, ComHigh, ResLow][(h % 5) as usize],
        d if d < 1500.0 => [ResLow, ResLow, ComLow, ResHigh, Industrial][(h % 5) as usize],
        _ => [ResLow, Industrial, Industrial, ResLow][(h % 4) as usize],
    }
}

/// Builds an `n`×`n` street grid, paints zones on every block and lets the spawner run
/// until `target` buildings exist (or `max_ticks` passed). Then dresses the result for a
/// showcase: higher-level variants where the lot allows, some buildings still under
/// construction (at all stages), a few abandoned and burned. Returns the building count.
pub fn build_city(sim: &mut SimRunner, n: usize, target: usize, max_ticks: u64) -> usize {
    use cp_building::{BuildingGrid, BuildingState};
    use cp_zone::{ZoneCellRef, ZoneGrid};
    // A scripted scene is not waited for: every zone is wanted while it grows.
    // The scene has no water or power lines and shows buildings in every state: no decay.
    *sim.world_mut().resource_mut::<cp_economy::Decay>() = cp_economy::Decay::disabled();
    let demand = *sim.world().resource::<cp_building::Demand>();
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(1.0);
    let _ = build_grid_with(sim, n, 5);
    // Zone blocks exist after a few quiet ticks.
    for _ in 0..4 {
        sim.tick_once();
    }
    let centre = Vec2::splat(cp_terrain::MAP_SIZE / 2.0);
    let paint: Vec<(ZoneCellRef, cp_content::ZoneType)> = {
        let zones = sim.world().resource::<ZoneGrid>();
        zones
            .blocks()
            .flat_map(|(id, b)| {
                let p = b.curve.point(0.5);
                let d = Vec2::new(p.x, p.z).distance(centre);
                let zone = zone_for(id.index, d);
                (0..b.cell_count() as u16).map(move |cell| (ZoneCellRef { block: id, cell }, zone))
            })
            .collect()
    };
    {
        let mut zones = sim.world_mut().resource_mut::<ZoneGrid>();
        for (cell, zone) in paint {
            zones.set_zone(cell, zone);
        }
    }
    for _ in 0..max_ticks {
        sim.tick_once();
        if sim.world().resource::<BuildingGrid>().len() >= target {
            break;
        }
    }
    *sim.world_mut().resource_mut::<cp_building::Demand>() = demand;
    dress_city(sim);
    let _ = BuildingState::Active;
    sim.world().resource::<BuildingGrid>().len()
}

/// Showcase dressing (see [`build_city`]), deterministic from the building ids.
fn dress_city(sim: &mut SimRunner) {
    use cp_building::{BuildingGrid, BuildingState};
    let db = sim.world().resource::<cp_content::SimContent>().0.clone();
    let now = sim.tick().0;
    let ids: Vec<_> = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .map(|(id, _)| id)
        .collect();
    let mut grid = sim.world_mut().resource_mut::<BuildingGrid>();
    for id in ids {
        let h = id.index.wrapping_mul(2_246_822_519) >> 8;
        let Some(b) = grid.get(id).cloned() else {
            continue;
        };
        let def = db.building(b.def);
        // A taller variant of the same zone that fits the lot exactly.
        let want = 1 + (h % 3) as u8;
        let better = db
            .buildings()
            .filter(|(_, d)| {
                d.zone == def.zone && d.level == want && d.lot == def.lot && d.service.is_none()
            })
            .map(|(i, _)| i)
            .nth((h >> 4) as usize % 2);
        let state = match h % 100 {
            0..=11 => BuildingState::UnderConstruction,
            12..=14 => BuildingState::Abandoned,
            15 => BuildingState::Burned,
            _ => BuildingState::Active,
        };
        if let Some(m) = grid.get_mut(id) {
            if let Some(d) = better {
                m.def = d;
                m.level = want;
            }
            m.state = state;
            if state == BuildingState::UnderConstruction {
                // Spread over the construction stages.
                m.ready_at = cp_core::Tick(now + 200 + u64::from(h % 4) * 1000);
            }
        }
    }
}

#[cfg(test)]
mod city_tests {
    use super::*;
    use cp_terrain::Terrain;

    #[test]
    fn city_scene_spawns_and_dresses_buildings() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands");
        let mut sim = SimRunner::new(1);
        sim.set_terrain(Arc::new(Terrain::load(&dir).unwrap()));
        sim.set_content(embedded_content());
        let t = std::time::Instant::now();
        let n = build_city(&mut sim, 6, 150, 800);
        eprintln!("CITY n={n} in {:?}", t.elapsed());
        assert!(n >= 100, "{n}");
        let v = sim.view();
        let states: std::collections::BTreeSet<_> =
            v.buildings.iter().map(|(_, b)| b.state as u8).collect();
        assert!(states.len() >= 3, "{states:?}");
        assert_eq!(parse_city("city:12"), Some(12));
        assert_eq!(parse_city("city:1"), None);
    }
}

/// Places one of every building def (rows by zone family, levels left to right, variants
/// together) plus a row showing construction stages, abandoned and burned. Positions are
/// centred on the map; `rot` is 0 so every front faces -z (towards the default camera).
pub fn build_showcase(sim: &mut SimRunner) -> usize {
    use cp_building::{AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingState, Lot};
    use cp_content::ZoneType::*;
    let db = embedded_content();
    *sim.world_mut().resource_mut::<cp_economy::Decay>() = cp_economy::Decay::disabled();
    // One road to borrow a real segment id from (the access point is only a hint).
    let c = cp_terrain::MAP_SIZE / 2.0;
    let basic = db.road_id("basic_2l").expect("basic_2l");
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: basic,
            path: RoadPath::Straight {
                from: Vec2::new(c - 400.0, c - 600.0),
                to: Vec2::new(c - 330.0, c - 600.0),
            },
            elevation: ElevationMode::Ground,
        }),
        sim.tick(),
    );
    sim.tick_once();
    sim.tick_once();
    let segment = sim
        .world()
        .resource::<cp_net::NetGraph>()
        .segments()
        .next()
        .map(|(id, _)| id)
        .expect("a segment");
    let spacing = 44.0;
    let mut placed = 0;
    let mut grid = sim.world_mut().resource_mut::<BuildingGrid>();
    let add = |grid: &mut BuildingGrid, def_id, col: usize, row: usize, state, ready: u64| {
        let d = db.building(def_id);
        let b = Building {
            def: def_id,
            footprint: cp_building::Footprint::Lot(Lot {
                block: cp_core::Id::new(0, 0),
                col: 0,
                w: d.lot.0,
                d: d.lot.1,
            }),
            level: d.level,
            state,
            ready_at: cp_core::Tick(ready),
            pos: Vec2::new(
                c - 200.0 + col as f32 * spacing,
                c - 130.0 + row as f32 * spacing,
            ),
            rot: 0.0,
            access: Some(AccessPoint { segment, t: 0.5 }),
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        };
        grid.insert(b);
    };
    let zones = [ResLow, ResHigh, ComLow, ComHigh, Industrial, Office];
    for (row, zone) in zones.into_iter().enumerate() {
        let mut defs: Vec<_> = db
            .buildings()
            .filter(|(_, d)| d.zone == zone && d.service.is_none())
            .map(|(i, d)| (d.level, i))
            .collect();
        defs.sort();
        for (col, (_, id)) in defs.into_iter().enumerate() {
            add(&mut grid, id, col, row, BuildingState::Active, 0);
            placed += 1;
        }
    }
    // States: a 4-floor-ish office and a house, at every construction stage, abandoned, burned.
    let row = zones.len();
    for (k, zone) in [ResHigh, Office].into_iter().enumerate() {
        let id = db
            .buildings()
            .find(|(_, d)| d.zone == zone && d.level == 3 && d.service.is_none())
            .map(|(i, _)| i)
            .expect("def");
        let now = sim.tick().0;
        let day = cp_core::time::TICKS_PER_DAY;
        let mut grid = sim.world_mut().resource_mut::<BuildingGrid>();
        for stage in 0..4u64 {
            // `ready_at` such that progress lands in the middle of `stage`.
            let left = day - (stage * day / 4 + day / 8);
            add(
                &mut grid,
                id,
                stage as usize,
                row + k,
                BuildingState::UnderConstruction,
                now + left,
            );
        }
        add(&mut grid, id, 4, row + k, BuildingState::Abandoned, 0);
        add(&mut grid, id, 5, row + k, BuildingState::Burned, 0);
        add(&mut grid, id, 6, row + k, BuildingState::Active, 0);
        placed += 7;
    }
    placed
}
