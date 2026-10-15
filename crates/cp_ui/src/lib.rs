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

pub mod budget;
pub mod buildings;
pub mod debug;
pub mod files;
pub mod frame;
pub mod icons;
pub mod info;
pub mod metrics;
pub mod net_summary;
pub mod palette;
pub mod panels;
pub mod roads;
pub mod windows;

pub use debug::{DebugData, PhaseTimes, TickRate};
pub use files::{FileMenu, FileRequest, SaveEntry, file_window};
pub use icons::Icon;
pub use info::{InfoRects, info_panel};
pub use palette::{Category, palette, tool_panel};
pub use panels::{TopRects, UiState, UiWindow, overlay_buttons, root_ui, top_bar};

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<UiState>()
            .init_resource::<OverlayState>()
            .init_resource::<LightingSettings>()
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
            .add_systems(Update, (toggle_debug, measure_tick_rate))
            .add_systems(EguiPrimaryContextPass, draw_ui);
    }
}

fn toggle_debug(keys: Res<ButtonInput<KeyCode>>, mut state: ResMut<UiState>) {
    if keys.just_pressed(KeyCode::F3) {
        state.debug_open = !state.debug_open;
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

#[allow(clippy::too_many_arguments)]
fn draw_ui(
    mut contexts: EguiContexts,
    view: Option<Res<SimView>>,
    mut state: ResMut<UiState>,
    mut outbox: ResMut<CommandOutbox>,
    mut overlay: ResMut<OverlayState>,
    mut lighting: ResMut<LightingSettings>,
    rate: Res<TickRate>,
    extra: Option<Res<debug::PhaseTimes>>,
    mut tools: ResMut<ToolState>,
    mut files: ResMut<FileMenu>,
    inspected: Res<cp_tools::Inspected>,
    mut picked: ResMut<cp_tools::Picked>,
    mut loaders_installed: Local<bool>,
) -> Result {
    let Some(view) = view else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    if !*loaders_installed {
        egui_extras::install_image_loaders(ctx);
        *loaders_installed = true;
    }
    let available = Category::available(view.content.as_deref());
    if let Some(c) = palette::hotkey(ctx, &available) {
        palette::toggle(&mut state, c);
    }
    let mut root = root_ui(ctx);
    top_bar(
        &mut root,
        &view,
        &mut outbox,
        &mut overlay,
        &mut lighting,
        &mut state,
        &mut files,
    );
    palette(&mut root, &mut state, view.content.as_deref());
    tool_panel(&mut root, &mut state, Some(&view), &mut tools, &mut overlay);
    {
        let st = &mut *state;
        budget::budget_window(ctx, &mut st.budget_open, &view, &mut st.budget, &mut outbox);
        windows::demand_window(ctx, &mut st.demand_open, &view);
        metrics::metrics_window(ctx, &mut st.metrics_open, &view);
    }
    if state.debug_open {
        debug::debug_window(
            ctx,
            &DebugData {
                view: &view,
                ticks_per_sec: rate.per_sec(),
                phases: extra.as_deref(),
            },
            &mut state.debug_open,
        );
    }
    file_window(ctx, &mut files);
    info_panel(ctx, &inspected, &mut picked);
    Ok(())
}

/// What egui owns, given the pointer position of the frame that just began and the layout of
/// the last drawn one: `(pointer, keyboard)`.
pub fn egui_owns_input(ctx: &egui::Context) -> (bool, bool) {
    (
        ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input(),
        ctx.egui_wants_keyboard_input(),
    )
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
}
