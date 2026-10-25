//! T-027 headless: the zoning tools paint through `PaintZones` via the command layer, painted
//! types follow the road when it is split, and they are saved and loaded.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::{ContentDb, RoadDefId, ZoneType};
use cp_core::{CommandError, CommandOutcome};
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_save::{load_into_runner, save_runner};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::{CommandOutbox, ZoneTool, ZoneToolKind};
use cp_zone::{ZoneCommand, ZoneGrid};

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

fn painted(sim: &SimRunner, zone: ZoneType) -> usize {
    sim.world()
        .resource::<ZoneGrid>()
        .blocks()
        .map(|(_, b)| b.zones.iter().filter(|z| **z == zone).count())
        .sum()
}

/// A tool click on the first cell of the first block, as a `PaintZones` command.
fn click(sim: &SimRunner, tool: &mut ZoneTool, at: Vec2) -> Option<SimCommand> {
    let view = sim.view();
    tool.click(&view.zones, at)
        .map(|(cells, zone)| SimCommand::Zone(ZoneCommand::PaintZones { cells, zone }))
}

/// Centre of the first valid cell on the +Z side of the road along z = 4600.
fn a_cell_centre(sim: &SimRunner) -> Vec2 {
    let z = sim.world().resource::<ZoneGrid>();
    z.blocks()
        .flat_map(|(id, b)| {
            (0..b.cell_count() as u16).map(move |c| cp_zone::ZoneCellRef { block: id, cell: c })
        })
        .filter(|c| z.is_valid(*c))
        .map(|c| z.cell_footprint(c).unwrap().c)
        .find(|p| p.y > 4600.0)
        .expect("a cell beside the road")
}

#[test]
fn fill_block_and_brush_paint_through_the_command_layer() {
    let (mut sim, road) = setup();
    let out = run(
        &mut sim,
        vec![build(road, (4600.0, 4600.0), (4690.0, 4600.0))],
    );
    assert!(out[0].result.is_ok());
    assert!(sim.world().resource::<ZoneGrid>().block_count() > 0);
    let at = a_cell_centre(&sim);

    // Fill block.
    let before = sim.view().zones;
    let mut fill = ZoneTool {
        kind: ZoneToolKind::FillBlock,
        zone: ZoneType::ResHigh,
        ..Default::default()
    };
    let cmd = click(&sim, &mut fill, at).expect("a block under the cursor");
    let out = run(&mut sim, vec![cmd]);
    assert!(out[0].result.is_ok(), "{:?}", out[0].result);
    let block_cells = sim.world().resource::<ZoneGrid>().valid_cells() / 2; // one side of one road
    assert!(
        painted(&sim, ZoneType::ResHigh) >= block_cells / 2 && painted(&sim, ZoneType::ResHigh) > 0
    );
    // The view snapshot reflects it, and unchanged ticks reuse the snapshot.
    let after = sim.view().zones;
    assert!(!Arc::ptr_eq(&before, &after));
    sim.tick_once();
    assert!(
        Arc::ptr_eq(&after, &sim.view().zones),
        "snapshot cached while nothing changed"
    );

    // Painting the same again is rejected (nothing to paint) and changes nothing; the tool
    // does not even offer the cells.
    assert!(click(&sim, &mut fill, at).is_none());

    // A brush of another type repaints cells under it.
    let mut brush = ZoneTool {
        kind: ZoneToolKind::BrushLarge,
        zone: ZoneType::Office,
        ..Default::default()
    };
    let cmd = click(&sim, &mut brush, at).unwrap();
    let n = painted(&sim, ZoneType::Office);
    assert!(run(&mut sim, vec![cmd])[0].result.is_ok());
    assert!(painted(&sim, ZoneType::Office) > n);

    // Eraser clears them again.
    let mut eraser = ZoneTool {
        kind: ZoneToolKind::Eraser,
        ..Default::default()
    };
    let cmd = click(&sim, &mut eraser, at).unwrap();
    assert!(run(&mut sim, vec![cmd])[0].result.is_ok());
}

#[test]
fn a_command_that_changes_nothing_is_rejected_with_a_reason() {
    let (mut sim, road) = setup();
    run(
        &mut sim,
        vec![build(road, (4600.0, 4600.0), (4690.0, 4600.0))],
    );
    let at = a_cell_centre(&sim);
    let mut tool = ZoneTool {
        kind: ZoneToolKind::FillBlock,
        zone: ZoneType::ComLow,
        ..Default::default()
    };
    let cmd = click(&sim, &mut tool, at).unwrap();
    assert!(run(&mut sim, vec![cmd.clone()])[0].result.is_ok());
    // Sending the same command again (a stale click) is rejected with the zone error text.
    let out = run(&mut sim, vec![cmd]);
    match &out[0].result {
        Err(CommandError::Rejected { domain, reason }) => {
            assert_eq!(*domain, "zone");
            assert!(reason.contains("nothing to paint"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn rectangle_paints_the_cells_between_two_corners() {
    let (mut sim, road) = setup();
    run(
        &mut sim,
        vec![build(road, (4600.0, 4600.0), (4696.0, 4600.0))],
    );
    let a = a_cell_centre(&sim);
    let b = a + Vec2::new(40.0, 20.0);
    let mut rect = ZoneTool {
        kind: ZoneToolKind::Rectangle,
        zone: ZoneType::Industrial,
        ..Default::default()
    };
    assert!(click(&sim, &mut rect, a).is_none());
    let cmd = click(&sim, &mut rect, b).expect("second corner paints");
    assert!(run(&mut sim, vec![cmd])[0].result.is_ok());
    assert!(painted(&sim, ZoneType::Industrial) >= 4);
}

#[test]
fn painted_zones_survive_a_road_split_and_a_save_load() {
    let (mut sim, road) = setup();
    run(
        &mut sim,
        vec![build(road, (4600.0, 4600.0), (4696.0, 4600.0))],
    );
    let at = a_cell_centre(&sim);
    let mut fill = ZoneTool {
        kind: ZoneToolKind::FillBlock,
        zone: ZoneType::ResLow,
        ..Default::default()
    };
    let cmd = click(&sim, &mut fill, at).unwrap();
    run(&mut sim, vec![cmd]);
    let before = painted(&sim, ZoneType::ResLow);
    assert!(before > 0);

    // A street ending on the axis (segment snap) splits the painted road.
    let out = run(
        &mut sim,
        vec![build(road, (4650.0, 4500.0), (4650.0, 4597.0))],
    );
    assert!(out[0].result.is_ok(), "{:?}", out[0].result);
    let after = painted(&sim, ZoneType::ResLow);
    assert!(
        after * 10 >= before * 6,
        "painted cells {before} -> {after} after the split"
    );

    // Save and load into a fresh runner: same grid, same checksum.
    let bytes = save_runner(&sim, 7, "m").unwrap();
    let mut fresh = SimRunner::new(9);
    load_into_runner(&mut fresh, &bytes, 7).unwrap();
    assert_eq!(
        fresh.world().resource::<ZoneGrid>(),
        sim.world().resource::<ZoneGrid>()
    );
    assert_eq!(painted(&fresh, ZoneType::ResLow), after);
    assert_eq!(fresh.checksum(), sim.checksum());
}

/// BUG-093: a city whose streets run at `deg` to the map: two blocks of 18 x 8 cells between
/// 2-cell streets, one above the other. Returns the sim, the city's corner and its axes.
fn rotated_city(deg: f32) -> (SimRunner, Vec2, Vec2, Vec2) {
    let (mut sim, road) = setup();
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance =
        cp_core::Cost::from_units(1_000_000_000.0);
    let o = Vec2::new(4400.0, 4400.0);
    let u = Vec2::from_angle(deg.to_radians());
    let v = u.perp();
    let at = |x: f32, y: f32| {
        let p = o + u * x + v * y;
        (p.x, p.y)
    };
    let mut cmds = Vec::new();
    for y in [0.0, 80.0, 160.0] {
        cmds.push(build(road, at(0.0, y), at(160.0, y)));
    }
    for x in [0.0, 160.0] {
        cmds.push(build(road, at(x, 0.0), at(x, 80.0)));
        cmds.push(build(road, at(x, 80.0), at(x, 160.0)));
    }
    for c in cmds {
        let out = run(&mut sim, vec![c]);
        assert!(out.iter().all(|o| o.result.is_ok()), "{deg}°: {out:?}");
    }
    (sim, o, u, v)
}

/// BUG-093: the rectangle is square to the block under its first click, whatever angle the
/// streets have on the map: corner to corner it paints the whole block, 3 x 5 cells paint 15,
/// and across a street it takes whole rows of the parallel block beyond. Clicks land anywhere
/// in their cells; the preview is what the click paints.
#[test]
fn the_rectangle_follows_the_blocks_cells_at_any_angle() {
    for deg in [0.0f32, 23.0, 45.0, 67.0] {
        let (sim, o, u, v) = rotated_city(deg);
        let view = sim.view();
        let zones = &view.zones;
        let world = |x: f32, y: f32| o + u * x + v * y;
        // Valid cells whose centre lies in `lo..=hi` of the city's frame.
        let inside = |lo: Vec2, hi: Vec2| -> BTreeSet<cp_zone::ZoneCellRef> {
            zones
                .blocks()
                .flat_map(|(id, b)| {
                    (0..b.cell_count() as u16)
                        .map(move |cell| cp_zone::ZoneCellRef { block: id, cell })
                })
                .filter(|c| zones.is_valid(*c))
                .filter(|c| {
                    let p = zones.cell_footprint(*c).unwrap().c - o;
                    let q = Vec2::new(p.dot(u), p.dot(v));
                    q.cmpge(lo - 0.5).all() && q.cmple(hi + 0.5).all()
                })
                .collect()
        };
        let paint = |a: Vec2, b: Vec2| -> BTreeSet<cp_zone::ZoneCellRef> {
            let mut t = ZoneTool {
                kind: ZoneToolKind::Rectangle,
                zone: ZoneType::ResLow,
                ..Default::default()
            };
            assert!(t.click(zones, a).is_none());
            let shown: BTreeSet<_> = t.preview(zones, b).into_iter().collect();
            let done: BTreeSet<_> = t
                .click(zones, b)
                .map(|(c, _)| c.into_iter().collect())
                .unwrap_or_default();
            assert_eq!(shown, done, "{deg}°: the preview is what the click paints");
            done
        };
        // The lower block: cell centres 12..=148 along, 12..=68 across.
        let block = inside(Vec2::new(12.0, 12.0), Vec2::new(148.0, 68.0));
        assert_eq!(
            block.len(),
            18 * 8,
            "{deg}°: the block is 18 x 8 whole cells"
        );
        // Corner to corner, clicks off the centres (inward, outward), either way round.
        for (a, b) in [
            (world(10.0, 14.5), world(150.5, 66.0)),
            (world(14.0, 9.5), world(146.0, 70.5)),
            (world(149.0, 69.0), world(11.0, 11.0)),
        ] {
            let got = paint(a, b);
            let local = |c: &cp_zone::ZoneCellRef| {
                let p = zones.cell_footprint(*c).unwrap().c - o;
                Vec2::new(p.dot(u), p.dot(v)).round()
            };
            let extra: Vec<Vec2> = got.difference(&block).map(local).collect();
            let missing: Vec<Vec2> = block.difference(&got).map(local).collect();
            assert!(
                extra.is_empty() && missing.is_empty(),
                "{deg}°: corner to corner, {a} to {b}: extra {extra:?}, missing {missing:?}"
            );
        }
        // 3 x 5 cells.
        let small = paint(world(13.0, 10.0), world(29.5, 45.0));
        assert_eq!(small.len(), 15, "{deg}°: 3 x 5");
        assert_eq!(
            small,
            inside(Vec2::new(12.0, 12.0), Vec2::new(28.0, 44.0)),
            "{deg}°"
        );
        // Across the middle street into the upper block: 4 columns of 8 + 8 rows.
        let both = paint(world(11.0, 13.0), world(37.0, 147.0));
        assert_eq!(both.len(), 4 * 16, "{deg}°: across the street");
        assert_eq!(
            both,
            inside(Vec2::new(12.0, 12.0), Vec2::new(36.0, 148.0)),
            "{deg}°"
        );
    }
}
