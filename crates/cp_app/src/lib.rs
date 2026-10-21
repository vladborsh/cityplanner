//! `cp_app` — the game app shared by `apps/desktop` and `apps/web`.
//!
//! [`GamePlugin`] loads a map through Bevy's asset server (file IO natively, HTTP `fetch`
//! in the browser), builds the sim from it, steps `SimRunner` in `PreUpdate`, publishes
//! [`SimView`], and adds terrain rendering, the RTS camera and the UI. The binaries only add
//! what is platform specific: windowing, CLI, screenshots and benchmarks.

pub mod audio;
pub mod grid_shots;
mod map;
pub mod road_scenes;
pub mod saves;
pub mod scene;
pub mod settings;
pub mod town;

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
    AgentRenderPlugin, AgentRenderStats, BuildingRenderPlugin, FireFxPlugin, FurnitureRenderPlugin,
    OverlayPlugin, ProblemIconPlugin, RoadRenderPlugin, RtsCamera, RtsCameraPlugin,
    TerrainRenderPlugin, TickFraction, UndergroundPlugin, UtilityRenderPlugin,
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
    /// Start with the net overlay on (`--net-debug`, T-025): lanes, connections and signals.
    pub net_debug: bool,
    /// Optional test scene, e.g. `grid:10` (see [`scene`]).
    pub scene: Option<String>,
    /// Render into this image instead of a window (`--hidden`: no window exists).
    pub target: Option<Handle<Image>>,
    /// Colour grading for this run (`--grading name=value,...`, T-273); else the player's.
    pub grading: Option<cp_render::GradingSettings>,
    /// Open the colour grading panel at start (`--grading-panel`, for screenshots).
    pub grading_panel: bool,
    /// `--fold-overlays`: start with the overlays panel folded to its strip (T-289 screenshots).
    pub fold_overlays: bool,
    /// Open this palette category at start (`--category`, for screenshots).
    pub category: Option<cp_ui::Category>,
    /// No cloud shadows in this run (`--no-clouds`, T-272: shots without them).
    pub no_clouds: bool,
    /// No parked cars in this run (`--no-parked`, T-276: shots without them).
    pub no_parked: bool,
    /// The language of this run (`--lang ru`, T-284); else the player's setting.
    pub lang: Option<cp_locale::Lang>,
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
        // The player's settings file (T-069); only the UI client has a settings menu. A hidden
        // run (screenshots) neither reads nor writes it: its picture is the defaults' and flags'.
        let hidden = self.target.is_some();
        let settings = if self.ui && !hidden {
            settings::Settings::load(&saves::data_dir())
        } else {
            settings::Settings::default()
        };
        let lang = self
            .lang
            .or_else(|| cp_locale::Lang::from_code(&settings.language))
            .unwrap_or_default();
        cp_locale::set_lang(lang);
        app.insert_resource(GameConfig {
            map: self.map.clone(),
            limits: self.limits,
            hour: self.hour,
            phase_timing: self.phase_timing,
            scene: self.scene.clone(),
        })
        .insert_resource(cp_render::LightingSettings {
            always_day: !self.day_night && (self.always_day || settings.always_day),
            cloud_shadows: settings.cloud_shadows && !self.no_clouds,
            parked_cars: settings.parked_cars && !self.no_parked,
        })
        .insert_resource(self.grading.unwrap_or(settings.grading.settings()))
        .insert_resource(cp_tools::NetDebug {
            lanes: self.net_debug,
            connections: self.net_debug,
            conflicts: self.net_debug,
            signals: self.net_debug,
        })
        .add_systems(Update, cp_tools::net_debug::draw_net_debug)
        .insert_resource(settings.camera_settings())
        .init_resource::<PhaseTimes>()
        .init_resource::<FrameSim>()
        .init_resource::<CommandOutbox>()
        .add_plugins((
            map::MapPlugin,
            TerrainRenderPlugin,
            RoadRenderPlugin,
            FurnitureRenderPlugin,
            OverlayPlugin,
            UtilityRenderPlugin,
            UndergroundPlugin,
            cp_render::TrainRenderPlugin,
            cp_render::CargoRenderPlugin,
            cp_render::PortalRenderPlugin,
            BuildingRenderPlugin,
            ProblemIconPlugin,
            FireFxPlugin,
            AgentRenderPlugin,
            RtsCameraPlugin,
        ))
        .add_plugins((
            cp_render::GradingPlugin,
            cp_render::PipelineWarmUpPlugin,
            cp_render::EventFxPlugin,
        ))
        .add_systems(PreUpdate, step_sim);
        if self.ui {
            app.insert_resource(cp_ui::UiState {
                prefs: cp_ui::Prefs {
                    ui_scale: settings.ui_scale,
                    camera: settings.camera_settings(),
                    advisor: settings.advisor,
                    advisor_problems_only: settings.advisor_problems_only,
                    audio: cp_ui::AudioPrefs {
                        master: settings.audio.master,
                        effects: settings.audio.effects,
                        mute: settings.audio.mute,
                    },
                    lang,
                },
                grading_open: self.grading_panel,
                overlays_collapsed: self.fold_overlays,
                category: self.category,
                // T-289: a menu started open takes up its tool and shows its overlay, as a click would.
                entered: self.category,
                ..default()
            })
            .insert_resource(settings::SettingsSaver::new(settings, hidden))
            .add_plugins((UiPlugin, cp_tools::ToolsPlugin));
            // A hidden run with the UI (`--ui`, screenshots of the panels) has no audio device.
            if app.is_plugin_added::<bevy::audio::AudioPlugin>() {
                app.add_plugins(audio::SoundPlugin);
            }
            app.add_systems(
                Update,
                (
                    file_requests,
                    inspect_selection,
                    settings::save_settings,
                    count_evictions
                        .after(cp_tools::plugin::tool_hover)
                        .after(cp_tools::plugin::place_demo),
                ),
            );
        }
        app.add_systems(Update, hand_dust_demo);
        if let Some(n) = grid_shots::shot_of_cam(&self.cam) {
            app.add_systems(
                Update,
                move |sim: Option<Res<Sim>>,
                      mut commands: Commands,
                      held: Option<Res<cp_tools::GhostDemo>>| {
                    hold_grid_shot_ghost(n, sim, &mut commands, held.is_some());
                },
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

/// How much faster than the base rate (20 ticks/s per speed step) game time runs in the app:
/// ×1 runs at the base rate, 20 ticks/s, so a 50 km/h car moves 111 m/s on screen (T-280: cars
/// half as fast as BUG-061's 40 ticks/s; the trips keep their share of the day); ×2 at 60 and
/// ×3 at 160 ticks/s (as fast as before, to grow a city). A game day lasts 205 s at ×1, 68 s at
/// ×2, 26 s at ×3. The sim itself keeps its whole-step multipliers; only the real time fed to
/// `step` is scaled.
pub fn game_speed_scale(speed: cp_core::SimSpeed) -> f64 {
    match speed {
        cp_core::SimSpeed::X2 => 1.5,
        cp_core::SimSpeed::X3 => 8.0 / 3.0,
        _ => 1.0,
    }
}

/// Real seconds one game day lasts at `speed` in the app (`None` paused).
pub fn real_seconds_per_day(speed: cp_core::SimSpeed) -> Option<f64> {
    let ticks_per_s = match speed {
        cp_core::SimSpeed::Pause => return None,
        cp_core::SimSpeed::X1 => 1.0,
        cp_core::SimSpeed::X2 => 2.0,
        cp_core::SimSpeed::X3 => 3.0,
    } * game_speed_scale(speed)
        * 1000.0
        / f64::from(cp_core::time::TICK_MILLIS);
    Some(cp_core::time::TICKS_PER_DAY as f64 / ticks_per_s)
}

/// `wide-<feature>[-top]` camera: the feature's index in [`scene::WIDE_FEATURES`] and whether it
/// looks from straight above.
fn wide_feature(name: &str) -> Option<(usize, bool)> {
    let f = name.strip_prefix("wide-")?;
    let (f, top) = f.strip_suffix("-top").map_or((f, false), |f| (f, true));
    Some((scene::WIDE_FEATURES.iter().position(|w| *w == f)?, top))
}

/// Camera start poses for measuring: `(position, look_at)`. `overview` is the highest
/// allowed view (4000 m).
pub fn camera_pose(name: &str) -> (Vec3, Vec3) {
    let c = MAP_SIZE / 2.0;
    // `at:x,y,z,lx,ly,lz`: eye and look-at in metres from the map centre (x, z) and the
    // ground (y), for exploring a scene (BUG-098).
    if let Some(n) = name.strip_prefix("at:").map(|s| {
        s.split(',')
            .filter_map(|v| v.parse::<f32>().ok())
            .collect::<Vec<_>>()
    }) && n.len() == 6
    {
        return (
            Vec3::new(c + n[0], n[1], c + n[2]),
            Vec3::new(c + n[3], n[4], c + n[5]),
        );
    }
    // The city of `--scene grid-shots` (T-275).
    if let Some(pose) = grid_shots::camera(name) {
        return pose;
    }
    // The scenes of the T-261 sweeps.
    if let Some(pose) = road_scenes::camera(name) {
        return pose;
    }
    // The town of `--scene town:N` (T-281).
    if let Some(pose) = town::camera(name) {
        return pose;
    }
    match name {
        "ground" => (
            Vec3::new(5120.0, 90.0, 5600.0),
            Vec3::new(5120.0, 65.0, 4800.0),
        ),
        // Close to the grid centre, looking along a street (see `scene`).
        "street" => (Vec3::new(c - 140.0, 35.0, c + 60.0), Vec3::new(c, 0.0, c)),
        // The same, moved 5 and 10 cm sideways (a pixel or two): shadows must hold still as
        // the camera moves (BUG-094).
        "street-b" | "street-c" => {
            let dx = if name == "street-b" { 0.05 } else { 0.10 };
            (
                Vec3::new(c - 140.0 + dx, 35.0, c + 60.0),
                Vec3::new(c + dx, 0.0, c),
            )
        }
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
        // The road tunnel under the river of `river_valley` (`--scene tunnel`).
        "tunnel" => (
            Vec3::new(6400.0, 330.0, 5220.0),
            Vec3::new(6400.0, 0.0, 4800.0),
        ),
        // The station at the dead end of the flatlands railway (`--scene train`).
        "train" => (
            Vec3::new(5880.0, 55.0, 5640.0),
            Vec3::new(5900.0, 28.0, 5500.0),
        ),
        // The cargo station at the dead end of the flatlands railway with its train
        // (`--scene cargo-train`).
        "cargo-train" => (
            Vec3::new(5845.0, 46.0, 5572.0),
            Vec3::new(5875.0, 28.0, 5490.0),
        ),
        // The ports on the river of `river_valley` (`--scene cargo-ship --map river_valley`):
        // the berthed ship, and the one sailing in.
        "cargo-ship" => (
            Vec3::new(6330.0, 70.0, 5060.0),
            Vec3::new(6420.0, 0.0, 4900.0),
        ),
        "cargo-ship-in" => (
            Vec3::new(6600.0, 150.0, 4860.0),
            Vec3::new(6450.0, 0.0, 4560.0),
        ),
        // The junctions of `--scene net-debug`, from above: a T, an X and a 6-lane X.
        "net-t" | "net-x" | "net-x6" => {
            let i = match name {
                "net-t" => 0.0,
                "net-x" => 1.0,
                _ => 2.0,
            };
            let x = c - 250.0 + 250.0 * i;
            (Vec3::new(x, 150.0, c + 40.0), Vec3::new(x, 0.0, c))
        }
        // Over the junctions of the corner scene (`--scene corners`).
        "corners" => (
            Vec3::new(c - 20.0, 120.0, c + 120.0),
            Vec3::new(c - 20.0, 0.0, c + 25.0),
        ),
        // The 90 degree bend at the south-east and the 45 degree one on the east street.
        "bends" => (
            Vec3::new(c + 110.0, 40.0, c + 125.0),
            Vec3::new(c + 60.0, 0.0, c + 75.0),
        ),
        "grown" => (
            Vec3::new(c + 20.0, 230.0, c + 190.0),
            Vec3::new(c + 20.0, 0.0, c + 40.0),
        ),
        // Close over the north street of `--scene pipes`, the pipe under it (T-286).
        "pipes" => (
            Vec3::new(c - 20.0, 45.0, c - 5.0),
            Vec3::new(c - 20.0, 0.0, c - 60.0),
        ),
        // Over both residential blocks of `--scene road-over` (BUG-096).
        "road-over" => (
            Vec3::new(c - 10.0, 170.0, c + 140.0),
            Vec3::new(c - 10.0, 0.0, c - 20.0),
        ),
        // Close over the residential block of `--scene grown`, where `--place-demo
        // clinic,5140,5078` holds a clinic's ghost over houses (T-252).
        "place-over" => (
            Vec3::new(c + 20.0, 48.0, c + 20.0),
            Vec3::new(c + 20.0, 0.0, c - 40.0),
        ),
        // T-294: overview height (400 m) over `--scene road-over` and over the water pipes of
        // `--scene city:10`, to see the red tint and the pipes the way the player sees them.
        "road-over-high" => (
            Vec3::new(c - 10.0, 400.0, c + 330.0),
            Vec3::new(c - 10.0, 0.0, c - 20.0),
        ),
        "city-high" => (Vec3::new(c, 420.0, c + 300.0), Vec3::new(c, 0.0, c)),
        "zones-big" => (
            Vec3::new(c + 20.0, 560.0, c + 330.0),
            Vec3::new(c + 20.0, 0.0, c + 20.0),
        ),
        // Over the turned city of `--scene zone-rect` (BUG-093).
        "zone-rect" => {
            let (o, u, v) = scene::zone_rect_frame();
            let m = o + u * 80.0 + v * 80.0;
            (
                Vec3::new(m.x - 20.0, 260.0, m.y + 160.0),
                Vec3::new(m.x, 0.0, m.y),
            )
        }
        // From the south over the user's two rows of blocks of `--scene zone-grid` (BUG-098).
        "zone-grid" => (Vec3::new(c, 330.0, c + 380.0), Vec3::new(c, 0.0, c - 10.0)),
        "zones" => (
            Vec3::new(c - 10.0, 420.0, c + 330.0),
            Vec3::new(c - 10.0, 0.0, c + 150.0),
        ),
        // The specialisation buildings, kinds in columns and sizes in rows (`--scene specs`).
        "specs" => (Vec3::new(c, 170.0, c + 120.0), Vec3::new(c, 5.0, c - 85.0)),
        // The five cargo trucks queued at the east end of the street (`--scene trucks`).
        // The town of the BUG-087 shot (`--scene power-town --overlay power`).
        "power-town" => (
            Vec3::new(c + 170.0, 300.0, c + 360.0),
            Vec3::new(c + 170.0, 0.0, c + 50.0),
        ),
        "trucks" => (
            Vec3::new(c - 10.0, 6.0, c + 92.0),
            Vec3::new(c - 10.0, 2.0, c),
        ),
        // Close side views of the log truck and the tanker of the same scene.
        "trucks-logs" => (
            Vec3::new(c - 22.0, 3.5, c + 30.0),
            Vec3::new(c - 22.0, 1.8, c),
        ),
        "trucks-tanker" => (
            Vec3::new(c + 38.0, 3.5, c + 30.0),
            Vec3::new(c + 38.0, 1.8, c),
        ),
        // The low-density homes with their gardens (`--scene gardens`).
        "gardens" => (
            Vec3::new(c - 30.0, 14.0, c + 55.0),
            Vec3::new(c - 30.0, 2.0, c),
        ),
        // The night city (`--scene night:N`, T-245): high over the south-west corner looking
        // across the centre, down a street of the centre, and at the district without power.
        "night-wide" => (
            Vec3::new(c - 650.0, 420.0, c + 750.0),
            Vec3::new(c + 120.0, 0.0, c - 120.0),
        ),
        "night-street" => (
            Vec3::new(c - 120.0, 28.0, c + 52.0),
            Vec3::new(c + 40.0, 6.0, c + 2.0),
        ),
        "night-dark" => (
            Vec3::new(c - 50.0, 260.0, c + 300.0),
            Vec3::new(c + 250.0, 0.0, c - 150.0),
        ),
        // The post office (x = c - 45), the sorting facility (x = c + 20) behind the street at
        // z = c, and the mail van between them (`--scene post`, T-248).
        "post" => (
            Vec3::new(c - 15.0, 32.0, c + 48.0),
            Vec3::new(c - 10.0, 2.0, c - 15.0),
        ),
        // The services grid from the south-east, high enough for the whole 1200 m (T-254).
        "services" => (
            Vec3::new(c + 150.0, 900.0, c + 820.0),
            Vec3::new(c + 50.0, 0.0, c - 20.0),
        ),
        // Across the street, the houses with mail piled up (`--overlay mail`).
        "post-mail" => (Vec3::new(c, 110.0, c + 120.0), Vec3::new(c, 0.0, c + 5.0)),
        "post-office" => (
            Vec3::new(c - 52.0, 9.0, c + 14.0),
            Vec3::new(c - 45.0, 3.0, c - 14.0),
        ),
        "post-sorting" => (
            Vec3::new(c + 6.0, 16.0, c + 26.0),
            Vec3::new(c + 20.0, 3.0, c - 20.0),
        ),
        "post-van" => (
            Vec3::new(c + 14.0, 4.5, c + 13.0),
            Vec3::new(c + 24.0, 1.0, c + 1.0),
        ),
        // The street lamps and poles along two streets (`--scene lamps`).
        "lamps" => (
            Vec3::new(c - 30.0, 9.0, c + 78.0),
            Vec3::new(c - 30.0, 3.0, c + 40.0),
        ),
        // A lamp, a utility pole, a pylon and the traffic up close (`--scene shadows`).
        "shadows" => (
            Vec3::new(c - 48.0, 17.0, c + 66.0),
            Vec3::new(c - 30.0, 2.0, c + 8.0),
        ),
        "shadows-close" => (
            Vec3::new(c + 14.0, 5.0, c + 18.0),
            Vec3::new(c + 2.0, 0.5, c + 4.0),
        ),
        "lamps-avenue" => (
            Vec3::new(c - 20.0, 12.0, c + 5.0),
            Vec3::new(c - 20.0, 3.0, c - 40.0),
        ),
        // Footpaths leaving a street across the lawn (`--scene paths`).
        "paths" => (
            Vec3::new(c + 5.0, 14.0, c + 78.0),
            Vec3::new(c + 5.0, 0.0, c + 30.0),
        ),
        "gardens-close" => (
            Vec3::new(c - 30.0, 9.0, c + 34.0),
            Vec3::new(c - 30.0, 3.0, c - 6.0),
        ),
        // The construction stages in a row, seen from the south (`--scene construction`).
        "construction" => (Vec3::new(c, 45.0, c + 120.0), Vec3::new(c, 12.0, c - 130.0)),
        // The four city parks in a row (`--scene parks`).
        "parks" => (Vec3::new(c, 55.0, c + 20.0), Vec3::new(c, 2.0, c - 120.0)),
        // The park areas (`--scene park-areas`): the rows from the props side, and the props.
        // (The showcase rows stand at z = c - 130, c - 175, c - 220, c - 265 and c - 310.)
        "park-areas" => (Vec3::new(c, 75.0, c - 40.0), Vec3::new(c, 0.0, c - 180.0)),
        "park-props" => (Vec3::new(c, 26.0, c - 82.0), Vec3::new(c, 0.0, c - 132.0)),
        "park-city" => (Vec3::new(c, 34.0, c - 120.0), Vec3::new(c, 0.0, c - 178.0)),
        "park-nature" => (Vec3::new(c, 36.0, c - 165.0), Vec3::new(c, 2.0, c - 223.0)),
        "park-zoo" => (Vec3::new(c, 70.0, c - 182.0), Vec3::new(c, 0.0, c - 268.0)),
        "park-amusement" => (Vec3::new(c, 88.0, c - 208.0), Vec3::new(c, 4.0, c - 313.0)),
        // Up close: the zoo gate, the elephant house, the ferris wheel and the roller coaster.
        "park-zoo-gate" => (
            Vec3::new(c - 62.0, 16.0, c - 238.0),
            Vec3::new(c - 80.0, 4.0, c - 266.0),
        ),
        "park-elephant" => (
            Vec3::new(c + 58.0, 24.0, c - 228.0),
            Vec3::new(c + 80.0, 4.0, c - 266.0),
        ),
        "park-ferris" => (
            Vec3::new(c + 30.0, 22.0, c - 262.0),
            Vec3::new(c + 62.0, 9.0, c - 310.0),
        ),
        // The unique factories, the cargo station and the cargo port from their street side
        // (`--scene factories`, the row at z = c - 130, x = c - 96 .. c + 144, fronts towards
        // +z), and up close.
        "factories" => (Vec3::new(c, 62.0, c - 45.0), Vec3::new(c, 0.0, c - 135.0)),
        "factory-food" => (
            Vec3::new(c - 112.0, 24.0, c - 88.0),
            Vec3::new(c - 96.0, 5.0, c - 132.0),
        ),
        "factory-mid" => (
            Vec3::new(c - 24.0, 34.0, c - 80.0),
            Vec3::new(c - 24.0, 3.0, c - 132.0),
        ),
        "factory-cars" => (
            Vec3::new(c + 34.0, 30.0, c - 84.0),
            Vec3::new(c + 48.0, 3.0, c - 132.0),
        ),
        "factory-port" => (
            Vec3::new(c + 118.0, 30.0, c - 84.0),
            Vec3::new(c + 144.0, 7.0, c - 130.0),
        ),
        "factory-cargo" => (
            Vec3::new(c + 76.0, 22.0, c - 96.0),
            Vec3::new(c + 96.0, 4.0, c - 130.0),
        ),
        // The M5 chain buildings, a row per resource (`--scene chains`).
        "chains" => (Vec3::new(c, 190.0, c + 150.0), Vec3::new(c, 5.0, c - 55.0)),
        // The whole middle of the map for the resource overlays (`--scene industry`).
        "industry" => (Vec3::new(c, 5200.0, c + 2600.0), Vec3::new(c, 0.0, c)),
        // The ten unique buildings in a row (`--scene uniques`).
        "uniques" => (
            Vec3::new(c, 150.0, c + 190.0),
            Vec3::new(c, 30.0, c - 130.0),
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
        // One feature of the wide-road corner scene (`--scene corners-wide`), from the south;
        // `wide-<f>-top` from straight above.
        wide if wide_feature(wide).is_some() => {
            let (i, top) = wide_feature(wide).unwrap_or_default();
            let o = scene::wide_feature_centre(i);
            if top {
                (Vec3::new(o.x, 48.0, o.y + 1.0), Vec3::new(o.x, 0.0, o.y))
            } else {
                (
                    Vec3::new(o.x + 6.0, 42.0, o.y + 46.0),
                    Vec3::new(o.x, 0.0, o.y + 4.0),
                )
            }
        }
        _ => (Vec3::new(c, 4000.0, c + 4200.0), Vec3::new(c, 0.0, c)),
    }
}

/// `--dust-demo` (T-288 shots): once the scene stands, its puffs go to the renderer.
fn hand_dust_demo(sim: Option<Res<Sim>>, demo: Option<ResMut<cp_render::DustDemo>>) {
    let (Some(sim), Some(mut demo)) = (sim, demo) else {
        return;
    };
    if demo.puffs.is_none()
        && let Some(p) = sim.0.world().get_resource::<scene::DustPuffs>()
    {
        demo.puffs = Some(p.0.clone());
    }
}

/// `--cam grid-shotN` over `--scene grid-shots` (T-255): once the scene stands, the road tool
/// holds the ghost of layout N's last street (`cp_tools::GhostDemo`).
fn hold_grid_shot_ghost(n: usize, sim: Option<Res<Sim>>, commands: &mut Commands, held: bool) {
    let Some(sim) = sim.filter(|_| !held) else {
        return;
    };
    let Some(cursors) = sim.0.world().get_resource::<grid_shots::GridShotCursors>() else {
        return;
    };
    if let Some(demo) = grid_shots::ghost(&sim.0.view(), cursors, n) {
        commands.insert_resource(demo);
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
    } else if scene == Some(scene::TUNNEL) {
        let ends = scene::build_tunnel(&mut sim);
        info!("scene tunnel: portals {ends:?}");
    } else if scene == Some(scene::TRAIN) {
        let ok = scene::build_train(&mut sim);
        info!("scene train: a train at the station: {ok}");
    } else if scene == Some(scene::CARGO_TRAIN) {
        let ok = scene::build_cargo_train(&mut sim);
        info!("scene cargo-train: a cargo train at the station: {ok}");
    } else if scene == Some(scene::CARGO_SHIP) {
        let at = scene::build_cargo_ship(&mut sim);
        info!("scene cargo-ship: ports at {at:?}");
    } else if scene == Some(scene::CORNERS) {
        scene::build_corners(&mut sim);
        info!("scene corners");
    } else if scene == Some(scene::NET_DEBUG) {
        scene::build_net_debug(&mut sim);
        info!("scene net-debug");
    } else if scene == Some(scene::CORNERS_WIDE) {
        scene::build_corners_wide(&mut sim);
        info!("scene corners-wide");
    } else if scene == Some(scene::DUST) {
        scene::build_dust(&mut sim);
        info!("scene dust");
    } else if scene == Some(scene::SCHOOL) {
        scene::build_school(&mut sim);
        info!("scene school");
    } else if scene == Some(scene::PIPES) {
        scene::build_pipes(&mut sim);
        info!("scene pipes");
    } else if scene == Some(scene::ROAD_OVER) {
        scene::build_road_over(&mut sim);
        info!("scene road-over");
    } else if scene == Some(scene::GROWN) {
        scene::build_grown(&mut sim);
        info!("scene grown");
    } else if scene == Some(scene::ZONES_BIG) {
        scene::build_zones_big(&mut sim);
        info!("scene zones-big");
    } else if scene == Some(scene::ZONE_RECT) {
        scene::build_zone_rect(&mut sim);
        info!("scene zone-rect");
    } else if scene == Some(scene::ZONE_GRID) {
        scene::build_zone_grid(&mut sim);
        info!("scene zone-grid");
    } else if scene == Some(scene::ZONES) {
        scene::build_zones(&mut sim);
        info!("scene zones");
    } else if scene == Some(scene::METRO) {
        let n = scene::build_metro(&mut sim);
        info!("scene metro: {n} stations");
    } else if scene == Some(scene::INDUSTRY) {
        let at = scene::build_industry(&mut sim);
        info!("scene industry: areas at {at:?}");
    } else if scene == Some(scene::CHAINS) {
        let n = scene::build_chains(&mut sim);
        info!("scene chains: {n} buildings");
    } else if scene == Some(scene::SHADOWS) {
        for reason in scene::build_shadows(&mut sim) {
            warn!("scene command rejected: {reason}");
        }
        info!("scene shadows");
    } else if scene == Some(scene::POWER_TOWN) {
        scene::build_power_town(&mut sim);
        info!("scene power-town");
    } else if scene == Some(scene::LAMPS) {
        scene::build_lamps(&mut sim);
        info!("scene lamps");
    } else if scene == Some(scene::PATHS) {
        scene::build_paths(&mut sim);
        info!("scene paths");
    } else if scene == Some(scene::GARDENS) {
        let n = scene::build_gardens(&mut sim, 14);
        info!("scene gardens: {n} buildings");
    } else if scene == Some(scene::CONSTRUCTION) {
        let n = scene::build_construction(&mut sim, "apt_high_2");
        info!("scene construction: {n} buildings");
    } else if scene == Some(scene::TRUCKS) {
        let n = scene::build_trucks(&mut sim);
        // Until every truck is on the road.
        for _ in 0..600 {
            sim.tick_once();
            let agents = sim.world().resource::<cp_traffic::Agents>();
            if agents.vehicles().count() == n && agents.vehicles().all(|(_, v)| v.spawned) {
                break;
            }
        }
        // Hold the moment: the trucks stand where they set out, clear of each other.
        sim.world_mut()
            .insert_resource(cp_core::time::SimSpeed::Pause);
        info!("scene trucks: {n} trucks");
    } else if scene == Some(scene::POST) {
        let n = scene::build_post(&mut sim);
        // Until the van has driven a little way down the street, then hold the moment.
        let mut moving = 0;
        for _ in 0..900 {
            sim.tick_once();
            let agents = sim.world().resource::<cp_traffic::Agents>();
            if agents.vehicles().any(|(_, v)| v.spawned) {
                moving += 1;
                if moving == 25 {
                    break;
                }
            }
        }
        sim.world_mut()
            .insert_resource(cp_core::time::SimSpeed::Pause);
        // The scene has no power lines: held, the buildings show as powered (lit at night).
        let mut grid = sim.world_mut().resource_mut::<cp_building::BuildingGrid>();
        let ids: Vec<_> = grid.iter().map(|(id, _)| id).collect();
        for id in ids {
            if let Some(b) = grid.get_mut(id) {
                b.flags.0 &= !cp_building::BuildingFlags::UTILITY_MASK;
            }
        }
        scene::add_mail_houses(&mut sim);
        info!("scene post: {n} mail vans");
    } else if scene == Some(road_scenes::JOINS) {
        for reason in road_scenes::build_joins(&mut sim) {
            warn!("scene command rejected: {reason}");
        }
        info!("scene joins");
    } else if scene == Some(road_scenes::PLACE) {
        for reason in road_scenes::build_place(&mut sim) {
            warn!("scene command rejected: {reason}");
        }
        info!("scene place");
    } else if scene == Some(grid_shots::GRID_SHOTS) {
        for reason in grid_shots::build_grid_shots(&mut sim) {
            warn!("scene command rejected: {reason}");
        }
        info!("scene grid-shots");
    } else if scene == Some(scene::SERVICES_PAUSED) {
        for reason in scene::build_services_paused(&mut sim) {
            warn!("scene command rejected: {reason}");
        }
        info!("scene services-paused");
    } else if scene == Some(scene::SERVICES) {
        for reason in scene::build_services(&mut sim) {
            warn!("scene command rejected: {reason}");
        }
        info!("scene services");
    } else if scene == Some(scene::PARKS) {
        let n = scene::build_parks(&mut sim);
        info!("scene parks: {n} buildings");
    } else if scene == Some(scene::PARK_AREAS) {
        let n = scene::build_park_areas(&mut sim);
        info!("scene park-areas: {n} buildings");
    } else if scene == Some(scene::FACTORIES) {
        let n = scene::build_factories(&mut sim);
        info!("scene factories: {n} buildings");
    } else if scene == Some(scene::SPECS) {
        let n = scene::build_specs(&mut sim);
        info!("scene specs: {n} buildings");
    } else if scene == Some(scene::UNIQUES) {
        let n = scene::build_uniques(&mut sim);
        info!("scene uniques: {n} buildings");
    } else if scene == Some(scene::SHOWCASE) {
        let n = scene::build_showcase(&mut sim);
        info!("scene showcase: {n} buildings");
    } else if let Some(n) = scene.and_then(scene::parse_night) {
        let (built, dark) = scene::build_night(&mut sim, n, hour);
        info!("scene night:{n}: {built} buildings, {dark} without power");
    } else if let Some(n) = scene.and_then(town::parse_town) {
        let (residents, rejected) = town::build_town(&mut sim, n);
        for reason in rejected {
            warn!("scene command rejected: {reason}");
        }
        info!("scene town:{n}: {residents} residents");
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
        // The player starts the simulation (BUG-059): no money is spent before that.
        sim.world_mut()
            .insert_resource(cp_core::time::SimSpeed::Pause);
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
/// What the sim cost this frame (T-281, for the bench's hitch report): the ticks stepped, the
/// wall time of the step and of publishing the view, ms.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct FrameSim {
    pub ticks: u32,
    pub step_ms: f32,
    pub view_ms: f32,
}

/// The last tick's phases, ns each, in [`cp_sim::SimPhase::ORDER`] (T-281: the slowest ticks of
/// `--headless`).
pub fn last_tick_phases(sim: &SimRunner) -> Option<Vec<u64>> {
    sim.world()
        .get_resource::<PhaseClock>()
        .and_then(|c| c.samples.back().map(|row| row.to_vec()))
}

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
#[allow(clippy::too_many_arguments)]
fn step_sim(
    time: Res<Time>,
    sim: Option<ResMut<Sim>>,
    view: Option<ResMut<SimView>>,
    mut outbox: ResMut<CommandOutbox>,
    mut phases: ResMut<PhaseTimes>,
    mut fraction: ResMut<TickFraction>,
    mut agent_stats: ResMut<AgentRenderStats>,
    real: Res<Time<Real>>,
    prefs: Option<Res<cp_ui::UiState>>,
    mut sounds: Option<ResMut<audio::SoundQueue>>,
    mut played: Option<ResMut<audio::AudioPrefsRes>>,
    mut frame_sim: ResMut<FrameSim>,
) {
    let (Some(mut sim), Some(mut view)) = (sim, view) else {
        return;
    };
    // What the player asked for this frame, to sound out once the sim has said yes or no.
    let asked: Vec<cp_sim::SimCommand> = if sounds.is_some() {
        outbox.iter().cloned().collect()
    } else {
        Vec::new()
    };
    let submitted = outbox.flush_into(&mut sim.0) > 0;
    if let (Some(queue), Some(played), Some(ui)) = (sounds.as_mut(), played.as_mut(), prefs) {
        let a = ui.prefs.audio;
        played.0 = audio::AudioSettings {
            master: a.master,
            effects: a.effects,
            mute: a.mute,
        };
        if submitted {
            let outcomes: Vec<bool> = sim
                .0
                .world_mut()
                .resource_mut::<bevy::ecs::message::Messages<cp_core::CommandOutcome>>()
                .drain()
                .map(|o| o.result.is_ok())
                .collect();
            if outcomes.len() == asked.len() {
                audio::queue_outcomes(queue, &asked, &outcomes, real.elapsed_secs_f64(), &played.0);
            }
        }
    }
    let scale = game_speed_scale(sim.0.speed());
    let started = bevy::platform::time::Instant::now();
    let ticks = sim
        .0
        .step(Duration::from_secs_f64(time.delta_secs_f64() * scale));
    if ticks > 0 {
        scene::top_up_traffic(&mut sim.0);
    }
    *frame_sim = FrameSim {
        ticks,
        step_ms: started.elapsed().as_secs_f32() * 1000.0,
        view_ms: 0.0,
    };
    if submitted || ticks > 0 {
        let start = bevy::platform::time::Instant::now();
        *view = sim.0.view();
        agent_stats.view_ms = start.elapsed().as_secs_f32() * 1000.0;
        frame_sim.view_ms = agent_stats.view_ms;
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
                Ok(path) => cp_locale::trf!("Saved {path}", path = path.display()),
                Err(e) => cp_locale::trf!("Save failed: {e}", e = e),
            };
            refresh(menu);
        }
        FileRequest::Load(name) => {
            menu.status = match saves::load_game(sim, data, &name) {
                Ok((loaded, warnings)) => {
                    *sim = loaded;
                    // A loaded game starts paused too (BUG-059).
                    sim.world_mut()
                        .insert_resource(cp_core::time::SimSpeed::Pause);
                    *view = sim.view();
                    if warnings.is_empty() {
                        cp_locale::trf!("Loaded {name}", name = name)
                    } else {
                        cp_locale::trf!(
                            "Loaded {name} ({warnings})",
                            name = name,
                            warnings = warnings.join(", ")
                        )
                    }
                }
                Err(e) => cp_locale::trf!("Load failed: {e}", e = e),
            };
        }
    }
}

/// Fills in how many people the placement ghost would displace (T-252): the view has no
/// citizens, the sim does.
fn count_evictions(sim: Option<Res<Sim>>, mut tools: ResMut<cp_tools::ToolState>) {
    let Some(sim) = sim else { return };
    let Some(g) = tools.place_ghost.as_ref() else {
        return;
    };
    let evicted = (!g.demolish.is_empty()).then(|| sim.0.eviction(&g.demolish));
    if g.evicted != evicted
        && let Some(g) = tools.place_ghost.as_mut()
    {
        g.evicted = evicted;
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

#[cfg(test)]
mod start_tests {
    use super::*;

    /// BUG-059: a new game starts paused — the sim does not advance until the player starts it —
    /// and a speed command (the 1/2/3 keys) runs it.
    #[test]
    fn a_new_game_starts_paused_until_a_speed_key() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let terrain = Terrain::load(&root.join("assets/maps/flatlands")).unwrap();
        let mut sim = build_sim(terrain, SimLimits::desktop(), 8.0, false, None);
        assert_eq!(sim.speed(), cp_core::SimSpeed::Pause);
        let at = sim.tick();
        sim.step(std::time::Duration::from_secs(1));
        assert_eq!(sim.tick(), at, "no tick while paused");
        let mut outbox = cp_tools::CommandOutbox::default();
        outbox.push(cp_sim::SimCommand::SetSpeed {
            speed: cp_core::SimSpeed::X2,
        });
        outbox.flush_into(&mut sim);
        sim.step(std::time::Duration::from_secs(1));
        assert_eq!(sim.speed(), cp_core::SimSpeed::X2);
        assert!(sim.tick() > at, "running after the speed key");
    }
}

#[cfg(test)]
mod pacing_tests {
    use super::*;
    use cp_core::SimSpeed;

    /// The length of a game day in real time at each speed: BUG-061 set ×3 to 26 s; T-280 runs
    /// ×1 at 20 ticks/s (cars half as fast on screen) and ×2 at 60.
    #[test]
    fn a_game_day_lasts_205_68_and_26_real_seconds() {
        let day = |s| real_seconds_per_day(s).unwrap();
        assert!((day(SimSpeed::X1) - 204.8).abs() < 0.1);
        assert!((day(SimSpeed::X2) - 68.3).abs() < 0.1);
        assert!((day(SimSpeed::X3) - 25.6).abs() < 0.1);
        assert!((day(SimSpeed::X1) / day(SimSpeed::X3) - 8.0).abs() < 1e-6);
        assert_eq!(real_seconds_per_day(SimSpeed::Pause), None);
        // At 60 frames per second even ×3 needs under 3 ticks a frame (the sim runs at most 4).
        let per_frame = 160.0 / 60.0;
        assert!(per_frame < 4.0);
    }
}
