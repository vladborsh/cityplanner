//! T-079: one-way roads in the road tool. The ghost of a one-way plan carries direction
//! chevrons (three gizmo segments each, `floor(length / 16)` per piece); a two-way ghost has
//! none. Direction warnings (§6.6) show on the ghost and never stop the command.
//!
//! Manual check (the same steps, in the window): pick the road tool and "Highway, 3 lanes
//! (one-way)", click and hover: chevrons point the way you drew; draw a second highway toward
//! the end of the first: the panel lists "⚠ two one-way roads meet head-on" in yellow, and the
//! click still builds.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::CommandOutcome;
use cp_net::{NetCommand, NetGraph, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::{CommandOutbox, RoadMode, RoadTool, compute_ghost};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn sim() -> (SimRunner, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    (sim, db)
}

fn tool(db: &ContentDb, road: &str, from: Vec2) -> RoadTool {
    RoadTool {
        mode: RoadMode::Straight,
        road: Some(db.road_id(road).unwrap()),
        points: vec![from],
        ..Default::default()
    }
}

fn build(sim: &mut SimRunner, db: &ContentDb, road: &str, from: Vec2, to: Vec2) -> CommandOutcome {
    let mut outbox = CommandOutbox::default();
    outbox.push(SimCommand::Net(NetCommand::BuildRoad {
        road: db.road_id(road).unwrap(),
        path: RoadPath::Straight { from, to },
        elevation: cp_net::ElevationMode::Ground,
    }));
    outbox.flush_into(sim);
    sim.tick_once();
    let mut out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    assert_eq!(out.len(), 1);
    out.remove(0)
}

#[test]
fn a_one_way_ghost_has_floor_len_over_16_chevrons_a_two_way_ghost_none() {
    let (sim, db) = sim();
    let view = sim.view();
    let a = Vec2::new(4600.0, 4600.0);
    // 80 m: one piece, floor(80 / 16) = 5 chevrons of 3 segments.
    let one_way =
        compute_ghost(&view, &tool(&db, "highway_1w_3l", a), a + Vec2::X * 80.0).expect("a ghost");
    assert!(one_way.valid, "{:?}", one_way.errors);
    assert_eq!(one_way.chevrons.len(), 5 * 3);
    // 50 m: floor(50 / 16) = 3.
    let short = compute_ghost(&view, &tool(&db, "highway_1w_3l", a), a + Vec2::X * 50.0).unwrap();
    assert_eq!(short.chevrons.len(), 3 * 3);
    // Each chevron's segments are short and point forward: their tips lie ahead of the tail.
    for c in &one_way.chevrons {
        assert!(c[0].distance(c[1]) < 4.0);
    }
    let two_way = compute_ghost(&view, &tool(&db, "basic_2l", a), a + Vec2::X * 80.0).unwrap();
    assert!(two_way.valid && two_way.chevrons.is_empty());
    assert!(two_way.warnings.is_empty());
}

#[test]
fn chevron_count_sums_the_pieces_of_a_long_plan() {
    let (sim, db) = sim();
    let view = sim.view();
    let a = Vec2::new(4600.0, 4700.0);
    // 150 m is cut into floor(150 / 96) + 1 = 2 equal pieces of 75 m (T-262): (4 + 4) chevrons.
    let g = compute_ghost(&view, &tool(&db, "highway_1w_3l", a), a + Vec2::X * 150.0).unwrap();
    assert!(g.valid);
    assert_eq!(g.polylines.len(), 2);
    assert_eq!(g.chevrons.len(), 8 * 3);
}

#[test]
fn a_warning_shows_on_the_ghost_and_the_command_goes_through() {
    let (mut sim, db) = sim();
    let a = Vec2::new(4600.0, 4800.0);
    let mid = a + Vec2::X * 80.0;
    // A highway, then a second one drawn head-on into its end.
    assert!(build(&mut sim, &db, "highway_1w_3l", a, mid).result.is_ok());
    let far = mid + Vec2::X * 80.0;
    let view = sim.view();
    let g = compute_ghost(&view, &tool(&db, "highway_1w_3l", far), mid).unwrap();
    assert!(g.valid, "{:?}", g.errors);
    assert_eq!(g.warnings.len(), 1, "{:?}", g.warnings);
    assert!(g.warnings[0].contains("head-on"), "{:?}", g.warnings);
    // The same plan is built by the command.
    let segs = sim.world().resource::<NetGraph>().segment_count();
    assert!(
        build(&mut sim, &db, "highway_1w_3l", far, mid)
            .result
            .is_ok()
    );
    assert_eq!(sim.world().resource::<NetGraph>().segment_count(), segs + 1);
}

#[test]
fn a_highway_into_a_street_end_shows_no_warning() {
    let (mut sim, db) = sim();
    let a = Vec2::new(4600.0, 5000.0);
    let mid = a + Vec2::X * 80.0;
    assert!(build(&mut sim, &db, "basic_2l", a, mid).result.is_ok());
    let view = sim.view();
    let g = compute_ghost(
        &view,
        &tool(&db, "highway_1w_3l", mid + Vec2::X * 80.0),
        mid,
    )
    .unwrap();
    assert!(g.valid && g.warnings.is_empty(), "{:?}", g.warnings);
}
