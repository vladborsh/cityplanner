//! T-091: `FlipSegment` turns a one-way road round. The lane connections at both ends are made
//! again, so a route that was impossible before the flip exists after it; a two-way road is
//! refused; the result is deterministic (the same commands twice give one checksum).

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::CommandOutcome;
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath, SegmentId};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_traffic::{Access, PathNetwork, Side, find_car_path};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn sim() -> (SimRunner, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    sim.world_mut().insert_resource(TileOwnership::only((2, 2)));
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    sim.tick_once();
    (sim, db)
}

fn send(sim: &mut SimRunner, cmd: SimCommand) -> CommandOutcome {
    sim.push_command(cmd, sim.tick());
    sim.tick_once();
    let mut out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    sim.tick_once();
    out.pop().expect("an outcome")
}

fn build(sim: &mut SimRunner, db: &ContentDb, road: &str, from: Vec2, to: Vec2) {
    let o = send(
        sim,
        SimCommand::Net(NetCommand::BuildRoad {
            road: db.road_id(road).unwrap(),
            path: RoadPath::Straight { from, to },
            elevation: ElevationMode::Ground,
        }),
    );
    assert!(o.result.is_ok(), "{:?}", o.result);
}

/// A one-way highway A -> N and a two-way street N -> C from the same node.
fn town(sim: &mut SimRunner, db: &ContentDb) -> (SegmentId, SegmentId) {
    let (a, n, c) = (
        Vec2::new(4600.0, 4600.0),
        Vec2::new(4680.0, 4600.0),
        Vec2::new(4680.0, 4680.0),
    );
    build(sim, db, "highway_1w_2l", a, n);
    build(sim, db, "basic_2l", n, c);
    let net = sim.world().resource::<NetGraph>();
    let find = |name: &str| {
        let road = db.road_id(name).unwrap();
        net.segments().find(|(_, s)| s.road == road).unwrap().0
    };
    (find("highway_1w_2l"), find("basic_2l"))
}

fn route(sim: &SimRunner, from: Access, to: Access) -> bool {
    let pnet = sim.world().resource::<PathNetwork>();
    find_car_path(pnet.graph(), &Default::default(), &from, &to).is_ok()
}

fn at(segment: SegmentId, s: f32) -> Access {
    Access {
        segment,
        s,
        side: Side::Right,
    }
}

#[test]
fn a_flipped_one_way_road_opens_the_route_that_was_closed_and_closes_the_other() {
    let (mut sim, db) = sim();
    let (hw, st) = town(&mut sim, &db);
    let (hw_len, st_len) = {
        let net = sim.world().resource::<NetGraph>();
        (
            net.segment(hw).unwrap().length,
            net.segment(st).unwrap().length,
        )
    };
    // Where the highway starts and where the street ends. The highway segment was built from A
    // to N: its start is A. (Coordinates, not ids: the highway runs A -> N.)
    let from_street = at(st, st_len - 5.0);
    let to_highway_start = at(hw, 5.0);
    let from_highway_start = at(hw, 5.0);
    let to_street = at(st, st_len - 5.0);
    // Before: A -> street works (with the lanes), street -> A does not.
    assert!(route(&sim, from_highway_start, to_street));
    assert!(!route(&sim, from_street, to_highway_start));
    let o = send(
        &mut sim,
        SimCommand::Net(NetCommand::FlipSegment { seg: hw }),
    );
    assert!(o.result.is_ok(), "{:?}", o.result);
    // The segment keeps its place and type, now running the other way: its start is where its
    // end was.
    {
        let net = sim.world().resource::<NetGraph>();
        let s = net.segment(hw).unwrap();
        assert!((s.length - hw_len).abs() < 0.01);
        assert!(s.curve.p0.x > s.curve.p3.x, "now runs east to west");
    }
    // After: from the street one can drive to the highway's (old) start, not the other way.
    // The old start is the new end: s from the new start is `length - 5`.
    let to_old_start = at(hw, hw_len - 5.0);
    let from_old_start = at(hw, hw_len - 5.0);
    assert!(route(&sim, from_street, to_old_start));
    assert!(!route(&sim, from_old_start, to_street));
}

#[test]
fn a_two_way_road_cannot_be_flipped_and_the_flip_is_deterministic() {
    let (mut sim, db) = sim();
    let (_, st) = town(&mut sim, &db);
    let o = send(
        &mut sim,
        SimCommand::Net(NetCommand::FlipSegment { seg: st }),
    );
    assert!(o.result.is_err(), "a two-way street is refused");
    assert!(
        o.result
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("one-way"),
        "{:?}",
        o.result
    );
    let run = || {
        let mut sim = sim_with_flip();
        for _ in 0..50 {
            sim.tick_once();
        }
        sim.checksum()
    };
    // The same commands in the same order, run twice, give the same state.
    assert_eq!(run(), run());
}

fn sim_with_flip() -> SimRunner {
    let (mut sim, db) = sim();
    let (hw, _) = town(&mut sim, &db);
    let o = send(
        &mut sim,
        SimCommand::Net(NetCommand::FlipSegment { seg: hw }),
    );
    assert!(o.result.is_ok());
    sim
}
