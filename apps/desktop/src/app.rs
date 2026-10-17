//! The game app: SimRunner stepped in `PreUpdate`, `SimView` published as a resource,
//! terrain rendering and the RTS camera from `cp_render`.
//!
//! `cargo run --release -p cp_desktop -- [--map flatlands|river_valley] [--cam overview|ground|valley]`
//! `  [--hour 0..24] [--always-day] [--day-night] [--overlay zones] [--screenshot out.png] [--bench seconds] [--no-vsync]`
//! `cargo run --release -p cp_desktop -- --map flatlands --headless 512` runs N ticks without a
//! window and prints the numbers the F3 window shows (for comparing with `cp_bench`).

use std::path::PathBuf;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::render::diagnostic::RenderDiagnosticsPlugin;
use bevy::window::PresentMode;
use cp_app::{GamePlugin, MapLoadFailed, phase_times};
use cp_core::SimLimits;
use cp_render::{AgentRenderStats, BuildingStats, RoadStats, TerrainStats};
use cp_sim::SimRunner;

use crate::ASSETS_DIR;
use crate::capture::{Capture, CapturePlugin};

pub struct Options {
    pub map: String,
    pub cam: String,
    pub hour: Option<f32>,
    pub screenshot: Option<PathBuf>,
    pub bench: Option<f32>,
    pub vsync: bool,
    pub headless: Option<u64>,
    pub scene: Option<String>,
    pub hidden: bool,
    pub always_day: bool,
    /// `--day-night` (or an explicit `--hour`): run the day and night cycle (T-178: the
    /// default is the always-day sun).
    pub day_night: bool,
    /// `--overlay zones|traffic|...`: start with that map overlay on (for screenshots).
    pub overlay: Option<cp_render::Overlay>,
    /// `--ghost-demo basic_2l,x1,z1,x2,z2`: hold the road tool's ghost from (x1, z1) to the
    /// cursor at (x2, z2) without a cursor (screenshots of the zone strip, T-185).
    pub ghost_demo: Option<cp_tools::GhostDemo>,
}

impl Options {
    pub fn parse(args: &[String]) -> Self {
        let value = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let bench = value("--bench").and_then(|v| v.parse().ok());
        Self {
            map: value("--map").unwrap_or_else(|| "flatlands".into()),
            cam: value("--cam").unwrap_or_else(|| "overview".into()),
            hour: value("--hour").and_then(|v| v.parse().ok()),
            screenshot: value("--screenshot").map(PathBuf::from),
            bench,
            headless: value("--headless").and_then(|v| v.parse().ok()),
            scene: value("--scene"),
            overlay: value("--overlay").and_then(|n| {
                cp_render::Overlay::ALL
                    .into_iter()
                    .find(|o| o.label().eq_ignore_ascii_case(&n))
            }),
            ghost_demo: value("--ghost-demo").and_then(|v| {
                let mut it = v.split(',');
                let road = it.next()?.to_string();
                let n: Vec<f32> = it.filter_map(|x| x.parse().ok()).collect();
                (n.len() == 4).then(|| cp_tools::GhostDemo {
                    road,
                    from: bevy::math::Vec2::new(n[0], n[1]),
                    to: bevy::math::Vec2::new(n[2], n[3]),
                })
            }),
            always_day: args.iter().any(|a| a == "--always-day"),
            day_night: args.iter().any(|a| a == "--day-night") || value("--hour").is_some(),
            hidden: crate::capture::hidden_requested(args),
            vsync: !args.iter().any(|a| a == "--no-vsync") && bench.is_none(),
        }
    }
}

/// The default plugins, without Bevy's audio when the run is hidden (screenshots, scripts and
/// tests never open an audio device, T-189).
pub fn audio_plugins(hidden: bool) -> bevy::app::PluginGroupBuilder {
    let plugins = DefaultPlugins.build();
    if crate::capture::audio_wanted(hidden) {
        plugins
    } else {
        plugins.disable::<bevy::audio::AudioPlugin>()
    }
}

#[derive(Resource)]
struct Job {
    /// A screenshot was requested (see `capture`).
    screenshot: bool,
    /// The benchmark report was printed.
    done: bool,
    bench_secs: Option<f32>,
    frame: u32,
    frame_ms: Vec<f32>,
}

pub fn run(opts: Options) -> AppExit {
    let hour = opts.hour.unwrap_or(10.0);
    if let Some(ticks) = opts.headless {
        let dir = PathBuf::from(ASSETS_DIR).join("maps").join(&opts.map);
        match cp_app::load_sim(
            &dir,
            SimLimits::desktop(),
            hour,
            true,
            opts.scene.as_deref(),
        ) {
            Ok(sim) => {
                headless(sim, ticks);
                return AppExit::Success;
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        }
    }

    let mut app = App::new();
    app.add_plugins(
        audio_plugins(opts.hidden)
            .set(AssetPlugin {
                file_path: ASSETS_DIR.into(),
                ..default()
            })
            .set(crate::capture::window_plugin(
                Window {
                    title: format!("cityplanner — {}", opts.map),
                    resolution: (1600, 900).into(),
                    present_mode: if opts.vsync {
                        PresentMode::AutoVsync
                    } else {
                        PresentMode::AutoNoVsync
                    },
                    ..default()
                },
                opts.hidden,
            )),
    );
    // `--hidden`: no window; cameras render into an image (see `capture::window_plugin`).
    let target = opts.hidden.then(|| {
        crate::capture::offscreen_target(&mut app.world_mut().resource_mut::<Assets<Image>>())
    });
    if opts.hidden {
        // With no window the loop counts as unfocused and would idle at 60 Hz.
        app.insert_resource(bevy::winit::WinitSettings::continuous());
    }
    if let Some(active) = opts.overlay {
        app.insert_resource(cp_render::OverlayState { active });
    }
    if let Some(demo) = opts.ghost_demo {
        // The tools only run with the UI; a held ghost for a hidden screenshot needs them.
        if opts.hidden {
            app.add_plugins(cp_tools::ToolsPlugin);
        }
        app.insert_resource(demo);
    }
    app.add_plugins((
        GamePlugin {
            map: opts.map,
            limits: SimLimits::desktop(),
            hour,
            cam: opts.cam,
            // Wall-clock phase timing for the F3 window.
            phase_timing: true,
            ui: !opts.hidden,
            always_day: opts.always_day,
            day_night: opts.day_night,
            scene: opts.scene,
            target: target.clone(),
        },
        FrameTimeDiagnosticsPlugin::default(),
        RenderDiagnosticsPlugin,
    ))
    .add_plugins(CapturePlugin)
    .insert_resource(Job {
        screenshot: opts.screenshot.is_some(),
        done: false,
        bench_secs: opts.bench,
        frame: 0,
        frame_ms: Vec::new(),
    })
    .add_systems(Update, (job, exit_on_load_failure));
    if let Some(path) = opts.screenshot {
        let mut capture = Capture::new(
            path,
            std::time::Duration::from_secs_f32(opts.bench.unwrap_or(0.0)),
        );
        if let Some(t) = target {
            capture = capture.with_target(t);
        }
        app.insert_resource(capture);
    }
    app.run()
}

fn exit_on_load_failure(failed: Option<Res<MapLoadFailed>>, mut exit: MessageWriter<AppExit>) {
    if let Some(f) = failed {
        eprintln!("cannot load {}", f.0);
        exit.write(AppExit::from_code(2));
    }
}

/// `--headless N`: N ticks without a window, then the F3 window's numbers on stdout:
/// the periodic checksum from `SimView`, an on-demand checksum, and median ms per phase.
fn headless(mut sim: SimRunner, ticks: u64) {
    for _ in 0..ticks {
        sim.tick_once();
    }
    let view = sim.view();
    match view.stats.checksum {
        Some((t, c)) => println!("DEBUG periodic_checksum={c:016x} at_tick={}", t.0),
        None => println!("DEBUG periodic_checksum=none"),
    }
    println!(
        "DEBUG ticks={} checksum={:016x}",
        sim.tick().0,
        sim.checksum()
    );
    for (phase, ms) in phase_times(&sim).phases {
        println!("DEBUG phase {phase:?} median_ms={ms:.6}");
    }
}

/// Screenshot and benchmark driver.
#[allow(clippy::too_many_arguments)]
fn job(
    mut job: ResMut<Job>,
    time: Res<Time<Real>>,
    stats: Res<TerrainStats>,
    roads: Res<RoadStats>,
    buildings: Res<BuildingStats>,
    agents: Res<AgentRenderStats>,
    diagnostics: Res<DiagnosticsStore>,
    capture: Option<ResMut<Capture>>,
    mut exit: MessageWriter<AppExit>,
) {
    if job.done {
        return;
    }
    // The map loads through the asset server, so count frames from the first one that has
    // terrain; then let LOD meshes, asset upload and shadows settle.
    if stats.chunks == 0 {
        return;
    }
    job.frame += 1;
    let frame = job.frame;
    const WARMUP: u32 = 120;
    if job.screenshot && job.bench_secs.is_none() {
        if frame == WARMUP
            && let Some(mut capture) = capture
        {
            // `capture` retries blank frames and ends the app itself.
            capture.arm();
            job.done = true;
        }
        return;
    }
    let Some(secs) = job.bench_secs else { return };
    if frame <= WARMUP {
        return;
    }
    job.frame_ms.push(time.delta_secs() * 1000.0);
    let total: f32 = job.frame_ms.iter().sum::<f32>() / 1000.0;
    if frame == WARMUP + 1 {
        info!("bench: warm-up done, measuring {secs} s");
    }
    if total < secs {
        return;
    }
    let mut sorted = job.frame_ms.clone();
    sorted.sort_by(f32::total_cmp);
    let avg = total * 1000.0 / sorted.len() as f32;
    let pct = |p: f32| sorted[((sorted.len() - 1) as f32 * p) as usize];
    println!(
        "BENCH frames={} avg_ms={avg:.2} fps={:.1} p50_ms={:.2} p95_ms={:.2} p99_ms={:.2} max_ms={:.2}",
        sorted.len(),
        1000.0 / avg,
        pct(0.5),
        pct(0.95),
        pct(0.99),
        sorted.last().copied().unwrap_or(0.0)
    );
    println!(
        "BENCH terrain chunks={} visible={} by_lod={:?} terrain_tris={}",
        stats.chunks, stats.visible_chunks, stats.visible_by_lod, stats.terrain_triangles
    );
    println!(
        "BENCH roads pieces={} chunks={} near_tris={} last_rebuilt={} last_update_ms={:.2}",
        roads.pieces, roads.chunks, roads.triangles, roads.last_rebuilt, roads.last_update_ms
    );
    println!(
        "BENCH buildings count={} chunks={} constructing={} near_tris={} drawn_tris={} last_rebuilt={} last_update_ms={:.2}",
        buildings.buildings,
        buildings.chunks,
        buildings.constructing,
        buildings.near_triangles,
        buildings.drawn_triangles,
        buildings.last_rebuilt,
        buildings.last_update_ms
    );
    println!(
        "BENCH agents vehicles_in_sim={} pedestrians_in_sim={} vehicles_drawn={} pedestrians_drawn={} draws={} prepare_ms={:.2} view_ms={:.2}",
        agents.vehicles_in_sim,
        agents.pedestrians_in_sim,
        agents.vehicles_drawn,
        agents.pedestrians_drawn,
        agents.draws,
        agents.prepare_ms,
        agents.view_ms
    );
    for d in diagnostics.iter() {
        if let Some(v) = d.smoothed() {
            println!("BENCH diag {} = {v:.3}{}", d.path(), d.suffix);
        }
    }
    job.done = true;
    match capture {
        // The screenshot, if any, is taken after the report and ends the app.
        Some(mut capture) if job.screenshot => capture.arm(),
        _ => {
            exit.write(AppExit::Success);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_core::Tick;
    use std::time::Duration;

    fn dir(map: &str) -> PathBuf {
        PathBuf::from(ASSETS_DIR).join("maps").join(map)
    }

    #[test]
    fn both_maps_load_into_the_sim_and_reach_the_view() {
        for map in ["flatlands", "river_valley"] {
            let mut sim =
                cp_app::load_sim(&dir(map), SimLimits::desktop(), 12.0, true, None).unwrap();
            let v = sim.view();
            assert_eq!(v.terrain.as_ref().unwrap().meta().name, map);
            assert_eq!(v.game_time.hour(), 12);
            // The camera bounds read these: both maps open the 3x3 around (2, 2).
            assert_eq!(v.tiles, cp_terrain::TileOwnership::m1_around((2, 2)));
            // PreUpdate stepping: one 50 ms frame at x1 is one tick.
            assert_eq!(sim.step(Duration::from_millis(50)), 1);
            assert_eq!(sim.view().tick, Tick(v.tick.0 + 1));
            assert_eq!(
                phase_times(&sim).phases.len(),
                cp_sim::SimPhase::ORDER.len()
            );
        }
    }

    #[test]
    fn unknown_map_is_an_error_not_a_panic() {
        assert!(cp_app::load_sim(&dir("nope"), SimLimits::desktop(), 10.0, false, None).is_err());
    }

    #[test]
    fn options_default_to_flatlands() {
        let o = Options::parse(&["cp".into()]);
        assert_eq!((o.map.as_str(), o.vsync), ("flatlands", true));
        let o = Options::parse(&[
            "cp".into(),
            "--map".into(),
            "river_valley".into(),
            "--bench".into(),
            "5".into(),
        ]);
        assert_eq!(
            (o.map.as_str(), o.bench, o.vsync),
            ("river_valley", Some(5.0), false)
        );
    }
}
