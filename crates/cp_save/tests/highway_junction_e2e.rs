//! T-078: a street meets a one-way highway at grade. The junction code needed no change for
//! the new road class: the street's lanes get links onto the highway's downstream lanes, the
//! highway's lanes get a way off onto the street, nothing runs against the highway, and the
//! rules of `system-design.md` §6.5 (priority by rank, lights by lane count) apply.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::{ContentDb, RoadDefId};
use cp_core::{CommandOutcome, Tick};
use cp_net::{
    ElevationMode, JunctionGeometry, NetCommand, NetGraph, NodeId, Priority, RoadPath, SegmentId,
    Turn,
};
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

/// A highway running +x through (5000, 5000) with a medium street ending on it from the
/// south (z = 5100 side, which is on the right of the highway's travel), built by commands.
/// Returns the 3-arm node, the highway segment that ends at it and the one that starts there,
/// and the street segment.
fn tee(
    sim: &mut SimRunner,
    db: &ContentDb,
    highway: &str,
    street_from_right: bool,
) -> (NodeId, SegmentId, SegmentId, SegmentId) {
    let hw = db.road_id(highway).unwrap();
    let st = db.road_id("medium_4l").unwrap();
    let z = if street_from_right { 5100.0 } else { 4900.0 };
    let o = run(
        sim,
        vec![
            build(hw, Vec2::new(4900.0, 5000.0), Vec2::new(5100.0, 5000.0)),
            build(st, Vec2::new(5000.0, z), Vec2::new(5000.0, 5000.0)),
        ],
    );
    assert!(o.iter().all(|o| o.result.is_ok()), "{o:?}");
    let g = sim.world().resource::<NetGraph>();
    let (node, n) = g
        .nodes()
        .find(|(_, n)| n.segments.len() == 3)
        .expect("a 3-way node");
    let (mut upstream, mut downstream, mut street) = (None, None, None);
    for &s in &n.segments {
        let seg = g.segment(s).unwrap();
        if seg.road == st {
            street = Some(s);
        } else if seg.end == node {
            upstream = Some(s);
        } else {
            downstream = Some(s);
        }
    }
    (
        node,
        upstream.unwrap(),
        downstream.unwrap(),
        street.unwrap(),
    )
}

fn seg_of(g: &NetGraph, lane: cp_net::LaneId) -> SegmentId {
    g.lane(lane).unwrap().segment
}

#[test]
fn a_street_joins_and_leaves_a_one_way_highway_at_grade() {
    for side_right in [true, false] {
        let db = content();
        let mut sim = sim_with(&db);
        let (node, up, down, street) = tee(&mut sim, &db, "highway_1w_2l", side_right);
        let g = sim.world().resource::<NetGraph>();
        let j = sim
            .world()
            .resource::<JunctionGeometry>()
            .get(node)
            .unwrap();
        let conns = &j.links.connections;
        let from_to = |a: SegmentId, b: SegmentId| -> Vec<Turn> {
            conns
                .iter()
                .filter(|c| seg_of(g, c.from) == a && seg_of(g, c.to) == b)
                .map(|c| c.turn)
                .collect()
        };
        // On the highway: the street's inbound lanes link onto the downstream lanes only.
        let join = from_to(street, down);
        assert!(!join.is_empty(), "no link from the street onto the highway");
        let want = if side_right { Turn::Right } else { Turn::Left };
        assert!(join.iter().all(|&t| t == want), "{join:?}");
        assert!(from_to(street, up).is_empty(), "never against the one-way");
        // Off the highway: a way from the upstream lanes onto the street.
        let off = from_to(up, street);
        assert!(!off.is_empty(), "no exit from the highway");
        // And the highway goes straight on.
        assert!(from_to(up, down).iter().all(|&t| t == Turn::Straight));
        assert!(!from_to(up, down).is_empty());
        // Nothing leaves the downstream lanes into the node or enters the upstream ones.
        assert!(conns.iter().all(|c| seg_of(g, c.from) != down));
        assert!(conns.iter().all(|c| seg_of(g, c.to) != up));
        // The merge conflicts with the highway's through lanes.
        assert!(!j.links.conflicts.is_empty());
    }
}

#[test]
fn priority_follows_rank_and_a_four_lane_highway_gets_a_light() {
    // 2-lane highway + 4-lane street: one big arm, no light by the rule; the highway
    // (rank 4) is the main road and the street yields.
    let db = content();
    let mut sim = sim_with(&db);
    let (node, up, down, street) = tee(&mut sim, &db, "highway_1w_2l", true);
    let g = sim.world().resource::<NetGraph>();
    let j = sim
        .world()
        .resource::<JunctionGeometry>()
        .get(node)
        .unwrap();
    assert!(j.control.light.is_none());
    for (c, p) in j.links.connections.iter().zip(&j.control.priority) {
        let from = seg_of(g, c.from);
        if from == up {
            assert_eq!(*p, Priority::Main, "{c:?}");
        }
        if from == street && seg_of(g, c.to) == down {
            assert_eq!(*p, Priority::Yield, "the merge yields: {c:?}");
        }
    }

    // 4-lane highway + 4-lane street: two big arms meet, a light is installed; no two
    // conflicting links share a phase.
    let db = content();
    let mut sim = sim_with(&db);
    let (node, ..) = tee(&mut sim, &db, "highway_1w_4l", true);
    let j = sim
        .world()
        .resource::<JunctionGeometry>()
        .get(node)
        .unwrap();
    let light = j.control.light.as_ref().expect("a traffic light");
    assert!(light.phases.len() >= 2);
    for &(a, b) in &j.links.conflicts {
        let phase = |i: u16| light.phases.iter().position(|p| p.connections.contains(&i));
        assert_ne!(
            phase(a),
            phase(b),
            "conflicting links {a} and {b} share a phase"
        );
    }
}
