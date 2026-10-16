//! BUG-027: a click that belongs to the UI (over a panel) or only activates the window never
//! reaches the tools. Headless Bevy `App`, synthetic input: the flag the UI writes
//! (`UiCapture`) and `WindowFocused` messages are fed by hand.
//!
//! Manual check (in the window): pick Road; click a palette icon or a panel — no first road
//! point appears; click another app, then click the game window — the activation click puts
//! no point; a click on the map still does. Same for Zones and the bulldozer. Scroll the
//! wheel over a panel: the map does not zoom; type in the save-name field: WASD does not move
//! the camera.

use std::path::Path;
use std::sync::Arc;

use bevy::input::ButtonState;
use bevy::input::InputPlugin;
use bevy::input::mouse::MouseButtonInput;
use bevy::prelude::*;
use bevy::window::WindowFocused;
use cp_content::ContentDb;
use cp_net::{NetCommand, RoadPath};
use cp_render::{RtsCameraPlugin, UiCapture};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::{ActiveTool, CommandOutbox, ToolState, tool_click, tool_hotkeys};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const SPOT: Vec2 = Vec2::new(4700.0, 4700.0);

fn sim() -> (SimRunner, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    (sim, db)
}

/// An app with the tools and the camera plugin (which owns the focus tracking), the cursor
/// pretending to hover `SPOT`.
fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, InputPlugin, RtsCameraPlugin))
        .add_message::<WindowFocused>()
        .init_resource::<ToolState>()
        .init_resource::<CommandOutbox>()
        .add_systems(Update, (tool_hotkeys, tool_click).chain());
    app.world_mut().resource_mut::<ToolState>().hover = Some(SPOT);
    app
}

fn click(app: &mut App) {
    app.world_mut().write_message(MouseButtonInput {
        button: MouseButton::Left,
        state: ButtonState::Pressed,
        window: Entity::PLACEHOLDER,
    });
    app.update();
    app.world_mut().write_message(MouseButtonInput {
        button: MouseButton::Left,
        state: ButtonState::Released,
        window: Entity::PLACEHOLDER,
    });
    app.update();
}

fn road_tool(app: &mut App, db: &ContentDb) {
    let mut state = app.world_mut().resource_mut::<ToolState>();
    state.select(ActiveTool::Road);
    state.road.road = Some(db.road_id("basic_2l").unwrap());
}

fn road_points(app: &App) -> usize {
    app.world().resource::<ToolState>().road.points.len()
}

#[test]
fn a_click_over_the_ui_does_not_start_a_road() {
    let (_, db) = sim();
    let mut app = app();
    road_tool(&mut app, &db);
    app.world_mut().resource_mut::<UiCapture>().pointer_over_ui = true;
    click(&mut app);
    assert_eq!(road_points(&app), 0);
    assert!(app.world().resource::<CommandOutbox>().is_empty());
}

#[test]
fn a_click_on_the_map_starts_a_road() {
    let (_, db) = sim();
    let mut app = app();
    road_tool(&mut app, &db);
    click(&mut app);
    assert_eq!(road_points(&app), 1);
}

#[test]
fn the_click_that_activates_the_window_is_not_a_road_point() {
    let (_, db) = sim();
    let mut app = app();
    road_tool(&mut app, &db);
    // Same frame: focus gained, then the button.
    app.world_mut().write_message(WindowFocused {
        window: Entity::PLACEHOLDER,
        focused: true,
    });
    click(&mut app);
    assert_eq!(road_points(&app), 0, "activation click in the focus frame");
    // The focus message one frame before the click.
    app.world_mut().write_message(WindowFocused {
        window: Entity::PLACEHOLDER,
        focused: true,
    });
    app.update();
    click(&mut app);
    assert_eq!(
        road_points(&app),
        0,
        "activation click one frame after focus"
    );
    // Later clicks are the game's again.
    for _ in 0..3 {
        app.update();
    }
    click(&mut app);
    assert_eq!(road_points(&app), 1);
}

#[test]
fn the_bulldozer_ignores_ui_and_activation_clicks_but_not_map_clicks() {
    let (mut sim, db) = sim();
    let tick = sim.tick();
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: db.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: SPOT,
                to: SPOT + Vec2::X * 60.0,
            },
            elevation: cp_net::ElevationMode::Ground,
        }),
        tick,
    );
    sim.tick_once();
    sim.tick_once();
    let view = sim.view();
    let seg = view.net.segments().next().expect("a road").0;
    let mut app = app();
    {
        let mut state = app.world_mut().resource_mut::<ToolState>();
        state.select(ActiveTool::Bulldozer);
    }
    let arm = |app: &mut App| {
        app.world_mut().resource_mut::<ToolState>().highlight = vec![seg];
    };
    let commands = |app: &App| app.world().resource::<CommandOutbox>().len();

    app.world_mut().resource_mut::<UiCapture>().pointer_over_ui = true;
    arm(&mut app);
    click(&mut app);
    assert_eq!(commands(&app), 0, "over the UI");

    app.world_mut().resource_mut::<UiCapture>().pointer_over_ui = false;
    app.world_mut().write_message(WindowFocused {
        window: Entity::PLACEHOLDER,
        focused: true,
    });
    arm(&mut app);
    click(&mut app);
    assert_eq!(commands(&app), 0, "window activation");

    for _ in 0..3 {
        app.update();
    }
    arm(&mut app);
    click(&mut app);
    assert!(commands(&app) > 0, "a map click removes the road");

    // The same click on a run outside the bought tiles sends nothing (T-116).
    *app.world_mut().resource_mut::<CommandOutbox>() = CommandOutbox::default();
    app.world_mut()
        .resource_mut::<ToolState>()
        .highlight_outside = true;
    arm(&mut app);
    app.world_mut()
        .resource_mut::<ToolState>()
        .highlight_outside = true;
    click(&mut app);
    assert_eq!(commands(&app), 0, "outside your land");
}

#[test]
fn zone_painting_waits_for_the_ui_to_let_go() {
    let (sim, _) = sim();
    let mut app = app();
    app.insert_resource(sim.view());
    app.world_mut()
        .resource_mut::<ToolState>()
        .select(ActiveTool::Zone);
    app.world_mut().resource_mut::<UiCapture>().pointer_over_ui = true;
    click(&mut app);
    assert!(app.world().resource::<CommandOutbox>().is_empty());
    assert!(app.world().resource::<ToolState>().zone.corner.is_none());
}

/// BUG-035: every tool, armed as far as a headless world allows, ignores a click while the
/// pointer is over the UI, and takes it once the pointer is on the map.
#[test]
fn no_tool_reacts_to_a_click_over_the_ui() {
    let (sim, db) = sim();
    let tools = [
        ActiveTool::Road,
        ActiveTool::Bulldozer,
        ActiveTool::Upgrade,
        ActiveTool::Zone,
        ActiveTool::District,
        ActiveTool::Place,
        ActiveTool::Utility,
        ActiveTool::UtilityRemove,
        ActiveTool::Line,
    ];
    for tool in tools {
        let mut app = app();
        app.insert_resource(sim.view());
        {
            let mut state = app.world_mut().resource_mut::<ToolState>();
            state.select(tool);
            state.road.road = Some(db.road_id("basic_2l").unwrap());
            state.building.def = db.building_id("bus_depot");
        }
        app.world_mut().resource_mut::<UiCapture>().pointer_over_ui = true;
        click(&mut app);
        click(&mut app);
        let state = app.world().resource::<ToolState>();
        assert!(
            app.world().resource::<CommandOutbox>().is_empty(),
            "{tool:?} sent a command through the UI"
        );
        assert_eq!(state.road.points.len(), 0, "{tool:?}: road point");
        assert!(state.zone.corner.is_none(), "{tool:?}: zone corner");
        assert!(state.line.stops.is_empty(), "{tool:?}: line stop");
    }
}

fn key(app: &mut App, code: KeyCode) {
    use bevy::input::keyboard::{Key, KeyboardInput};
    for state in [ButtonState::Pressed, ButtonState::Released] {
        app.world_mut().write_message(KeyboardInput {
            key_code: code,
            logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }
}

fn speed_commands(app: &App) -> Vec<cp_core::SimSpeed> {
    app.world()
        .resource::<CommandOutbox>()
        .iter()
        .filter_map(|c| match c {
            SimCommand::SetSpeed { speed } => Some(*speed),
            _ => None,
        })
        .collect()
}

/// T-175: Space pauses and resumes the last speed; not while a text field has the keyboard.
#[test]
fn space_toggles_pause_and_resumes_the_last_speed() {
    let (mut sim, _) = sim();
    sim.push_command(
        SimCommand::SetSpeed {
            speed: cp_core::SimSpeed::X3,
        },
        sim.tick(),
    );
    sim.apply_due_commands();
    let mut app = app();
    app.insert_resource(sim.view());
    key(&mut app, KeyCode::Space);
    assert_eq!(speed_commands(&app), [cp_core::SimSpeed::Pause]);

    // The sim is paused now: Space resumes ×3.
    let mut paused = sim.view();
    paused.speed = cp_core::SimSpeed::Pause;
    app.insert_resource(paused);
    key(&mut app, KeyCode::Space);
    assert_eq!(
        speed_commands(&app),
        [cp_core::SimSpeed::Pause, cp_core::SimSpeed::X3]
    );

    // A text field owns the keyboard: Space is a space.
    app.world_mut()
        .resource_mut::<UiCapture>()
        .keyboard_captured = true;
    key(&mut app, KeyCode::Space);
    key(&mut app, KeyCode::KeyB);
    assert_eq!(speed_commands(&app).len(), 2);
    assert_eq!(app.world().resource::<ToolState>().active, ActiveTool::None);
}

/// T-175: B picks the bulldozer.
#[test]
fn b_selects_the_bulldozer() {
    let mut app = app();
    key(&mut app, KeyCode::KeyB);
    assert_eq!(
        app.world().resource::<ToolState>().active,
        ActiveTool::Bulldozer
    );
}
