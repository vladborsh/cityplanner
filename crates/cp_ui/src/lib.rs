//! `cp_ui` — egui panels (`architecture/system-design.md` §13.6).
//!
//! The UI reads [`SimView`] and never touches the sim: speed buttons push
//! `SimCommand::SetSpeed` into the [`CommandOutbox`] and the app submits it. Panel
//! functions take an `egui::Context` so they run (and are tested) without a window.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use cp_core::SimSpeed;
use cp_render::OverlayState;
use cp_sim::{SimCommand, SimView};
use cp_tools::{CommandOutbox, ToolState, UiCapture};

pub mod debug;
pub mod panels;

pub use debug::{DebugData, PhaseTimes, TickRate};
pub use panels::{Category, UiState, bottom_bar, overlay_buttons, root_ui, top_bar};

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<UiState>()
            .init_resource::<OverlayState>()
            .init_resource::<TickRate>()
            .init_resource::<CommandOutbox>()
            .init_resource::<ToolState>()
            .init_resource::<UiCapture>()
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
    rate: Res<TickRate>,
    extra: Option<Res<debug::PhaseTimes>>,
    mut tools: ResMut<ToolState>,
    mut capture: ResMut<UiCapture>,
) -> Result {
    let Some(view) = view else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let mut root = root_ui(ctx);
    top_bar(&mut root, &view, &mut outbox, &mut overlay);
    bottom_bar(&mut root, &mut state, Some(&view), &mut tools, &mut overlay);
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
}
