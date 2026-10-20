//! T-182, T-286 (§13.5 v7.1): the pipe and power-line tools snap by the road tool's rules —
//! square to a neighbouring link within 7°, whole cells long, a street's direction for a start in
//! the open, nodes of their own network — and `Alt` turns it all off.

use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_content::{ContentDb, UtilityKind};
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph, SnapReportKind};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::{UtilityMode, UtilityTool, compute_utility_ghost_alt};
use cp_utility::UtilityCommand;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const C: f32 = 5000.0;

fn town(kind: UtilityKind) -> SimRunner {
    let db = Arc::new(ContentDb::load_dir(format!("{ROOT}/../../assets/content")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::new(
        Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap(),
    ));
    sim.set_content(Arc::clone(&db));
    // A street along z = C + 100 (tilted 0°), and one link from (C, C) east to (C + 60, C).
    let road = db.road_id("basic_2l").unwrap();
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            db.road(road),
            road,
            CubicBezier3::line(
                Vec3::new(C - 100.0, 0.0, C + 100.0),
                Vec3::new(C + 100.0, 0.0, C + 100.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.push_command(
        SimCommand::Utility(UtilityCommand::BuildLink {
            kind,
            path: vec![Vec2::new(C, C), Vec2::new(C + 60.0, C)],
        }),
        sim.tick(),
    );
    sim.tick_once();
    sim.tick_once();
    sim
}

fn drawing(kind: UtilityKind, from: Vec2) -> UtilityTool {
    UtilityTool {
        kind,
        mode: UtilityMode::Straight,
        points: vec![from],
    }
}

#[test]
fn a_pipe_from_a_node_at_88_degrees_stands_at_90_with_its_label() {
    let sim = town(UtilityKind::Water);
    let view = sim.view();
    assert_eq!(view.utilities.link_count(), 1);
    let tool = drawing(UtilityKind::Water, Vec2::new(C + 60.0, C));
    // Straight up from the east end is 90° to the link; the cursor is 2° off.
    let p = Vec2::new(
        C + 60.0 + 30.0 * 2f32.to_radians().sin(),
        C + 30.0 * 2f32.to_radians().cos(),
    );
    let g = compute_utility_ghost_alt(&view, &tool, p, false).unwrap();
    // Square (the guide line shows it), 30 m is 4 whole cells.
    assert_eq!(g.snap.kind, SnapReportKind::CellStep(32));
    assert!(g.snap.guide.is_some());
    let end = g.polylines[0].last().unwrap();
    assert!((end.x - (C + 60.0)).abs() < 1e-3, "{end:?}");
    assert!((end.z - (C + 32.0)).abs() < 1e-3, "{end:?}");
    // Alt: no snaps, the cursor stays where it is.
    let g = compute_utility_ghost_alt(&view, &tool, p, true).unwrap();
    assert_eq!(g.snap.kind, SnapReportKind::None);
    let end = g.polylines[0].last().unwrap();
    assert!((end.x - p.x).abs() < 0.5, "{end:?}");
}

#[test]
fn a_run_started_beside_a_street_runs_parallel_to_it() {
    let sim = town(UtilityKind::Water);
    let view = sim.view();
    // A start in the open 20 m from the street (far from the link): the street's direction is
    // the reference, a cursor 3 m off it gives a run parallel to the street.
    let tool = drawing(UtilityKind::Water, Vec2::new(C - 80.0, C + 80.0));
    let p = Vec2::new(C + 20.0, C + 83.0);
    let g = compute_utility_ghost_alt(&view, &tool, p, false).unwrap();
    assert_eq!(g.snap.kind, SnapReportKind::CellStep(104));
    let end = g.polylines[0].last().unwrap();
    assert!((end.z - (C + 80.0)).abs() < 1e-3, "{end:?}");
}

#[test]
fn power_lines_snap_the_same_way_and_a_node_pulls_the_cursor() {
    let sim = town(UtilityKind::Power);
    let view = sim.view();
    let tool = drawing(UtilityKind::Power, Vec2::new(C + 60.0, C));
    let p = Vec2::new(C + 62.0, C + 30.0);
    let g = compute_utility_ghost_alt(&view, &tool, p, false).unwrap();
    assert_eq!(g.snap.kind, SnapReportKind::CellStep(32));
    let end = g.polylines[0].last().unwrap();
    assert!((end.x - (C + 60.0)).abs() < 1e-3, "square: {end:?}");
    // Near the other end of the line: the cursor jumps to that node.
    let p = Vec2::new(C + 4.0, C + 4.0);
    let g = compute_utility_ghost_alt(&view, &tool, p, false).unwrap();
    assert_eq!(g.snap.kind, SnapReportKind::Node);
    let end = g.polylines[0].last().unwrap();
    assert!(
        (end.x - C).abs() < 0.1 && (end.z - C).abs() < 0.1,
        "{end:?}"
    );
    // A pipe tool does not see a power node.
    let pipe = drawing(UtilityKind::Water, Vec2::new(C + 60.0, C));
    let g = compute_utility_ghost_alt(&view, &pipe, p, false).unwrap();
    assert_ne!(g.snap.kind, SnapReportKind::Node);
}
