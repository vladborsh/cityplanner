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
fn draw(
    sim: &mut SimRunner,
    db: &ContentDb,
    level: Option<u8>,
    a: Vec2,
    b: Vec2,
) -> Result<Cost, String> {
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
    // The ghost is drawn where the road will stand: raised by the level, or below the ground.
    let ys: Vec<f32> = ghost.polylines.iter().flatten().map(|p| p.y).collect();
    let (lo, hi) = ys
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), y| (l.min(*y), h.max(*y)));
    match level {
        Some(l) => assert!(
            (hi - lo - f32::from(l) * ELEVATION_STEP).abs() < 1.5,
            "level {l}: the ghost rises {} m",
            hi - lo
        ),
        None => assert!(hi - lo > 5.0, "the tunnel ghost dips {} m", hi - lo),
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
                (Vec2::new(4600.0, 3960.0), Vec2::new(4600.0, 4840.0))
            } else {
                (Vec2::new(5000.0, 3960.0), Vec2::new(5000.0, 4840.0))
            };
            let r = draw(&mut sim, &db, level, a, b);
            let all = kinds(&sim);
            println!(
                "level {level:?} crossing {crossing}: {r:?} -> {:?}",
                &all[before.min(all.len())..]
            );
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
                // The flatlands stand at about 30 m: the deck rises `level * step` above its ends.
                let low = new.iter().map(|s| s.1.min(s.2)).fold(f32::MAX, f32::min);
                let high = new.iter().map(|s| s.1.max(s.2)).fold(f32::MIN, f32::max);
                let top = f32::from(l) * ELEVATION_STEP;
                assert!(
                    (high - low - top).abs() < 1.5,
                    "level {l}: rises {} m, want {top}: {new:?}",
                    high - low
                );
            }
        }
    }
}

/// Drawn from the end of a street, a raised road or a tunnel starts at that node and the lanes
/// of the two meet there (the ramp starts on the ground). Before BUG-121 every level above 1 and
/// every tunnel was refused "too steep (12%, max 12%)": the ramp was measured along the arc of
/// the first piece, the check along its chord.
#[test]
fn a_raised_road_or_tunnel_started_at_a_street_end_joins_it() {
    for level in [Some(1u8), Some(2), Some(3), Some(4), None] {
        let (mut sim, db) = world();
        run(
            &mut sim,
            SimCommand::Net(NetCommand::BuildRoad {
                road: db.road_id(ROAD).unwrap(),
                path: RoadPath::Straight {
                    from: Vec2::new(4400.0, 4400.0),
                    to: Vec2::new(4800.0, 4400.0),
                },
                elevation: ElevationMode::Ground,
            }),
        )
        .unwrap();
        let nodes_before = sim.world().resource::<NetGraph>().nodes().count();
        let r = draw(
            &mut sim,
            &db,
            level,
            Vec2::new(4800.0, 4400.0),
            Vec2::new(5500.0, 4400.0),
        );
        r.unwrap_or_else(|e| panic!("level {level:?}: {e}"));
        let g = sim.world().resource::<NetGraph>();
        let end = g
            .nodes()
            .find(|(_, n)| (n.pos.x - 4800.0).abs() < 1.0 && (n.pos.z - 4400.0).abs() < 1.0)
            .expect("the street end");
        let kinds: Vec<_> = end
            .1
            .segments
            .iter()
            .map(|s| g.segment(*s).unwrap().elevation)
            .collect();
        println!(
            "level {level:?}: nodes {nodes_before} -> {}, at the end: {kinds:?}",
            g.nodes().count()
        );
        assert_eq!(end.1.segments.len(), 2, "level {level:?}: {kinds:?}");
        assert!(kinds.contains(&Elevation::Ground));
    }
}
