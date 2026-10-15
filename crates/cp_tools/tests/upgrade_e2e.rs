//! T-081 headless: the upgrade tool's preview and commands on a small street cross.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::{ContentDb, RoadDefId, ZoneType};
use cp_core::{CommandOutcome, Cost};
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath, SegmentId, segment_cost};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::{CommandOutbox, pick_segment, upgrade_commands, upgrade_preview};
use cp_zone::{ZoneCellRef, ZoneCommand, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

struct World {
    sim: SimRunner,
    db: Arc<ContentDb>,
}

fn setup() -> World {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let terrain = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::new(terrain));
    sim.set_content(db.clone());
    World { sim, db }
}

fn run(sim: &mut SimRunner, cmds: Vec<SimCommand>) -> Vec<CommandOutcome> {
    let mut outbox = CommandOutbox::default();
    for c in cmds {
        outbox.push(c);
    }
    outbox.flush_into(sim);
    sim.tick_once();
    sim.world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect()
}

fn build(road: RoadDefId, a: (f32, f32), b: (f32, f32)) -> SimCommand {
    SimCommand::Net(NetCommand::BuildRoad {
        road,
        path: RoadPath::Straight {
            from: Vec2::new(a.0, a.1),
            to: Vec2::new(b.0, b.1),
        },
        elevation: ElevationMode::Ground,
    })
}

/// A cross of two basic_2l streets: four arms (each cut into several segments) meeting in one
/// junction. Returns a segment of the west arm.
fn cross(w: &mut World) -> SegmentId {
    let basic = w.db.road_id("basic_2l").unwrap();
    let out = run(
        &mut w.sim,
        vec![
            build(basic, (4400.0, 4600.0), (4800.0, 4600.0)),
            build(basic, (4600.0, 4400.0), (4600.0, 4800.0)),
        ],
    );
    assert!(out.iter().all(|o| o.result.is_ok()), "{out:?}");
    let view = w.sim.view();
    let arm = pick_segment(&view.net, Vec2::new(4500.0, 4600.0), 8.0).unwrap();
    assert!(view.net.segments().count() >= 4);
    arm
}

fn paint_all(w: &mut World) -> usize {
    let cells: Vec<ZoneCellRef> = {
        let z = w.sim.world().resource::<ZoneGrid>();
        z.blocks()
            .flat_map(|(id, _)| z.block_cells(id))
            .filter(|c| z.is_valid(*c))
            .collect()
    };
    run(
        &mut w.sim,
        vec![SimCommand::Zone(ZoneCommand::PaintZones {
            cells,
            zone: ZoneType::ResLow,
        })],
    );
    painted(&w.sim)
}

fn painted(sim: &SimRunner) -> usize {
    sim.world()
        .resource::<ZoneGrid>()
        .blocks()
        .map(|(_, b)| b.zones.iter().filter(|z| **z == ZoneType::ResLow).count())
        .sum()
}

fn preview_for(w: &World, arm: SegmentId, target: RoadDefId) -> cp_tools::UpgradePreview {
    let view = w.sim.view();
    upgrade_preview(&view.net, &view.zones, &w.db, arm, target)
}

#[test]
fn hovering_gives_the_run_and_the_price_and_a_click_sends_one_command_per_segment() {
    let mut w = setup();
    let arm = cross(&mut w);
    let medium = w.db.road_id("medium_4l").unwrap();
    let p = preview_for(&w, arm, medium);
    assert!(
        p.run.len() > 1 && p.run.contains(&arm),
        "the whole arm is one run"
    );
    assert_eq!(p.changes, p.run);
    let net = w.sim.world().resource::<NetGraph>();
    let want = p.run.iter().fold(Cost::ZERO, |acc, id| {
        let len = net.segment(*id).unwrap().length;
        Cost(acc.0 + segment_cost(len, w.db.road(medium).cost_per_m).0)
    });
    assert_eq!(p.cost, want);
    assert!(p.cost > Cost::ZERO && p.warning().is_none() && p.errors.is_empty());
    let cmds = upgrade_commands(&p, medium);
    assert_eq!(cmds.len(), p.changes.len());
    assert!(cmds.iter().all(|c| matches!(
        c,
        SimCommand::Net(NetCommand::UpgradeSegment { road, .. }) if *road == medium
    )));
}

#[test]
fn upgrading_to_a_zoned_type_keeps_the_zoning() {
    let mut w = setup();
    let arm = cross(&mut w);
    let before = paint_all(&mut w);
    assert!(before > 0);
    let medium = w.db.road_id("medium_4l").unwrap();
    let p = preview_for(&w, arm, medium);
    let out = run(&mut w.sim, upgrade_commands(&p, medium));
    assert!(out.iter().all(|o| o.result.is_ok()), "{out:?}");
    let net = w.sim.world().resource::<NetGraph>();
    assert!(
        p.run
            .iter()
            .all(|id| net.segment(*id).unwrap().road == medium)
    );
    assert!(painted(&w.sim) > 0, "zoning kept (BUG-015)");
}

#[test]
fn upgrading_to_a_highway_warns_first_and_then_removes_the_blocks() {
    let mut w = setup();
    let arm = cross(&mut w);
    paint_all(&mut w);
    // A building covers some cells of the arm's blocks.
    let covered: Vec<ZoneCellRef> = {
        let z = w.sim.world().resource::<ZoneGrid>();
        z.blocks_of(arm)
            .into_iter()
            .flatten()
            .flat_map(|b| z.block_cells(b))
            .take(3)
            .collect()
    };
    assert_eq!(covered.len(), 3);
    w.sim
        .world_mut()
        .resource_mut::<ZoneGrid>()
        .occupy(&covered);
    w.sim.tick_once();
    let highway = w.db.road_id("highway_1w_2l").unwrap();
    let p = preview_for(&w, arm, highway);
    // The three we occupied, plus any building the sim itself put on a painted cell.
    assert!(p.dropped_building_cells >= 3, "{p:?}");
    assert!(p.run.len() > 1);
    assert!(p.warning().unwrap().contains("NO_ROAD"));
    let out = run(&mut w.sim, upgrade_commands(&p, highway));
    assert!(out.iter().all(|o| o.result.is_ok()), "{out:?}");
    let z = w.sim.world().resource::<ZoneGrid>();
    assert!(
        z.blocks_of(arm).iter().all(Option::is_none),
        "blocks removed"
    );
}

#[test]
fn a_segment_of_the_target_type_sends_nothing() {
    let mut w = setup();
    let arm = cross(&mut w);
    let basic = w.db.road_id("basic_2l").unwrap();
    let p = preview_for(&w, arm, basic);
    assert!(p.is_noop() && p.cost == Cost::ZERO);
    assert!(upgrade_commands(&p, basic).is_empty());
}
