//! Result JSON, aggregation over runs and the regression gate.

use serde::{Deserialize, Serialize};

pub const SCHEMA: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Stat {
    pub median_ms: f64,
    pub p95_ms: f64,
    /// Mean over the sampled ticks: what a frame budget really sees, spikes included (a phase
    /// that runs every 64th tick costs nothing at the median). Absent in older results.
    #[serde(default)]
    pub mean_ms: f64,
    /// Slowest sampled tick. Absent in older results.
    #[serde(default)]
    pub max_ms: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NamedStat {
    pub name: String,
    #[serde(flatten)]
    pub stat: Stat,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MapInfo {
    pub path: String,
    pub load_ms: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SaveInfo {
    pub path: String,
    pub format_version: u32,
    pub tick: u64,
    pub map_id: String,
    pub load_ms: f64,
    /// Non-fatal load findings from `cp_save` (e.g. content-hash mismatch), as text.
    pub warnings: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Report {
    pub schema: u32,
    pub scenario: String,
    pub seed: u64,
    pub ticks: u64,
    pub warmup_ticks: u64,
    /// Runs this report aggregates (1 for a single run).
    pub runs: u32,
    pub git_sha: Option<String>,
    /// Threads of the global compute task pool that `par_iter` systems and the multi-threaded
    /// schedule executor run on (T-057). Absent in reports older than that.
    #[serde(default)]
    pub pool_threads: Option<u32>,
    pub map: Option<MapInfo>,
    /// Set when the run started from `--save`.
    #[serde(default)]
    pub save: Option<SaveInfo>,
    /// Final state checksum, hex. Identical across runs of the same inputs.
    pub checksum: String,
    /// Arena name → used slots. Empty until the sim has arenas.
    pub arenas: Vec<(String, u64)>,
    pub phases: Vec<NamedStat>,
    /// Wall clock around one whole `tick_once` (phases + checksum + bookkeeping).
    pub total: Stat,
}

/// Nearest-rank percentile of an unsorted sample, `q` in 0..=1.
pub fn percentile(samples: &[f64], q: f64) -> f64 {
    assert!(!samples.is_empty());
    let mut v = samples.to_vec();
    v.sort_by(f64::total_cmp);
    let rank = (q * v.len() as f64).ceil().max(1.0) as usize;
    v[rank - 1]
}

pub fn median(samples: &[f64]) -> f64 {
    let mut v = samples.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

pub fn stat_of(samples_ms: &[f64]) -> Stat {
    Stat {
        median_ms: median(samples_ms),
        p95_ms: percentile(samples_ms, 0.95),
        mean_ms: samples_ms.iter().sum::<f64>() / samples_ms.len() as f64,
        max_ms: samples_ms.iter().copied().fold(0.0, f64::max),
    }
}

/// Merges several runs of the same configuration: per phase, the median of the runs'
/// medians (and of their p95s). Errors if the runs disagree on inputs or checksum.
pub fn aggregate(runs: &[Report]) -> Result<Report, String> {
    let first = runs.first().ok_or("no runs to aggregate")?;
    for r in runs {
        if (&r.scenario, r.seed, r.ticks, &r.checksum)
            != (&first.scenario, first.seed, first.ticks, &first.checksum)
        {
            return Err("runs disagree on scenario/seed/ticks/checksum (non-determinism?)".into());
        }
        if r.phases
            .iter()
            .map(|p| &p.name)
            .ne(first.phases.iter().map(|p| &p.name))
        {
            return Err("runs disagree on phase list".into());
        }
    }
    let merge = |pick: &dyn Fn(&Report) -> &Stat| Stat {
        median_ms: median(&runs.iter().map(|r| pick(r).median_ms).collect::<Vec<_>>()),
        p95_ms: median(&runs.iter().map(|r| pick(r).p95_ms).collect::<Vec<_>>()),
        mean_ms: median(&runs.iter().map(|r| pick(r).mean_ms).collect::<Vec<_>>()),
        max_ms: median(&runs.iter().map(|r| pick(r).max_ms).collect::<Vec<_>>()),
    };
    let phases = (0..first.phases.len())
        .map(|i| NamedStat {
            name: first.phases[i].name.clone(),
            stat: merge(&|r| &r.phases[i].stat),
        })
        .collect();
    let mut out = first.clone();
    out.runs = runs.len() as u32;
    out.phases = phases;
    out.total = merge(&|r| &r.total);
    Ok(out)
}

#[derive(Debug, PartialEq)]
pub struct Regression {
    pub name: String,
    pub baseline_ms: f64,
    pub current_ms: f64,
    pub limit_ms: f64,
}

/// Gate rule: a phase (or the total) regresses if its **median** exceeds
/// `max(baseline, floor_ms) × (1 + threshold)`. The floor keeps sub-noise phases (an empty
/// phase costs microseconds) from tripping on ratios of tiny numbers, while still catching
/// a phase that jumps above `floor × (1 + threshold)`. p95 is reported but not gated.
pub fn compare(
    baseline: &Report,
    current: &Report,
    threshold: f64,
    floor_ms: f64,
) -> Result<Vec<Regression>, String> {
    if (&baseline.scenario, baseline.seed, baseline.ticks)
        != (&current.scenario, current.seed, current.ticks)
    {
        return Err(
            "baseline and current differ in scenario/seed/ticks; regenerate the baseline".into(),
        );
    }
    let mut rows: Vec<(&str, f64, f64)> =
        vec![("total", baseline.total.median_ms, current.total.median_ms)];
    for p in &current.phases {
        let b = baseline
            .phases
            .iter()
            .find(|b| b.name == p.name)
            .ok_or_else(|| format!("phase {} missing from baseline", p.name))?;
        rows.push((&p.name, b.stat.median_ms, p.stat.median_ms));
    }
    Ok(rows
        .into_iter()
        .filter_map(|(name, b, c)| {
            let limit = b.max(floor_ms) * (1.0 + threshold);
            (c > limit).then(|| Regression {
                name: name.to_string(),
                baseline_ms: b,
                current_ms: c,
                limit_ms: limit,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(total: f64, phase: f64) -> Report {
        let s = |m| Stat {
            median_ms: m,
            p95_ms: m * 2.0,
            mean_ms: m,
            max_ms: m * 3.0,
        };
        Report {
            schema: SCHEMA,
            scenario: "empty".into(),
            seed: 1,
            ticks: 10,
            warmup_ticks: 0,
            runs: 1,
            git_sha: None,
            pool_threads: None,
            map: None,
            save: None,
            checksum: "00".into(),
            arenas: vec![],
            phases: vec![NamedStat {
                name: "Agents".into(),
                stat: s(phase),
            }],
            total: s(total),
        }
    }

    #[test]
    fn percentiles() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 0.95), 95.0);
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), 2.5);
    }

    #[test]
    fn small_numbers_under_floor_do_not_trip() {
        // 10x slower, but still below floor*(1+t) = 0.06 ms.
        assert!(
            compare(&report(0.004, 0.001), &report(0.04, 0.01), 0.2, 0.05)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn real_regressions_trip() {
        let r = compare(&report(1.0, 0.5), &report(1.5, 0.5), 0.2, 0.05).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "total");
        // A sleep in a previously empty phase exceeds the floor limit.
        let r = compare(&report(0.004, 0.001), &report(1.1, 1.0), 0.2, 0.05).unwrap();
        assert_eq!(r.len(), 2);
        // Within 20 % passes.
        assert!(
            compare(&report(1.0, 0.5), &report(1.19, 0.59), 0.2, 0.05)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn aggregate_takes_median_of_runs_and_rejects_checksum_mismatch() {
        let runs = [report(3.0, 1.0), report(1.0, 1.0), report(2.0, 1.0)];
        assert_eq!(aggregate(&runs).unwrap().total.median_ms, 2.0);
        let mut bad = runs.to_vec();
        bad[1].checksum = "ff".into();
        assert!(aggregate(&bad).is_err());
    }

    #[test]
    fn mismatched_inputs_are_an_error() {
        let mut b = report(1.0, 1.0);
        b.ticks = 99;
        assert!(compare(&b, &report(1.0, 1.0), 0.2, 0.05).is_err());
    }
}
