//! T-121: `RemovePart` through `SimRunner`: the remnants keep the painted zones, the log
//! replays to the same checksum and a save with the result round-trips byte for byte.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::{ContentDb, ZoneType};
use cp_core::{CommandOutcome, Tick};
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath, SegmentId};
use cp_save::{load_into_runner, save_runner};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_zone::ZoneGrid;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

fn sim_with(content: &Arc<ContentDb>) -> SimRunner {
    let terrain = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
    let mut sim = SimRunner::new(3);
    sim.set_terrain(Arc::new(terrain));
    sim.set_content(Arc::clone(content));
    sim
}

fn run(sim: &mut SimRunner, cmd: SimCommand) -> CommandOutcome {
    let at = sim.tick();
    sim.push_command(cmd, Tick(at.0));
    sim.tick_once();
    sim.world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .next()
        .expect("an outcome")
}

fn paint_all(sim: &mut SimRunner) {
    let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
    let cells: Vec<_> = grid
        .blocks()
        .flat_map(|(id, _)| grid.block_cells(id))
        .collect();
    for c in cells {
        grid.set_zone(c, ZoneType::ResLow);
    }
}

/// A 90 m street with both sides painted, then `RemovePart(20, 60)` of its middle.
fn scenario(content: &Arc<ContentDb>) -> (SimRunner, SegmentId) {
    let road = content.road_id("basic_2l").unwrap();
    let mut sim = sim_with(content);
    let built = run(
        &mut sim,
        SimCommand::Net(NetCommand::BuildRoad {
            road,
            path: RoadPath::Straight {
                from: Vec2::new(5000.0, 5000.0),
                to: Vec2::new(5090.0, 5000.0),
            },
            elevation: ElevationMode::Ground,
        }),
    );
    assert!(built.result.is_ok());
    sim.tick_once();
    paint_all(&mut sim);
    let seg = sim
        .world()
        .resource::<NetGraph>()
        .segments()
        .next()
        .unwrap()
        .0;
    let out = run(
        &mut sim,
        SimCommand::Net(NetCommand::RemovePart {
            seg,
            s0: 20.0,
            s1: 60.0,
        }),
    );
    assert!(out.result.is_ok(), "{:?}", out.result);
    for _ in 0..3 {
        sim.tick_once();
    }
    (sim, seg)
}

#[test]
fn the_remnants_keep_their_painted_zone_type() {
    let content = content();
    let (sim, seg) = scenario(&content);
    let graph = sim.world().resource::<NetGraph>();
    assert!(graph.segment(seg).is_none());
    assert_eq!(graph.segment_count(), 2);
    let zones = sim.world().resource::<ZoneGrid>();
    assert_eq!(zones.block_count(), 4, "two sides of two remnants");
    let mut painted = 0;
    for (id, b) in zones.blocks() {
        assert!(graph.segment(b.segment).is_some());
        for c in zones.block_cells(id) {
            if zones.is_valid(c) {
                assert_eq!(zones.zone_of(c), Some(ZoneType::ResLow), "{c:?}");
                painted += 1;
            }
        }
    }
    assert!(painted > 0);
}

#[test]
fn the_log_replays_to_the_same_checksum_and_the_save_round_trips() {
    let content = content();
    let (mut a, _) = scenario(&content);
    let (mut b, _) = scenario(&content);
    assert_eq!(a.checksum(), b.checksum());

    let hash = content.content_hash();
    let bytes = save_runner(&a, hash, "flatlands").unwrap();
    let mut loaded = sim_with(&content);
    let (_, warnings) = load_into_runner(&mut loaded, &bytes, hash).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(loaded.checksum(), a.checksum());
    assert_eq!(save_runner(&loaded, hash, "flatlands").unwrap(), bytes);
    for _ in 0..20 {
        a.tick_once();
        loaded.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum());
    assert_eq!(a.checksum(), loaded.checksum());
}
