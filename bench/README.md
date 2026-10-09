# Bench

`apps/bench` (`cp_bench`) runs the headless sim for N ticks and prints/writes ms per tick
per `SimPhase` (median and p95), a wall-clock `total` per tick, the final state checksum,
arena sizes (none yet), seed, tick count and git sha. See `cargo run -p cp_bench -- --help`.

```
cargo run --release -p cp_bench -- --scenario empty --ticks 20000 --out run.json
cp_bench aggregate result.json run-1.json ... run-5.json   # median over runs
cp_bench compare bench/baseline.json result.json           # exit 1 on regression
```

Phase times come from marker systems that `apps/bench` schedules between the phase sets
(sim crates never read the clock). Overhead is 11 `Instant::now()` calls plus 11 tiny
systems per tick, equal in baseline and current, so it cancels in the comparison. `total`
is measured around the whole `tick_once` and also includes the checksum and bookkeeping.

## CI gate (job `bench`)

Release build, 5 runs, per-phase **median of the 5 run medians**, compared with
`bench/baseline.json`. The job fails if any phase's (or the total's) median exceeds
`max(baseline, 0.05 ms) x 1.20`.

- The **0.05 ms floor** stops ratios of microsecond-sized numbers (every phase is empty at
  M0) from tripping on noise; a phase that grows past about 0.06 ms still fails.
- p95 is reported, not gated. Runs must agree on the checksum, otherwise the job errors.
- CI runners are not the reference hardware: only the relative change against a baseline
  taken on the same runner type is meaningful.

## Updating the baseline

The baseline must come from the CI runner, not a laptop. Run the `bench` job (any PR),
download its `bench-result` artifact, commit `result.json` as `bench/baseline.json`.
Do this deliberately when the sim legitimately gets slower or a new scenario is added;
`compare` errors if the baseline's scenario/seed/ticks differ from the current run.
