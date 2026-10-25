//! T-122: the road tool quantizes the path (8 m step, joins within one cell) before it previews
//! and before it sends `BuildRoad`, so what is drawn is what is built; control-point markers
//! show around the cursor.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::{ContentDb, RoadDefId};
use cp_core::CommandOutcome;
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath, SegmentId};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::{CommandOutbox, MarkerKind, RoadMode, RoadTool, preview, quantized, road_markers};

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

fn send(sim: &mut SimRunner, cmd: SimCommand) {
    let mut outbox = CommandOutbox::default();
    outbox.push(cmd);
    outbox.flush_into(sim);
    sim.tick_once();
    let outcomes: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    assert!(outcomes[0].result.is_ok(), "{:?}", outcomes[0].result);
}

fn build(sim: &mut SimRunner, road: RoadDefId, path: RoadPath) {
    send(
        sim,
        SimCommand::Net(NetCommand::BuildRoad {
            road,
            path,
            elevation: ElevationMode::Ground,
        }),
    );
}

fn street(sim: &mut SimRunner, road: RoadDefId) {
    build(
        sim,
        road,
        RoadPath::Straight {
            from: Vec2::new(4600.0, 4600.0),
            to: Vec2::new(4800.0, 4600.0),
        },
    );
}

fn segments(sim: &SimRunner) -> Vec<SegmentId> {
    sim.world()
        .resource::<NetGraph>()
        .segments()
        .map(|(id, _)| id)
        .collect()
}

#[test]
fn a_clicked_road_is_built_with_whole_cells_and_equals_the_preview() {
    let (mut sim, _, road) = setup();
    street(&mut sim, road);
    let old = segments(&sim);
    let mut tool = RoadTool {
        mode: RoadMode::Straight,
        road: Some(road),
        ..Default::default()
    };
    // Far from every road: the first point stays, the length goes 101 m -> 104 m: a whole number
    // of cells from where it starts (T-278).
    assert!(tool.click(Vec2::new(4700.0, 4800.0)).is_none());
    let raw = tool.click(Vec2::new(4700.0, 4901.0)).unwrap();
    let (q, _) = quantized(&sim.view(), raw, false);
    let plan = preview(&sim.view(), &q).unwrap().unwrap();
    let planned: f32 = plan.segments.iter().map(|s| s.length).sum();
    assert!((planned - 104.0).abs() < 0.01, "{planned}");

    send(
        &mut sim,
        SimCommand::Net(NetCommand::BuildRoad {
            road: q.road,
            path: q.path,
            elevation: q.elevation,
        }),
    );
    let graph = sim.world().resource::<NetGraph>();
    let built: f32 = graph
        .segments()
        .filter(|(id, _)| !old.contains(id))
        .map(|(_, s)| s.length)
        .sum();
    assert!((built - planned).abs() < 0.01, "{built} vs {planned}");
    let cells = built / 8.0;
    assert!((cells - cells.round()).abs() < 0.001);
}

#[test]
fn an_end_within_a_cell_of_a_street_joins_it_on_a_whole_cell_as_a_three_way_junction() {
    let (mut sim, db, road) = setup();
    street(&mut sim, road);
    let half = db.road(road).width * 0.5;
    let mut tool = RoadTool {
        mode: RoadMode::Straight,
        road: Some(road),
        ..Default::default()
    };
    tool.click(Vec2::new(4619.0, 4700.0));
    let raw = tool.click(Vec2::new(4619.0, 4600.0 + half + 6.0)).unwrap();
    let (q, _) = quantized(&sim.view(), raw, false);
    send(
        &mut sim,
        SimCommand::Net(NetCommand::BuildRoad {
            road: q.road,
            path: q.path,
            elevation: q.elevation,
        }),
    );
    let graph = sim.world().resource::<NetGraph>();
    // The street's whole cells from its nearer end (T-278): the one nearest the cursor (19 m
    // along) is at 16 m; the section marks (40, 80, 120, 160 m) are not within half a cell.
    let _ = half;
    let junction = graph
        .nodes()
        .find(|(_, n)| (n.pos.x - 4616.0).abs() < 0.6 && (n.pos.z - 4600.0).abs() < 0.6)
        .expect("a node on the whole cell");
    assert_eq!(junction.1.segments.len(), 3);
}

#[test]
fn alt_sends_the_path_as_drawn() {
    let (mut sim, _, road) = setup();
    street(&mut sim, road);
    let mut tool = RoadTool {
        mode: RoadMode::Straight,
        road: Some(road),
        ..Default::default()
    };
    tool.click(Vec2::new(4700.0, 4800.0));
    let raw = tool.click(Vec2::new(4700.0, 4901.0)).unwrap();
    let (q, _) = quantized(&sim.view(), raw, true);
    let RoadPath::Straight { to, .. } = q.path else {
        panic!()
    };
    assert!(to.distance(Vec2::new(4700.0, 4901.0)) < 0.01, "{to:?}");
}

#[test]
fn a_200_m_street_shows_its_section_marks_around_the_cursor() {
    let (mut sim, _, road) = setup();
    street(&mut sim, road);
    let markers = road_markers(&sim.view(), Vec2::new(4700.0, 4625.0));
    let control: Vec<_> = markers
        .iter()
        .filter(|m| m.kind == MarkerKind::Control)
        .collect();
    // Every 10 cells (80 m) from either end of the street (T-275): 40, 80, 120 and 160 m.
    assert_eq!(control.len(), 4, "{markers:?}");
    for (m, x) in control.iter().zip([4640.0, 4680.0, 4720.0, 4760.0]) {
        assert!(m.pos.distance(Vec2::new(x, 4600.0)) < 0.5, "{:?}", m.pos);
    }
    assert!(
        markers.iter().all(|m| m.kind == MarkerKind::Control),
        "only the section marks: no 1-cell ticks"
    );
}
