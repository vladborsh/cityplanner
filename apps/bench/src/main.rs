//! `cp_bench`: headless benchmark harness (`architecture/system-design.md` §14).
//!
//! ```text
//! cp_bench [--map DIR|NAME] [--save FILE] [--scenario empty] [--content DIR] [--ticks N] [--warmup N]
//!          [--seed S] [--out FILE.json] [--inject-sleep Phase=MS]
//! cp_bench aggregate OUT.json RUN.json...        median over runs
//! cp_bench compare BASELINE.json CURRENT.json [--threshold 0.2] [--floor-ms 0.05]
//! ```
//!
//! This is an app, not a sim crate, so it may read the wall clock. `--inject-sleep` exists
//! only to prove the CI gate fails on a slow phase.

mod report;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use cp_prof::{History, InjectedSleep, PHASES, PhaseClock, PhaseTiming};
use cp_sim::{SimPhase, SimRunner};
use report::{MapInfo, NamedStat, Report, SCHEMA, SaveInfo, stat_of};

#[derive(Debug)]
struct Opts {
    map: Option<String>,
    save: Option<String>,
    scenario: Option<String>,
    content: PathBuf,
    ticks: u64,
    warmup: u64,
    seed: u64,
    out: Option<PathBuf>,
    inject_sleep: Option<(SimPhase, Duration)>,
}

fn usage() -> &'static str {
    "usage: cp_bench [--map DIR|NAME] [--save FILE] [--scenario empty] [--content DIR] [--ticks N] [--warmup N] \
     [--seed S] [--out FILE.json] [--inject-sleep Phase=MS]\n       \
     cp_bench aggregate OUT.json RUN.json...\n       \
     cp_bench compare BASELINE.json CURRENT.json [--threshold 0.2] [--floor-ms 0.05]"
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
        content: PathBuf::from("assets/content"),
        ticks: 2000,
        warmup: 100,
        seed: 1,
        out: None,
        inject_sleep: None,
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{flag} needs a value"));
        let num = |s: String| s.parse::<u64>().map_err(|e| format!("{flag}: {e}"));
        match flag.as_str() {
            "--map" => o.map = Some(val()?),
            "--save" => o.save = Some(val()?),
            "--scenario" => o.scenario = Some(val()?),
            "--content" => o.content = val()?.into(),
            "--ticks" => o.ticks = num(val()?)?,
            "--warmup" => o.warmup = num(val()?)?,
            "--seed" => o.seed = num(val()?)?,
            "--out" => o.out = Some(val()?.into()),
            "--inject-sleep" => {
                let v = val()?;
                let (phase, ms) = v.split_once('=').ok_or("--inject-sleep wants Phase=MS")?;
                let ms: f64 = ms.parse().map_err(|e| format!("--inject-sleep: {e}"))?;
                o.inject_sleep = Some((parse_phase(phase)?, Duration::from_secs_f64(ms / 1000.0)));
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
        (None, Some(other)) => {
            return Err(format!("unknown scenario '{other}' (available: empty)"));
        }
    };
    let map = o
        .map
        .as_deref()
        .map(|m| {
            let dir = map_dir(m);
            let t = Instant::now();
            cp_terrain::Terrain::load(&dir).map_err(|e| format!("--map {}: {e}", dir.display()))?;
            Ok::<_, String>(MapInfo {
                path: dir.display().to_string(),
                load_ms: t.elapsed().as_secs_f64() * 1e3,
            })
        })
        .transpose()?;

    let mut sim = SimRunner::new(o.seed);
    sim.add_plugin(&PhaseTiming {
        sleep: o
            .inject_sleep
            .map(|(phase, duration)| InjectedSleep { phase, duration }),
        history: History::Unbounded,
    });
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
        map,
        save,
        checksum: format!("{checksum:016x}"),
        arenas: Vec::new(),
        phases,
        total: stat_of(&total_ms),
    })
}

/// Loads `path` into `sim` through `cp_save`'s public API. The current content hash comes
/// from the defs in `content_dir`, so a save made against other content yields a warning.
fn load_save(sim: &mut SimRunner, path: &str, content_dir: &Path) -> Result<SaveInfo, String> {
    let db = cp_content::ContentDb::load_dir(content_dir)
        .map_err(|e| format!("--content {}: {e:?}", content_dir.display()))?;
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
    println!("{:<10} {:>12} {:>12}", "phase", "median ms", "p95 ms");
    for p in &r.phases {
        println!(
            "{:<10} {:>12.5} {:>12.5}",
            p.name, p.stat.median_ms, p.stat.p95_ms
        );
    }
    println!(
        "{:<10} {:>12.5} {:>12.5}",
        "total", r.total.median_ms, r.total.p95_ms
    );
}

fn real_main(args: Vec<String>) -> Result<ExitCode, String> {
    match args.first().map(String::as_str) {
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
