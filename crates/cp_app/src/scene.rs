//! Test scenes for measuring and screenshots: `--scene grid:N` (desktop) or `?scene=grid:N`
//! (web) builds an N×N street lattice through the normal `BuildRoad` commands.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::{CommandOutcome, Tick};
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_sim::{SimCommand, SimRunner};

/// Road content the scenes need, compiled in so the browser build needs no extra fetches.
pub fn road_content() -> Arc<ContentDb> {
    macro_rules! road {
        ($name:literal) => {
            (
                concat!("roads/", $name, ".ron").to_string(),
                include_str!(concat!("../../../assets/content/roads/", $name, ".ron")).to_string(),
            )
        };
    }
    Arc::new(
        ContentDb::from_sources(vec![
            road!("basic_2l"),
            road!("medium_4l"),
            road!("large_6l"),
            road!("highway_1w_3l"),
        ])
        .expect("embedded road content is valid"),
    )
}

/// Distance between streets, metres (below the 96 m segment limit).
pub const SPACING: f32 = 100.0;

/// Parses `grid:N`.
pub fn parse_grid(spec: &str) -> Option<usize> {
    spec.strip_prefix("grid:")?
        .parse()
        .ok()
        .filter(|n| (1..=60).contains(n))
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
