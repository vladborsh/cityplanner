//! The screenshot scenes of the T-261 acceptance sweeps (`architecture/system-design.md`
//! §6.8.5), built from the same cases as the tests ([`cp_sim::road_cases`]):
//!
//! * `--scene joins`: the junction gallery of `junction_sweep`, one case per cell of a grid,
//!   `--cam joins` over all of it, `--cam joins-N` close over case `N` (in [`join_cases`] order);
//! * `--scene place`: the service sites of `place_sweep` with `basic_2l`, the largest lot of
//!   the sweep (4 x 4 when there is one) placed at every spot; `--cam place`, `--cam place-N`.
//!
//! Before/after proof for T-262…T-268.

use bevy::prelude::*;
use bevy_ecs::message::Messages;
use cp_core::CommandOutcome;
use cp_sim::road_cases::{build_roads, join_cases, place_sites};
use cp_sim::{SimCommand, SimRunner};

/// `--scene joins`.
pub const JOINS: &str = "joins";
/// `--scene place`.
pub const PLACE: &str = "place";

/// Distance between the cases of a scene, metres.
const PITCH: f32 = 260.0;
/// Cases per row of a scene.
const COLUMNS: usize = 10;

/// Where case `i` of a scene of `n` cases stands: rows of [`COLUMNS`] round the map's middle.
fn slot(i: usize, n: usize) -> Vec2 {
    let c = cp_terrain::MAP_SIZE / 2.0;
    let rows = n.div_ceil(COLUMNS);
    let (col, row) = ((i % COLUMNS) as f32, (i / COLUMNS) as f32);
    Vec2::new(
        c + (col - (COLUMNS as f32 - 1.0) / 2.0) * PITCH,
        c + (row - (rows as f32 - 1.0) / 2.0) * PITCH,
    )
}

/// The middle of a scene of `n` cases (its rows are full width, the last one maybe not filled).
fn whole(n: usize) -> Vec2 {
    let last = n.div_ceil(COLUMNS) * COLUMNS - 1;
    slot(0, n).lerp(slot(last, n), 0.5)
}

/// The middle of case `i` of the [`JOINS`] scene, or of the whole scene.
pub fn joins_focus(i: Option<usize>) -> Vec2 {
    let n = join_cases().len();
    i.map_or(whole(n), |i| slot(i.min(n - 1), n))
}

/// The middle of site `i` of the [`PLACE`] scene, or of the whole scene.
pub fn place_focus(i: Option<usize>) -> Vec2 {
    let n = place_sites("basic_2l", 6.0).len();
    i.map_or(whole(n), |i| slot(i.min(n - 1), n))
}

/// A scene is not paid for with the player's money.
fn fund(sim: &mut SimRunner) {
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance = cp_core::Cost(1_000_000_000_000);
}

/// Builds the [`JOINS`] scene; returns the rejected commands.
pub fn build_joins(sim: &mut SimRunner) -> Vec<String> {
    fund(sim);
    let cases = join_cases();
    let n = cases.len();
    let mut rejected = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        for (r, e) in build_roads(sim, slot(i, n), &case.roads) {
            rejected.push(format!("{} road {r}: {e}", case.name));
        }
    }
    rejected
}

/// Builds the [`PLACE`] scene; returns the rejected commands.
pub fn build_place(sim: &mut SimRunner) -> Vec<String> {
    fund(sim);
    let Some(db) = sim.view().content.clone() else {
        return vec!["no content".into()];
    };
    let road = "basic_2l";
    let hw = db.road(db.road_id(road).expect("basic_2l")).width / 2.0;
    // The largest road-access service lot on land, up to 4 x 4.
    let Some((def, lot)) = db
        .buildings()
        .filter(|(_, d)| {
            d.lot.0 <= 4
                && d.lot.1 <= 4
                && d.service
                    .as_deref()
                    .and_then(|s| db.service_id(s))
                    .is_some_and(|s| {
                        let s = db.service(s);
                        s.road_access && s.site == cp_content::ServiceSite::Land
                    })
        })
        .map(|(id, d)| (id, d.lot))
        .max_by_key(|(_, lot)| (u16::from(lot.0) * u16::from(lot.1), lot.0))
    else {
        return vec!["no service building".into()];
    };
    let sites = place_sites(road, hw);
    let n = sites.len();
    let mut rejected = Vec::new();
    for (i, site) in sites.iter().enumerate() {
        let at = slot(i, n);
        for (r, e) in build_roads(sim, at, &site.roads) {
            rejected.push(format!("{} road {r}: {e}", site.name));
        }
        for spot in &site.spots {
            let pos = at + spot.centre(f32::from(lot.0) * 8.0, f32::from(lot.1) * 8.0, 0.5);
            sim.push_command(
                SimCommand::Building(cp_building::BuildingCommand::Place { def, pos, rot: 0.0 }),
                sim.tick(),
            );
            sim.apply_due_commands();
            rejected.extend(
                sim.world_mut()
                    .resource_mut::<Messages<CommandOutcome>>()
                    .drain()
                    .filter_map(|o| {
                        o.result
                            .err()
                            .map(|e| format!("{} {}: {e}", site.name, spot.name))
                    }),
            );
        }
    }
    sim.tick_once();
    rejected
}

/// The camera of the scenes: `joins`, `joins-N`, `place`, `place-N`: `(position, look_at)`.
pub fn camera(name: &str) -> Option<(Vec3, Vec3)> {
    let (scene, rest) = name
        .strip_prefix("joins")
        .map(|r| (JOINS, r))
        .or_else(|| name.strip_prefix("place").map(|r| (PLACE, r)))?;
    let i = match rest {
        "" => None,
        r => Some(r.strip_prefix('-')?.parse::<usize>().ok()?),
    };
    let f = if scene == JOINS {
        joins_focus(i)
    } else {
        place_focus(i)
    };
    Some(if i.is_some() {
        (Vec3::new(f.x, 140.0, f.y + 90.0), Vec3::new(f.x, 0.0, f.y))
    } else {
        (
            Vec3::new(f.x, 2100.0, f.y + 1300.0),
            Vec3::new(f.x, 0.0, f.y),
        )
    })
}
