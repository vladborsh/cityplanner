//! T-286 (§13.5 v7.1): pipes and power lines by the road tool's rules, drawn by a sloppy hand
//! (every click up to 3 m off the point meant, every stroke wobbling up to 2 degrees) with the
//! real tool ([`snapped_cursor`], [`UtilityTool::click`], [`chain_commit`], the ghost
//! [`compute_utility_ghost_alt`]):
//!
//! * a leg is whole cells from its start, 1..25 cells exact, and within half a cell of the 10,
//!   20 and 30 cell marks it is the mark;
//! * a point within a cell of a node of its own network goes to the node, a point on its own link
//!   to a whole cell of it; another network does not pull;
//! * within 7 degrees of square it is square, past it not;
//! * the square through a road's end node guides the length; a pipe along a street runs parallel
//!   to it, beside it, never on its axis;
//! * the click is the ghost;
//! * the cursor before the first click is a 1-cell piece at the point the click puts down: a
//!   pole on a building is red, a pipe under one green, anything off the bought land red;
//! * the pipe ghost highlights exactly the buildings the built pipe brings into reach;
//! * the bulldozer in the water layer takes a pipe under a road and leaves the road, never takes
//!   a house, and B / Esc hand the pipe tool back; the same in the power layer.

use std::path::Path;
use std::sync::Arc;

use bevy::input::ButtonState;
use bevy::input::InputPlugin;
use bevy::input::mouse::MouseButtonInput;
use bevy::prelude::*;
use bevy::window::WindowFocused;
use cp_building::{Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint};
use cp_content::{ContentDb, UtilityKind};
use cp_core::{Cost, Tick, Treasury};
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath};
use cp_render::RtsCameraPlugin;
use cp_sim::{SimCommand, SimRunner, SimView};
use cp_terrain::Terrain;
use cp_tools::utility::chain_commit;
use cp_tools::{
    ActiveTool, BulldozeLayer, CommandOutbox, RoadMode, RoadTool, ToolState, UtilityMode,
    UtilityTool, bulldozer_hover, compute_utility_ghost_alt, road_cursor_piece, snapped_cursor,
    tool_click, tool_hotkeys,
};
use cp_utility::{Coverage, UtilityCommand, UtilityNet};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
/// Where the strokes start, on the bought land.
const A: Vec2 = Vec2::new(4400.0, 4400.0);
const CELL: f32 = 8.0;
const SLOP_M: f32 = 3.0;
const WOBBLE_DEG: f32 = 2.0;

fn sim() -> (SimRunner, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    sim.tick_once();
    sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(1_000_000_000.0);
    (sim, db)
}

fn xz(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.z)
}

/// A sloppy hand: numbers in `-1..1`, the same for the same seed.
struct Hand(u64);

impl Hand {
    fn rand(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    }

    fn slop(&mut self) -> Vec2 {
        Vec2::new(self.rand(), self.rand()) * SLOP_M
    }

    /// `v` turned by up to the wobble.
    fn wobble(&mut self, v: Vec2) -> Vec2 {
        Vec2::from_angle((self.rand() * WOBBLE_DEG).to_radians()).rotate(v)
    }
}

fn tool(kind: UtilityKind) -> UtilityTool {
    UtilityTool {
        kind,
        mode: UtilityMode::Straight,
        points: Vec::new(),
    }
}

/// A click at `p`: where the tool puts the point (the cursor's ghost point).
fn click(view: &SimView, t: &mut UtilityTool, p: Vec2) -> (Vec2, Option<Vec<Vec2>>) {
    let q = snapped_cursor(view, t, p, false).0;
    (q, t.click(q))
}

/// One stroke from the cursor `a` to the cursor `b`: the ghost at `b` must stand where the click
/// puts the end. Returns the two points (nothing is built).
fn stroke(view: &SimView, kind: UtilityKind, a: Vec2, b: Vec2) -> (Vec2, Vec2) {
    let mut t = tool(kind);
    let (q0, _) = click(view, &mut t, a);
    stroke_on(view, &mut t, q0, b)
}

/// The rest of a stroke started at `q0`.
fn stroke_on(view: &SimView, t: &mut UtilityTool, q0: Vec2, b: Vec2) -> (Vec2, Vec2) {
    let t = &mut *t;
    let ghost = compute_utility_ghost_alt(view, t, b, false).expect("a ghost");
    let (q1, path) = click(view, t, b);
    assert_eq!(ghost.resolved, Some(q1), "the click is the ghost");
    assert_eq!(path.as_deref(), Some(&[q0, q1][..]));
    (q0, q1)
}

/// Builds a link of `kind` along `path` through the tool's commit.
fn build(sim: &mut SimRunner, kind: UtilityKind, path: Vec<Vec2>) {
    let view = sim.view();
    let mut t = tool(kind);
    let before = t.clone();
    let cmd = chain_commit(Some(&view), &mut t, before, path).expect("a valid link");
    drop(view);
    let mut outbox = CommandOutbox::default();
    outbox.push(cmd);
    outbox.flush_into(sim);
    sim.tick_once();
}

fn road(sim: &mut SimRunner, db: &ContentDb, from: Vec2, to: Vec2) {
    let mut outbox = CommandOutbox::default();
    outbox.push(SimCommand::Net(NetCommand::BuildRoad {
        road: db.road_id("basic_2l").unwrap(),
        path: RoadPath::Straight { from, to },
        elevation: ElevationMode::Ground,
    }));
    outbox.flush_into(sim);
    sim.tick_once();
    assert!(sim.world().resource::<NetGraph>().segments().count() > 0);
}

fn station(sim: &mut SimRunner, db: &ContentDb, def: &str, pos: Vec2) -> BuildingId {
    let id = sim
        .world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
            def: db.building_id(def).unwrap(),
            footprint: Footprint::Free { w: 2, d: 2 },
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos,
            rot: 0.0,
            access: None,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
    sim.tick_once();
    id
}

#[test]
fn a_leg_is_whole_cells_from_its_start_and_the_ten_cell_marks_are_whole_cells_too() {
    let (sim, _) = sim();
    let view = sim.view();
    let mut hand = Hand(286);
    for kind in [UtilityKind::Water, UtilityKind::Power] {
        for n in 1..=25 {
            // The player drags the end out from where the start landed: the stroke wobbles and
            // the end is up to 3 m short or long.
            let mut t = tool(kind);
            let (q0, _) = click(&view, &mut t, A + hand.slop());
            let len = n as f32 * CELL + hand.rand() * SLOP_M;
            let b = q0 + hand.wobble(Vec2::X * len);
            let (q0, q1) = stroke_on(&view, &mut t, q0, b);
            assert!(
                (q1 - q0 - Vec2::X * (n as f32 * CELL)).length() < 1e-3,
                "{kind:?} {n} cells: {:?}",
                q1 - q0
            );
        }
        // Up to half a cell either side of the 10/20/30 marks: the mark.
        for n in [10.0, 20.0, 30.0] {
            for off in [-3.9, 3.9] {
                let a = A + Vec2::new(0.0, 200.0);
                let b = a + Vec2::new(n * CELL + off, 1.0);
                let (q0, q1) = stroke(&view, kind, a, b);
                assert!((q1.x - q0.x - n * CELL).abs() < 1e-3, "{n} {off}");
                assert!((q1.y - q0.y).abs() < 1e-3, "square to its start");
            }
        }
    }
}

#[test]
fn its_own_nodes_pull_within_a_cell_and_a_branch_leaves_a_whole_cell_of_its_link() {
    let (mut sim, _) = sim();
    let end = A + Vec2::X * (20.0 * CELL);
    build(&mut sim, UtilityKind::Water, vec![A, end]);
    let view = sim.view();
    let mut t = tool(UtilityKind::Water);
    // Within a cell of the end node: the node.
    let q = snapped_cursor(&view, &t, end + Vec2::new(5.0, 3.0), false).0;
    assert_eq!(q, end);
    // On the pipe, 7.3 cells along and 2 m off: the 7th cell.
    let q = snapped_cursor(&view, &t, A + Vec2::new(7.3 * CELL, 2.0), false).0;
    assert!((q - (A + Vec2::X * 7.0 * CELL)).length() < 1e-3, "{q:?}");
    // A branch from there leaves square to it, whole cells long.
    t.click(q);
    let g = compute_utility_ghost_alt(&view, &t, q + Vec2::new(1.5, 5.6 * CELL), false).unwrap();
    let r = g.resolved.unwrap();
    assert!((r - (q + Vec2::Y * 6.0 * CELL)).length() < 1e-3, "{r:?}");
    assert!(g.valid, "{:?}", g.errors);
    // A power line is not pulled by the pipe's node.
    let p = end + Vec2::new(5.0, 3.0);
    let q = snapped_cursor(&view, &tool(UtilityKind::Power), p, false).0;
    assert_eq!(q, p, "another network does not join");
}

#[test]
fn within_seven_degrees_of_square_it_is_square() {
    let (mut sim, _) = sim();
    let end = A + Vec2::X * (20.0 * CELL);
    build(&mut sim, UtilityKind::Power, vec![A, end]);
    let view = sim.view();
    let len = 12.0 * CELL;
    for (deg, square) in [(6.0f32, true), (-6.0, true), (10.0, false)] {
        let dir = Vec2::from_angle((90.0 + deg).to_radians());
        let (q0, q1) = stroke(&view, UtilityKind::Power, end, end + dir * len);
        assert_eq!(q0, end);
        let got = (q1 - q0).to_angle().to_degrees();
        if square {
            assert!((got - 90.0).abs() < 1e-3, "{deg}: {got}");
            assert!(((q1 - q0).length() - len).abs() < 1e-3);
        } else {
            assert!((got - 100.0).abs() < 0.5, "{deg}: {got}");
        }
    }
}

#[test]
fn a_road_guides_and_a_pipe_along_a_street_runs_beside_it() {
    let (mut sim, db) = sim();
    let r1 = A + Vec2::X * (20.0 * CELL);
    road(&mut sim, &db, A, r1);
    let view = sim.view();
    // The square through the road's end node: a pipe in the open ends on it, not on its own
    // whole cell (its start is 3 m off the road's cells).
    let s = A + Vec2::new(3.0, 40.0);
    let (_, q1) = stroke(
        &view,
        UtilityKind::Water,
        s,
        Vec2::new(r1.x + 2.0, s.y + 1.0),
    );
    assert!(
        (q1.x - r1.x).abs() < 1e-3,
        "on the square through the node: {q1:?}"
    );
    // Along the street, 12 m off its axis, wobbling: parallel to it, and never on it.
    let mut hand = Hand(7);
    for _ in 0..8 {
        let a = A + Vec2::new(CELL, 12.0) + hand.slop() * 0.3;
        let b = a + hand.wobble(Vec2::X * 120.0);
        let (q0, q1) = stroke(&view, UtilityKind::Water, a, b);
        assert!(
            (q1.y - q0.y).abs() < 1e-3,
            "parallel to the street: {q0:?} {q1:?}"
        );
        assert!(
            (q0.y - A.y).abs() > 8.0,
            "beside the street, not on its axis"
        );
    }
}

#[test]
fn the_cursor_is_a_one_cell_piece_at_the_point_the_click_puts_down() {
    let (mut sim, db) = sim();
    let house = station(&mut sim, &db, "police_station", A + Vec2::new(60.0, 60.0));
    let view = sim.view();
    let on_it = A + Vec2::new(60.0, 60.0);
    // A pole on a building: red; a pipe under it: green.
    let pole = compute_utility_ghost_alt(&view, &tool(UtilityKind::Power), on_it, false).unwrap();
    assert!(
        pole.cursor_piece && !pole.valid,
        "a pole may not stand on a building"
    );
    let pipe = compute_utility_ghost_alt(&view, &tool(UtilityKind::Water), on_it, false).unwrap();
    assert!(pipe.cursor_piece && pipe.valid, "{:?}", pipe.errors);
    assert_eq!(pipe.polylines[0].len(), 2);
    let piece = xz(pipe.polylines[0][0]).distance(xz(pipe.polylines[0][1]));
    assert!((piece - CELL).abs() < 1e-3, "one cell: {piece}");
    let _ = house;
    // The piece starts where the click puts the point, for pipe, line and road.
    for kind in [UtilityKind::Water, UtilityKind::Power] {
        let p = A + Vec2::new(13.3, -21.7);
        let g = compute_utility_ghost_alt(&view, &tool(kind), p, false).unwrap();
        let mut t = tool(kind);
        let (q, _) = click(&view, &mut t, p);
        assert_eq!(g.resolved, Some(q));
        assert_eq!(xz(g.polylines[0][0]), q);
    }
    let rt = RoadTool {
        mode: RoadMode::Straight,
        road: db.road_id("basic_2l"),
        ..Default::default()
    };
    let p = A + Vec2::new(13.3, -21.7);
    let g = road_cursor_piece(&view, &rt, p, false).unwrap();
    assert!(g.cursor_piece && g.valid, "{:?}", g.errors);
    let point = cp_tools::resolve_cursor(&view, &rt, p, false)
        .unwrap()
        .point;
    assert_eq!(g.resolved, Some(point));
    // Off the bought land: red, all three.
    let far = Vec2::new(100.0, 100.0);
    for kind in [UtilityKind::Water, UtilityKind::Power] {
        let g = compute_utility_ghost_alt(&view, &tool(kind), far, false).unwrap();
        assert!(!g.valid, "{kind:?} off the land");
    }
    assert!(!road_cursor_piece(&view, &rt, far, false).unwrap().valid);
}

#[test]
fn the_pipe_ghost_highlights_exactly_the_buildings_the_pipe_brings_into_reach() {
    let (mut sim, db) = sim();
    // Houses at 20..140 m north of the line the pipe will run along, and one far off.
    let homes: Vec<BuildingId> = [20.0f32, 50.0, 70.0, 100.0, 140.0, 400.0]
        .iter()
        .enumerate()
        .map(|(i, d)| {
            station(
                &mut sim,
                &db,
                "house_low_1",
                A + Vec2::new(40.0 * i as f32, -d),
            )
        })
        .collect();
    let path = vec![A, A + Vec2::X * (30.0 * CELL)];
    let view = sim.view();
    let mut t = tool(UtilityKind::Water);
    t.click(A);
    let g = compute_utility_ghost_alt(&view, &t, path[1], false).unwrap();
    assert!(g.valid, "{:?}", g.errors);
    assert!(
        !g.connect.is_empty() && g.connect.len() < homes.len(),
        "{:?}",
        g.connect
    );
    drop(view);
    build(&mut sim, UtilityKind::Water, path);
    let view = sim.view();
    let nets = &db.balance().networks;
    let cov = Coverage::new(&view.utilities, UtilityNet::Pipe, nets);
    let reached: Vec<BuildingId> = view
        .buildings
        .iter()
        .filter(|(_, b)| {
            let (w, d) = b.size();
            let half = f32::from(w.max(d)) * cp_building::FOOTPRINT_CELL_M * 0.5;
            cov.covers(&view.utilities, b.pos, half)
        })
        .map(|(id, _)| id)
        .collect();
    assert_eq!(g.connect, reached, "the highlight is what the pipe reaches");
}

/// An app with the tools' input systems, for B / Esc and a click.
fn app(view: SimView) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, InputPlugin, RtsCameraPlugin))
        .add_message::<WindowFocused>()
        .init_resource::<ToolState>()
        .init_resource::<CommandOutbox>()
        .insert_resource(view)
        .add_systems(Update, (tool_hotkeys, tool_click).chain());
    app
}

fn press(app: &mut App, code: KeyCode) {
    use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
    for state in [ButtonState::Pressed, ButtonState::Released] {
        app.world_mut().write_message(KeyboardInput {
            key_code: code,
            logical_key: Key::Unidentified(NativeKey::Unidentified),
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }
}

fn left_click(app: &mut App) {
    for state in [ButtonState::Pressed, ButtonState::Released] {
        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Left,
            state,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }
}

#[test]
fn the_bulldozer_in_a_utility_layer_takes_only_that_layer() {
    for (kind, layer, net) in [
        (UtilityKind::Water, BulldozeLayer::Water, UtilityNet::Pipe),
        (UtilityKind::Power, BulldozeLayer::Power, UtilityNet::Power),
    ] {
        let (mut sim, db) = sim();
        let r1 = A + Vec2::X * (20.0 * CELL);
        road(&mut sim, &db, A, r1);
        // A link across the road, under it (a pipe) or over it (a line).
        let x = A.x + 10.0 * CELL;
        build(
            &mut sim,
            kind,
            vec![Vec2::new(x, A.y - 40.0), Vec2::new(x, A.y + 40.0)],
        );
        let house = station(&mut sim, &db, "house_low_1", A + Vec2::new(40.0, -40.0));
        let mut app = app(sim.view());
        {
            let mut st = app.world_mut().resource_mut::<ToolState>();
            st.select(ActiveTool::Utility);
            st.utility.set_kind(kind);
        }
        press(&mut app, KeyCode::KeyB);
        let view = sim.view();
        {
            let mut st = app.world_mut().resource_mut::<ToolState>();
            assert_eq!(st.active, ActiveTool::Bulldozer);
            assert_eq!(st.bulldoze_layer, layer);
            // Over the house: nothing; over the road where the link crosses it: the link.
            bulldozer_hover(&mut st, &view, A + Vec2::new(40.0, -40.0));
            assert_eq!(st.building_highlight, None, "never a house");
            assert!(st.link_highlight.is_none() && st.highlight.is_empty());
            bulldozer_hover(&mut st, &view, Vec2::new(x, A.y));
            assert!(
                st.link_highlight.is_some(),
                "{kind:?}: the link under the cursor"
            );
            assert!(st.highlight.is_empty(), "never the road");
            st.hover = Some(Vec2::new(x, A.y));
        }
        left_click(&mut app);
        let mut cmds = std::mem::take(&mut *app.world_mut().resource_mut::<CommandOutbox>());
        assert!(
            cmds.iter()
                .all(|c| matches!(c, SimCommand::Utility(UtilityCommand::RemoveLink { .. }))),
            "only the link"
        );
        drop(view);
        let roads = sim.world().resource::<NetGraph>().segments().count();
        cmds.flush_into(&mut sim);
        sim.tick_once();
        let view = sim.view();
        assert_eq!(
            view.utilities.links().filter(|(_, l)| l.net == net).count(),
            0
        );
        assert_eq!(view.net.segments().count(), roads, "the road is intact");
        assert!(view.buildings.get(house).is_some());
        // B hands the tool back; so does Esc.
        press(&mut app, KeyCode::KeyB);
        assert_eq!(
            app.world().resource::<ToolState>().active,
            ActiveTool::Utility
        );
        assert_eq!(app.world().resource::<ToolState>().utility.kind, kind);
        press(&mut app, KeyCode::KeyB);
        press(&mut app, KeyCode::Escape);
        assert_eq!(
            app.world().resource::<ToolState>().active,
            ActiveTool::Utility
        );
    }
}
