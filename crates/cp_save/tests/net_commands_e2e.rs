//! T-018 acceptance: road commands through `SimRunner`, determinism, rejection leaves
//! the state untouched, and the built network survives a save/load round trip.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::{ContentDb, RoadDefId};
use cp_core::{CommandOutcome, Tick};
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath, SegmentId};
use cp_save::{load_into_runner, save_runner};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

fn sim_with(content: &Arc<ContentDb>) -> SimRunner {
    let terrain = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
    let mut sim = SimRunner::new(5);
    sim.set_terrain(Arc::new(terrain));
    sim.set_content(Arc::clone(content));
    sim
}

fn build(road: RoadDefId, from: Vec2, to: Vec2) -> SimCommand {
    SimCommand::Net(NetCommand::BuildRoad {
        road,
        path: RoadPath::Straight { from, to },
        elevation: ElevationMode::Ground,
    })
}

/// 5 horizontal and 5 vertical streets, 100 m apart, crossing in a 5x5 lattice.
fn grid_commands(road: RoadDefId) -> Vec<SimCommand> {
    let (x0, z0) = (4600.0, 4600.0);
    let mut v = Vec::new();
    for i in 0..5 {
        let o = i as f32 * 100.0;
        v.push(build(
            road,
            Vec2::new(x0, z0 + o),
            Vec2::new(x0 + 400.0, z0 + o),
        ));
    }
    for i in 0..5 {
        let o = i as f32 * 100.0;
        v.push(build(
            road,
            Vec2::new(x0 + o, z0),
            Vec2::new(x0 + o, z0 + 400.0),
        ));
    }
    v
}

/// Queues `cmds` one per tick starting at tick 1, runs them and returns the outcomes.
fn run(sim: &mut SimRunner, cmds: Vec<SimCommand>) -> Vec<CommandOutcome> {
    let start = sim.tick().0;
    let n = cmds.len() as u64;
    for (i, c) in cmds.into_iter().enumerate() {
        sim.push_command(c, Tick(start + i as u64));
    }
    // The outcome buffer is cleared as ticks advance: collect after every tick.
    let mut out = Vec::new();
    for _ in 0..n + 1 {
        sim.tick_once();
        out.extend(
            sim.world_mut()
                .resource_mut::<Messages<CommandOutcome>>()
                .drain(),
        );
    }
    out
}

fn graph(sim: &SimRunner) -> NetGraph {
    sim.world().resource::<NetGraph>().clone()
}

fn first_segment(g: &NetGraph) -> SegmentId {
    g.segments().next().unwrap().0
}

/// Builds the lattice, upgrades one segment, removes another.
fn full_scenario(content: &Arc<ContentDb>) -> SimRunner {
    let basic = content.road_id("basic_2l").unwrap();
    let medium = content.road_id("medium_4l").unwrap();
    let mut sim = sim_with(content);
    let outcomes = run(&mut sim, grid_commands(basic));
    assert_eq!(outcomes.len(), 10);
    for o in &outcomes {
        assert!(o.result.is_ok(), "{o:?}");
    }
    let g = graph(&sim);
    let seg = first_segment(&g);
    let other = g.segments().nth(5).unwrap().0;
    let out = run(
        &mut sim,
        vec![
            SimCommand::Net(NetCommand::UpgradeSegment { seg, road: medium }),
            SimCommand::Net(NetCommand::RemoveSegment { seg: other }),
        ],
    );
    assert!(out.iter().all(|o| o.result.is_ok()), "{out:?}");
    sim
}

#[test]
fn builds_a_5x5_grid_then_upgrades_and_removes() {
    let content = content();
    let basic = content.road_id("basic_2l").unwrap();
    let medium = content.road_id("medium_4l").unwrap();
    let mut sim = sim_with(&content);
    let outcomes = run(&mut sim, grid_commands(basic));
    let total: i64 = outcomes.iter().map(|o| o.result.clone().unwrap().0).sum();
    assert!(total > 0, "building costs money");

    let g = graph(&sim);
    // All 25 lattice points are nodes; the 9 interior ones are 4-way junctions.
    let mut junctions = 0;
    for ix in 0..5 {
        for iz in 0..5 {
            let p = Vec2::new(4600.0 + ix as f32 * 100.0, 4600.0 + iz as f32 * 100.0);
            let (_, n) = g
                .nodes()
                .find(|(_, n)| (Vec2::new(n.pos.x, n.pos.z) - p).length() < 0.5)
                .unwrap_or_else(|| panic!("no node at {p}"));
            if (1..4).contains(&ix) && (1..4).contains(&iz) {
                assert_eq!(n.segments.len(), 4, "{p}");
                junctions += 1;
            }
        }
    }
    assert_eq!(junctions, 9);
    let (segs, nodes) = (g.segment_count(), g.node_count());
    // Pieces are at most 96 m, so every 100 m block is cut at least once.
    assert!(segs >= 40, "{segs}");
    assert!(g.dirty_nodes().count() >= 25, "junction recompute input");

    let seg = first_segment(&g);
    let lanes_before = g.segment(seg).unwrap().lanes.len();
    let cost = run(
        &mut sim,
        vec![SimCommand::Net(NetCommand::UpgradeSegment {
            seg,
            road: medium,
        })],
    )[0]
    .result
    .clone()
    .unwrap();
    assert!(cost.0 > 0);
    let g2 = graph(&sim);
    let s = g2.segment(seg).unwrap();
    assert_eq!(s.road, medium);
    assert!(s.lanes.len() > lanes_before, "4-lane road has more lanes");
    assert_eq!(g2.segment_count(), segs);
    assert_eq!(g2.node_count(), nodes);

    let victim = g2.segments().nth(7).unwrap().0;
    let out = run(
        &mut sim,
        vec![SimCommand::Net(NetCommand::RemoveSegment { seg: victim })],
    );
    assert_eq!(out[0].result.clone().unwrap().0, 0);
    let g3 = graph(&sim);
    assert!(g3.segment(victim).is_none());
    assert_eq!(g3.segment_count(), segs - 1);
}

#[test]
fn the_same_commands_give_the_same_checksum() {
    let content = content();
    let (mut a, mut b) = (full_scenario(&content), full_scenario(&content));
    assert_eq!(a.checksum(), b.checksum());
    assert_eq!(graph(&a), graph(&b));
}

#[test]
fn invalid_commands_are_rejected_and_change_nothing() {
    let content = content();
    let basic = content.road_id("basic_2l").unwrap();
    let mut sim = sim_with(&content);
    run(&mut sim, grid_commands(basic));
    let before_graph = graph(&sim);
    let before = sim.checksum();

    let bad = [
        // Outside the owned 3x3 tiles.
        NetCommand::BuildRoad {
            road: basic,
            path: RoadPath::Straight {
                from: Vec2::new(100.0, 100.0),
                to: Vec2::new(300.0, 100.0),
            },
            elevation: ElevationMode::Ground,
        },
        // Too short.
        NetCommand::BuildRoad {
            road: basic,
            path: RoadPath::Straight {
                from: Vec2::new(6000.0, 6000.0),
                to: Vec2::new(6002.0, 6000.0),
            },
            elevation: ElevationMode::Ground,
        },
        NetCommand::UpgradeSegment {
            seg: first_segment(&before_graph),
            road: basic,
        },
        NetCommand::RemoveSegment {
            seg: SegmentId::new(60_000, 0),
        },
    ];
    for cmd in bad {
        let err = cp_net::apply(sim.world_mut(), &cmd).unwrap_err();
        assert!(!err.to_string().is_empty());
        assert_eq!(sim.checksum(), before, "{cmd:?}");
        assert_eq!(graph(&sim), before_graph, "{cmd:?}");
    }

    // Through the dispatcher: Err outcome, nothing logged, state unchanged.
    let log_len = sim
        .world()
        .resource::<cp_core::CommandLog<SimCommand>>()
        .len();
    let out = run(
        &mut sim,
        vec![SimCommand::Net(NetCommand::RemoveSegment {
            seg: SegmentId::new(60_000, 0),
        })],
    );
    assert!(out[0].result.is_err());
    assert_eq!(
        sim.world()
            .resource::<cp_core::CommandLog<SimCommand>>()
            .len(),
        log_len
    );
    assert_eq!(graph(&sim), before_graph);
}

#[test]
fn a_sim_without_terrain_or_content_rejects_instead_of_panicking() {
    let basic = RoadDefId(0);
    let mut sim = SimRunner::new(1);
    let cmd = NetCommand::BuildRoad {
        road: basic,
        path: RoadPath::Straight {
            from: Vec2::new(5000.0, 5000.0),
            to: Vec2::new(5100.0, 5000.0),
        },
        elevation: ElevationMode::Ground,
    };
    assert!(cp_net::apply(sim.world_mut(), &cmd).is_err());
    sim.set_content(content());
    assert!(cp_net::apply(sim.world_mut(), &cmd).is_err()); // still no terrain
}

#[test]
fn the_built_network_survives_save_and_load() {
    let content = content();
    let mut sim = full_scenario(&content);
    let hash = content.content_hash();
    let bytes = save_runner(&sim, hash, "flatlands").unwrap();

    let mut loaded = sim_with(&content);
    let (_, warnings) = load_into_runner(&mut loaded, &bytes, hash).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(graph(&loaded), graph(&sim));
    assert_eq!(loaded.checksum(), sim.checksum());

    // Both keep going identically, including a further command on the loaded network.
    let seg = first_segment(&graph(&sim));
    let cmd = SimCommand::Net(NetCommand::RemoveSegment { seg });
    run(&mut sim, vec![cmd.clone()]);
    run(&mut loaded, vec![cmd]);
    assert_eq!(loaded.checksum(), sim.checksum());
}
