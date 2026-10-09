use bevy::diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::window::PresentMode;
use cp_app::GamePlugin;
use cp_core::SimLimits;

/// Value of `?key=` in the page URL.
fn query_param(key: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    search
        .trim_start_matches('?')
        .split('&')
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
        .map(str::to_owned)
}

pub fn run() {
    let map = query_param("map").unwrap_or_else(|| "flatlands".into());
    let hour = query_param("hour")
        .and_then(|h| h.parse().ok())
        .unwrap_or(10.0);
    let scene = query_param("scene");
    let cam = query_param("cam").unwrap_or_else(|| "overview".into());
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                canvas: Some("#bevy".into()),
                fit_canvas_to_parent: true,
                prevent_default_event_handling: true,
                present_mode: PresentMode::AutoVsync,
                ..default()
            }),
            ..default()
        }))
        .add_plugins((
            GamePlugin {
                map,
                limits: SimLimits::wasm(),
                hour,
                cam,
                // std::time::Instant is unavailable in the browser.
                phase_timing: false,
                ui: true,
                scene,
            },
            FrameTimeDiagnosticsPlugin::default(),
            // FPS and frame time to the browser console every 5 s.
            LogDiagnosticsPlugin {
                wait_duration: std::time::Duration::from_secs(5),
                ..default()
            },
        ))
        .run();
}
