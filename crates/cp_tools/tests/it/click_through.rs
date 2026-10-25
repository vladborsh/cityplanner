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
        ActiveTool::Line,
        ActiveTool::Junctions,
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

/// T-177: a picked-up service building follows the placement rules of its type, a click sends
/// `Move` and puts the tool away, and Esc / right click leave the building where it was.
mod moving {
    use super::*;
    use bevy_math::Vec3;
    use cp_building::{BuildingCommand, BuildingGrid, BuildingId};
    use cp_core::curve::CubicBezier3;
    use cp_net::{Elevation, NetGraph};
    use cp_terrain::TileOwnership;
    use cp_zone::{ZoneCellRef, ZoneGrid};

    fn spot(sim: &SimRunner, z: f32) -> Vec2 {
        let zones = sim.world().resource::<ZoneGrid>();
        zones
            .blocks()
            .filter_map(|(id, _)| {
                zones.cell_footprint(ZoneCellRef {
                    block: id,
                    cell: 5 * 4 + 1,
                })
            })
            .map(|o| o.c)
            .find(|c| (c.y - z).abs() < 40.0)
            .unwrap()
    }

    /// Two roads (z = 5000, 5300) and a fire station beside the first.
    fn world() -> (SimRunner, BuildingId) {
        let (mut sim, db) = sim();
        let mut tiles = TileOwnership::none();
        for tz in 0..cp_terrain::TILES_PER_SIDE {
            for tx in 0..cp_terrain::TILES_PER_SIDE {
                tiles.set(tx, tz, true);
            }
        }
        sim.world_mut().insert_resource(tiles);
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        for z in [5000.0, 5300.0] {
            sim.world_mut()
                .resource_mut::<NetGraph>()
                .add_segment(
                    &def,
                    road,
                    CubicBezier3::line(Vec3::new(5000.0, 0.0, z), Vec3::new(5200.0, 0.0, z)),
                    Elevation::Ground,
                )
                .unwrap();
        }
        sim.tick_once();
        let at = spot(&sim, 5000.0);
        sim.push_command(
            SimCommand::Building(BuildingCommand::Place {
                def: db.building_id("fire_station").unwrap(),
                pos: at,
                rot: 0.0,
            }),
            sim.tick(),
        );
        sim.apply_due_commands();
        let id = sim
            .world()
            .resource::<BuildingGrid>()
            .iter()
            .next()
            .unwrap()
            .0;
        sim.tick_once();
        (sim, id)
    }

    fn picked_up(sim: &SimRunner, id: BuildingId, hover: Vec2) -> App {
        let mut app = app();
        app.insert_resource(sim.view());
        {
            let view = sim.view();
            let mut state = app.world_mut().resource_mut::<ToolState>();
            assert!(state.begin_move(&view, id));
            state.hover = Some(hover);
            state.place_ghost = cp_tools::building::preview(&view, &state.building, hover);
        }
        app
    }

    fn moves(app: &App) -> Vec<SimCommand> {
        app.world()
            .resource::<CommandOutbox>()
            .iter()
            .filter(|c| matches!(c, SimCommand::Building(BuildingCommand::Move { .. })))
            .cloned()
            .collect()
    }

    #[test]
    fn a_click_on_a_valid_spot_sends_move_and_ends_the_tool() {
        let (sim, id) = world();
        let to = spot(&sim, 5300.0);
        let mut app = picked_up(&sim, id, to);
        assert!(
            app.world()
                .resource::<ToolState>()
                .place_ghost
                .as_ref()
                .unwrap()
                .valid
        );
        click(&mut app);
        assert_eq!(
            moves(&app),
            [SimCommand::Building(BuildingCommand::Move {
                building: id,
                pos: to,
                rot: 0.0
            })]
        );
        assert_eq!(app.world().resource::<ToolState>().active, ActiveTool::None);
    }

    #[test]
    fn a_click_on_a_bad_spot_sends_nothing() {
        let (sim, id) = world();
        // Far from every road (T-267: on the road it would stand beside it): nowhere to stand.
        let mut app = picked_up(&sim, id, Vec2::new(5100.0, 5700.0));
        let ghost = app
            .world()
            .resource::<ToolState>()
            .place_ghost
            .clone()
            .unwrap();
        assert!(!ghost.valid && ghost.error.is_some());
        click(&mut app);
        assert!(moves(&app).is_empty());
        assert_eq!(
            app.world().resource::<ToolState>().active,
            ActiveTool::Place
        );
    }

    #[test]
    fn escape_and_right_click_leave_the_building_where_it_is() {
        let (sim, id) = world();
        let to = spot(&sim, 5300.0);
        let mut app = picked_up(&sim, id, to);
        key(&mut app, KeyCode::Escape);
        assert_eq!(app.world().resource::<ToolState>().active, ActiveTool::None);
        assert!(
            app.world()
                .resource::<ToolState>()
                .building
                .moving
                .is_none()
        );

        let mut app = picked_up(&sim, id, to);
        for state in [ButtonState::Pressed, ButtonState::Released] {
            app.world_mut().write_message(MouseButtonInput {
                button: MouseButton::Right,
                state,
                window: Entity::PLACEHOLDER,
            });
            app.update();
        }
        assert_eq!(app.world().resource::<ToolState>().active, ActiveTool::None);
        assert!(moves(&app).is_empty());
        assert!(sim.world().resource::<BuildingGrid>().get(id).is_some());
    }
}

/// BUG-108: a click with the district brush (and with the area brush of the Industry panel)
/// paints the cells under it in the sim.
#[test]
fn the_district_and_area_brushes_paint_the_sim_maps() {
    use cp_district::{AreaKind, AreaMap, DistrictCommand, DistrictMap};
    let (mut sim, _) = sim();
    let mut out = CommandOutbox::default();
    out.push(SimCommand::District(DistrictCommand::Create {
        name: "D".into(),
    }));
    out.push(SimCommand::District(DistrictCommand::CreateArea {
        kind: AreaKind::Industry(cp_content::ResourceKind::Farming),
        name: "A".into(),
    }));
    out.flush_into(&mut sim);
    sim.tick_once();
    let district = sim.view().districts.iter().next().expect("district").0;
    let area = sim.view().areas.iter().next().expect("area").0;
    for brush in ["district", "area"] {
        let mut app = app();
        app.insert_resource(sim.view());
        {
            let mut state = app.world_mut().resource_mut::<ToolState>();
            if brush == "district" {
                state.select(ActiveTool::District);
                state.district.selected = Some(district);
            } else {
                state.select(ActiveTool::Area);
                state.area.selected = Some(area);
            }
        }
        click(&mut app);
        let n = app
            .world_mut()
            .resource_mut::<CommandOutbox>()
            .flush_into(&mut sim);
        assert!(n > 0, "{brush}: the click sent a command");
        sim.tick_once();
    }
    let cell = DistrictMap::cell_of(SPOT).unwrap();
    let w = sim.world();
    assert_eq!(w.resource::<DistrictMap>().get(cell), Some(district));
    assert_eq!(w.resource::<AreaMap>().get(cell), Some(area));
}

/// BUG-108: a brush stroke with nothing chosen made nothing and said nothing. Now it makes a
/// district (an industry area of the richest resource under the brush) and paints it, in the
/// same tick.
#[test]
fn a_stroke_with_nothing_chosen_makes_a_district_or_an_area_and_paints_it() {
    use cp_district::{AreaKind, AreaMap, DistrictMap};
    let (mut sim, _) = sim();
    for tool in [ActiveTool::District, ActiveTool::Area] {
        let mut app = app();
        app.insert_resource(sim.view());
        app.world_mut().resource_mut::<ToolState>().select(tool);
        click(&mut app);
        app.world_mut()
            .resource_mut::<CommandOutbox>()
            .flush_into(&mut sim);
        sim.tick_once();
    }
    let view = sim.view();
    let cell = DistrictMap::cell_of(SPOT).unwrap();
    let (district, _) = view.districts.iter().next().expect("a district was made");
    assert_eq!(view.district_map.get(cell), Some(district));
    let (area, a) = view.areas.iter().next().expect("an area was made");
    let AreaKind::Industry(r) = a.kind else {
        panic!("an industry area: {:?}", a.kind)
    };
    let res = view.terrain.as_deref().unwrap().resources();
    for other in cp_content::ResourceKind::ALL {
        assert!(
            res.richness_at(r, SPOT) >= res.richness_at(other, SPOT),
            "{r:?} the richest"
        );
    }
    assert_eq!(sim.world().resource::<AreaMap>().get(cell), Some(area));
}

/// BUG-111: what each bulldozer layer takes. A pipe goes only with the Water layer (a power line
/// only with Power); the plain bulldozer never picks a pipe or a line; a water tower goes with
/// the plain bulldozer and with the Water layer.
mod bulldozer_layers {
    use super::*;
    use bevy_math::Vec3;
    use cp_building::{BuildingCommand, BuildingGrid};
    use cp_core::curve::CubicBezier3;
    use cp_net::{Elevation, NetGraph};
    use cp_terrain::TileOwnership;
    use cp_tools::{BulldozeLayer, bulldozer_hover};
    use cp_utility::UtilityCommand;

    const TOWER: Vec2 = Vec2::new(5100.0, 5120.0);
    const PIPE_AT: Vec2 = Vec2::new(5100.0, 5200.0);

    /// A road, a water tower beside it on open ground and a pipe farther off.
    fn world() -> SimRunner {
        let (mut sim, db) = sim();
        let mut tiles = TileOwnership::none();
        for tz in 0..cp_terrain::TILES_PER_SIDE {
            for tx in 0..cp_terrain::TILES_PER_SIDE {
                tiles.set(tx, tz, true);
            }
        }
        sim.world_mut().insert_resource(tiles);
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(
                    Vec3::new(5000.0, 0.0, 5000.0),
                    Vec3::new(5200.0, 0.0, 5000.0),
                ),
                Elevation::Ground,
            )
            .unwrap();
        sim.tick_once();
        let mut out = CommandOutbox::default();
        out.push(SimCommand::Building(BuildingCommand::Place {
            def: db.building_id("water_tower").unwrap(),
            pos: TOWER,
            rot: 0.0,
        }));
        out.push(SimCommand::Utility(UtilityCommand::BuildLink {
            kind: cp_content::UtilityKind::Water,
            path: vec![Vec2::new(5040.0, 5200.0), Vec2::new(5160.0, 5200.0)],
        }));
        out.push(SimCommand::Utility(UtilityCommand::BuildLink {
            kind: cp_content::UtilityKind::Power,
            path: vec![Vec2::new(5040.0, 5260.0), Vec2::new(5160.0, 5260.0)],
        }));
        out.flush_into(&mut sim);
        sim.tick_once();
        assert_eq!(towers(&sim), 1, "the tower stands");
        assert!(pipes(&sim) >= 1, "the pipe is laid");
        sim
    }

    fn towers(sim: &SimRunner) -> usize {
        sim.world().resource::<BuildingGrid>().iter().count()
    }

    fn links_of(sim: &SimRunner, net: cp_utility::UtilityNet) -> usize {
        sim.view()
            .utilities
            .links()
            .filter(|(_, l)| l.net == net)
            .count()
    }

    fn pipes(sim: &SimRunner) -> usize {
        links_of(sim, cp_utility::UtilityNet::Pipe)
    }

    /// The bulldozer of `layer` hovering with a ray from `origin` towards `target`; the ground
    /// point of the ray and the tool's state after the hover.
    fn hover_ray(
        sim: &SimRunner,
        layer: BulldozeLayer,
        origin: Vec3,
        target: Vec3,
    ) -> (Vec2, ToolState) {
        let ground = sim
            .view()
            .terrain
            .as_deref()
            .unwrap()
            .height(Vec2::new(target.x, target.z));
        let dir = (target - origin).normalize();
        let t = -(origin.y - ground) / dir.y;
        let hit = origin + dir * t;
        let p = Vec2::new(hit.x, hit.z);
        let view = sim.view();
        let mut state = ToolState::default();
        state.bulldoze_in(layer);
        state.hover = Some(p);
        state.cursor_ray = Some((origin, dir));
        bulldozer_hover(&mut state, &view, p);
        (p, state)
    }

    /// A bulldozer click of `layer` at `p`; the commands it sent, applied.
    fn bulldoze(sim: &mut SimRunner, layer: BulldozeLayer, p: Vec2) -> usize {
        let mut app = app();
        app.insert_resource(sim.view());
        {
            let view = sim.view();
            let mut state = app.world_mut().resource_mut::<ToolState>();
            state.bulldoze_in(layer);
            state.hover = Some(p);
            bulldozer_hover(&mut state, &view, p);
        }
        click(&mut app);
        let n = app
            .world_mut()
            .resource_mut::<CommandOutbox>()
            .flush_into(sim);
        sim.tick_once();
        n
    }

    #[test]
    fn the_plain_bulldozer_leaves_a_pipe_and_the_water_layer_takes_it() {
        let mut sim = world();
        let laid = pipes(&sim);
        assert_eq!(bulldoze(&mut sim, BulldozeLayer::All, PIPE_AT), 0);
        assert_eq!(pipes(&sim), laid, "the plain bulldozer leaves pipes alone");
        assert_eq!(bulldoze(&mut sim, BulldozeLayer::Power, PIPE_AT), 0);
        assert_eq!(pipes(&sim), laid, "the power layer leaves pipes alone");
        assert!(bulldoze(&mut sim, BulldozeLayer::Water, PIPE_AT) > 0);
        assert!(pipes(&sim) < laid, "the water layer takes the pipe");
    }

    /// The user's case: the tower's tank stands 13 m up, so a click on it meets the ground well
    /// behind the 16 m footprint. The pick follows the cursor ray through the tower's body.
    #[test]
    fn a_click_on_the_towers_tank_picks_the_tower_and_shows_its_refund() {
        let mut sim = world();
        let ground = sim.view().terrain.as_deref().unwrap().height(TOWER);
        // A camera 60 m up and 80 m south, aimed at the tank: the ray meets the ground ~30 m
        // north of the tower, outside its footprint.
        let origin = Vec3::new(TOWER.x, ground + 60.0, TOWER.y + 80.0);
        let target = Vec3::new(TOWER.x, ground + 12.0, TOWER.y);
        let dir = (target - origin).normalize();
        let t = -(origin.y - ground) / dir.y;
        let hit = origin + dir * t;
        let p = Vec2::new(hit.x, hit.z);
        assert!(
            p.distance(TOWER) > 12.0,
            "the ground hit is off the footprint: {p}"
        );
        let mut app = app();
        app.insert_resource(sim.view());
        {
            let view = sim.view();
            let mut state = app.world_mut().resource_mut::<ToolState>();
            state.bulldoze_in(BulldozeLayer::All);
            state.hover = Some(p);
            state.cursor_ray = Some((origin, dir));
            bulldozer_hover(&mut state, &view, p);
            assert!(state.building_highlight.is_some(), "the tower is picked");
            assert_eq!(
                state.bulldoze_refund,
                cp_core::Cost::from_units(700.0),
                "70 % back"
            );
        }
        click(&mut app);
        app.world_mut()
            .resource_mut::<CommandOutbox>()
            .flush_into(&mut sim);
        sim.tick_once();
        assert_eq!(towers(&sim), 0, "the tower is gone");
    }

    #[test]
    fn a_power_line_goes_only_with_the_power_layer() {
        use cp_utility::UtilityNet::Power;
        let mut sim = world();
        let laid = links_of(&sim, Power);
        assert!(laid >= 1);
        let at = Vec2::new(5100.0, 5260.0);
        assert_eq!(bulldoze(&mut sim, BulldozeLayer::All, at), 0);
        assert_eq!(bulldoze(&mut sim, BulldozeLayer::Water, at), 0);
        assert_eq!(links_of(&sim, Power), laid, "All and Water leave the line");
        assert!(bulldoze(&mut sim, BulldozeLayer::Power, at) > 0);
        assert!(links_of(&sim, Power) < laid, "Power takes it");
    }

    /// Reviewer's case 1: a tall building beside a road; the click's ray crosses its box and
    /// meets the ground on the road. The road is what stands at the ground point: it wins.
    #[test]
    fn a_building_before_the_road_does_not_hide_the_road() {
        let (mut sim, db) = (world(), super::sim().1);
        let mut out = CommandOutbox::default();
        out.push(SimCommand::Building(BuildingCommand::Place {
            def: db.building_id("water_tower").unwrap(),
            pos: Vec2::new(5100.0, 5030.0),
            rot: 0.0,
        }));
        out.flush_into(&mut sim);
        sim.tick_once();
        assert_eq!(towers(&sim), 2);
        let ground = sim
            .view()
            .terrain
            .as_deref()
            .unwrap()
            .height(Vec2::new(5100.0, 5000.0));
        let (p, state) = hover_ray(
            &sim,
            BulldozeLayer::All,
            Vec3::new(5100.0, ground + 40.0, 5110.0),
            Vec3::new(5100.0, ground, 5000.0),
        );
        assert!(
            p.distance(Vec2::new(5100.0, 5000.0)) < 1.0,
            "the ground point is on the road"
        );
        assert!(
            state.building_highlight.is_none(),
            "the tower's box does not hide the road"
        );
        assert!(!state.spans.is_empty(), "the road is picked");
    }

    /// Reviewer's case 2: a building stands right where the tank click meets the ground. The
    /// buildings compete along the ray: the tower, met first, wins.
    #[test]
    fn the_building_the_ray_meets_first_wins_over_the_one_at_the_ground_point() {
        let (mut sim, db) = (world(), super::sim().1);
        let ground = sim.view().terrain.as_deref().unwrap().height(TOWER);
        // From 160 m south: the ray through the tank meets the ground ~40 m north of the tower.
        let origin = Vec3::new(TOWER.x, ground + 60.0, TOWER.y + 160.0);
        let target = Vec3::new(TOWER.x, ground + 12.0, TOWER.y);
        let (p, _) = hover_ray(&sim, BulldozeLayer::All, origin, target);
        assert!(p.distance(TOWER) > 20.0, "room for another tower at {p}");
        let mut out = CommandOutbox::default();
        out.push(SimCommand::Building(BuildingCommand::Place {
            def: db.building_id("water_tower").unwrap(),
            pos: p,
            rot: 0.0,
        }));
        out.flush_into(&mut sim);
        sim.tick_once();
        assert_eq!(towers(&sim), 2);
        let behind = sim
            .world()
            .resource::<BuildingGrid>()
            .iter()
            .find(|(_, b)| b.pos.distance(p) < 1.0)
            .map(|(id, _)| id)
            .expect("the tower at the ground point");
        let (_, state) = hover_ray(&sim, BulldozeLayer::All, origin, target);
        let picked = state.building_highlight.expect("a building");
        assert_ne!(
            picked, behind,
            "the tower the ray meets first, not the one behind"
        );
    }

    #[test]
    fn a_water_tower_goes_with_the_plain_bulldozer_and_with_the_water_layer() {
        for layer in [BulldozeLayer::All, BulldozeLayer::Water] {
            let mut sim = world();
            bulldoze(&mut sim, layer, TOWER);
            assert_eq!(towers(&sim), 0, "{layer:?} takes the tower");
        }
    }
}
