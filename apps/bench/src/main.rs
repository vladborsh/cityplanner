//! `cp_bench`: headless benchmark harness (`architecture/system-design.md` §14).
//!
//! ```text
//! cp_bench [--map DIR|NAME] [--save FILE] [--scenario empty|grid_city] [--pop N] [--content DIR]
//!          [--ticks N] [--warmup N] [--seed S] [--out FILE.json] [--inject-sleep Phase=MS] [--periodic-checksum]
//! cp_bench fixture --pop N [--seed S] [--map DIR|NAME] [--content DIR] --out FILE.sav
//!                                                        (writes a grid_city save)
//! cp_bench aggregate OUT.json RUN.json...        median over runs
//! cp_bench paths [--grid N] [--requests N] [--threads 1,8] [--seed S]   (pathfinding throughput)
//! cp_bench compare BASELINE.json CURRENT.json [--threshold 0.2] [--floor-ms 0.05]
//! ```
//!
//! This is an app, not a sim crate, so it may read the wall clock. `--inject-sleep` exists
//! only to prove the CI gate fails on a slow phase.

mod paths;
mod report;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use bevy_tasks::ComputeTaskPool;
use cp_prof::{History, InjectedSleep, PHASES, PhaseClock, PhaseTiming};
use cp_sim::{SimPhase, SimRunner};
use report::{MapInfo, NamedStat, Report, SCHEMA, SaveInfo, stat_of};

#[derive(Debug)]
struct Opts {
    map: Option<String>,
    save: Option<String>,
    scenario: Option<String>,
    /// Target population of `--scenario grid_city`.
    pop: usize,
    content: PathBuf,
    ticks: u64,
    warmup: u64,
    seed: u64,
    out: Option<PathBuf>,
    inject_sleep: Option<(SimPhase, Duration)>,
    /// Keep the periodic checksum (a ~30 ms hitch every 256 ticks at 100k): off by default, as
    /// in the game.
    periodic_checksum: bool,
}

fn usage() -> &'static str {
    "usage: cp_bench [--map DIR|NAME] [--save FILE] [--scenario empty|grid_city] [--pop N] [--content DIR] \
     [--ticks N] [--warmup N] [--seed S] [--out FILE.json] [--inject-sleep Phase=MS] [--periodic-checksum]\n       \
     cp_bench fixture --pop N [--seed S] [--map DIR|NAME] [--content DIR] --out FILE.sav\n       \
     cp_bench aggregate OUT.json RUN.json...\n       \
     cp_bench compare BASELINE.json CURRENT.json [--threshold 0.2] [--floor-ms 0.05]"
}

/// Milliseconds → `Duration`, rejecting anything `Duration` cannot hold (negative, NaN,
/// infinite, overflowing) instead of panicking.
fn parse_sleep_ms(text: &str) -> Result<Duration, String> {
    let ms: f64 = text
        .parse()
        .map_err(|e| format!("--inject-sleep: '{text}' is not a number ({e})"))?;
    if !ms.is_finite() || ms < 0.0 {
        return Err(format!(
            "--inject-sleep: '{text}' must be a finite, non-negative number of milliseconds"
        ));
    }
    Duration::try_from_secs_f64(ms / 1000.0)
        .map_err(|e| format!("--inject-sleep: '{text}' is out of range ({e})"))
}

fn parse_phase(name: &str) -> Result<SimPhase, String> {
    SimPhase::ORDER
        .into_iter()
        .find(|p| format!("{p:?}") == name)
        .ok_or_else(|| format!("unknown phase '{name}'"))
}

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        map: None,
        save: None,
        scenario: None,
        pop: 10_000,
        content: PathBuf::from("assets/content"),
        ticks: 2000,
        warmup: 100,
        seed: 1,
        out: None,
        inject_sleep: None,
        periodic_checksum: false,
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{flag} needs a value"));
        let num = |s: String| s.parse::<u64>().map_err(|e| format!("{flag}: {e}"));
        match flag.as_str() {
            "--map" => o.map = Some(val()?),
            "--save" => o.save = Some(val()?),
            "--scenario" => o.scenario = Some(val()?),
            "--pop" => o.pop = num(val()?)? as usize,
            "--periodic-checksum" => o.periodic_checksum = true,
            "--content" => o.content = val()?.into(),
            "--ticks" => o.ticks = num(val()?)?,
            "--warmup" => o.warmup = num(val()?)?,
            "--seed" => o.seed = num(val()?)?,
            "--out" => o.out = Some(val()?.into()),
            "--inject-sleep" => {
                let v = val()?;
                let (phase, ms) = v.split_once('=').ok_or("--inject-sleep wants Phase=MS")?;
                o.inject_sleep = Some((parse_phase(phase)?, parse_sleep_ms(ms)?));
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if o.save.is_some() && o.scenario.is_some() {
        return Err("--save and --scenario are mutually exclusive".into());
    }
    if o.ticks == 0 {
        return Err("--ticks must be > 0".into());
    }
    Ok(o)
}

fn git_sha() -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `--map NAME` means `assets/maps/NAME`; anything with a path separator is used as is.
fn map_dir(arg: &str) -> PathBuf {
    if arg.contains('/') || arg.contains('\\') {
        PathBuf::from(arg)
    } else {
        Path::new("assets/maps").join(arg)
    }
}

fn run(o: &Opts) -> Result<Report, String> {
    let scenario = match (&o.save, o.scenario.as_deref()) {
        (Some(_), _) => "save",
        (None, None | Some("empty")) => "empty",
        (None, Some("grid_city")) => "grid_city",
        (None, Some(other)) => {
            return Err(format!(
                "unknown scenario '{other}' (available: empty, grid_city)"
            ));
        }
    };
    // The map is attached to the sim exactly like `cp_app::build_sim` does (`set_terrain` opens
    // the start tiles in `TileOwnership`, which is part of the checksum), so a bench run starts
    // from the same state as `cp_desktop --map M --hour 0 --headless N`.
    let mut terrain = None;
    let map = o
        .map
        .as_deref()
        .map(|m| {
            let dir = map_dir(m);
            let t = Instant::now();
            let loaded = cp_terrain::Terrain::load(&dir)
                .map_err(|e| format!("--map {}: {e}", dir.display()))?;
            let load_ms = t.elapsed().as_secs_f64() * 1e3;
            terrain = Some(std::sync::Arc::new(loaded));
            Ok::<_, String>(MapInfo {
                path: dir.display().to_string(),
                load_ms,
            })
        })
        .transpose()?;

    let mut sim = if scenario == "grid_city" {
        // The scenario runs untimed; the timed ticks start from the finished city.
        let terrain = terrain.ok_or("--scenario grid_city needs --map")?;
        build_grid_city(o, terrain)?.0
    } else {
        let mut sim = SimRunner::new(o.seed);
        if let Some(terrain) = terrain {
            sim.set_terrain(terrain);
        }
        sim
    };
    sim.world_mut()
        .resource_mut::<cp_sim::SimChecksum>()
        .skip_periodic = !o.periodic_checksum;
    sim.add_plugin(&PhaseTiming {
        sleep: o
            .inject_sleep
            .map(|(phase, duration)| InjectedSleep { phase, duration }),
        history: History::Unbounded,
    });
    if o.save.is_some() {
        // Without the content the building, citizen and agent systems have nothing to run on.
        sim.set_content(load_content(&o.content)?);
    }
    let save = o
        .save
        .as_deref()
        .map(|path| load_save(&mut sim, path, &o.content))
        .transpose()?;
    let mut total_ms = Vec::with_capacity(o.ticks as usize);
    for i in 0..o.warmup + o.ticks {
        let t = Instant::now();
        sim.tick_once();
        let dt = t.elapsed().as_secs_f64() * 1e3;
        if i >= o.warmup {
            total_ms.push(dt);
        }
    }
    let checksum = sim.checksum();
    let clock = sim.world().resource::<PhaseClock>();
    let samples: Vec<_> = clock.samples.iter().skip(o.warmup as usize).collect();
    let phases = (0..PHASES)
        .map(|p| NamedStat {
            name: format!("{:?}", SimPhase::ORDER[p]),
            stat: stat_of(
                &samples
                    .iter()
                    .map(|row| row[p] as f64 / 1e6)
                    .collect::<Vec<_>>(),
            ),
        })
        .collect();
    Ok(Report {
        schema: SCHEMA,
        scenario: scenario.into(),
        seed: o.seed,
        ticks: o.ticks,
        warmup_ticks: o.warmup,
        runs: 1,
        git_sha: git_sha(),
        pool_threads: ComputeTaskPool::try_get().map(|p| p.thread_num() as u32),
        map,
        save,
        checksum: format!("{checksum:016x}"),
        arenas: Vec::new(),
        phases,
        total: stat_of(&total_ms),
    })
}

fn load_content(dir: &Path) -> Result<std::sync::Arc<cp_content::ContentDb>, String> {
    cp_content::ContentDb::load_dir(dir)
        .map(std::sync::Arc::new)
        .map_err(|e| {
            e.0.iter()
                .map(|err| format!("--content {}: {err}", dir.display()))
                .collect::<Vec<_>>()
                .join("\nerror: ")
        })
}

/// Runs `cp_sim::scenario::grid_city` for `o.pop` and `o.seed`.
fn build_grid_city(
    o: &Opts,
    terrain: std::sync::Arc<cp_terrain::Terrain>,
) -> Result<(SimRunner, cp_sim::scenario::GridCityReport), String> {
    let content = load_content(&o.content)?;
    let (sim, report) = cp_sim::scenario::grid_city(
        o.pop,
        o.seed,
        terrain,
        content,
        &cp_sim::scenario::GridCityConfig::default(),
    );
    if !report.reached || !report.rejected.is_empty() {
        return Err(format!("grid_city: {report:?}"));
    }
    Ok((sim, report))
}

/// `cp_bench fixture`: builds the grid city and writes it as a save.
fn make_fixture(args: &[String]) -> Result<(), String> {
    let mut o = parse_opts(&["--ticks".into(), "1".into()])?;
    let mut out: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--pop" => o.pop = val()?.parse().map_err(|e| format!("--pop: {e}"))?,
            "--seed" => o.seed = val()?.parse().map_err(|e| format!("--seed: {e}"))?,
            "--map" => o.map = Some(val()?),
            "--content" => o.content = val()?.into(),
            "--out" => out = Some(val()?.into()),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    let out = out.ok_or("fixture needs --out FILE.sav")?;
    let map = o.map.clone().unwrap_or_else(|| "flatlands".into());
    let dir = map_dir(&map);
    let terrain = std::sync::Arc::new(
        cp_terrain::Terrain::load(&dir).map_err(|e| format!("--map {}: {e}", dir.display()))?,
    );
    let (mut sim, report) = build_grid_city(&o, terrain)?;
    let content = load_content(&o.content)?;
    let map_id = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("map")
        .to_string();
    let bytes = cp_save::save_runner(&sim, content.content_hash(), &map_id)
        .map_err(|e| format!("save: {e}"))?;
    std::fs::write(&out, &bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    println!(
        "{}: {} bytes; grid_city pop {} (target {}), {} buildings, tick {}, {} streets, seed {}, checksum {:016x}",
        out.display(),
        bytes.len(),
        report.population,
        report.target_pop,
        report.buildings,
        report.ticks,
        report.streets,
        o.seed,
        sim.checksum()
    );
    Ok(())
}

/// `cp_bench netcmd --save FILE [--map M] [--content DIR] [--n N]`: time of one `BuildRoad`
/// (a new 60 m street piece at the edge of the city) and one `UpgradeSegment` (a lattice
/// street between its two road types) applied by the dispatcher's `cp_net::apply`, on the
/// loaded city. Prints the mean and the slowest of `N` commands each (default 200).
fn netcmd(args: &[String]) -> Result<(), String> {
    use cp_net::{ElevationMode, NetCommand, RoadPath};
    let (mut save, mut map, mut content_dir, mut n) =
        (None, "flatlands".to_string(), None, 200usize);
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--save" => save = Some(val()?),
            "--map" => map = val()?,
            "--content" => content_dir = Some(PathBuf::from(val()?)),
            "--n" => n = val()?.parse().map_err(|e| format!("--n: {e}"))?,
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    let save = save.ok_or("netcmd needs --save FILE")?;
    let content = load_content(&content_dir.unwrap_or_else(|| PathBuf::from("assets/content")))?;
    let dir = map_dir(&map);
    let terrain = std::sync::Arc::new(
        cp_terrain::Terrain::load(&dir).map_err(|e| format!("--map {}: {e}", dir.display()))?,
    );
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain);
    sim.set_content(std::sync::Arc::clone(&content));
    let bytes = std::fs::read(&save).map_err(|e| format!("--save {save}: {e}"))?;
    cp_save::load_into_runner(&mut sim, &bytes, content.content_hash())
        .map_err(|e| format!("--save {save}: {e}"))?;
    sim.tick_once();
    let basic = content.road_id("basic_2l").ok_or("basic_2l")?;
    let medium = content.road_id("medium_4l").ok_or("medium_4l")?;
    let segs: Vec<_> = sim
        .world()
        .resource::<cp_net::NetGraph>()
        .segments()
        .map(|(id, s)| (id, s.road))
        .collect();
    let mut time = |label: &str, cmds: Vec<NetCommand>| {
        let (mut total, mut worst, mut ok) = (Duration::ZERO, Duration::ZERO, 0usize);
        for c in &cmds {
            let t = Instant::now();
            let r = cp_net::apply(sim.world_mut(), c);
            let dt = t.elapsed();
            total += dt;
            worst = worst.max(dt);
            ok += usize::from(r.is_ok());
        }
        println!(
            "{label}: {} commands ({ok} ok): mean {:.3} ms, max {:.3} ms ({} segments in the net)",
            cmds.len(),
            total.as_secs_f64() * 1e3 / cmds.len().max(1) as f64,
            worst.as_secs_f64() * 1e3,
            segs.len()
        );
    };
    let build: Vec<NetCommand> = (0..n)
        .map(|i| {
            let z = 2800.0 + (i as f32) * 20.0;
            NetCommand::BuildRoad {
                road: basic,
                path: RoadPath::Straight {
                    from: bevy_math::Vec2::new(7700.0, z),
                    to: bevy_math::Vec2::new(7760.0, z),
                },
                elevation: ElevationMode::Ground,
            }
        })
        .collect();
    time("BuildRoad", build);
    let upgrade: Vec<NetCommand> = segs
        .iter()
        .step_by((segs.len() / n.max(1)).max(1))
        .take(n)
        .map(|&(seg, road)| NetCommand::UpgradeSegment {
            seg,
            road: if road == basic { medium } else { basic },
        })
        .collect();
    time("UpgradeSegment", upgrade);
    println!("checksum {:016x}", sim.checksum());
    Ok(())
}

/// Loads `path` into `sim` through `cp_save`'s public API. The current content hash comes
/// from the defs in `content_dir`, so a save made against other content yields a warning.
fn load_save(sim: &mut SimRunner, path: &str, content_dir: &Path) -> Result<SaveInfo, String> {
    let db = cp_content::ContentDb::load_dir(content_dir).map_err(|e| {
        // One `error:`-prefixed line per problem: "file: field: message".
        let dir = content_dir.display();
        e.0.iter()
            .map(|err| format!("--content {dir}: {err}"))
            .collect::<Vec<_>>()
            .join("\nerror: ")
    })?;
    let bytes = std::fs::read(path).map_err(|e| format!("--save {path}: {e}"))?;
    let t = Instant::now();
    let (header, warnings) = cp_save::load_into_runner(sim, &bytes, db.content_hash())
        .map_err(|e| format!("--save {path}: {e}"))?;
    Ok(SaveInfo {
        path: path.to_string(),
        format_version: header.format_version,
        tick: header.tick,
        map_id: header.map_id,
        load_ms: t.elapsed().as_secs_f64() * 1e3,
        warnings: warnings.iter().map(|w| format!("{w:?}")).collect(),
    })
}

fn read_report(path: &str) -> Result<Report, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

fn write_report(r: &Report, out: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(r).map_err(|e| e.to_string())?;
    std::fs::write(out, json + "\n").map_err(|e| format!("{}: {e}", out.display()))
}

fn print_table(r: &Report) {
    println!(
        "scenario={} seed={} ticks={} (+{} warmup) runs={} checksum={}",
        r.scenario, r.seed, r.ticks, r.warmup_ticks, r.runs, r.checksum
    );
    match r.pool_threads {
        Some(n) => println!("task pool threads={n} (bevy_ecs multi_threaded executor)"),
        None => println!("task pool threads=unknown (report predates T-057)"),
    }
    if let Some(m) = &r.map {
        println!("map {} loaded in {:.2} ms", m.path, m.load_ms);
    }
    if let Some(sv) = &r.save {
        println!(
            "save {} (v{}, tick {}, map {}) loaded in {:.2} ms",
            sv.path, sv.format_version, sv.tick, sv.map_id, sv.load_ms
        );
        for w in &sv.warnings {
            println!("warning: {w}");
        }
    }
    println!(
        "{:<10} {:>12} {:>12} {:>12} {:>12}",
        "phase", "median ms", "p95 ms", "mean ms", "max ms"
    );
    for p in &r.phases {
        println!(
            "{:<10} {:>12.5} {:>12.5} {:>12.5} {:>12.5}",
            p.name, p.stat.median_ms, p.stat.p95_ms, p.stat.mean_ms, p.stat.max_ms
        );
    }
    println!(
        "{:<10} {:>12.5} {:>12.5} {:>12.5} {:>12.5}",
        "total", r.total.median_ms, r.total.p95_ms, r.total.mean_ms, r.total.max_ms
    );
}

fn real_main(args: Vec<String>) -> Result<ExitCode, String> {
    match args.first().map(String::as_str) {
        Some("paths") => paths::run(&args[1..]),
        Some("fixture") => make_fixture(&args[1..]).map(|()| ExitCode::SUCCESS),
        Some("netcmd") => netcmd(&args[1..]).map(|()| ExitCode::SUCCESS),
        Some("aggregate") => {
            let [_, out, runs @ ..] = args.as_slice() else {
                return Err(usage().into());
            };
            let runs = runs
                .iter()
                .map(|p| read_report(p))
                .collect::<Result<Vec<_>, _>>()?;
            let agg = report::aggregate(&runs)?;
            write_report(&agg, Path::new(out))?;
            print_table(&agg);
            Ok(ExitCode::SUCCESS)
        }
        Some("compare") => {
            let [_, base, cur, rest @ ..] = args.as_slice() else {
                return Err(usage().into());
            };
            let (mut threshold, mut floor) = (0.2, 0.05);
            let mut it = rest.iter();
            while let Some(f) = it.next() {
                let v: f64 = it
                    .next()
                    .ok_or(format!("{f} needs a value"))?
                    .parse()
                    .map_err(|e| format!("{f}: {e}"))?;
                match f.as_str() {
                    "--threshold" => threshold = v,
                    "--floor-ms" => floor = v,
                    _ => return Err(format!("unknown argument '{f}'")),
                }
            }
            let (b, c) = (read_report(base)?, read_report(cur)?);
            let regressions = report::compare(&b, &c, threshold, floor)?;
            if b.pool_threads != c.pool_threads {
                eprintln!(
                    "note: task pool threads differ (baseline {:?}, current {:?}): the numbers are only comparable on the same machine",
                    b.pool_threads, c.pool_threads
                );
            }
            if regressions.is_empty() {
                println!(
                    "bench OK: no phase or total median above max(baseline, {floor} ms) x {:.2}",
                    1.0 + threshold
                );
                return Ok(ExitCode::SUCCESS);
            }
            for r in &regressions {
                eprintln!(
                    "REGRESSION {}: {:.5} ms > limit {:.5} ms (baseline {:.5} ms)",
                    r.name, r.current_ms, r.limit_ms, r.baseline_ms
                );
            }
            Ok(ExitCode::FAILURE)
        }
        _ => {
            if args.iter().any(|a| a == "-h" || a == "--help") {
                println!("{}", usage());
                return Ok(ExitCode::SUCCESS);
            }
            let opts = parse_opts(&args)?;
            let report = run(&opts)?;
            print_table(&report);
            if let Some(out) = &opts.out {
                write_report(&report, out)?;
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn main() -> ExitCode {
    match real_main(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(extra: &[&str]) -> Opts {
        let args: Vec<String> = ["--ticks", "50", "--warmup", "5"]
            .iter()
            .chain(extra)
            .map(|s| s.to_string())
            .collect();
        parse_opts(&args).unwrap()
    }

    #[test]
    fn empty_scenario_times_every_phase_and_is_deterministic() {
        let (a, b) = (run(&opts(&[])).unwrap(), run(&opts(&[])).unwrap());
        assert_eq!(a.phases.len(), SimPhase::ORDER.len());
        assert_eq!(a.checksum, b.checksum);
        assert!(a.total.median_ms > 0.0);
    }

    #[test]
    fn the_report_names_the_task_pool_threads() {
        let r = run(&opts(&[])).unwrap();
        let n = r
            .pool_threads
            .expect("the sim runner starts the compute pool");
        assert!(n >= 1);
        assert_eq!(
            Some(n as usize),
            ComputeTaskPool::try_get().map(|p| p.thread_num())
        );
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"pool_threads\""));
        // Reports written before T-057 have no such field and still load.
        let old = json.replace(&format!("\"pool_threads\":{n},"), "");
        assert_eq!(
            serde_json::from_str::<Report>(&old).unwrap().pool_threads,
            None
        );
    }

    #[test]
    fn injected_sleep_lands_in_the_chosen_phase_only() {
        let r = run(&opts(&["--inject-sleep", "Economy=2"])).unwrap();
        let ms = |n: &str| {
            r.phases
                .iter()
                .find(|p| p.name == n)
                .unwrap()
                .stat
                .median_ms
        };
        assert!(ms("Economy") >= 2.0, "Economy {}", ms("Economy"));
        assert!(ms("Citizens") < 1.0);
        assert!(r.total.median_ms >= 2.0);
    }

    fn repo(rel: &str) -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel)
            .to_str()
            .unwrap()
            .to_string()
    }

    /// The checksum of `fixtures/saves/bench-10k.sav` as loaded, recorded in
    /// `bench/baseline.json` ("fixtures") when the fixture was made.
    #[test]
    fn the_committed_10k_fixture_loads_to_the_recorded_state() {
        let content = load_content(Path::new(&repo("assets/content"))).unwrap();
        let terrain = std::sync::Arc::new(
            cp_terrain::Terrain::load(Path::new(&repo("assets/maps/flatlands"))).unwrap(),
        );
        let mut sim = SimRunner::new(1);
        sim.set_terrain(terrain);
        sim.set_content(std::sync::Arc::clone(&content));
        let bytes = std::fs::read(repo("fixtures/saves/bench-10k.sav")).unwrap();
        let (_, warnings) =
            cp_save::load_into_runner(&mut sim, &bytes, content.content_hash()).unwrap();
        // New content (T-078's roads) changes the content hash; that is the only warning allowed.
        let others: Vec<_> = warnings
            .iter()
            .filter(|w| !matches!(w, cp_save::LoadWarning::ContentHashMismatch { .. }))
            .collect();
        assert!(others.is_empty(), "{others:?}");
        let baseline: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(repo("bench/baseline.json")).unwrap())
                .unwrap();
        let recorded = baseline["fixtures"]["bench-10k"]["checksum"]
            .as_str()
            .unwrap();
        assert_eq!(format!("{:016x}", sim.checksum()), recorded);
    }

    /// Rebuilds the 10k city from its seed and compares it byte for byte with the committed
    /// fixture. Slow in debug.
    #[test]
    #[ignore = "grid_city 10k: run in release (cargo test -p cp_bench --release grid_city_10k -- --ignored)"]
    fn grid_city_10k_rebuilds_the_committed_fixture_byte_for_byte() {
        let terrain = std::sync::Arc::new(
            cp_terrain::Terrain::load(Path::new(&repo("assets/maps/flatlands"))).unwrap(),
        );
        let mut o = opts(&[]);
        o.content = repo("assets/content").into();
        o.pop = 10_000;
        o.seed = 1;
        let (sim, _) = build_grid_city(&o, terrain).unwrap();
        let content = load_content(&o.content).unwrap();
        let bytes = cp_save::save_runner(&sim, content.content_hash(), "flatlands").unwrap();
        assert!(bytes == std::fs::read(repo("fixtures/saves/bench-10k.sav")).unwrap());
    }

    #[test]
    fn save_loads_runs_ticks_and_reports_header() {
        let save = repo("fixtures/saves/v0-basic.sav");
        let content = repo("assets/content");
        let args = ["--save", save.as_str(), "--content", content.as_str()];
        let (a, b) = (run(&opts(&args)).unwrap(), run(&opts(&args)).unwrap());
        let info = a.save.as_ref().unwrap();
        assert_eq!(a.scenario, "save");
        // The header reports the file's own version; cp_save migrates it on load.
        assert_eq!(info.format_version, 0);
        // The fixture was written against other content, so the warning must surface.
        assert!(
            info.warnings
                .iter()
                .any(|w| w.contains("ContentHashMismatch"))
        );
        assert_eq!(a.checksum, b.checksum);
        assert_eq!(a.phases.len(), SimPhase::ORDER.len());
    }

    #[test]
    fn save_errors_are_reported() {
        let content = repo("assets/content");
        let c = content.as_str();
        assert!(run(&opts(&["--save", "/nonexistent.sav", "--content", c])).is_err());
        let not_a_save = repo("Cargo.toml");
        let e = run(&opts(&["--save", not_a_save.as_str(), "--content", c])).unwrap_err();
        assert!(e.contains("CPSV"), "{e}");
    }

    #[test]
    fn save_and_scenario_are_mutually_exclusive_and_unknown_scenario_fails() {
        let args: Vec<String> = ["--save", "x.sav", "--scenario", "empty"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(
            parse_opts(&args)
                .unwrap_err()
                .contains("mutually exclusive")
        );
        assert!(run(&opts(&["--scenario", "boom"])).is_err());
    }

    #[test]
    fn map_flag_loads_terrain() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands");
        let r = run(&opts(&["--map", root.to_str().unwrap()])).unwrap();
        assert!(r.map.unwrap().load_ms >= 0.0);
        assert!(run(&opts(&["--map", "/nonexistent/map"])).is_err());
    }

    #[test]
    fn bad_args_are_errors() {
        assert!(parse_opts(&["--bogus".into()]).is_err());
        assert!(parse_opts(&["--inject-sleep".into(), "Nope=1".into()]).is_err());
    }
}
