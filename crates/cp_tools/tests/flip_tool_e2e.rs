//! T-091: the flip tool. Hovering a one-way road gives its whole run and one `FlipSegment` per
//! segment; a two-way road says it cannot be flipped and sends nothing; sending the commands
//! turns the run round.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_tools::{CommandOutbox, NOT_ONE_WAY, flip_commands, flip_preview};

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

fn build(sim: &mut SimRunner, db: &ContentDb, road: &str, from: Vec2, to: Vec2) {
    let mut outbox = CommandOutbox::default();
    outbox.push(SimCommand::Net(NetCommand::BuildRoad {
        road: db.road_id(road).unwrap(),
        path: RoadPath::Straight { from, to },
        elevation: ElevationMode::Ground,
    }));
    outbox.flush_into(sim);
    sim.tick_once();
}

#[test]
fn the_flip_tool_turns_a_whole_one_way_run_and_leaves_a_two_way_road_alone() {
    let (mut sim, db) = sim();
    // A 150 m one-way highway (two pieces: a run) and a two-way street.
    build(
        &mut sim,
        &db,
        "highway_1w_2l",
        Vec2::new(4600.0, 4600.0),
        Vec2::new(4750.0, 4600.0),
    );
    build(
        &mut sim,
        &db,
        "basic_2l",
        Vec2::new(4600.0, 4700.0),
        Vec2::new(4700.0, 4700.0),
    );
    let view = sim.view();
    let content = view.content.as_deref().unwrap();
    let hw_road = db.road_id("highway_1w_2l").unwrap();
    let hw: Vec<_> = view
        .net
        .segments()
        .filter(|(_, s)| s.road == hw_road)
        .map(|(id, _)| id)
        .collect();
    assert!(hw.len() >= 2, "a run of {} piece(s)", hw.len());
    let street = view
        .net
        .segments()
        .find(|(_, s)| s.road != hw_road)
        .unwrap()
        .0;
    let before: Vec<f32> = hw
        .iter()
        .map(|&id| view.net.segment(id).unwrap().curve.p0.x)
        .collect();
    let p = flip_preview(&view.net, &view.tiles, content, hw[0]);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    assert_eq!(p.run.len(), hw.len(), "the whole run");
    let commands = flip_commands(&p);
    assert_eq!(commands.len(), hw.len());
    // A two-way street: an error and no commands.
    let q = flip_preview(&view.net, &view.tiles, content, street);
    assert_eq!(q.errors, vec![NOT_ONE_WAY.to_string()]);
    assert!(flip_commands(&q).is_empty());
    drop(view);
    let mut outbox = CommandOutbox::default();
    for c in commands {
        outbox.push(c);
    }
    outbox.flush_into(&mut sim);
    sim.tick_once();
    let net = sim.world().resource::<NetGraph>();
    for (k, &id) in hw.iter().enumerate() {
        let s = net.segment(id).unwrap();
        assert!(
            (s.curve.p3.x - before[k]).abs() < 0.5,
            "piece {k} runs the other way now"
        );
    }
}
