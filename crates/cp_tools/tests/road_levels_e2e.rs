//! BUG-121: the road tool at every level (1..4) and in a tunnel, as a player draws it: pick the
//! level, click twice, the ghost is valid and the click builds, on flat land and across an
//! existing street. What gets built is checked against the level that was picked.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::{CommandOutcome, Cost, Treasury};
use cp_net::{Elevation, ElevationMode, NetCommand, NetGraph, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::{ELEVATION_STEP, RoadMode, RoadTool, compute_ghost_alt, resolve_cursor};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const ROAD: &str = "basic_2l";

fn run(sim: &mut SimRunner, cmd: SimCommand) -> Result<Cost, String> {
    let at = sim.tick();
    sim.push_command(cmd, at);
    sim.tick_once();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last()
        .expect("an outcome")
        .result
        .clone()
        .map_err(|e| e.to_string())
}

fn world() -> (SimRunner, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(121);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    sim.tick_once();
    sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(1_000_000_000.0);
    (sim, db)
}

/// The tool at `level` (`None` = underground), two clicks from `a` to `b`: the built plan's
/// outcome, with the ghost's verdict checked on the way.
fn draw(sim: &mut SimRunner, db: &ContentDb, level: Option<u8>, a: Vec2, b: Vec2) -> Result<Cost, String> {
    let mut tool = RoadTool {
        mode: RoadMode::Straight,
        road: Some(db.road_id(ROAD).unwrap()),
        ..Default::default()
    };
    match level {
        Some(l) => (0..l).for_each(|_| tool.raise()),
        None => tool.lower(),
    }
    let view = sim.view();
    let qa = resolve_cursor(&view, &tool, a, false).unwrap().cursor;
    let _ = tool.click(qa);
    let ghost = compute_ghost_alt(&view, &tool, b, false).expect("a ghost");
    if !ghost.valid {
        return Err(format!("ghost invalid: {:?}", ghost.errors));
    }
    let plan = ghost.plan.clone().expect("a plan");
    drop(view);
    run(sim, SimCommand::Net(NetCommand::BuildPlan { plan }))
}

fn kinds(sim: &SimRunner) -> Vec<(Elevation, f32, f32)> {
    let g = sim.world().resource::<NetGraph>();
    g.segments()
        .map(|(_, s)| {
            let n = g.node(s.start).unwrap();
            let m = g.node(s.end).unwrap();
            (s.elevation, n.pos.y, m.pos.y)
        })
        .collect()
}

#[test]
fn every_level_and_the_tunnel_build_on_flat_land_and_across_a_street() {
    for level in [Some(1u8), Some(2), Some(3), Some(4), None] {
        for crossing in [false, true] {
            let (mut sim, db) = world();
            let street = db.road_id(ROAD).unwrap();
            run(
                &mut sim,
                SimCommand::Net(NetCommand::BuildRoad {
                    road: street,
                    path: RoadPath::Straight {
                        from: Vec2::new(4400.0, 4400.0),
                        to: Vec2::new(4800.0, 4400.0),
                    },
                    elevation: ElevationMode::Ground,
                }),
            )
            .unwrap();
            let before = kinds(&sim).len();
            let (a, b) = if crossing {
                (Vec2::new(4600.0, 4240.0), Vec2::new(4600.0, 4560.0))
            } else {
                (Vec2::new(5000.0, 4240.0), Vec2::new(5000.0, 4560.0))
            };
            let r = draw(&mut sim, &db, level, a, b);
            let all = kinds(&sim);
            println!("level {level:?} crossing {crossing}: {r:?} -> {:?}", &all[before.min(all.len())..]);
            r.unwrap_or_else(|e| panic!("level {level:?} crossing {crossing}: {e}"));
            let new = &all[before..];
            assert!(!new.is_empty(), "level {level:?}: nothing built");
            let want = match level {
                Some(_) => Elevation::Elevated,
                None => Elevation::Tunnel,
            };
            assert!(
                new.iter().any(|s| s.0 == want),
                "level {level:?} crossing {crossing}: {new:?}"
            );
            if let Some(l) = level {
                let top = f32::from(l) * ELEVATION_STEP;
                assert!(
                    new.iter().any(|s| (s.1 - s.2).abs() < 0.01 && s.1 > top - 0.5),
                    "level {l}: no piece at {top} m: {new:?}"
                );
            }
        }
    }
}
