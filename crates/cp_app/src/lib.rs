//! `cp_app` — the game app shared by `apps/desktop` and `apps/web`.
//!
//! [`GamePlugin`] loads a map through Bevy's asset server (file IO natively, HTTP `fetch`
//! in the browser), builds the sim from it, steps `SimRunner` in `PreUpdate`, publishes
//! [`SimView`], and adds terrain rendering, the RTS camera and the UI. The binaries only add
//! what is platform specific: windowing, CLI, screenshots and benchmarks.

mod map;
pub mod scene;

pub use map::{MapLoadFailed, RawFile};

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{SimLimits, Tick};
use cp_prof::{PhaseClock, PhaseTiming};
use cp_render::{
    BuildingRenderPlugin, OverlayPlugin, RoadRenderPlugin, RtsCamera, RtsCameraPlugin,
    TerrainRenderPlugin,
};
use cp_sim::{SimPhase, SimRunner, SimView};
use cp_terrain::{MAP_SIZE, Terrain};
use cp_tools::CommandOutbox;
use cp_ui::{PhaseTimes, UiPlugin};

/// Ticks of wall-clock phase timing kept for the F3 window.
pub const PHASE_WINDOW_TICKS: usize = 256;

/// The sim, owned by the Bevy app. Exists once the map has loaded.
#[derive(Resource)]
pub struct Sim(pub SimRunner);

pub struct GamePlugin {
    /// Directory name under `assets/maps/`.
    pub map: String,
    pub limits: SimLimits,
    /// Starting hour of the game day, `0..24`.
    pub hour: f32,
    /// Camera preset, see [`camera_pose`].
    pub cam: String,
    /// Record wall-clock time per sim phase for the F3 window. Off in the browser, where
    /// `std::time::Instant` is unavailable.
    pub phase_timing: bool,
    /// Add the egui UI (`cp_ui`). The browser build cannot yet: see `apps/web/README.md`.
    pub ui: bool,
    /// Optional test scene, e.g. `grid:10` (see [`scene`]).
    pub scene: Option<String>,
    /// Render into this image instead of a window (`--hidden`: no window exists).
    pub target: Option<Handle<Image>>,
}

#[derive(Resource, Clone)]
pub(crate) struct GameConfig {
    pub map: String,
    pub limits: SimLimits,
    pub hour: f32,
    pub phase_timing: bool,
    pub scene: Option<String>,
}

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        let (pos, look) = camera_pose(&self.cam);
        let target = self.target.clone();
        app.insert_resource(GameConfig {
            map: self.map.clone(),
            limits: self.limits,
            hour: self.hour,
            phase_timing: self.phase_timing,
            scene: self.scene.clone(),
        })
        .init_resource::<PhaseTimes>()
        .init_resource::<CommandOutbox>()
        .add_plugins((
            map::MapPlugin,
            TerrainRenderPlugin,
            RoadRenderPlugin,
            OverlayPlugin,
            BuildingRenderPlugin,
            RtsCameraPlugin,
        ))
        .add_systems(PreUpdate, step_sim);
        if self.ui {
            app.add_plugins((UiPlugin, cp_tools::ToolsPlugin));
        }
        app.add_systems(Startup, move |mut commands: Commands| {
            let mut camera = commands.spawn((
                Camera3d::default(),
                Projection::Perspective(PerspectiveProjection {
                    far: 40_000.0,
                    near: 1.0,
                    ..default()
                }),
                Transform::from_translation(pos).looking_at(look, Vec3::Y),
                RtsCamera::from_pose(pos, look),
            ));
            if let Some(image) = target.clone() {
                camera.insert(RenderTarget::Image(image.into()));
            }
        });
    }
}

/// Camera start poses for measuring: `(position, look_at)`. `overview` is the highest
/// allowed view (4000 m).
pub fn camera_pose(name: &str) -> (Vec3, Vec3) {
    let c = MAP_SIZE / 2.0;
    match name {
        "ground" => (
            Vec3::new(5120.0, 90.0, 5600.0),
            Vec3::new(5120.0, 65.0, 4800.0),
        ),
        // Close to the grid centre, looking along a street (see `scene`).
        "street" => (Vec3::new(c - 140.0, 35.0, c + 60.0), Vec3::new(c, 0.0, c)),
        "junction" => (Vec3::new(c - 30.0, 16.0, c + 38.0), Vec3::new(c, 0.0, c)),
        "showcase" => (
            Vec3::new(c + 20.0, 330.0, c + 440.0),
            Vec3::new(c + 20.0, 0.0, c + 20.0),
        ),
        "showcase-close" => (
            Vec3::new(c - 60.0, 90.0, c + 330.0),
            Vec3::new(c - 60.0, 0.0, c + 190.0),
        ),
        "valley" => (
            Vec3::new(c, 1500.0, c + 2500.0),
            Vec3::new(c, 0.0, c - 500.0),
        ),
        _ => (Vec3::new(c, 4000.0, c + 4200.0), Vec3::new(c, 0.0, c)),
    }
}

/// A sim for `terrain` at the given hour of day. Both the native and the asset-server
/// loading paths end here.
pub fn build_sim(
    terrain: Terrain,
    limits: SimLimits,
    hour: f32,
    phase_timing: bool,
    scene: Option<&str>,
) -> SimRunner {
    let mut sim = SimRunner::with_limits(1, limits);
    sim.set_terrain(Arc::new(terrain));
    sim.set_content(scene::road_content());
    sim.world_mut()
        .insert_resource(Tick((hour / 24.0 * TICKS_PER_DAY as f32) as u64));
    if phase_timing {
        sim.add_plugin(&PhaseTiming::window(PHASE_WINDOW_TICKS));
    }
    if scene == Some(scene::SHOWCASE) {
        let n = scene::build_showcase(&mut sim);
        info!("scene showcase: {n} buildings");
    } else if let Some(n) = scene.and_then(scene::parse_city) {
        let built = scene::build_city(&mut sim, n, 5200, 6000);
        info!("scene city:{n}: {built} buildings");
    } else if let Some(n) = scene.and_then(scene::parse_grid) {
        for reason in scene::build_grid(&mut sim, n) {
            warn!("scene command rejected: {reason}");
        }
    }
    sim
}

/// Loads `<dir>` (a map directory) synchronously. Native only; the browser goes through
/// the asset server.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_sim(
    dir: &Path,
    limits: SimLimits,
    hour: f32,
    phase_timing: bool,
    scene: Option<&str>,
) -> Result<SimRunner, String> {
    let terrain = Terrain::load(dir).map_err(|e| format!("cannot load map {dir:?}: {e}"))?;
    Ok(build_sim(terrain, limits, hour, phase_timing, scene))
}

/// Median ms per phase over the last [`PHASE_WINDOW_TICKS`] ticks, as the F3 window shows it.
pub fn phase_times(sim: &SimRunner) -> PhaseTimes {
    let means = sim
        .world()
        .get_resource::<PhaseClock>()
        .map(|c| c.median_ms())
        .unwrap_or_default();
    PhaseTimes {
        phases: SimPhase::ORDER.into_iter().zip(means).collect(),
    }
}

/// The single entry point for player input: UI and tools only fill the `CommandOutbox`; it
/// is submitted here, before the frame's ticks, then the sim is stepped. Frame time goes in
/// as an argument; the sim itself never reads a clock.
fn step_sim(
    time: Res<Time>,
    sim: Option<ResMut<Sim>>,
    view: Option<ResMut<SimView>>,
    mut outbox: ResMut<CommandOutbox>,
    mut phases: ResMut<PhaseTimes>,
) {
    let (Some(mut sim), Some(mut view)) = (sim, view) else {
        return;
    };
    let submitted = outbox.flush_into(&mut sim.0) > 0;
    let ticks = sim.0.step(Duration::from_secs_f64(time.delta_secs_f64()));
    if submitted || ticks > 0 {
        *view = sim.0.view();
    }
    if ticks > 0 {
        *phases = phase_times(&sim.0);
    }
}
