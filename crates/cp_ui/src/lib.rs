//! `cp_ui` — egui panels (`architecture/system-design.md` §13.6).
//!
//! The UI reads [`SimView`] and never touches the sim: speed buttons push
//! `SimCommand::SetSpeed` into the [`CommandOutbox`] and the app submits it. Panel
//! functions take an `egui::Context` so they run (and are tested) without a window.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use cp_core::SimSpeed;
use cp_sim::{SimCommand, SimView};
use cp_tools::CommandOutbox;

pub mod debug;
pub mod panels;

pub use debug::{DebugData, TickRate};
pub use panels::{Category, UiState, bottom_bar, root_ui, top_bar};

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<UiState>()
            .init_resource::<TickRate>()
            .init_resource::<CommandOutbox>()
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

fn draw_ui(
    mut contexts: EguiContexts,
    view: Option<Res<SimView>>,
    mut state: ResMut<UiState>,
    mut outbox: ResMut<CommandOutbox>,
    rate: Res<TickRate>,
    extra: Option<Res<debug::PhaseTimes>>,
) -> Result {
    let Some(view) = view else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let mut root = root_ui(ctx);
    top_bar(&mut root, &view, &mut outbox);
    bottom_bar(&mut root, &mut state);
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
