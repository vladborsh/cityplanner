//! `cp_ui` — egui panels (`architecture/system-design.md` §13.6).
//!
//! The UI reads [`SimView`] and never touches the sim: speed buttons push
//! `SimCommand::SetSpeed` into the [`CommandOutbox`] and the app submits it. Panel
//! functions take an `egui::Context` so they run (and are tested) without a window.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use cp_core::SimSpeed;
use cp_render::{LightingSettings, OverlayState};
use cp_sim::{SimCommand, SimView};
use cp_tools::{CommandOutbox, ToolState, UiCapture};

pub mod budget;
pub mod debug;
pub mod files;
pub mod frame;
pub mod icons;
pub mod info;
pub mod metrics;
pub mod palette;
pub mod panels;
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
    mut capture: ResMut<UiCapture>,
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
    tool_panel(&mut root, &state, Some(&view), &mut tools, &mut overlay);
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
    // Clicks over a panel or window belong to the UI, not to the tools.
    capture.pointer_over_ui = ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input();
    Ok(())
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
