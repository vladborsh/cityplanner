//! `cp_app` — the game app shared by `apps/desktop` and `apps/web`.
//!
//! [`GamePlugin`] loads a map through Bevy's asset server (file IO natively, HTTP `fetch`
//! in the browser), builds the sim from it, steps `SimRunner` in `PreUpdate`, publishes
//! [`SimView`], and adds terrain rendering, the RTS camera and the UI. The binaries only add
//! what is platform specific: windowing, CLI, screenshots and benchmarks.

mod map;
pub mod saves;
pub mod scene;
pub mod settings;

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
    AgentRenderPlugin, AgentRenderStats, BuildingRenderPlugin, FireFxPlugin, OverlayPlugin,
    ProblemIconPlugin, RoadRenderPlugin, RtsCamera, RtsCameraPlugin, TerrainRenderPlugin,
    TickFraction, UndergroundPlugin, UtilityRenderPlugin,
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
    /// Light the world with the fixed afternoon sun at any hour (`--always-day`); toggled at
    /// runtime in the UI. The player's setting defaults to on (T-178).
    pub always_day: bool,
    /// Run the day and night cycle whatever the settings say (`--day-night`, or `--hour`).
    pub day_night: bool,
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
        // The player's settings file (T-069); only the UI client has a settings menu.
        let settings = if self.ui {
            settings::Settings::load(&saves::data_dir())
        } else {
            settings::Settings::default()
        };
        app.insert_resource(GameConfig {
            map: self.map.clone(),
            limits: self.limits,
            hour: self.hour,
            phase_timing: self.phase_timing,
            scene: self.scene.clone(),
        })
        .insert_resource(cp_render::LightingSettings {
            always_day: !self.day_night && (self.always_day || settings.always_day),
        })
        .insert_resource(settings.camera_settings())
        .init_resource::<PhaseTimes>()
        .init_resource::<CommandOutbox>()
        .add_plugins((
            map::MapPlugin,
            TerrainRenderPlugin,
            RoadRenderPlugin,
            OverlayPlugin,
            UtilityRenderPlugin,
            UndergroundPlugin,
            BuildingRenderPlugin,
            ProblemIconPlugin,
            FireFxPlugin,
            AgentRenderPlugin,
            RtsCameraPlugin,
        ))
        .add_systems(PreUpdate, step_sim);
        if self.ui {
            app.insert_resource(cp_ui::UiState {
                prefs: cp_ui::Prefs {
                    ui_scale: settings.ui_scale,
                    camera: settings.camera_settings(),
                },
                ..default()
            })
            .insert_resource(settings::SettingsSaver::new(settings))
            .add_plugins((UiPlugin, cp_tools::ToolsPlugin))
            .add_systems(
                Update,
                (file_requests, inspect_selection, settings::save_settings),
            );
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
                // Agents are drawn with a custom instanced draw (cp_render::agent_render).
                bevy::render::view::NoIndirectDrawing,
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
        // Low over the grid looking into the always-day sun (it stands west-south-west): the
        // glint on the asphalt, shadows lying towards the camera (T-178).
        "sun" => (
            Vec3::new(c + 160.0, 12.0, c - 90.0),
            Vec3::new(c - 100.0, 6.0, c + 50.0),
        ),
        // Over the stations of the metro scene (`--scene metro --overlay underground`).
        "metro" => (
            Vec3::new(c - 60.0, 330.0, c + 330.0),
            Vec3::new(c - 60.0, -6.0, c + 10.0),
        ),
        "zones" => (
            Vec3::new(c - 60.0, 300.0, c + 150.0),
            Vec3::new(c - 60.0, 0.0, c - 10.0),
        ),
        "showcase" => (
            Vec3::new(c + 20.0, 330.0, c + 440.0),
            Vec3::new(c + 20.0, 0.0, c + 20.0),
        ),
        "showcase-close" => (
            Vec3::new(c - 60.0, 90.0, c + 330.0),
            Vec3::new(c - 60.0, 0.0, c + 190.0),
        ),
        // Above the grid centre, wide enough to see many streets of the traffic scene.
        "traffic" => (Vec3::new(c, 190.0, c + 230.0), Vec3::new(c, 0.0, c - 20.0)),
        // Straight down on the centre of a small traffic grid: everything in view.
        "traffic-top" => (Vec3::new(c, 240.0, c + 40.0), Vec3::new(c, 0.0, c)),
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
    // The periodic desync checksum is for tests and replays; the game does not pay for it.
    sim.world_mut()
        .resource_mut::<cp_sim::SimChecksum>()
        .skip_periodic = true;
    sim.world_mut()
        .insert_resource(Tick((hour / 24.0 * TICKS_PER_DAY as f32) as u64));
    if phase_timing {
        sim.add_plugin(&PhaseTiming::window(PHASE_WINDOW_TICKS));
    }
    if let Some((n, cars, peds)) = scene.and_then(scene::parse_traffic) {
        for reason in scene::build_grid(&mut sim, n) {
            warn!("scene command rejected: {reason}");
        }
        sim.world_mut()
            .insert_resource(scene::DemoTraffic { cars, peds });
        scene::top_up_traffic(&mut sim);
        info!("scene traffic:{n}: up to {cars} cars, {peds} pedestrians");
    } else if scene == Some(scene::ZONES) {
        scene::build_zones(&mut sim);
        info!("scene zones");
    } else if scene == Some(scene::METRO) {
        let n = scene::build_metro(&mut sim);
        info!("scene metro: {n} stations");
    } else if scene == Some(scene::SHOWCASE) {
        let n = scene::build_showcase(&mut sim);
        info!("scene showcase: {n} buildings");
    } else if let Some(n) = scene.and_then(scene::parse_city) {
        let built = scene::build_city(&mut sim, n, 5200, 6000);
        info!("scene city:{n}: {built} buildings");
    } else if let Some(path) = scene.and_then(|s| s.strip_prefix("save:")) {
        // A saved city (the bench fixtures), to look at and to time the client on.
        let content = scene::embedded_content();
        match std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                cp_save::load_into_runner(&mut sim, &bytes, content.content_hash())
                    .map_err(|e| e.to_string())
            }) {
            Ok((header, warnings)) => info!(
                "scene save:{path}: tick {}, {} warnings",
                header.tick,
                warnings.len()
            ),
            Err(e) => warn!("scene save:{path}: {e}"),
        }
    } else if let Some(n) = scene.and_then(scene::parse_grid) {
        for reason in scene::build_grid(&mut sim, n) {
            warn!("scene command rejected: {reason}");
        }
    }
    if scene.is_none() {
        // A new game: the highway that enters the start tile (§5.1). Scenes and saves start
        // from an empty network, as the benches do.
        match sim.new_game() {
            Ok(()) => info!("new game: entry highway built"),
            Err(e) => warn!("new game: {e}"),
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
    mut fraction: ResMut<TickFraction>,
    mut agent_stats: ResMut<AgentRenderStats>,
) {
    let (Some(mut sim), Some(mut view)) = (sim, view) else {
        return;
    };
    let submitted = outbox.flush_into(&mut sim.0) > 0;
    let ticks = sim.0.step(Duration::from_secs_f64(time.delta_secs_f64()));
    if ticks > 0 {
        scene::top_up_traffic(&mut sim.0);
    }
    if submitted || ticks > 0 {
        let start = bevy::platform::time::Instant::now();
        *view = sim.0.view();
        agent_stats.view_ms = start.elapsed().as_secs_f32() * 1000.0;
    }
    if ticks > 0 {
        *phases = phase_times(&sim.0);
    }
    fraction.0 = sim.0.tick_fraction();
}

/// The system behind the save / load window: see [`handle_file_request`].
fn file_requests(
    mut menu: ResMut<cp_ui::FileMenu>,
    sim: Option<ResMut<Sim>>,
    view: Option<ResMut<SimView>>,
    cfg: Option<Res<GameConfig>>,
) {
    let (Some(mut sim), Some(mut view)) = (sim, view) else {
        return;
    };
    let map = cfg.map_or_else(|| "unknown".to_string(), |c| c.map.clone());
    handle_file_request(&mut menu, &mut sim.0, &mut view, &saves::data_dir(), &map);
}

/// Carries out what the save / load window asked for (see [`saves`]): the list of files, a
/// save, or a load, which replaces the whole `SimRunner` and the published view. A failure
/// only writes the status line; the running game stays as it was.
pub fn handle_file_request(
    menu: &mut cp_ui::FileMenu,
    sim: &mut SimRunner,
    view: &mut SimView,
    data: &std::path::Path,
    map: &str,
) {
    use cp_ui::{FileRequest, SaveEntry};
    if menu.dir.is_empty() {
        menu.dir = saves::saves_dir(data).display().to_string();
    }
    let Some(request) = menu.request.take() else {
        return;
    };
    let refresh = |menu: &mut cp_ui::FileMenu| {
        menu.files = saves::list_saves(data)
            .into_iter()
            .map(|e| SaveEntry {
                name: e.name,
                bytes: e.bytes,
                tick: e.tick,
            })
            .collect();
    };
    match request {
        FileRequest::Refresh => refresh(menu),
        FileRequest::Save(name) => {
            menu.status = match saves::save_game(sim, data, &name, map) {
                Ok(path) => format!("Saved {}", path.display()),
                Err(e) => format!("Save failed: {e}"),
            };
            refresh(menu);
        }
        FileRequest::Load(name) => {
            menu.status = match saves::load_game(sim, data, &name) {
                Ok((loaded, warnings)) => {
                    *sim = loaded;
                    *view = sim.view();
                    if warnings.is_empty() {
                        format!("Loaded {name}")
                    } else {
                        format!("Loaded {name} ({})", warnings.join(", "))
                    }
                }
                Err(e) => format!("Load failed: {e}"),
            };
        }
    }
}

/// Asks the sim for the details of what is selected (every frame: a car moves, a building fills
/// up). A selection that no longer exists (the car arrived, the building is gone) is dropped.
fn inspect_selection(
    sim: Option<Res<Sim>>,
    mut picked: ResMut<cp_tools::Picked>,
    mut inspected: ResMut<cp_tools::Inspected>,
) {
    let (Some(sim), Some(sel)) = (sim, picked.0) else {
        if inspected.0.is_some() {
            inspected.0 = None;
        }
        return;
    };
    match sim.0.inspect(sel) {
        Some(info) => {
            if inspected.0.as_ref() != Some(&info) {
                inspected.0 = Some(info);
            }
        }
        None => {
            picked.0 = None;
            inspected.0 = None;
        }
    }
}
