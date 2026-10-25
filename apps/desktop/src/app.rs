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
use cp_sim::{SimRunner, SimView};

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
    pub net_debug: bool,
    /// `--day-night` (or an explicit `--hour`): run the day and night cycle (T-178: the
    /// default is the always-day sun).
    pub day_night: bool,
    /// `--overlay zones|traffic|...`: start with that map overlay on (for screenshots).
    pub overlay: Option<cp_render::Overlay>,
    /// `--ghost-demo basic_2l,x1,z1,x2,z2[,level][,build]`: hold the road tool's ghost from
    /// (x1, z1) to the cursor at (x2, z2) without a cursor (screenshots of the zone strip,
    /// T-185); `level` 1..4 raised, -1 underground (BUG-121); `build`: build it, show the road.
    pub ghost_demo: Option<cp_tools::GhostDemo>,
    /// `--utility-demo water,x1,z1,...,x,z` (or `power`): hold the pipe / line tool with the
    /// points clicked and the cursor at the last (x, z); `water-bulldoze,x,z`: the bulldozer in
    /// the water layer over (x, z) (T-286 shots).
    pub utility_demo: Option<cp_tools::UtilityDemo>,
    /// `--mood-demo elementary_school,0.3`: the houses' emoji wave of that building as if it had
    /// just opened, held 0.3 s in (T-287 shots).
    pub mood_demo: Option<cp_render::MoodDemo>,
    /// `--thought-demo 2.0`: the hour's thought bubbles over the unhappy homes in view start at
    /// once and are held 2 s in (T-318d shots; `--scene town:N`).
    pub thought_demo: Option<f32>,
    /// `--dust-demo 0.4`: the dust the scene leaves (`--scene school`, `--scene dust`), held
    /// 0.4 s in (T-288 shots).
    pub dust_demo: Option<f32>,
    /// `--place-demo fire_station,x,z[,move]`: hold the placement tool's ghost of that building at
    /// (x, z) without a cursor (screenshots of the coverage preview, T-254).
    pub place_demo: Option<cp_tools::PlaceDemo>,
    /// `--upgrade-demo road,x,z[,whole]`: the upgrade tool held over (x, z) (BUG-103 shots).
    pub upgrade_demo: Option<cp_tools::UpgradeDemo>,
    /// `--trees-demo x,z[,whole]`: the Trees tool over a point (metres from the map centre).
    pub trees_demo: Option<cp_tools::TreesDemo>,
    pub sign_demo: Option<cp_tools::SignDemo>,
    /// `--lane-demo x,z`: the Junctions tool's Lanes mode with the arm nearest (x, z) chosen (T-313).
    pub lane_demo: Option<cp_tools::LaneDemo>,
    /// `--brush-demo district|area,small|large|erase,x,z[,x1,z1][,ghost]`: one click of the
    /// district or area brush at (x, z), or a stroke to (x1, z1), then its ghost held there;
    /// `ghost`: no stroke (BUG-108 and BUG-112 shots).
    pub brush_demo: Option<cp_tools::BrushDemo>,
    /// `--grading exposure=0.1,contrast=1.1,...`: this run's colour grading (T-273).
    pub grading: Option<cp_render::GradingSettings>,
    /// `--grading-panel`: open the F8 colour grading panel at start.
    pub grading_panel: bool,
    /// `--fold-overlays`: the overlays panel starts folded (T-289 screenshots).
    pub fold_overlays: bool,
    /// `--open jobs,metrics`: windows that start open (screenshots).
    pub open_windows: Vec<cp_ui::UiWindow>,
    /// `--budget-tab budgets`: the economy window's tab at start (screenshots).
    pub budget_tab: Option<cp_ui::budget::BudgetTab>,
    /// `--metrics-section Tourism`: the metrics window starts scrolled to that section (shots).
    pub metrics_section: Option<String>,
    /// `--pointer x,y`: hold the pointer there in a hidden shot, so the cursor hint shows.
    pub pointer: Option<(f32, f32)>,
    /// `--ui`: keep the egui UI in a `--hidden` run, so screenshots show the panels.
    pub hidden_ui: bool,
    /// `--category roads|zones|...`: open that palette category at start (screenshots).
    pub category: Option<cp_ui::Category>,
    /// `--tray-tab Large`: the tab (English name) of that category's tray at start (T-317f).
    pub tray_tab: Option<String>,
    /// `--no-clouds`: no cloud shadows in this run (T-272).
    pub no_clouds: bool,
    /// `--no-parked`: no parked cars in this run (T-276).
    pub no_parked: bool,
    /// `--lang en|ru`: the language of the UI in this run (T-284 screenshots).
    pub lang: Option<cp_locale::Lang>,
    /// `--inspect`: the first building of the city is picked a second into the run, so the
    /// inspector shows (T-284 screenshots).
    pub inspect: bool,
    /// `--inspect-home`: the same with the first home that has complaints, else the first scored
    /// home (T-318c screenshots of the happiness rows).
    pub inspect_home: bool,
    /// `--door-demo N`: aim the camera at a walker stepping out of (or into) its door once one
    /// shows up, and take the screenshot N frames later (T-329b shots).
    pub door_demo: Option<u32>,
    /// `--yard-demo N`: the same with a car pulling out of (or into) its yard (T-330b shots).
    pub yard_demo: Option<u32>,
    /// `--speed 1|2|3`: run the game at that speed from the start (T-281 benches).
    pub speed: Option<cp_core::SimSpeed>,
    /// `--no-qos`: leave the game's threads at the default quality of service (T-281: to
    /// measure what [`crate::qos`] gives).
    pub qos: bool,
    /// `--size WxH`: the pixels of a `--hidden` run's target (default 1600x900).
    pub size: Option<(u32, u32)>,
    /// `--log-pipelines`: log every new render pipeline with the time (T-281: a pipeline is
    /// compiled on the render thread, synchronously on macOS).
    pub log_pipelines: bool,
    /// `--milestone N` / `--milestone N,live`: hold the city at milestone N (an index of
    /// `milestones.ron`) from the start; with `live` it starts at N-1 and reaches N a moment
    /// later, so the congratulation window shows (T-283 screenshots).
    pub milestone: Option<(u8, bool)>,
    /// `--pan`: the camera moves by itself, round a 300 m circle in 20 s while it turns once in
    /// 40 s (T-281: the user's stutter shows when the camera moves).
    pub pan: bool,
    /// `--zoning` / `--zoning x,z`: switch the zone tool on a second into the run, so the zone
    /// grid shows in zoning mode after it was first built without; with a point (metres from
    /// the map centre) the cursor hovers there (BUG-098 screenshots).
    pub zoning: Option<Option<bevy::math::Vec2>>,
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
                let rest: Vec<&str> = it.collect();
                let build = rest.contains(&"build");
                let n: Vec<f32> = rest.iter().filter_map(|x| x.parse().ok()).collect();
                (n.len() == 4 || n.len() == 5).then(|| cp_tools::GhostDemo {
                    road,
                    from: bevy::math::Vec2::new(n[0], n[1]),
                    to: bevy::math::Vec2::new(n[2], n[3]),
                    level: n.get(4).map_or(0, |l| *l as i8),
                    build,
                })
            }),
            utility_demo: value("--utility-demo").and_then(|v| cp_tools::UtilityDemo::parse(&v)),
            mood_demo: value("--mood-demo").and_then(|v| cp_render::MoodDemo::parse(&v)),
            thought_demo: value("--thought-demo").and_then(|v| v.parse().ok()),
            dust_demo: value("--dust-demo").and_then(|v| v.parse().ok()),
            place_demo: value("--place-demo").and_then(|v| {
                let mut it = v.split(',');
                let def = it.next()?.to_string();
                let rest: Vec<&str> = it.collect();
                let moving = rest.contains(&"move");
                let n: Vec<f32> = rest.iter().filter_map(|x| x.parse().ok()).collect();
                (n.len() == 2).then(|| cp_tools::PlaceDemo {
                    def,
                    at: bevy::math::Vec2::new(n[0], n[1]),
                    moving,
                })
            }),
            brush_demo: value("--brush-demo").and_then(|v| {
                let mut it = v.split(',');
                let area = it.next()? == "area";
                let kind = match it.next()? {
                    "large" => cp_tools::DistrictToolKind::BrushLarge,
                    "erase" => cp_tools::DistrictToolKind::Erase,
                    _ => cp_tools::DistrictToolKind::BrushSmall,
                };
                let rest: Vec<&str> = it.collect();
                let n: Vec<f32> = rest.iter().filter_map(|x| x.parse().ok()).collect();
                // `ghost`: no stroke, only the ghost with nothing chosen; four numbers: a
                // stroke from the first point to the second (BUG-112).
                (n.len() == 2 || n.len() == 4).then(|| cp_tools::BrushDemo {
                    area,
                    kind,
                    at: bevy::math::Vec2::new(n[0], n[1]),
                    to: (n.len() == 4).then(|| bevy::math::Vec2::new(n[2], n[3])),
                    done: rest.contains(&"ghost"),
                })
            }),
            upgrade_demo: value("--upgrade-demo").and_then(|v| {
                let mut it = v.split(',');
                let road = it.next()?.to_string();
                let rest: Vec<&str> = it.collect();
                let n: Vec<f32> = rest.iter().filter_map(|x| x.parse().ok()).collect();
                (n.len() == 2).then(|| cp_tools::UpgradeDemo {
                    road,
                    at: bevy::math::Vec2::new(n[0], n[1]),
                    whole: rest.contains(&"whole"),
                })
            }),
            trees_demo: value("--trees-demo").and_then(|v| {
                let mut it = v.split(',');
                let x: f32 = it.next()?.parse().ok()?;
                let z: f32 = it.next()?.parse().ok()?;
                let c = cp_terrain::MAP_SIZE / 2.0;
                Some(cp_tools::TreesDemo {
                    at: bevy::math::Vec2::new(c + x, c + z),
                    whole: it.next() == Some("whole"),
                })
            }),
            sign_demo: value("--sign-demo").and_then(|v| cp_tools::SignDemo::parse(&v)),
            lane_demo: value("--lane-demo").and_then(|v| cp_tools::LaneDemo::parse(&v)),
            grading: value("--grading").and_then(|v| match cp_render::GradingSettings::parse(&v) {
                Ok(g) => Some(g),
                Err(e) => {
                    eprintln!("--grading: {e}");
                    None
                }
            }),
            grading_panel: args.iter().any(|a| a == "--grading-panel"),
            fold_overlays: args.iter().any(|a| a == "--fold-overlays"),
            open_windows: value("--open")
                .map(|v| v.split(',').filter_map(cp_ui::UiWindow::parse).collect())
                .unwrap_or_default(),
            hidden_ui: args.iter().any(|a| a == "--ui"),
            pointer: value("--pointer").and_then(|v| {
                let (x, y) = v.split_once(',')?;
                Some((x.parse().ok()?, y.parse().ok()?))
            }),
            no_clouds: args.iter().any(|a| a == "--no-clouds"),
            no_parked: args.iter().any(|a| a == "--no-parked"),
            lang: value("--lang").and_then(|v| cp_locale::Lang::from_code(&v)),
            inspect: args.iter().any(|a| a == "--inspect"),
            inspect_home: args.iter().any(|a| a == "--inspect-home"),
            door_demo: value("--door-demo").and_then(|v| v.parse().ok()),
            yard_demo: value("--yard-demo").and_then(|v| v.parse().ok()),
            pan: args.iter().any(|a| a == "--pan"),
            zoning: args.iter().position(|a| a == "--zoning").map(|i| {
                let n: Vec<f32> = args
                    .get(i + 1)
                    .map(|v| v.split(',').filter_map(|x| x.parse().ok()).collect())
                    .unwrap_or_default();
                (n.len() == 2).then(|| bevy::math::Vec2::new(n[0], n[1]))
            }),
            milestone: value("--milestone").and_then(|v| {
                let (n, live) = match v.split_once(',') {
                    Some((n, "live")) => (n, true),
                    _ => (v.as_str(), false),
                };
                Some((n.parse().ok()?, live))
            }),
            log_pipelines: args.iter().any(|a| a == "--log-pipelines"),
            qos: !args.iter().any(|a| a == "--no-qos"),
            size: value("--size").and_then(|v| {
                let (w, h) = v.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?))
            }),
            speed: value("--speed").and_then(|v| match v.as_str() {
                "1" => Some(cp_core::SimSpeed::X1),
                "2" => Some(cp_core::SimSpeed::X2),
                "3" => Some(cp_core::SimSpeed::X3),
                _ => None,
            }),
            category: value("--category").and_then(|n| {
                cp_ui::Category::ALL
                    .into_iter()
                    .find(|c| c.label().eq_ignore_ascii_case(&n))
            }),
            tray_tab: value("--tray-tab"),
            budget_tab: value("--budget-tab").and_then(|v| cp_ui::budget::BudgetTab::parse(&v)),
            metrics_section: value("--metrics-section"),
            always_day: args.iter().any(|a| a == "--always-day"),
            net_debug: args.iter().any(|a| a == "--net-debug"),
            day_night: args.iter().any(|a| a == "--day-night") || value("--hour").is_some(),
            hidden: crate::capture::hidden_requested(args),
            // A bench runs without vsync unless `--vsync` (T-281: frame pacing on the display).
            vsync: !args.iter().any(|a| a == "--no-vsync")
                && (bench.is_none() || args.iter().any(|a| a == "--vsync")),
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
    /// Per measured frame: its ms, what the sim cost in it, and the render stats' update times
    /// (T-281: what a hitch was).
    frames: Vec<FrameRecord>,
    /// A `--scene` was asked for: its roads and buildings come from sim ticks, so a screenshot
    /// waits for them (BUG-017).
    scene: bool,
    /// Consecutive frames for which the scene has been built and has not changed.
    built_frames: u32,
    /// Road pieces, buildings and drawn building triangles of the last frame.
    last_counts: (usize, usize, usize),
    /// Real time of the first frame of the screenshot wait.
    scene_since: Option<f32>,
}

/// One measured frame of a bench.
#[derive(Clone, Copy, Debug)]
struct FrameRecord {
    ms: f32,
    sim: cp_app::FrameSim,
    roads_ms: f32,
    buildings_ms: f32,
    agents_ms: f32,
}

/// A frame slower than this is a hitch (two frames at 60 Hz).
const HITCH_MS: f32 = 33.0;

/// The speed of `--speed`, set once the sim stands.
#[derive(Resource)]
struct StartSpeed(cp_core::SimSpeed);

fn start_speed(
    speed: Option<Res<StartSpeed>>,
    sim: Option<ResMut<cp_app::Sim>>,
    mut commands: Commands,
) {
    let (Some(speed), Some(mut sim)) = (speed, sim) else {
        return;
    };
    sim.0.world_mut().insert_resource(speed.0);
    commands.remove_resource::<StartSpeed>();
}

/// The clock of `--log-pipelines`, shared by the render and the main world.
static LOG_T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// `--milestone`: the milestone to hold the city at, and the frames since the sim stood.
#[derive(Resource)]
struct MilestoneDemo {
    index: u8,
    live: bool,
    frames: u32,
}

/// Holds the progress at the demo's milestone; with `live` it starts one below and reaches the
/// milestone after a few frames, as the city growing would (the UI then shows its window).
fn milestone_demo(demo: Option<ResMut<MilestoneDemo>>, sim: Option<ResMut<cp_app::Sim>>) {
    let (Some(mut demo), Some(mut sim)) = (demo, sim) else {
        return;
    };
    demo.frames += 1;
    let start = if demo.live {
        demo.index.saturating_sub(1)
    } else {
        demo.index
    };
    if demo.frames == 1 {
        // The jump to `start` is a "load" to the UI (the clock jumps too), so it congratulates
        // nobody; the rise to the milestone a moment later is the city growing.
        let tick = sim.0.world().resource::<cp_core::Tick>().0;
        sim.0
            .world_mut()
            .insert_resource(cp_core::Tick(tick + 100_000));
        sim.0
            .world_mut()
            .insert_resource(cp_sim::Progress::at(start));
    }
    if demo.live && demo.frames == 40 {
        sim.0
            .world_mut()
            .insert_resource(cp_sim::Progress::at(demo.index));
    }
}

/// `--log-pipelines`: each pipeline the cache holds that was not there last frame, by label.
fn log_pipelines(cache: Res<bevy::render::render_resource::PipelineCache>, mut seen: Local<usize>) {
    let t0 = *LOG_T0.get_or_init(std::time::Instant::now);
    use bevy::render::render_resource::PipelineDescriptor;
    let all: Vec<_> = cache.pipelines().collect();
    for p in &all[(*seen).min(all.len())..] {
        let label = match &p.descriptor {
            PipelineDescriptor::RenderPipelineDescriptor(d) => d.label.clone(),
            PipelineDescriptor::ComputePipelineDescriptor(d) => d.label.clone(),
            PipelineDescriptor::MeshPipelineDescriptor(d) => d.label.clone(),
        };
        println!(
            "PIPELINE t={:.3} {}",
            t0.elapsed().as_secs_f32(),
            label.as_deref().unwrap_or("?")
        );
    }
    *seen = all.len();
}

/// `--log-pipelines`: each main-world frame over 50 ms, on the same clock (T-281b: the start-up
/// stall while the pipelines compile).
fn log_slow_frames(mut last: Local<Option<std::time::Instant>>) {
    let t0 = *LOG_T0.get_or_init(std::time::Instant::now);
    let now = std::time::Instant::now();
    if let Some(prev) = last.replace(now) {
        let ms = (now - prev).as_secs_f32() * 1000.0;
        if ms > 50.0 {
            println!("SLOW t={:.3} ms={ms:.0}", t0.elapsed().as_secs_f32());
        }
    }
}

/// `--pan`: the camera's target moves round a circle and turns, from where it started.
fn pan_camera(
    time: Res<Time<Real>>,
    mut cams: Query<&mut cp_render::camera::RtsCamera>,
    mut start: Local<Option<(cp_render::camera::Rig, f32)>>,
) {
    let Some(mut cam) = cams.iter_mut().next() else {
        return;
    };
    let now = time.elapsed_secs();
    let (rig, t0) = *start.get_or_insert((cam.target, now));
    let t = now - t0;
    let a = std::f32::consts::TAU * t / 20.0;
    let mut target = rig;
    target.focus = rig.focus + Vec2::new(a.cos() - 1.0, a.sin()) * 300.0;
    target.yaw = rig.yaw + std::f32::consts::TAU * t / 40.0;
    cam.target = target;
}

/// Frame of the run at which `--zoning` switches the zone tool on.
const ZONING_FRAME: u32 = 60;

/// `--inspect`: from [`ZONING_FRAME`] on, the first hotel (T-321), else the first building, is
/// picked while nothing is.
fn inspect_demo(
    mut frame: Local<u32>,
    view: Option<Res<SimView>>,
    picked: Option<ResMut<cp_tools::Picked>>,
) {
    *frame += 1;
    let (Some(view), Some(mut picked)) = (view, picked) else {
        return;
    };
    if *frame >= ZONING_FRAME
        && picked.0.is_none()
        && let Some((id, _)) = view
            .buildings
            .iter()
            .find(|(_, b)| {
                view.content
                    .as_ref()
                    .is_some_and(|db| db.building(b.def).hotel.is_some())
            })
            .or_else(|| view.buildings.iter().next())
    {
        picked.0 = Some(cp_sim::inspect::Selection::Building(id));
    }
}

/// `--inspect-home`: like [`inspect_demo`], a home with complaints (else any scored home).
fn inspect_home_demo(
    mut frame: Local<u32>,
    view: Option<Res<SimView>>,
    picked: Option<ResMut<cp_tools::Picked>>,
) {
    *frame += 1;
    let (Some(view), Some(mut picked)) = (view, picked) else {
        return;
    };
    if *frame < ZONING_FRAME || picked.0.is_some() {
        return;
    }
    let homes = &view.home_contentment;
    let home = homes
        .homes()
        .find(|(_, s)| s.complaints[0] != Default::default())
        .or_else(|| homes.homes().next());
    if let Some((id, _)) = home {
        picked.0 = Some(cp_sim::inspect::Selection::Building(id));
    }
}

/// `--door-demo`: frames since the camera was aimed at a walker by its door.
#[derive(Resource)]
struct DoorDemo {
    aimed: Option<u32>,
    /// Frames the aimed camera stands before the screenshot.
    wait: u32,
}

/// `--door-demo`: a walker on its way through its front yard (leaving preferred, about a third
/// of the way), seen from 9 m away, once.
fn door_demo(
    mut demo: ResMut<DoorDemo>,
    mut frame: Local<u32>,
    view: Option<Res<SimView>>,
    mut cams: Query<&mut cp_render::RtsCamera>,
) {
    *frame += 1;
    if let Some(n) = demo.aimed.as_mut() {
        *n += 1;
        return;
    }
    let Some(view) = view else { return };
    let peds = &view.agents.cur.pedestrians;
    if frame.is_multiple_of(120) {
        let near = peds.iter().filter(|p| p.portal.is_some()).count();
        eprintln!("door demo: {} walkers, {near} by a door", peds.len());
    }
    let Some((p, door)) = peds
        .iter()
        .filter_map(|p| {
            let pt = p.portal?;
            let b = view.buildings.get(pt.building)?;
            let close = if pt.leaving {
                pt.progress
            } else {
                1.0 - pt.progress
            };
            (close < 0.6).then_some((p, b.portals(&view.net)?.door, pt.leaving, close))
        })
        .min_by(|a, b| {
            let k = |c: f32| (c - 0.35).abs();
            (!a.2, k(a.3)).partial_cmp(&(!b.2, k(b.3))).unwrap()
        })
        .map(|(p, d, ..)| (p, d))
    else {
        return;
    };
    let look = (p.pos + door) * 0.5;
    let away = (p.pos - door).normalize_or_zero();
    let side = bevy::math::Vec2::new(-away.y, away.x);
    let eye = look + away * 6.0 + side * 5.0;
    let y = view.terrain.as_ref().map_or(0.0, |t| t.height(look));
    for mut cam in &mut cams {
        *cam = cp_render::RtsCamera::from_pose(
            Vec3::new(eye.x, y + 5.5, eye.y),
            Vec3::new(look.x, y, look.y),
        );
    }
    eprintln!("door demo: aimed at walker {} by its door {door}", p.key);
    demo.aimed = Some(0);
}

/// `--yard-demo`: a car about a third of the way out of its yard, seen low from across the
/// street. Once one is found the game is paused, so the shot `N` frames later still has it
/// there. Shares [`DoorDemo`].
fn yard_demo(
    mut demo: ResMut<DoorDemo>,
    mut frame: Local<u32>,
    view: Option<Res<SimView>>,
    sim: Option<ResMut<cp_app::Sim>>,
    mut cams: Query<&mut cp_render::RtsCamera>,
) {
    *frame += 1;
    if let Some(n) = demo.aimed.as_mut() {
        *n += 1;
        return;
    }
    let (Some(view), Some(mut sim)) = (view, sim) else {
        return;
    };
    let cars = &view.agents.cur.vehicles;
    if frame.is_multiple_of(120) {
        let near = cars.iter().filter(|v| v.portal.is_some()).count();
        eprintln!("yard demo: {} cars, {near} by a yard", cars.len());
    }
    let Some((v, yard)) = cars
        .iter()
        .filter_map(|v| {
            let pt = v
                .portal
                .filter(|p| p.leaving && (0.25..0.45).contains(&p.progress))?;
            let b = view.buildings.get(pt.building)?;
            (b.size().0 >= 2).then_some((v, b.portals(&view.net)?))
        })
        .next()
    else {
        return;
    };
    sim.0.world_mut().insert_resource(cp_core::SimSpeed::Pause);
    let look = yard.drive + yard.out * 3.0;
    let side = bevy::math::Vec2::new(-yard.out.y, yard.out.x);
    let eye = look + yard.out * 16.0 + side * 8.0;
    let y = view.terrain.as_ref().map_or(0.0, |t| t.height(look));
    for mut cam in &mut cams {
        *cam = cp_render::RtsCamera::from_pose(
            Vec3::new(eye.x, y + 9.0, eye.y),
            Vec3::new(look.x, y, look.y),
        );
    }
    eprintln!(
        "yard demo: paused on car {} by its yard {}",
        v.key, yard.drive
    );
    demo.aimed = Some(0);
}

/// Where `--zoning x,z` holds the cursor (world metres), if anywhere.
#[derive(Resource)]
struct ZoningDemo(Option<Vec2>);

/// `--zoning`: the zone tool goes on at [`ZONING_FRAME`], as a player opens it in a running city;
/// from then on the cursor hovers at the demo's point, if it has one, with the tool's preview
/// there (a hidden run has no cursor for `tool_hover` to find).
fn zoning_demo(
    mut frame: Local<u32>,
    demo: Res<ZoningDemo>,
    view: Option<Res<SimView>>,
    mut tools: ResMut<cp_tools::ToolState>,
) {
    *frame += 1;
    if *frame == ZONING_FRAME {
        tools.select(cp_tools::ActiveTool::Zone);
    }
    if *frame >= ZONING_FRAME
        && let (Some(p), Some(view)) = (demo.0, view)
    {
        tools.hover = Some(p);
        tools.zone_preview = tools.zone.preview(&view.zones, p);
    }
}

/// Frames a built scene must stand before the screenshot, so the chunk meshes are uploaded.
const SETTLE_FRAMES: u32 = 30;
/// Extra wall-clock time the screenshot job of a scene may take.
const SCENE_EXTRA: std::time::Duration = std::time::Duration::from_secs(120);
/// Seconds after which a screenshot is taken even if the scene never settles.
const SCENE_GIVE_UP_S: f32 = 100.0;
/// Frames after which a screenshot is taken even if the scene never settles.
const SCENE_GIVE_UP: u32 = 3000;

/// Whether a scene has appeared: roads or buildings stand (bare terrain is not a scene). The
/// sites under construction never finish in a paused scene, so a screenshot waits for the
/// counts to stop changing instead.
fn scene_built(road_pieces: usize, buildings: usize) -> bool {
    road_pieces > 0 || buildings > 0
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
            .set(if opts.qos {
                crate::qos::task_pool_plugin()
            } else {
                bevy::app::TaskPoolPlugin::default()
            })
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
        crate::capture::offscreen_target_sized(
            &mut app.world_mut().resource_mut::<Assets<Image>>(),
            opts.size.unwrap_or(crate::capture::HIDDEN_SIZE),
        )
    });
    if opts.hidden && opts.bench.is_some() {
        // With no window the loop counts as unfocused and would idle at 60 Hz: a bench runs
        // uncapped. A screenshot keeps the 60 Hz pace (T-245): uncapped, a paused city seen
        // obliquely queued frames faster than the GPU drew them into the off-screen image
        // until Metal lost the device.
        app.insert_resource(bevy::winit::WinitSettings::continuous());
    }
    if let Some(active) = opts.overlay {
        app.insert_resource(cp_render::OverlayState {
            active,
            ..Default::default()
        });
    }
    if let Some(demo) = opts.ghost_demo {
        // The tools only run with the UI; a held ghost for a hidden screenshot needs them.
        if opts.hidden && !opts.hidden_ui {
            app.add_plugins(cp_tools::ToolsPlugin);
        }
        app.insert_resource(demo);
    }
    if let Some(demo) = opts.mood_demo {
        app.insert_resource(demo);
    }
    if let Some(age) = opts.thought_demo {
        app.insert_resource(cp_render::ThoughtDemo { age });
    }
    if let Some(age) = opts.dust_demo {
        app.insert_resource(cp_render::DustDemo { age, puffs: None });
    }
    if let Some(demo) = opts.utility_demo {
        if opts.hidden && !opts.hidden_ui && !app.is_plugin_added::<cp_tools::ToolsPlugin>() {
            app.add_plugins(cp_tools::ToolsPlugin);
        }
        app.insert_resource(demo);
    }
    // The held ghost of `--scene grid-shots --cam grid-shotN` (T-255) needs the tools too.
    if opts.hidden
        && cp_app::grid_shots::shot_of_cam(&opts.cam).is_some()
        && !app.is_plugin_added::<cp_tools::ToolsPlugin>()
    {
        app.add_plugins(cp_tools::ToolsPlugin);
    }
    if opts.inspect {
        app.add_systems(Update, inspect_demo);
    }
    if opts.inspect_home {
        app.add_systems(Update, inspect_home_demo);
    }
    if let Some(wait) = opts.door_demo {
        app.insert_resource(DoorDemo { aimed: None, wait })
            .add_systems(Update, door_demo);
    }
    if let Some(wait) = opts.yard_demo {
        app.insert_resource(DoorDemo { aimed: None, wait })
            .add_systems(Update, yard_demo);
    }
    if let Some(hover) = opts.zoning {
        if opts.hidden && !opts.hidden_ui && !app.is_plugin_added::<cp_tools::ToolsPlugin>() {
            app.add_plugins(cp_tools::ToolsPlugin);
        }
        let c = cp_terrain::MAP_SIZE / 2.0;
        app.insert_resource(ZoningDemo(hover.map(|p| p + bevy::math::Vec2::splat(c))))
            .add_systems(
                Update,
                zoning_demo
                    .after(cp_tools::plugin::tool_hover)
                    .before(cp_tools::zone_lines::sync_zone_lines),
            );
    }
    if let Some(demo) = opts.place_demo {
        if opts.hidden && !opts.hidden_ui && !app.is_plugin_added::<cp_tools::ToolsPlugin>() {
            app.add_plugins(cp_tools::ToolsPlugin);
        }
        app.insert_resource(demo);
    }
    if let Some(demo) = opts.brush_demo {
        if opts.hidden && !opts.hidden_ui && !app.is_plugin_added::<cp_tools::ToolsPlugin>() {
            app.add_plugins(cp_tools::ToolsPlugin);
        }
        app.insert_resource(demo);
    }
    if let Some(demo) = opts.upgrade_demo {
        if opts.hidden && !opts.hidden_ui && !app.is_plugin_added::<cp_tools::ToolsPlugin>() {
            app.add_plugins(cp_tools::ToolsPlugin);
        }
        app.insert_resource(demo);
    }
    if let Some(demo) = opts.trees_demo {
        if opts.hidden && !opts.hidden_ui && !app.is_plugin_added::<cp_tools::ToolsPlugin>() {
            app.add_plugins(cp_tools::ToolsPlugin);
        }
        app.insert_resource(demo);
    }
    if let Some(demo) = opts.lane_demo {
        if opts.hidden && !opts.hidden_ui && !app.is_plugin_added::<cp_tools::ToolsPlugin>() {
            app.add_plugins(cp_tools::ToolsPlugin);
        }
        app.insert_resource(demo);
    }
    if let Some(demo) = opts.sign_demo {
        if opts.hidden && !opts.hidden_ui && !app.is_plugin_added::<cp_tools::ToolsPlugin>() {
            app.add_plugins(cp_tools::ToolsPlugin);
        }
        app.insert_resource(demo);
    }
    // GPU timestamps of the render passes: a hidden run (no window) on Metal cannot allocate
    // their counter buffers and loses the device a few seconds in (T-276: `--hidden --bench`).
    if !opts.hidden {
        app.add_plugins(RenderDiagnosticsPlugin);
    }
    app.add_plugins((
        GamePlugin {
            map: opts.map,
            limits: SimLimits::desktop(),
            hour,
            cam: opts.cam,
            // Wall-clock phase timing for the F3 window.
            phase_timing: true,
            ui: !opts.hidden || opts.hidden_ui,
            always_day: opts.always_day,
            net_debug: opts.net_debug,
            day_night: opts.day_night,
            scene: opts.scene.clone(),
            target: target.clone(),
            grading: opts.grading,
            grading_panel: opts.grading_panel,
            fold_overlays: opts.fold_overlays,
            open_windows: opts.open_windows.clone(),
            budget_tab: opts.budget_tab,
            metrics_section: opts.metrics_section.clone(),
            category: opts.category,
            tray_tab: opts.tray_tab.clone(),
            pointer: opts.pointer,
            no_clouds: opts.no_clouds,
            no_parked: opts.no_parked,
            lang: opts.lang,
        },
        FrameTimeDiagnosticsPlugin::default(),
    ))
    .add_plugins(CapturePlugin)
    .insert_resource(Job {
        screenshot: opts.screenshot.is_some(),
        done: false,
        bench_secs: opts.bench,
        frame: 0,
        frame_ms: Vec::new(),
        frames: Vec::new(),
        scene: opts.scene.is_some(),
        built_frames: 0,
        last_counts: (0, 0, 0),
        scene_since: None,
    })
    .add_systems(Update, (job, exit_on_load_failure, start_speed));
    if let Some(speed) = opts.speed {
        app.insert_resource(StartSpeed(speed));
    }
    if opts.pan {
        app.add_systems(Update, pan_camera);
    }
    if let Some((index, live)) = opts.milestone {
        app.insert_resource(MilestoneDemo {
            index,
            live,
            frames: 0,
        });
        app.add_systems(Update, milestone_demo);
    }
    if opts.qos {
        crate::qos::set_current_thread();
        crate::qos::add_render_thread(&mut app);
    }
    if opts.log_pipelines {
        app.add_systems(First, log_slow_frames);
        if let Some(render) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render.add_systems(bevy::render::Render, log_pipelines);
        }
    }
    if let Some(path) = opts.screenshot {
        let mut capture = Capture::new(
            path,
            // A scene is built and its meshes streamed in before the shot (BUG-017).
            std::time::Duration::from_secs_f32(opts.bench.unwrap_or(0.0))
                + if opts.scene.is_some() {
                    SCENE_EXTRA
                } else {
                    std::time::Duration::ZERO
                },
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
    // The slowest ticks with their phases (T-281: what a sim spike is).
    let mut slow: Vec<(f64, u64, Vec<u64>)> = Vec::new();
    for _ in 0..ticks {
        sim.tick_once();
        if let Some(row) = cp_app::last_tick_phases(&sim) {
            let total = row.iter().sum::<u64>() as f64 / 1e6;
            slow.push((total, sim.tick().0, row));
        }
    }
    slow.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (total, tick, row) in slow.iter().take(25) {
        let phases: Vec<String> = cp_sim::SimPhase::ORDER
            .iter()
            .zip(row)
            .filter(|(_, ns)| **ns > 500_000)
            .map(|(p, ns)| format!("{p:?}={:.1}", *ns as f64 / 1e6))
            .collect();
        println!(
            "DEBUG slow_tick tick={tick} total_ms={total:.1} {}",
            phases.join(" ")
        );
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
    frame_sim: Res<cp_app::FrameSim>,
    diagnostics: Res<DiagnosticsStore>,
    capture: Option<ResMut<Capture>>,
    mut exit: MessageWriter<AppExit>,
    door_demo: Option<Res<DoorDemo>>,
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
        let counts = (roads.pieces, buildings.buildings, 0);
        if scene_built(counts.0, counts.1) && counts == job.last_counts {
            job.built_frames += 1;
        } else {
            job.built_frames = 0;
        }
        job.last_counts = counts;
        // On a busy machine the frames are slow: give up by the clock as well as by frames.
        let since = *job.scene_since.get_or_insert(time.elapsed_secs());
        let waited = time.elapsed_secs() - since > SCENE_GIVE_UP_S;
        let door_wait =
            door_demo.is_some_and(|d| d.aimed.is_none_or(|n| n < d.wait.max(1))) && !waited;
        let ready = !door_wait
            && (!job.scene
                || job.built_frames >= SETTLE_FRAMES
                || frame >= SCENE_GIVE_UP
                || waited);
        if frame >= WARMUP
            && ready
            && let Some(mut capture) = capture
        {
            if job.scene && job.built_frames < SETTLE_FRAMES {
                eprintln!("screenshot: the scene did not settle in {frame} frames");
            }
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
    job.frames.push(FrameRecord {
        ms: time.delta_secs() * 1000.0,
        sim: *frame_sim,
        roads_ms: roads.last_update_ms,
        buildings_ms: buildings.last_update_ms,
        agents_ms: agents.prepare_ms,
    });
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
    print_hitches(&job.frames);
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

/// The bench's hitch report (T-281): frames over [`HITCH_MS`] with what the sim and the
/// renderers cost in them, and the frame time by the ticks the frame stepped.
fn print_hitches(frames: &[FrameRecord]) {
    let hitches: Vec<(usize, &FrameRecord)> = frames
        .iter()
        .enumerate()
        .filter(|(_, f)| f.ms > HITCH_MS)
        .collect();
    println!(
        "BENCH hitches over {HITCH_MS} ms: {} of {}",
        hitches.len(),
        frames.len()
    );
    for (i, f) in hitches.iter().take(40) {
        println!(
            "BENCH hitch frame={i} ms={:.1} ticks={} step_ms={:.1} view_ms={:.2} roads_ms={:.2} buildings_ms={:.2} agents_ms={:.2}",
            f.ms,
            f.sim.ticks,
            f.sim.step_ms,
            f.sim.view_ms,
            f.roads_ms,
            f.buildings_ms,
            f.agents_ms
        );
    }
    for ticks in 0..=cp_sim::MAX_TICKS_PER_STEP {
        let mut ms: Vec<f32> = frames
            .iter()
            .filter(|f| f.sim.ticks == ticks)
            .map(|f| f.ms)
            .collect();
        let mut step: Vec<f32> = frames
            .iter()
            .filter(|f| f.sim.ticks == ticks)
            .map(|f| f.sim.step_ms)
            .collect();
        if ms.is_empty() {
            continue;
        }
        ms.sort_by(f32::total_cmp);
        step.sort_by(f32::total_cmp);
        let pct = |v: &[f32], p: f32| v[((v.len() - 1) as f32 * p) as usize];
        println!(
            "BENCH by_ticks ticks={ticks} frames={} frame_p50={:.2} frame_p95={:.2} frame_max={:.2} step_p50={:.2} step_p95={:.2} step_max={:.2}",
            ms.len(),
            pct(&ms, 0.5),
            pct(&ms, 0.95),
            ms[ms.len() - 1],
            pct(&step, 0.5),
            pct(&step, 0.95),
            step[step.len() - 1]
        );
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
            // A new game starts paused (BUG-059); PreUpdate stepping: one 50 ms frame at x1 is
            // one tick.
            assert_eq!(sim.speed(), cp_core::SimSpeed::Pause);
            sim.world_mut().insert_resource(cp_core::SimSpeed::X1);
            assert_eq!(sim.step(Duration::from_millis(50)), 1);
            assert_eq!(sim.view().tick, Tick(v.tick.0 + 1));
            assert_eq!(
                phase_times(&sim).phases.len(),
                cp_sim::SimPhase::ORDER.len()
            );
        }
    }

    #[test]
    fn a_scene_has_appeared_when_roads_or_buildings_stand() {
        assert!(!scene_built(0, 0), "bare terrain");
        assert!(scene_built(40, 0), "roads alone count");
        assert!(scene_built(40, 900));
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
