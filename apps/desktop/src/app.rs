//! The game app: SimRunner stepped in `PreUpdate`, `SimView` published as a resource,
//! terrain rendering and the RTS camera from `cp_render`.
//!
//! `cargo run --release -p cp_desktop -- [--map flatlands|river_valley] [--cam overview|ground|valley]`
//! `  [--hour 0..24] [--screenshot out.png] [--bench seconds] [--no-vsync]`

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::render::diagnostic::RenderDiagnosticsPlugin;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::PresentMode;
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_render::{RtsCamera, RtsCameraPlugin, TerrainRenderPlugin, TerrainStats};
use cp_sim::{SimRunner, SimView};
use cp_terrain::{MAP_SIZE, Terrain};

use crate::ASSETS_DIR;

pub struct Options {
    pub map: String,
    pub cam: String,
    pub hour: Option<f32>,
    pub screenshot: Option<PathBuf>,
    pub bench: Option<f32>,
    pub vsync: bool,
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
            vsync: !args.iter().any(|a| a == "--no-vsync") && bench.is_none(),
        }
    }
}

#[derive(Resource)]
struct Sim(SimRunner);

#[derive(Resource)]
struct Job {
    screenshot: Option<PathBuf>,
    bench_secs: Option<f32>,
    frame: u32,
    frame_ms: Vec<f32>,
}

/// Loads `assets/maps/<name>` into a fresh sim.
pub fn load_sim(map: &str, hour: Option<f32>) -> Result<SimRunner, String> {
    let dir = PathBuf::from(ASSETS_DIR).join("maps").join(map);
    let terrain = Terrain::load(&dir).map_err(|e| format!("cannot load map '{map}': {e}"))?;
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::new(terrain));
    let hour = hour.unwrap_or(10.0);
    sim.world_mut()
        .insert_resource(Tick((hour / 24.0 * TICKS_PER_DAY as f32) as u64));
    Ok(sim)
}

pub fn run(opts: Options) {
    let sim = match load_sim(&opts.map, opts.hour) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let (pos, look) = camera_pose(&opts.cam, sim.view().terrain.as_deref());

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(AssetPlugin {
                file_path: ASSETS_DIR.into(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: format!("cityplanner — {}", opts.map),
                    resolution: (1600, 900).into(),
                    present_mode: if opts.vsync {
                        PresentMode::AutoVsync
                    } else {
                        PresentMode::AutoNoVsync
                    },
                    ..default()
                }),
                ..default()
            }),
    )
    .add_plugins((
        TerrainRenderPlugin,
        RtsCameraPlugin,
        FrameTimeDiagnosticsPlugin::default(),
        RenderDiagnosticsPlugin,
    ))
    .insert_resource(sim.view())
    .insert_resource(Sim(sim))
    .insert_resource(Job {
        screenshot: opts.screenshot,
        bench_secs: opts.bench,
        frame: 0,
        frame_ms: Vec::new(),
    })
    .add_systems(Startup, move |mut commands: Commands| {
        commands.spawn((
            Camera3d::default(),
            Projection::Perspective(PerspectiveProjection {
                far: 40_000.0,
                near: 1.0,
                ..default()
            }),
            Transform::from_translation(pos).looking_at(look, Vec3::Y),
            RtsCamera::from_pose(pos, look),
        ));
    })
    .add_systems(PreUpdate, step_sim)
    .add_systems(Update, job);
    app.run();
}

/// Camera start poses for measuring. `overview` is the highest allowed view (4000 m).
fn camera_pose(name: &str, terrain: Option<&Terrain>) -> (Vec3, Vec3) {
    let c = MAP_SIZE / 2.0;
    match name {
        "ground" => {
            let (x, z) = (5120.0, 5600.0);
            let y = terrain.map_or(30.0, |t| t.height(Vec2::new(x, z))) + 60.0;
            (Vec3::new(x, y, z), Vec3::new(x, y - 25.0, z - 800.0))
        }
        "valley" => (
            Vec3::new(c, 1500.0, c + 2500.0),
            Vec3::new(c, 0.0, c - 500.0),
        ),
        _ => (Vec3::new(c, 4000.0, c + 4200.0), Vec3::new(c, 0.0, c)),
    }
}

fn step_sim(time: Res<Time>, mut sim: ResMut<Sim>, mut view: ResMut<SimView>) {
    if sim.0.step(Duration::from_secs_f64(time.delta_secs_f64())) > 0 {
        *view = sim.0.view();
    }
}

/// Screenshot and benchmark driver.
fn job(
    mut commands: Commands,
    mut job: ResMut<Job>,
    time: Res<Time<Real>>,
    stats: Res<TerrainStats>,
    diagnostics: Res<DiagnosticsStore>,
    mut exit: MessageWriter<AppExit>,
) {
    job.frame += 1;
    let frame = job.frame;
    // Let chunk generation, asset upload and shadows settle first.
    const WARMUP: u32 = 120;
    if let Some(path) = job.screenshot.clone()
        && job.bench_secs.is_none()
    {
        if frame == WARMUP {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path));
        } else if frame == WARMUP + 60 {
            exit.write(AppExit::Success);
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
    if let Some(path) = job.screenshot.clone() {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
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
    for d in diagnostics.iter() {
        if let Some(v) = d.smoothed() {
            println!("BENCH diag {} = {v:.3}{}", d.path(), d.suffix);
        }
    }
    // Give a requested screenshot a few frames to be written.
    if job.screenshot.is_none() || total > secs + 0.5 {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_maps_load_into_the_sim_and_reach_the_view() {
        for map in ["flatlands", "river_valley"] {
            let mut sim = load_sim(map, Some(12.0)).unwrap();
            let v = sim.view();
            assert_eq!(v.terrain.as_ref().unwrap().meta().name, map);
            assert_eq!(v.game_time.hour(), 12);
            // The camera bounds read these: both maps open the 3x3 around (2, 2).
            assert_eq!(v.tiles, cp_terrain::TileOwnership::m1_around((2, 2)));
            // PreUpdate stepping: one 50 ms frame at x1 is one tick.
            assert_eq!(sim.step(Duration::from_millis(50)), 1);
            assert_eq!(sim.view().tick, Tick(v.tick.0 + 1));
        }
    }

    #[test]
    fn unknown_map_is_an_error_not_a_panic() {
        assert!(load_sim("nope", None).is_err());
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
