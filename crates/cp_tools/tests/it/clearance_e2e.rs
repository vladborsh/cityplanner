//! T-124: a road drawn closer than one cell (8 m between the edges) to another, side by side,
//! gets a red ghost and no command from the tool; the command itself still accepts the path.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::{ContentDb, RoadDefId};
use cp_core::CommandOutcome;
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::{CommandOutbox, RoadMode, RoadTool, TOO_CLOSE, compute_ghost, quantized, too_close};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn setup() -> (SimRunner, Arc<ContentDb>, RoadDefId) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    let road = db.road_id("basic_2l").unwrap();
    (sim, db, road)
}

fn send(sim: &mut SimRunner, road: RoadDefId, from: Vec2, to: Vec2) -> bool {
    let mut outbox = CommandOutbox::default();
    outbox.push(SimCommand::Net(NetCommand::BuildRoad {
        road,
        path: RoadPath::Straight { from, to },
        elevation: ElevationMode::Ground,
    }));
    outbox.flush_into(sim);
    sim.tick_once();
    sim.world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .all(|o| o.result.is_ok())
}

fn tool(road: RoadDefId, first: Vec2) -> RoadTool {
    let mut t = RoadTool {
        mode: RoadMode::Straight,
        road: Some(road),
        ..Default::default()
    };
    t.click(first);
    t
}

#[test]
fn a_parallel_road_within_a_cell_is_a_red_ghost_and_sends_nothing_but_the_command_accepts_it() {
    let (mut sim, db, road) = setup();
    let w = db.road(road).width;
    assert!(send(
        &mut sim,
        road,
        Vec2::new(4600.0, 4600.0),
        Vec2::new(4704.0, 4600.0)
    ));

    let z = 4600.0 + w + 6.0;
    let (from, to) = (Vec2::new(4600.0, z), Vec2::new(4704.0, z));
    let mut t = tool(road, from);
    let ghost = compute_ghost(&sim.view(), &t, to).expect("a ghost");
    assert!(!ghost.valid);
    assert_eq!(ghost.errors, vec![TOO_CLOSE.to_string()]);

    // The click: the tool would send nothing.
    let req = t.click(to).unwrap();
    let (req, _) = quantized(&sim.view(), req, false);
    assert!(too_close(&sim.view(), &req));

    // BuildRoad of the same path directly still goes through.
    let before = sim.world().resource::<NetGraph>().segment_count();
    assert!(send(&mut sim, road, from, to));
    assert!(sim.world().resource::<NetGraph>().segment_count() > before);
}

#[test]
fn with_a_full_cell_between_the_edges_the_ghost_is_valid() {
    let (mut sim, db, road) = setup();
    let w = db.road(road).width;
    assert!(send(
        &mut sim,
        road,
        Vec2::new(4600.0, 4600.0),
        Vec2::new(4704.0, 4600.0)
    ));
    let z = 4600.0 + w + 8.5;
    let t = tool(road, Vec2::new(4600.0, z));
    let ghost = compute_ghost(&sim.view(), &t, Vec2::new(4704.0, z)).expect("a ghost");
    assert!(ghost.valid, "{:?}", ghost.errors);
}
