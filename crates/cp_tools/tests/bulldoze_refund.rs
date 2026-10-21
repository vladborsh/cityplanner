//! BUG-097: the "+N" the bulldozer shows before the click is the money the click gives back.
//! The cursor hovers a road, then a pipe, through the tool's own hover; the commands a click
//! would send run in the sim, and the treasury grows by exactly the refund shown.

use std::path::Path;
use std::sync::Arc;

use bevy::math::Vec2;
use cp_content::{ContentDb, UtilityKind};
use cp_core::{Cost, Treasury};
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::plugin::{bulldozer_commands, bulldozer_hover};
use cp_tools::{ActiveTool, ToolState};
use cp_utility::UtilityCommand;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const A: Vec2 = Vec2::new(4400.0, 4400.0);

fn sim() -> SimRunner {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(5);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    sim.tick_once();
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: db.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: A,
                to: A + Vec2::new(160.0, 0.0),
            },
            elevation: ElevationMode::Ground,
        }),
        sim.tick(),
    );
    sim.push_command(
        SimCommand::Utility(UtilityCommand::BuildLink {
            kind: UtilityKind::Water,
            path: vec![A + Vec2::new(0.0, 80.0), A + Vec2::new(100.0, 80.0)],
        }),
        sim.tick(),
    );
    sim.tick_once();
    sim
}

fn balance(sim: &SimRunner) -> Cost {
    sim.world().resource::<Treasury>().balance
}

/// Hovers `p` with the bulldozer, clicks: the refund shown and the money that came back.
fn shown_and_paid(sim: &mut SimRunner, p: Vec2) -> (Cost, Cost) {
    let view = sim.view();
    let mut state = ToolState::default();
    state.select(ActiveTool::Bulldozer);
    bulldozer_hover(&mut state, &view, p);
    let commands = bulldozer_commands(&state, Some(&view));
    assert!(!commands.is_empty(), "nothing to bulldoze at {p}");
    let before = balance(sim);
    for c in commands {
        sim.push_command(c, sim.tick());
    }
    sim.tick_once();
    (state.bulldoze_refund, Cost(balance(sim).0 - before.0))
}

#[test]
fn the_refund_shown_before_the_click_is_the_money_that_comes_back() {
    let mut sim = sim();
    // A pipe: 100 m at 4.00/m, 280.00 back.
    let (shown, paid) = shown_and_paid(&mut sim, A + Vec2::new(50.0, 80.0));
    assert_eq!(shown, paid);
    assert_eq!(shown, Cost(28_000));
    // The street under the cursor (the run between the stops the tool picks).
    let (shown, paid) = shown_and_paid(&mut sim, A + Vec2::new(40.0, 0.0));
    assert_eq!(shown, paid);
    assert!(shown.0 > 0, "the street gives something back");
}
