//! `cp_ui` — egui panels (`architecture/system-design.md` §13.6).
//!
//! The UI reads [`SimView`] and never touches the sim: speed buttons push
//! `SimCommand::SetSpeed` into the [`CommandOutbox`] and the app submits it. Panel
//! functions take an `egui::Context` so they run (and are tested) without a window.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPreUpdateSet, EguiPrimaryContextPass, egui};
use cp_core::SimSpeed;
use cp_render::{LightingSettings, OverlayState};
use cp_sim::{SimCommand, SimView};
use cp_tools::{CommandOutbox, ToolState, UiCapture};

pub mod advisor;
pub mod budget;
pub mod buildings;
pub mod bulldozer_bar;
pub mod cursor_label;
pub mod debug;
pub mod demand_panel;
pub mod demolish_confirm;
pub mod districts;
pub mod files;
pub mod frame;
pub mod gauges;
mod glyphs;
pub mod grading;
pub mod icons;
pub mod industry;
pub mod info;
pub mod land;
pub mod lines;
pub mod menus;
pub mod metrics;
pub mod milestone;
pub mod overlays_panel;
pub mod palette;
pub mod panels;
pub mod parks;
pub mod policies;
pub mod roads;
pub mod service_marks;
pub mod services;
pub mod specs;
pub mod style;
pub mod tile;
pub mod tiles;
pub mod transit;
pub mod utility_legend;
pub mod windows;

pub use debug::{DebugData, PhaseTimes, TickRate};
pub use files::{FileMenu, FileRequest, SaveEntry, file_window};
pub use icons::Icon;
pub use info::{InfoRects, info_panel};
pub use palette::{Category, palette, tool_panel};
pub use panels::{
    AudioPrefs, Prefs, TopRects, UI_SCALE_RANGE, UiState, UiWindow, root_ui, top_bar,
};

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<UiState>()
            .add_message::<advisor::AdvisorShown>()
            .init_resource::<OverlayState>()
            .init_resource::<LightingSettings>()
            .init_resource::<cp_render::GradingSettings>()
            .init_resource::<TickRate>()
            .init_resource::<CommandOutbox>()
            .init_resource::<ToolState>()
            .init_resource::<UiCapture>()
            .init_resource::<FileMenu>()
            .init_resource::<cp_tools::Picked>()
            .init_resource::<cp_tools::Inspected>()
            // After egui has taken this frame's pointer position, before any `Update` system:
            // a click on a panel is already "over the UI" when the tools see it.
            .add_systems(
                PreUpdate,
                update_ui_capture.after(EguiPreUpdateSet::BeginPass),
            )
            .add_systems(
                Update,
                (
                    toggle_debug,
                    measure_tick_rate,
                    apply_camera_goal,
                    draw_stub_marker,
                ),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (
                    draw_ui,
                    grading::grading_ui,
                    districts::draw_labels,
                    land::land_overlay,
                    transit::transit_labels,
                    utility_legend::source_captions,
                    parks::draw_labels,
                    service_marks::draw_service_marks,
                )
                    .chain(),
            );
    }
}

/// The "show" button of the not-connected notice: moves the camera to the goal.
fn apply_camera_goal(mut state: ResMut<UiState>, mut cameras: Query<&mut cp_render::RtsCamera>) {
    let Some(goal) = state.camera_goal.take() else {
        return;
    };
    for mut cam in &mut cameras {
        cam.target.focus = goal;
    }
}

/// A pulsing ring over the dead end of the entry road while the city is not connected.
fn draw_stub_marker(view: Option<Res<SimView>>, time: Res<Time<Real>>, mut gizmos: Gizmos) {
    let Some(stub) = view.as_deref().and_then(demand_panel::unconnected_stub) else {
        return;
    };
    let Some(terrain) = view.as_deref().and_then(|v| v.terrain.as_deref()) else {
        return;
    };
    let pulse = 0.5 + 0.5 * (time.elapsed_secs() * 4.0).sin();
    let at = Vec3::new(stub.x, terrain.height(stub) + 1.0, stub.y);
    let iso = Isometry3d::new(at, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
    gizmos.circle(iso, 14.0 + 10.0 * pulse, Color::srgb(1.0, 0.85, 0.1));
}

fn toggle_debug(keys: Res<ButtonInput<KeyCode>>, mut state: ResMut<UiState>) {
    if keys.just_pressed(KeyCode::F3) {
        state.debug_open = !state.debug_open;
    }
    // City services (T-232).
    if keys.just_pressed(KeyCode::F4) {
        state.services_open = !state.services_open;
    }
    // The colour grading panel (T-273).
    if keys.just_pressed(KeyCode::F8) {
        state.grading_open = !state.grading_open;
    }
}

fn measure_tick_rate(
    time: Res<Time<Real>>,
    view: Option<Res<SimView>>,
    mut rate: ResMut<TickRate>,
) {
    if let Some(view) = view {
        rate.update(time.elapsed_secs_f64(), view.tick.0);
    }
}

/// Everything one frame of the UI reads and writes.
pub(crate) struct UiFrame<'a> {
    pub view: &'a SimView,
    pub state: &'a mut UiState,
    pub outbox: &'a mut CommandOutbox,
    pub overlay: &'a mut OverlayState,
    pub lighting: &'a mut LightingSettings,
    pub camera: &'a mut cp_render::CameraSettings,
    pub rate: &'a TickRate,
    pub extra: Option<&'a debug::PhaseTimes>,
    pub tools: &'a mut ToolState,
    pub net_debug: &'a mut cp_tools::NetDebug,
    pub files: &'a mut FileMenu,
    pub inspected: &'a cp_tools::Inspected,
    pub picked: &'a mut cp_tools::Picked,
    pub toast: &'a mut milestone::MilestoneWindow,
    pub advisor: &'a mut advisor::Advisor,
}

#[allow(clippy::too_many_arguments)]
fn draw_ui(
    mut contexts: EguiContexts,
    view: Option<Res<SimView>>,
    mut state: ResMut<UiState>,
    mut outbox: ResMut<CommandOutbox>,
    mut overlay: ResMut<OverlayState>,
    mut lighting: ResMut<LightingSettings>,
    mut camera: ResMut<cp_render::CameraSettings>,
    rate: Res<TickRate>,
    extra: Option<Res<debug::PhaseTimes>>,
    mut tools: ResMut<ToolState>,
    mut net_debug: ResMut<cp_tools::NetDebug>,
    mut files: ResMut<FileMenu>,
    inspected: Res<cp_tools::Inspected>,
    mut picked: ResMut<cp_tools::Picked>,
    mut local: Local<(bool, milestone::MilestoneWindow, advisor::Advisor)>,
    mut shown: MessageWriter<advisor::AdvisorShown>,
) -> Result {
    let Some(view) = view else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let (loaders_installed, toast, advisor) = &mut *local;
    if !*loaders_installed {
        egui_extras::install_image_loaders(ctx);
        style::install(ctx);
        *loaders_installed = true;
    }
    draw_frame(
        ctx,
        UiFrame {
            view: &view,
            state: &mut state,
            outbox: &mut outbox,
            overlay: &mut overlay,
            lighting: &mut lighting,
            camera: &mut camera,
            rate: &rate,
            extra: extra.as_deref(),
            tools: &mut tools,
            net_debug: &mut net_debug,
            files: &mut files,
            inspected: &inspected,
            picked: &mut picked,
            toast,
            advisor,
        },
    );
    if let Some(severity) = advisor.just_shown.take() {
        shown.write(advisor::AdvisorShown { severity });
    }
    Ok(())
}

/// The whole UI of one frame, drawn into `ctx`.
pub(crate) fn draw_frame(ctx: &egui::Context, f: UiFrame) {
    let UiFrame {
        view,
        state,
        outbox,
        overlay,
        lighting,
        camera,
        rate,
        extra,
        tools,
        net_debug,
        files,
        inspected,
        picked,
        toast,
        advisor,
    } = f;
    let available = Category::available(view.content.as_deref());
    if let Some(c) = palette::hotkey(ctx, &available) {
        palette::toggle(state, c);
    }
    keyboard_shortcuts(ctx, state, tools, overlay, outbox);
    if panels::view_hotkey(ctx) {
        state.overlays_collapsed = !state.overlays_collapsed;
    }
    let mut root = root_ui(ctx);
    top_bar(&mut root, view, outbox);
    // The settings menu edits `state.prefs`; push it out to where it takes effect (T-069).
    if *camera != state.prefs.camera {
        *camera = state.prefs.camera.clone();
    }
    if (ctx.zoom_factor() - state.prefs.ui_scale).abs() > f32::EPSILON {
        ctx.set_zoom_factor(state.prefs.ui_scale);
    }
    palette(&mut root, state, view.content.as_deref());
    menus::apply(state, tools, overlay, Some(view));
    tool_panel(&mut root, state, Some(view), tools, overlay, outbox);
    demand_panel::demand_panel(&mut root, view, state, tools, overlay);
    overlays_panel::overlays_panel(ctx, state, overlay, lighting, files);
    remember_map_rect(ctx, &root);
    bulldozer_bar::bulldozer_bar(ctx, tools);
    cursor_label::cursor_label(ctx, tools);
    {
        let st = &mut *state;
        budget::budget_window(ctx, &mut st.budget_open, view, &mut st.budget, outbox);
        windows::demand_window(ctx, &mut st.demand_open, view);
        metrics::metrics_window(ctx, &mut st.metrics_open, view);
        services::services_window(ctx, &mut st.services_open, view, overlay);
        services::overlay_services_panel(ctx, view, overlay.active, overlays_panel::width(st));
        lines::lines_window(ctx, &mut st.lines_open, view, &mut st.lines, outbox);
        policies::city_policies_window(ctx, &mut st.policies_open, view, outbox);
    }
    if state.debug_open {
        debug::debug_window(
            ctx,
            &DebugData {
                view,
                ticks_per_sec: rate.per_sec(),
                phases: extra,
            },
            net_debug,
            &mut state.debug_open,
        );
    }
    file_window(ctx, files);
    milestone::milestone_window(ctx, view, toast, outbox);
    advisor::advisor_snackbar(ctx, view, state, overlay, advisor);
    land::confirm_window(ctx, view, &mut state.budget.buy_tile, outbox);
    demolish_confirm::confirm_window(ctx, tools, outbox);
    info_panel(ctx, inspected, picked, Some(view), tools, outbox);
    let line_out = lines::line_info_panel(ctx, view, picked, &mut state.lines, outbox);
    if let Some(line) = line_out.edit {
        // The tool resets itself when it is selected, so fill it afterwards.
        tools.select(cp_tools::ActiveTool::Line);
        tools.line.begin_edit(view, line);
        overlay.open_auto(cp_render::Overlay::Transit);
        picked.0 = None;
    }
    menus::remember(state, tools, view.content.as_deref());
    // An overlay a tool opened by itself goes with that tool (BUG-083).
    panels::release_auto_overlay(state, tools, overlay);
}

/// What egui owns, given the pointer position of the frame that just began and the layout of
/// the last drawn one: `(pointer, keyboard)`.
pub fn egui_owns_input(ctx: &egui::Context) -> (bool, bool) {
    (
        ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input() || pointer_over_panels(ctx),
        ctx.egui_wants_keyboard_input(),
    )
}

/// Where the map shows between the docked panels, as of the last frame (BUG-035).
const MAP_RECT: &str = "cp_map_rect";

/// Remembers the part of the screen the docked panels left free. The panels are drawn into a
/// background-layer `Ui`, which egui's own `is_pointer_over_egui` does not count, so the
/// pointer is "over the UI" when it is on the screen but outside this rectangle.
fn remember_map_rect(ctx: &egui::Context, root: &egui::Ui) {
    let free = root.available_rect_before_wrap();
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(MAP_RECT), free));
}

/// The pointer is on a docked panel (top bar, palette, tool panel, demand bar).
pub fn pointer_over_panels(ctx: &egui::Context) -> bool {
    let (Some(pos), Some(free)) = (
        ctx.pointer_latest_pos(),
        ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(MAP_RECT))),
    ) else {
        return false;
    };
    ctx.viewport_rect().contains(pos) && !free.contains(pos)
}

/// Writes [`UiCapture`] for the camera and the tools. Runs in `PreUpdate`, right after
/// egui began its pass with this frame's input; the flag used to be written at the end of
/// the previous frame's pass, so a click that moved the pointer onto a button the same
/// frame was still "over the map".
fn update_ui_capture(mut contexts: EguiContexts, mut capture: ResMut<UiCapture>) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    (capture.pointer_over_ui, capture.keyboard_captured) = egui_owns_input(ctx);
}

/// Bare 1 / 2 / 3 set the game speed and start the simulation; Esc, when no tool is active,
/// steps back to the default view: first the open palette panel, then the overlay (BUG-059).
/// Nothing happens while a text field has the keyboard. The tool's own Esc (`cp_tools`) runs in
/// the same frame, so a tool that was active a frame ago counts as active.
pub(crate) fn keyboard_shortcuts(
    ctx: &egui::Context,
    state: &mut panels::UiState,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    outbox: &mut CommandOutbox,
) {
    let had_tool = std::mem::replace(
        &mut state.tool_was_active,
        tools.active != cp_tools::ActiveTool::None,
    );
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let pressed = |k: egui::Key| {
        ctx.input(|i| {
            i.events.iter().any(|e| {
                matches!(e, egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. }
                    if *key == k && !modifiers.any())
            })
        })
    };
    for (k, speed) in [
        (egui::Key::Num1, SimSpeed::X1),
        (egui::Key::Num2, SimSpeed::X2),
        (egui::Key::Num3, SimSpeed::X3),
    ] {
        if pressed(k) {
            tools.resume_speed = speed;
            outbox.push(set_speed(speed));
        }
    }
    if pressed(egui::Key::Escape) && !had_tool && tools.active == cp_tools::ActiveTool::None {
        // BUG-077: the layout closes before the build menu (tool -> layout -> menu).
        if overlay.active != cp_render::Overlay::Off {
            overlay.close();
        } else if state.category.is_some() {
            state.category = None;
        }
    }
}

/// What a speed button sends.
pub fn set_speed(speed: SimSpeed) -> SimCommand {
    SimCommand::SetSpeed { speed }
}

#[cfg(test)]
pub(crate) mod testutil {
    use bevy_egui::egui;

    /// One headless egui frame; the texture upload egui wants handled is discarded.
    pub fn frame(ctx: &egui::Context, input: egui::RawInput, f: impl FnMut(&mut egui::Ui)) {
        let mut out = ctx.run_ui(input, f);
        out.textures_delta.clear();
    }

    /// A 1280×720 screen at time `t` with `events`.
    pub fn input(t: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            time: Some(t),
            events,
            ..Default::default()
        }
    }

    /// Like [`frame`], returning all text the frame painted with the position it starts at.
    pub fn frame_texts(
        ctx: &egui::Context,
        input: egui::RawInput,
        f: impl FnMut(&mut egui::Ui),
    ) -> Vec<(String, egui::Pos2)> {
        fn collect(shape: &egui::epaint::Shape, out: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::epaint::Shape::Text(t) => out.push((t.galley.text().to_string(), t.pos)),
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let mut full = ctx.run_ui(input, f);
        full.textures_delta.clear();
        let mut texts = Vec::new();
        for clipped in &full.shapes {
            collect(&clipped.shape, &mut texts);
        }
        texts
    }

    /// Press and release events of the primary button at `pos` (move + press, then release).
    pub fn click(pos: egui::Pos2) -> [Vec<egui::Event>; 2] {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        [
            vec![egui::Event::PointerMoved(pos), button(true)],
            vec![button(false)],
        ]
    }

    /// A press and release of `key`.
    pub fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;
    use crate::testutil::{frame, input};

    fn panel(ui: &mut egui::Ui) {
        egui::Window::new("p")
            .fixed_pos(egui::pos2(100.0, 100.0))
            .fixed_size(egui::vec2(200.0, 200.0))
            .show(ui.ctx(), |ui| {
                ui.label("panel");
            });
    }

    /// The flag is read at the start of the frame the click arrives in, when the pointer has
    /// just jumped onto the panel: the layout is the previous frame's, the position is new.
    #[test]
    fn a_pointer_that_jumps_onto_a_panel_is_over_the_ui_in_the_same_frame() {
        let ctx = egui::Context::default();
        frame(&ctx, input(0.0, vec![]), panel);
        frame(&ctx, input(0.05, vec![]), panel);
        assert_eq!(egui_owns_input(&ctx), (false, false));
        // Checked first thing in the pass, before the panel is drawn again.
        let at = |t: f64, pos: egui::Pos2| {
            let mut owned = (false, false);
            let events = vec![egui::Event::PointerMoved(pos)];
            frame(&ctx, input(t, events), |ui| {
                owned = egui_owns_input(ui.ctx());
                panel(ui);
            });
            owned
        };
        assert!(
            at(0.1, egui::pos2(150.0, 150.0)).0,
            "over the panel at the frame start"
        );
        assert!(!at(0.2, egui::pos2(900.0, 500.0)).0, "back over the map");
    }

    #[test]
    fn a_focused_text_field_owns_the_keyboard() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        let mut field = |ui: &mut egui::Ui| {
            let r = ui.text_edit_singleline(&mut text);
            r.request_focus();
        };
        frame(&ctx, input(0.0, vec![]), &mut field);
        frame(&ctx, input(0.1, vec![]), &mut field);
        assert!(egui_owns_input(&ctx).1);
    }

    /// Everything a frame of the real UI reads and writes.
    struct Rig {
        view: SimView,
        state: UiState,
        outbox: CommandOutbox,
        overlay: OverlayState,
        lighting: LightingSettings,
        camera: cp_render::CameraSettings,
        rate: TickRate,
        tools: ToolState,
        files: FileMenu,
        net_debug: cp_tools::NetDebug,
        inspected: cp_tools::Inspected,
        picked: cp_tools::Picked,
        toast: milestone::MilestoneWindow,
        advisor: advisor::Advisor,
    }

    impl Rig {
        fn new() -> Self {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let db = std::sync::Arc::new(
                cp_content::ContentDb::load_dir(root.join("assets/content")).unwrap(),
            );
            let mut sim = cp_sim::SimRunner::new(1);
            sim.set_content(db);
            sim.tick_once();
            Rig {
                view: sim.view(),
                state: UiState::default(),
                outbox: CommandOutbox::default(),
                overlay: OverlayState::default(),
                lighting: LightingSettings::default(),
                camera: cp_render::CameraSettings::default(),
                rate: TickRate::default(),
                tools: ToolState::default(),
                net_debug: cp_tools::NetDebug::default(),
                files: FileMenu::default(),
                inspected: cp_tools::Inspected::default(),
                picked: cp_tools::Picked::default(),
                toast: milestone::MilestoneWindow::default(),
                advisor: advisor::Advisor::default(),
            }
        }

        fn draw(&mut self, ctx: &egui::Context) {
            draw_frame(
                ctx,
                UiFrame {
                    view: &self.view,
                    state: &mut self.state,
                    outbox: &mut self.outbox,
                    overlay: &mut self.overlay,
                    lighting: &mut self.lighting,
                    camera: &mut self.camera,
                    rate: &self.rate,
                    extra: None,
                    tools: &mut self.tools,
                    net_debug: &mut self.net_debug,
                    files: &mut self.files,
                    inspected: &self.inspected,
                    picked: &mut self.picked,
                    toast: &mut self.toast,
                    advisor: &mut self.advisor,
                },
            );
        }

        /// The pointer positions of a 80 px grid over the screen that the UI owns at the start
        /// of a frame (what `update_ui_capture` would write), after the UI settled.
        fn owned(&mut self) -> Vec<(u32, u32)> {
            let ctx = egui::Context::default();
            egui_extras::install_image_loaders(&ctx);
            let mut t = 0.0;
            for _ in 0..4 {
                t += 0.05;
                frame(&ctx, input(t, vec![]), |ui| self.draw(ui.ctx()));
            }
            let mut out = Vec::new();
            for y in (8..640).step_by(80).chain([648, 710]) {
                for x in (8..1280).step_by(80) {
                    t += 0.05;
                    let mut owned = false;
                    let ev = vec![egui::Event::PointerMoved(egui::pos2(x as f32, y as f32))];
                    frame(&ctx, input(t, ev), |ui| {
                        owned = egui_owns_input(ui.ctx()).0;
                        self.draw(ui.ctx());
                    });
                    if owned {
                        out.push((x, y));
                    }
                }
            }
            out
        }
    }

    fn row(points: &[(u32, u32)], y: u32) -> usize {
        points.iter().filter(|p| p.1 == y).count()
    }

    #[test]
    fn the_docked_panels_own_the_pointer_and_the_map_between_them_does_not() {
        let mut rig = Rig::new();
        let base = rig.owned();
        // Top bar and bottom demand bar across the whole width.
        assert_eq!(row(&base, 8), 16, "top bar");
        // T-289: the palette, and the overlays panel at the top right (two points of the grid).
        assert_eq!(
            row(&base, 328),
            3,
            "the palette and the overlays panel are owned in the middle row"
        );
        assert_eq!(row(&base, 710), 16, "bottom bar");
        // The palette on the left.
        assert!(base.iter().any(|p| p.0 == 8 && p.1 == 328), "palette");
        // The middle of the map is the map's.
        assert!(
            !base
                .iter()
                .any(|p| (408..=808).contains(&p.0) && (168..=568).contains(&p.1))
        );
        // The tool panel of an open category adds to what the UI owns.
        rig.state.category = Some(Category::Roads);
        assert!(rig.owned().len() > base.len(), "tool panel");
    }

    #[test]
    fn every_window_and_menu_owns_the_pointer_where_it_is_drawn() {
        let base = Rig::new().owned().len();
        type Open = fn(&mut Rig);
        let surfaces: [(&str, Open); 9] = [
            ("budget", |r| r.state.budget_open = true),
            ("demand", |r| r.state.demand_open = true),
            ("metrics", |r| r.state.metrics_open = true),
            ("services", |r| r.state.services_open = true),
            ("lines", |r| r.state.lines_open = true),
            ("policies", |r| r.state.policies_open = true),
            ("debug", |r| r.state.debug_open = true),
            ("saves", |r| r.files.open = true),
            ("settings", |r| r.state.settings_open = true),
        ];
        for (name, open) in surfaces {
            let mut rig = Rig::new();
            open(&mut rig);
            let n = rig.owned().len();
            assert!(n > base, "{name} owns no pointer area ({n} vs {base})");
        }
    }

    /// The texts one full frame of the UI paints, with where each starts (the UI settled).
    fn texts(rig: &mut Rig) -> Vec<(String, egui::Pos2)> {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let mut out = Vec::new();
        for n in 0..5 {
            out =
                crate::testutil::frame_texts(&ctx, input(0.05 * f64::from(n + 1), vec![]), |ui| {
                    rig.draw(ui.ctx())
                });
        }
        out
    }

    /// T-289: the balance and the population are at the left of the bottom panel, not in the top
    /// bar; the bottom-left legend of the power and water overlays is gone (T-279).
    #[test]
    fn money_and_population_are_in_the_bottom_panel_and_no_legend_stands_at_the_bottom_left() {
        let mut rig = Rig::new();
        rig.overlay.open(cp_render::Overlay::Power);
        let all = texts(&mut rig);
        let money = all
            .iter()
            .find(|(t, _)| t.starts_with("💰"))
            .expect("money");
        let pop = all
            .iter()
            .find(|(t, _)| t.starts_with("☻"))
            .expect("population");
        for (what, at) in [("money", money.1), ("population", pop.1)] {
            assert!(at.y > 560.0, "{what} is in the bottom panel, not at {at:?}");
            assert!(at.x < 420.0, "{what} is at the left, not at {at:?}");
        }
        for (t, at) in &all {
            assert!(
                !t.contains("supply / demand (") && !t.starts_with("Network 1"),
                "a legend: {t:?} at {at:?}"
            );
        }
        // The clock and the speeds are all the top bar holds.
        assert!(all.iter().any(|(t, at)| at.y < 44.0 && t.contains("day")));
        assert!(!all.iter().any(|(t, at)| at.y < 44.0 && t.starts_with("💰")));
    }

    #[test]
    fn the_collapsed_demand_bar_still_owns_the_bottom_strip() {
        let mut rig = Rig::new();
        rig.state.demand_collapsed = true;
        let owned = rig.owned();
        assert_eq!(row(&owned, 710), 16);
        assert_eq!(
            row(&owned, 648),
            1,
            "the strip is thin: the map is free above it"
        );
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;
    use crate::testutil::{frame, input, key};

    struct Rig {
        ctx: egui::Context,
        state: panels::UiState,
        tools: ToolState,
        overlay: OverlayState,
        outbox: CommandOutbox,
        t: f64,
    }

    impl Rig {
        fn new() -> Self {
            Rig {
                ctx: egui::Context::default(),
                state: panels::UiState::default(),
                tools: ToolState::default(),
                overlay: OverlayState::default(),
                outbox: CommandOutbox::default(),
                t: 0.0,
            }
        }

        fn press(&mut self, k: egui::Key) {
            self.t += 0.1;
            let Rig {
                ctx,
                state,
                tools,
                overlay,
                outbox,
                t,
            } = self;
            frame(ctx, input(*t, vec![key(k)]), |ui| {
                keyboard_shortcuts(ui.ctx(), state, tools, overlay, outbox);
            });
            // Let go, or egui reads the next press as a repeat.
            let up = egui::Event::Key {
                key: k,
                physical_key: Some(k),
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            *t += 0.05;
            frame(ctx, input(*t, vec![up]), |_| {});
        }
    }

    fn speeds(rig: &Rig) -> Vec<SimSpeed> {
        rig.outbox
            .iter()
            .filter_map(|c| match c {
                SimCommand::SetSpeed { speed } => Some(*speed),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn keys_1_2_3_set_the_speed_and_unpause() {
        let mut rig = Rig::new();
        rig.press(egui::Key::Num2);
        rig.press(egui::Key::Num1);
        rig.press(egui::Key::Num3);
        rig.press(egui::Key::Num4);
        assert_eq!(speeds(&rig), [SimSpeed::X2, SimSpeed::X1, SimSpeed::X3]);
        assert_eq!(rig.tools.resume_speed, SimSpeed::X3);
    }

    #[test]
    fn esc_without_a_tool_resets_the_overlay_then_the_panel() {
        let mut rig = Rig::new();
        rig.overlay.active = cp_render::Overlay::Water;
        rig.state.category = Some(palette::Category::Roads);
        rig.press(egui::Key::Escape);
        assert_eq!(rig.overlay.active, cp_render::Overlay::Off);
        assert_eq!(rig.state.category, Some(palette::Category::Roads));
        rig.press(egui::Key::Escape);
        assert_eq!(rig.state.category, None);
        rig.press(egui::Key::Escape);
        assert_eq!(rig.state.category, None);
    }

    #[test]
    fn esc_with_a_tool_leaves_the_overlay_and_the_clearing_esc_does_not_reset_it() {
        let mut rig = Rig::new();
        rig.overlay.active = cp_render::Overlay::Power;
        rig.tools.select(cp_tools::ActiveTool::Road);
        rig.press(egui::Key::Escape);
        assert_eq!(rig.overlay.active, cp_render::Overlay::Power, "tool active");
        // `cp_tools` clears the tool in the same frame as the Esc; the overlay stays.
        rig.tools.select(cp_tools::ActiveTool::None);
        rig.press(egui::Key::Escape);
        assert_eq!(rig.overlay.active, cp_render::Overlay::Power, "same Esc");
        rig.press(egui::Key::Escape);
        assert_eq!(rig.overlay.active, cp_render::Overlay::Off);
    }

    /// BUG-077 with BUG-083: an automatic layout is closed by Esc in the same order as a manual
    /// one — tool, layout, menu — and leaving the tool for none does not close it.
    #[test]
    fn esc_closes_an_automatic_layout_after_the_tool_and_before_the_menu() {
        let mut rig = Rig::new();
        rig.state.category = Some(palette::Category::Buildings);
        rig.tools.select(cp_tools::ActiveTool::Utility);
        rig.overlay.open_auto(cp_render::Overlay::Water);
        let frame = |rig: &mut Rig| {
            panels::release_auto_overlay(&mut rig.state, &rig.tools, &mut rig.overlay)
        };
        frame(&mut rig);
        rig.press(egui::Key::Escape);
        assert_eq!(rig.overlay.active, cp_render::Overlay::Water, "tool active");
        // `cp_tools` clears the tool in the same frame as the Esc.
        rig.tools.select(cp_tools::ActiveTool::None);
        frame(&mut rig);
        assert_eq!(
            rig.overlay.active,
            cp_render::Overlay::Water,
            "tool left for none"
        );
        rig.press(egui::Key::Escape);
        assert_eq!(
            rig.overlay.active,
            cp_render::Overlay::Water,
            "the clearing Esc"
        );
        rig.press(egui::Key::Escape);
        assert_eq!(rig.overlay.active, cp_render::Overlay::Off);
        assert_eq!(rig.state.category, Some(palette::Category::Buildings));
        rig.press(egui::Key::Escape);
        assert_eq!(rig.state.category, None);
    }

    #[test]
    fn digits_typed_into_a_text_field_do_nothing() {
        let mut rig = Rig::new();
        let mut text = String::new();
        let id = egui::Id::new("rename");
        for t in [0.0, 0.1] {
            frame(&rig.ctx, input(t, vec![]), |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut text).id(id));
                r.request_focus();
            });
        }
        rig.t = 1.0;
        let Rig {
            ctx,
            state,
            tools,
            overlay,
            outbox,
            ..
        } = &mut rig;
        frame(ctx, input(1.0, vec![key(egui::Key::Num2)]), |ui| {
            let _ = ui.add(egui::TextEdit::singleline(&mut text).id(id));
            keyboard_shortcuts(ui.ctx(), state, tools, overlay, outbox);
        });
        assert!(outbox.is_empty());
    }
}
