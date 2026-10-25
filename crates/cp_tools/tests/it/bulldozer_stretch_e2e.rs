//! T-123: the bulldozer takes the road up to the nearest junction, corner, dead end or control
//! point (`RemovePart` for the cut ones); Shift takes the whole run.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::{ContentDb, RoadDefId};
use cp_core::CommandOutcome;
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::{CommandOutbox, demolition, span_commands, spans_length};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const R: f32 = 8.0;

fn setup() -> (SimRunner, RoadDefId) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    let road = db.road_id("basic_2l").unwrap();
    (sim, road)
}

fn flush(sim: &mut SimRunner, cmds: Vec<SimCommand>) {
    let mut outbox = CommandOutbox::default();
    for c in cmds {
        outbox.push(c);
    }
    outbox.flush_into(sim);
    sim.tick_once();
    for o in sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
    {
        assert!(o.result.is_ok(), "{:?}", o.result);
    }
}

fn build(sim: &mut SimRunner, road: RoadDefId, path: RoadPath) {
    flush(
        sim,
        vec![SimCommand::Net(NetCommand::BuildRoad {
            road,
            path,
            elevation: ElevationMode::Ground,
        })],
    );
}

fn line(a: (f32, f32), b: (f32, f32)) -> RoadPath {
    RoadPath::Straight {
        from: Vec2::new(a.0, a.1),
        to: Vec2::new(b.0, b.1),
    }
}

/// A 192 m street (three 64 m pieces, T-127) from x = 4600 with a cross street at its start.
fn t_street() -> (SimRunner, RoadDefId) {
    let (mut sim, road) = setup();
    build(&mut sim, road, line((4600.0, 4600.0), (4792.0, 4600.0)));
    build(&mut sim, road, line((4600.0, 4540.0), (4600.0, 4600.0)));
    (sim, road)
}

fn total(sim: &SimRunner) -> f32 {
    sim.world()
        .resource::<NetGraph>()
        .segments()
        .map(|(_, s)| s.length)
        .sum()
}

/// Clicks the bulldozer at `p`: returns the highlighted length and sends the commands.
fn click(sim: &mut SimRunner, p: Vec2, shift: bool) -> f32 {
    let view = sim.view();
    let spans = demolition(&view.net, p, R, shift);
    assert!(!spans.is_empty());
    let cmds = span_commands(&view.net, &spans);
    flush(sim, cmds);
    spans_length(&spans)
}

#[test]
fn a_click_at_a_junction_takes_64_m_and_the_next_click_the_next_64_m() {
    let (mut sim, _) = t_street();
    let before = total(&sim);
    let lit = click(&mut sim, Vec2::new(4610.0, 4600.0), false);
    assert!((lit - 64.0).abs() < 0.01, "{lit}");
    assert!(
        (before - total(&sim) - lit).abs() < 0.01,
        "highlight = removed"
    );
    {
        let g = sim.world().resource::<NetGraph>();
        // A dead end where the cut is; the cross street and the junction are intact.
        let end = g
            .nodes()
            .find(|(_, n)| (n.pos.x - 4664.0).abs() < 0.6 && (n.pos.z - 4600.0).abs() < 0.6)
            .expect("a node at 64 m");
        assert_eq!(end.1.segments.len(), 1);
    }
    let again = click(&mut sim, Vec2::new(4680.0, 4600.0), false);
    assert!((again - 64.0).abs() < 0.01, "{again}");
    let g = sim.world().resource::<NetGraph>();
    let end = g
        .nodes()
        .find(|(_, n)| (n.pos.x - 4728.0).abs() < 0.6 && (n.pos.z - 4600.0).abs() < 0.6)
        .expect("a node at 128 m");
    assert_eq!(end.1.segments.len(), 1);
}

#[test]
fn a_corner_stops_the_bulldozer() {
    let (mut sim, road) = setup();
    build(
        &mut sim,
        road,
        RoadPath::Free(vec![
            Vec2::new(4600.0, 4800.0),
            Vec2::new(4650.0, 4800.0),
            Vec2::new(4650.0, 4850.0),
        ]),
    );
    let before = total(&sim);
    let lit = click(&mut sim, Vec2::new(4620.0, 4800.0), false);
    assert!((lit - 50.0).abs() < 0.01, "{lit}");
    assert!(
        (total(&sim) - (before - 50.0)).abs() < 0.01,
        "the second leg is whole"
    );
    assert_eq!(sim.world().resource::<NetGraph>().segment_count(), 1);
}

#[test]
fn a_junction_in_the_middle_of_a_street_is_not_crossed() {
    let (mut sim, road) = setup();
    // 224 m: pieces of 64, 64 and 96 m (cut on the control points, T-127).
    build(&mut sim, road, line((4600.0, 4600.0), (4824.0, 4600.0)));
    // A cross street on the node between the second and third pieces (x = 4728).
    build(&mut sim, road, line((4728.0, 4540.0), (4728.0, 4600.0)));
    let lit = click(&mut sim, Vec2::new(4740.0, 4600.0), false);
    assert!((lit - 64.0).abs() < 0.01, "to the control point: {lit}");
    let g = sim.world().resource::<NetGraph>();
    let junction = g
        .nodes()
        .find(|(_, n)| (n.pos.x - 4728.0).abs() < 0.6 && (n.pos.z - 4600.0).abs() < 0.6)
        .expect("the junction survives");
    assert_eq!(
        junction.1.segments.len(),
        2,
        "the first piece and the cross street"
    );
}

#[test]
fn shift_takes_the_whole_run_up_to_the_junctions() {
    let (mut sim, road) = setup();
    build(&mut sim, road, line((4600.0, 4600.0), (4824.0, 4600.0)));
    build(&mut sim, road, line((4728.0, 4540.0), (4728.0, 4600.0)));
    let lit = click(&mut sim, Vec2::new(4740.0, 4600.0), true);
    assert!((lit - 96.0).abs() < 0.01, "{lit}");
    // The first two pieces and the cross street are left.
    assert_eq!(sim.world().resource::<NetGraph>().segment_count(), 3);
}

#[test]
fn the_control_points_of_what_is_left_stay_where_they_were() {
    let (mut sim, _) = t_street();
    // Take the middle piece 64..128 first: the anchor (x = 4600) is kept.
    click(&mut sim, Vec2::new(4690.0, 4600.0), false);
    let g = sim.world().resource::<NetGraph>();
    let seg = g
        .nodes()
        .find(|(_, n)| (n.pos.x - 4600.0).abs() < 0.6 && n.segments.len() == 2)
        .and_then(|(_, n)| {
            n.segments
                .iter()
                .copied()
                .find(|s| g.segment(*s).is_some_and(|s| s.curve.p3.x > s.curve.p0.x))
        })
        .expect("the street's first remnant");
    let st = cp_net::stretch(g, seg).unwrap();
    assert!(st.length < 70.0, "{}", st.length);
}

#[test]
fn the_log_replays_to_the_same_checksum() {
    let run = || {
        let (mut sim, _) = t_street();
        click(&mut sim, Vec2::new(4610.0, 4600.0), false);
        click(&mut sim, Vec2::new(4680.0, 4600.0), false);
        sim.checksum()
    };
    assert_eq!(run(), run());
}
