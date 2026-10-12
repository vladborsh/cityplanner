# Bench

`apps/bench` (`cp_bench`) runs the headless sim for N ticks and prints/writes ms per tick
per `SimPhase` (median and p95), a wall-clock `total` per tick, the final state checksum,
arena sizes (none yet), seed, tick count and git sha. See `cargo run -p cp_bench -- --help`.

```
cargo run --release -p cp_bench -- --scenario empty --ticks 20000 --out run.json
cp_bench aggregate result.json run-1.json ... run-5.json   # median over runs
cp_bench compare bench/baseline.json result.json           # exit 1 on regression
```

`--save FILE` loads a save through `cp_save`'s public API (content hash taken from
`--content DIR`, default `assets/content`), reports the header and any load warnings
under `save` in the JSON, then runs the ticks. It is mutually exclusive with `--scenario`.

`--map M` loads the terrain **and attaches it to the sim** the way the client does
(`cp_app::build_sim`: `set_terrain`, which opens the start tiles in `TileOwnership`, part of the
checksum). So these print the same checksum, as long as the client starts at tick 0:

```
cp_bench   --map flatlands --ticks 512 --warmup 0 --seed 1
cp_desktop --map flatlands --hour 0 --headless 512
```

Note `cp_desktop` defaults to `--hour 10` (it starts at tick 1706, so `--headless 512` ends at
tick 2218 with another checksum); pass `--hour 0` to compare with the bench, which starts at
tick 0. Covered by `apps/bench/tests/map_checksum_e2e.rs`.

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

## City benches: `grid_city` 10k and 100k (T-047)

`cp_sim::scenario::grid_city(target_pop, seed, terrain, content, &GridCityConfig)` builds a
street lattice (every fifth street a 4-lane avenue), power lines and water/sewage pipes along
every street, wind turbines at the edge, paints every block (dense and tall in the middle,
low and industrial outside) and runs the sim, with demand held high, until `target_pop` citizens
live there; then the clock runs on to 07:00 so a bench starts in the morning rush. Everything
goes through commands; the starting balance is the scenario's documented `funds` setting.
Same `(target_pop, seed, map, content)` gives the same city (checksum and save bytes).
It needs a map without water in the way: use `flatlands` (the pump and outflow are only placed
where the owned tiles have water).

```
cp_bench fixture --pop 10000  --seed 1 --map flatlands --out fixtures/saves/bench-10k.sav
cp_bench --scenario grid_city --pop 10000 --map flatlands --ticks 600      # build + time in one go
scripts/bench.sh 10k            # 5 runs on the fixture, aggregate, compare with bench/baseline-10k.json
scripts/bench.sh 100k           # the nightly one; RUNS=3 scripts/bench.sh 100k for a quicker look
scripts/bench.sh 10k --update-baseline   # record a new baseline (deliberately, on a quiet machine)
```

* `fixtures/saves/bench-10k.sav` (0.99 MB) is committed, written at the current save version:
  whoever bumps the save must keep it loading through the migration chain.
* `fixtures/saves/bench-100k.sav` (9.4 MB) is **gitignored**: `scripts/bench.sh 100k` builds it
  on demand (about 80 s in release) and refuses it unless its state checksum equals the one in
  `bench/baseline.json` under `"fixtures"` (as the 10k fixture's checksum is checked by a
  `cp_bench` test).
* GitHub CI is unavailable, so "10k on every PR, 100k nightly" is this script run by hand (or by
  a local cron): it writes `<target dir>/bench/bench-<size>.json` (ms per phase, median and p95)
  and exits 1 on a regression over 20 %.
* `bench/baseline-10k.json` and `bench/baseline-100k.json` hold the measured numbers (release,
  Apple M4 Pro, **shared machine at load average ~8**: noisy; re-baseline on a quiet one). The
  empty-scenario gate keeps using `bench/baseline.json`.
* A bench run attaches the content to a loaded save (without it the citizen, building and agent
  systems do nothing); `--save` runs of the old fixtures change accordingly.
* Today the commuters of the routine are abstract trips, so the `Agents` phase of these cities is
  nearly empty (no cars or pedestrians on the road).
* The periodic 256-tick state checksum (a ~30 ms hitch at 100k) is off in bench runs, as in the
  game; `--periodic-checksum` turns it back on. The result's `checksum` is computed on demand
  at the end and is not affected. Results now carry `mean_ms` and `max_ms` per phase next to the
  median and p95: the mean and the slowest tick are what a frame budget sees.
