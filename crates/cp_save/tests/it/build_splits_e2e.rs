//! BUG-011: a long street is cut at the crossings first and only then into pieces of at most
//! 96 m, so no automatic cut lands a few metres from a crossing and leaves a piece under 8 m.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::{ContentDb, RoadDefId};
use cp_core::{CommandOutcome, Tick};
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn setup() -> (SimRunner, RoadDefId) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let terrain = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
    let road = db.road_id("basic_2l").unwrap();
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::new(terrain));
    sim.set_content(db);
    (sim, road)
}

fn build(road: RoadDefId, from: (f32, f32), to: (f32, f32)) -> SimCommand {
    SimCommand::Net(NetCommand::BuildRoad {
        road,
        path: RoadPath::Straight {
            from: Vec2::new(from.0, from.1),
            to: Vec2::new(to.0, to.1),
        },
        elevation: ElevationMode::Ground,
    })
}

/// Runs `cmds` one per tick and returns the outcomes in order.
fn run(sim: &mut SimRunner, cmds: Vec<SimCommand>) -> Vec<CommandOutcome> {
    let start = sim.tick().0;
    let n = cmds.len() as u64;
    for (i, c) in cmds.into_iter().enumerate() {
        sim.push_command(c, Tick(start + i as u64));
    }
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

fn assert_ok(out: &[CommandOutcome]) {
    for o in out {
        assert!(o.result.is_ok(), "{:?}", o.result);
    }
}

/// Every segment's length is within the build limits.
fn assert_piece_lengths(sim: &SimRunner) {
    for (id, s) in sim.world().resource::<NetGraph>().segments() {
        assert!(
            (8.0 - 1e-3..=96.0 + 1e-3).contains(&s.length),
            "{id:?} is {} m",
            s.length
        );
    }
}

fn has_node_at(sim: &SimRunner, x: f32, z: f32) -> bool {
    sim.world()
        .resource::<NetGraph>()
        .nodes()
        .any(|(_, n)| (n.pos.x - x).abs() < 0.6 && (n.pos.z - z).abs() < 0.6)
}

#[test]
fn single_crossing_with_104_and_108_metre_streets_is_built() {
    for len in [100.0f32, 104.0, 108.0, 120.0] {
        let (mut sim, road) = setup();
        // A cross street at x = 3000, then the long street over it (crossing at s = 50).
        let cmds = vec![
            build(road, (3000.0, 2955.0), (3000.0, 3045.0)),
            build(road, (2950.0, 3000.0), (2950.0 + len, 3000.0)),
        ];
        let out = run(&mut sim, cmds);
        assert_ok(&out);
        assert_piece_lengths(&sim);
        assert!(
            has_node_at(&sim, 3000.0, 3000.0),
            "len {len}: node at the crossing"
        );
    }
}

fn lattice(road: RoadDefId) -> Vec<SimCommand> {
    (0..30)
        .map(|k| {
            let x = 3000.0 + 100.0 * k as f32;
            build(road, (x, 2955.0), (x, 3045.0))
        })
        .collect()
}

#[test]
fn a_street_through_a_lattice_of_cross_streets_is_built_for_any_length() {
    for end_x in [4100.0f32, 5850.0, 3999.0, 4101.0, 4890.0] {
        let (mut sim, road) = setup();
        let mut cmds = lattice(road);
        cmds.push(build(road, (2950.0, 3000.0), (end_x, 3000.0)));
        let out = run(&mut sim, cmds);
        assert_ok(&out);
        assert_piece_lengths(&sim);
        let mut x = 3000.0;
        while x < end_x - 1.0 {
            assert!(
                has_node_at(&sim, x, 3000.0),
                "end {end_x}: node at crossing x = {x}"
            );
            x += 100.0;
        }
    }
}

#[test]
fn building_is_deterministic() {
    let digest = || {
        let (mut sim, road) = setup();
        let mut cmds = lattice(road);
        cmds.push(build(road, (2950.0, 3000.0), (5850.0, 3000.0)));
        run(&mut sim, cmds);
        sim.checksum()
    };
    assert_eq!(digest(), digest());
}

#[test]
fn a_crossing_really_closer_than_eight_metres_to_the_end_is_still_rejected() {
    let (mut sim, road) = setup();
    let cmds = vec![
        build(road, (3000.0, 2955.0), (3000.0, 3045.0)),
        // Ends 7 m past the cross street's axis (outside the 6 m segment snap radius, so the end
        // stays free): the last piece would be 7 m long, which is genuinely too short.
        build(road, (2900.0, 3000.0), (3007.0, 3000.0)),
    ];
    let out = run(&mut sim, cmds);
    assert!(out[0].result.is_ok());
    assert!(out[1].result.is_err(), "{:?}", out[1].result);
}
