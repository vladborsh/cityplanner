//! T-027 headless: the zoning tools paint through `PaintZones` via the command layer, painted
//! types follow the road when it is split, and they are saved and loaded.

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
