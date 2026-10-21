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
use cp_tools::{
    CommandOutbox, pick_segment, piece_preview, upgrade_commands, upgrade_piece, upgrade_preview,
};
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

fn cmds(w: &World, p: &cp_tools::UpgradePreview, target: RoadDefId) -> Vec<SimCommand> {
    upgrade_commands(w.sim.world().resource::<NetGraph>(), p, target)
}

fn preview_for(w: &World, arm: SegmentId, target: RoadDefId) -> cp_tools::UpgradePreview {
    let view = w.sim.view();
    upgrade_preview(&view.net, &view.tiles, &view.zones, &w.db, arm, target)
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
    let cmds = cmds(&w, &p, medium);
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
    let c = cmds(&w, &p, medium);
    let out = run(&mut w.sim, c);
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
    let c = cmds(&w, &p, highway);
    let out = run(&mut w.sim, c);
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
    assert!(cmds(&w, &p, basic).is_empty());
}

#[test]
fn a_run_outside_the_bought_tiles_cannot_be_upgraded() {
    let mut w = setup();
    let arm = cross(&mut w);
    let medium = w.db.road_id("medium_4l").unwrap();
    let view = w.sim.view();
    let none = cp_terrain::TileOwnership::none();
    let p = upgrade_preview(&view.net, &none, &view.zones, &w.db, arm, medium);
    assert_eq!(p.errors, vec![cp_tools::upgrade::OUTSIDE_LAND.to_string()]);
    assert!(cmds(&w, &p, medium).is_empty());
}

/// The x range of a piece of the east-west street (its segments run west to east).
fn x_range(net: &NetGraph, piece: &[cp_tools::Span]) -> (f32, f32) {
    piece
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &(id, s0, s1)| {
            let c = net.segment(id).unwrap().curve;
            let t = c.arc_table();
            let (a, b) = (c.point(t.s_to_t(s0)).x, c.point(t.s_to_t(s1)).x);
            (lo.min(a.min(b)), hi.max(a.max(b)))
        })
}

/// BUG-103: on a long street the upgrade takes only the piece around the cursor between the
/// nearest stops (here the section marks every 10 cells, 80 m), wherever on the road the cursor
/// is; Shift takes the whole street; what is highlighted is what changes.
#[test]
fn a_sloppy_cursor_on_a_long_street_upgrades_only_the_section_under_it() {
    let mut w = setup();
    let basic = w.db.road_id("basic_2l").unwrap();
    let medium = w.db.road_id("medium_4l").unwrap();
    let out = run(
        &mut w.sim,
        vec![build(basic, (4400.0, 4600.0), (4800.0, 4600.0))],
    );
    assert!(out.iter().all(|o| o.result.is_ok()), "{out:?}");
    let view = w.sim.view();
    for (x, dz) in [
        (4403.0, 2.5),
        (4455.0, -3.0),
        (4479.0, 1.0),
        (4531.0, -2.0),
        (4642.0, 3.5),
        (4797.0, -1.5),
    ] {
        let piece = upgrade_piece(&view.net, Vec2::new(x, 4600.0 + dz), 8.0, false);
        let (lo, hi) = x_range(&view.net, &piece);
        assert!(
            lo - 1e-2 <= x && x <= hi + 1e-2,
            "{x}: the piece {lo}..{hi} holds the cursor"
        );
        assert!((hi - lo - 80.0).abs() < 0.5, "{x}: one section, {lo}..{hi}");
        assert!(
            ((lo - 4400.0) / 80.0).fract().abs() < 0.01,
            "{x}: starts at a mark"
        );
    }
    let all = upgrade_piece(&view.net, Vec2::new(4531.0, 4602.0), 8.0, true);
    let (lo, hi) = x_range(&view.net, &all);
    assert!(
        lo < 4400.5 && hi > 4799.5,
        "Shift: the whole street, {lo}..{hi}"
    );

    // Click: only that section changes, for the price of its length.
    let cursor = Vec2::new(4531.0, 4602.0);
    let piece = upgrade_piece(&view.net, cursor, 8.0, false);
    let p = piece_preview(&view.net, &view.tiles, &view.zones, &w.db, &piece, medium);
    assert_eq!(p.cost, segment_cost(80.0, w.db.road(medium).cost_per_m));
    drop(view);
    let c = cmds(&w, &p, medium);
    let out = run(&mut w.sim, c);
    assert!(out.iter().all(|o| o.result.is_ok()), "{out:?}");
    let net = w.sim.world().resource::<NetGraph>();
    let (mut up, mut lo, mut hi) = (0.0, f32::MAX, f32::MIN);
    for (_, s) in net.segments() {
        if s.road == medium {
            up += s.length;
            lo = lo.min(s.curve.p0.x.min(s.curve.p3.x));
            hi = hi.max(s.curve.p0.x.max(s.curve.p3.x));
        }
    }
    assert!((up - 80.0).abs() < 0.5, "upgraded {up} m");
    assert!(
        (lo - 4480.0).abs() < 0.5 && (hi - 4560.0).abs() < 0.5,
        "{lo}..{hi}"
    );
    // Hovering the same spot now finds the new section, all of the new type.
    let again = upgrade_piece(net, cursor, 8.0, false);
    assert!(
        again
            .iter()
            .all(|s| net.segment(s.0).unwrap().road == medium)
    );
    let (a, b) = x_range(net, &again);
    assert!(
        (a - 4480.0).abs() < 0.5 && (b - 4560.0).abs() < 0.5,
        "{a}..{b}"
    );
}
