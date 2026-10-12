# Performance M1: 100k citizens (T-048)

Scenario: `fixtures/saves/bench-100k.sav` (grid_city, seed 1, 106k citizens, 37k buildings,
tick 29866 = 07:00, real traffic after BUG-022), 200 timed ticks after 50 warm-up, release
build, `cp_bench`. **Machine: Apple M4 Pro, shared with other agents (load average 6-11), not the
target hardware (M1 / GTX 1660)**: absolute numbers are noisy and, on a quiet M1, may differ.
The balance-analyst re-run required by the task is not possible here and is still open.
Tracy was not used (no new dependency in a sim crate); `sample(1)` on `cp_bench` found the
hot spots, `cp_prof` phase times measured them.

## Result (ms per tick)

"Before" is the median of 3 runs on master `a6cb066` (the old report had medians and p95 only);
"after" is the median of 3 runs of this branch, with the mean and the slowest tick, which the
old report lacked and which matter for a frame budget (a phase that runs every 64th tick is
invisible in a median).

| phase | before median | before p95 | after median | after p95 | after mean | after max |
|---|---|---|---|---|---|---|
| Commands | 0.001 | 0.001 | 0.001 | 0.001 | 0.001 | 0.00 |
| Network | 0.001 | 0.002 | 0.001 | 0.002 | 0.001 | 0.01 |
| Agents | 1.629 | 3.271 | 2.120 | 4.275 | 2.184 | 7.69 |
| Paths | 0.035 | 0.089 | 0.031 | 0.113 | 0.046 | 1.01 |
| **Citizens** | **49.951** | 71.166 | **0.266** | 1.085 | 0.384 | 1.99 |
| Buildings | 0.250 | 0.553 | 0.244 | 0.863 | 0.341 | 2.07 |
| Utilities | 0.001 | 0.001 | 0.001 | 0.002 | 0.389 | 30.85 |
| Economy | 0.137 | 0.162 | 0.035 | 0.064 | 0.087 | 9.64 |
| Services | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.00 |
| Stats | 0.244 | 0.281 | 0.168 | 0.363 | 0.194 | 1.12 |
| **total** | **52.265** | 79.253 | **3.148** | 6.838 | **3.819** | 40.31 |

Utilities before: not visible in a median, but the tick that runs `update_supply` (every 64th)
took about **3 s** (max 3019 ms, mean 24 ms/tick) on the pre-optimisation code (measured in an
intermediate run after the Citizens fix only). The Agents "before/after" difference is
noise (the code is unchanged; the runs sat at different machine loads: 1.4-2.1 ms).

Other runs of the final code on a quieter moment: total mean 2.6 ms/tick, Citizens 0.27 ms,
Agents 1.37 ms.

## Budget (§14, per frame at ×3 = three ticks)

| item | budget per frame | actual (3 × mean tick) | |
|---|---|---|---|
| sim total | ≤ 8 ms | 7.8-11.5 ms (mean tick 2.6-3.8 ms) | at the limit; over on a loaded machine |
| agents | ≤ 3 ms | 4.1-6.6 ms (1.4-2.2 ms/tick) | **over** |
| citizens + buildings + economy + utilities | ≤ 1.5 ms | 2.4 ms (0.38 + 0.34 + 0.09 + 0.39 per tick, ×3 = 3.6 loaded) | **over** (mostly the 64-tick utilities hitch) |
| frame ≤ 16.6 ms, render | not measured | | needs the client on target hardware |

So: the 50 ms Citizens phase is gone (a 190× drop); the sim total went from 52 ms to about
3-4 ms per tick (14×), roughly at the §14 sim budget at ×3 on this loaded machine, not
convincingly under it. The frame time (render) was not measured.

## What was done (checksum of the 100k run after 250 ticks: `b8938f66733aadef` before and after
every step, i.e. bit-identical)

1. **Per-building tables (cp_citizen)**: homes and filled job slots were `BTreeMap<BuildingId, _>`;
   now dense `Vec`s indexed by arena index with a generation check (a building that reuses a slot
   starts empty, as with the full-id key). This alone took Citizens from 50 ms to 2.2 ms (a
   BTreeMap lookup per candidate workplace per jobless citizen).
2. **Job-search index (cp_citizen)**: `JobIndex` bins the workplaces in a 128 m grid with free
   slots per cell and level (exact: kept up to date as the tick assigns jobs); the nearest free
   site is found by ring search instead of scanning every workplace for every seeker. Same
   choice as the scan (nearest, ties to the lower building id): proven by a randomised test
   against the brute-force scan (4000 queries while slots fill up) and the unchanged checksum.
   Citizens 2.2 -> 0.27 ms.
3. **Utility supply (cp_utility)**: `compute_supply` ran `nearest_xz` (200 curve evaluations)
   for every building against every link in 9 buckets, twice (pipes, lines). Links are now
   skipped by the bounding box of their control points (exact: a Bézier curve lies inside it),
   and `update_supply` keeps an `AttachCache` (building -> pipe/line component, valid for the
   graph fingerprint and the position), so after the first run only new buildings are searched.
   The BFS `layer.contains` was O(n²) and was redundant (visited flag). The hitch went
   from ~3000 ms to ~25 ms every 64th tick.
4. **Bench**: results now carry the mean and the slowest tick per phase (old results still load).
5. Zone overlay cache: already keyed by `ZoneGrid::revision` (T-074 / earlier); nothing to do.

## Remaining hot spots, ranked

1. **Agents 1.4-2.2 ms/tick** (real traffic at 07:00: cars and pedestrians): over the 1 ms/tick
   share of the §14 budget. `move_agents` (pedestrians, vehicles in `drive_tick`) dominates.
2. **Utilities hitch ~25 ms every 64 ticks** (> a frame): `compute_supply` still rebuilds the
   site list, the `missing` map and does BTreeMap lookups for all 37k buildings each time; could
   be made incremental (only changed buildings) or spread over the 64 ticks.
3. **Economy ~10 ms every 256 ticks** and **Buildings 0.25 ms/tick** (the per-building `get`
   and the 1/64 bucket scan).
4. `SimChecksum` (every 256 ticks, ~7 % of samples in a profile).
5. Job search still rebuilds the site list per tick when someone looks for work (O(buildings),
   ~0.1 ms); a cached index keyed on building changes would remove it.
6. `NetGraph` clone in `apply` is T-063 (not touched).

## Reproduce

```
scripts/bench.sh 100k     # builds fixtures/saves/bench-100k.sav on demand
```
`bench/baseline-100k.json` and `bench/baseline-10k.json` hold the new numbers.


---

# Round 2 (T-076)

Same scenario and caveats as above (shared, loaded M4 Pro, load average 10-12 during the final
runs; not target hardware; balance-analyst confirmation still open). 100k checksum after 250
ticks is `b8938f66733aadef` after **every** step of this round too (bit-identical); none of
the steps below changes behaviour.

## Before / after per phase (ms per tick, median of 3 runs each)

"Before" = master `ca1a5a0` (end of T-048) re-measured at the start of this round, on a busier
moment than the final run (Agents 2.6 vs 1.8 is machine noise: that code is unchanged).

| phase | before median | before p95 | before mean | before max | after median | after p95 | after mean | after max |
|---|---|---|---|---|---|---|---|---|
| Commands | 0.001 | 0.002 | 0.001 | 0.00 | 0.001 | 0.001 | 0.001 | 0.00 |
| Network | 0.001 | 0.003 | 0.001 | 0.02 | 0.001 | 0.001 | 0.001 | 0.00 |
| Agents | 2.612 | 8.322 | 3.639 | 51.83 | 1.837 | 3.035 | 1.784 | 4.66 |
| Paths | 0.037 | 0.280 | 0.098 | 1.56 | 0.032 | 0.119 | 0.063 | 1.25 |
| Citizens | 0.280 | 1.244 | 0.493 | 6.70 | 0.263 | 0.809 | 0.359 | 1.61 |
| Buildings | 0.249 | 1.017 | 0.404 | 4.36 | 0.228 | 0.604 | 0.278 | 1.18 |
| Utilities | 0.001 | 0.002 | 0.355 | 24.42 | 0.000 | 0.001 | 0.056 | 3.75 |
| Economy | 0.036 | 0.089 | 0.102 | 9.60 | 0.031 | 0.036 | 0.062 | 6.10 |
| Services | 0.000 | 0.000 | 0.000 | 0.00 | 0.000 | 0.000 | 0.000 | 0.00 |
| Stats | 0.176 | 0.599 | 0.247 | 2.76 | 0.147 | 0.215 | 0.154 | 0.52 |
| **total** | 3.773 | 11.947 | 5.560 | 54.63 | 2.660 | 4.258 | 2.761 | 12.16 |

Quieter single runs of the final code: total mean 2.37 ms, **max tick 11.1 ms**, Agents mean
1.55 ms, Utilities max 3.8 ms, Economy max 6.1 ms. The slowest tick is now under the 16 ms
target (before: 54 ms, mostly the `update_supply` and checksum ticks landing together every 256th tick).

## Budget (§14, per frame at ×3)

| item | budget | after (3 × mean tick) | |
|---|---|---|---|
| sim total | ≤ 8 ms | 7.1 ms (quiet) - 8.3 ms (loaded) | at the limit |
| agents | ≤ 3 ms | 4.7-5.4 ms (1.55-1.8 ms/tick) | **over**: pedestrian and vehicle stepping |
| citizens + buildings + economy + utilities | ≤ 1.5 ms | 2.3 ms (0.36 + 0.28 + 0.06 + 0.06 per tick) | **over**: Citizens and Buildings |
| render frame (100k overview, `--hidden --bench`) | ≤ 16.6 ms | avg 7.65 ms (131 fps), p50 2.9, p95 16.8 | within; but p99 107 ms / max 261 ms |

Render frame, first measurement: `cp_desktop --map flatlands --scene save:fixtures/saves/bench-100k.sav
--cam overview --hidden --bench 10` (new `save:` scene: loads a save into the client; 37k buildings,
4.4k cars and 3.3k pedestrians in the sim). The avg/p50/p95 are within 16.6 ms; the slow tail is the
mesh rebuild at start (`roads last_update_ms=83`, `buildings last_update_ms=7.5`) and sim ticks that
share a frame, not steady state. Agents were not drawn at that camera distance (`vehicles_drawn=0`).
The machine runs with other loads and the GPU is an M4 Pro: on a GTX 1660 / M1 the GPU part is unknown.

## What was done

1. **`update_supply` hitch 25 ms -> 3.8 ms every 64 ticks** (bit-identical): per-def needs/source
   computed once (was per building: BTreeMap iteration and a service-name lookup), the attach
   cache and the missing-supply bits are dense `Vec`s by arena index, the cache keeps the
   `Attach` lookups between runs, and the power BFS neighbour lookup is a dense CSR grid
   (`HopGrid`) instead of a `BTreeMap` of 9 lookups per building (6 ms -> <1 ms).
   (The first run after a load still pays the one-off ~265 ms attach build; it happens once.)
2. **Economy 9.6 -> 6.1 ms every 256 ticks** (bit-identical): `worker_education` was a
   `BTreeMap` built over 106k citizens; now dense by building index.
3. **Periodic checksum off in the client and the bench** (`SimChecksum::skip_periodic`,
   `cp_bench --periodic-checksum` to turn it back on): the checksum serialises and FNV-hashes
   the whole state (~30 ms at 100k, every 256th tick: the 37 ms max tick of T-048). It does not
   touch the sim state; `SimRunner::checksum()` and the on-demand value are unchanged, tests
   and replays keep the periodic one (default on). FNV-1a is byte-serial, so a faster
   identical hash is not possible; a parallel per-part hash would change every checksum.
4. **Shop search** (`nearest_shop`): the active shops are listed once per tick (was a scan of
   every building for each shopper); same pick (nearest, ties to the lower id).
5. **Measurement**: `cp_desktop --scene save:FILE` loads a save into the client (new
   `cp_save` dependency of `cp_app`; pure-Rust zstd, no browser issue), used for the render frame.

## Remaining, ranked

1. **Agents 1.55-1.8 ms/tick** (budget share 1 ms): pedestrians re-resolve their current leg
   (`Ctx::leg`: lane lookups, a search over the junction's links for crossings) every tick, and
   `position()` evaluates the lane/curve each tick; caching the resolved leg per pedestrian
   (it only changes on leg change or network change) would cut most of it. Not done: it touches
   the `Pedestrian` struct (serialized) - needs a save-format decision (dev-1 / T-046).
2. **Citizens 0.36 ms + Buildings 0.28 ms per tick** (budget share 0.5 + 0.5): per-citizen
   `decide` for 1/256 of 106k citizens each tick (400 decisions) and the 1/64 building bucket;
   both are linear in the bucket and now mostly allocation and arena-walk bound.
3. **Economy 6 ms every 256th tick** (`LandGrids::compute` over all links/segments/buildings;
   could be spread over the 256 ticks only with a behaviour change: the grids are saved and
   checksummed).
4. **Periodic checksum 30 ms** when it is on (tests/replays); a parallel per-part hash would
   fix it at the price of changing every checksum.
5. Render tail (p99 107 ms): road/building mesh rebuild at load.

---

# T-063: BuildRoad on a 100k city

`cp_bench netcmd --save fixtures/saves/bench-100k.sav` times the dispatcher's `cp_net::apply` for
200 `BuildRoad` (a new 60 m street piece at the city edge) and 200 `UpgradeSegment`. Release,
shared machine; the 10k/100k save checksums after the commands are `68b421e23c01dbc8` /
`3e2e19d0bd5d1df7` before and after (bit-identical).

| step | 10k (897 segments) | 100k (9176 segments) |
|---|---|---|
| before (master) | 4.5 ms mean, 32 ms max | **15.5 ms** mean, 26 ms max |
| + bounding-box prefilter in snapping | 0.131 ms | 1.14 ms |
| + no graph clone (`check_build` dry run) | 0.04-0.05 ms | **0.30-0.53 ms** (one run had a 21 ms outlier) |
| `UpgradeSegment` | 0.002 ms | 0.005-0.011 ms (never cloned) |

The profile showed the graph copy was *not* the cost (0.5 % of the time): 96 % went to
`CubicBezier3::nearest_xz` (200 curve evaluations) run against **every** segment by the snap
(`nearest_segment`) and the guide search. Both now skip a segment whose control-point box is
farther than the radius (exact: a Bézier curve lies inside it). The copy was then 30-40 % of what
was left, and is gone: `apply` decides everything before the first change
(`NetGraph::check_build` replays splits and segment additions on a virtual overlay, same rules and
order, error for error), and keeps a copy of the graph only under `debug_assertions` to restore and
fail loudly if a checked plan ever fails to apply. The remaining ~0.3 ms is the O(nodes) `find_node`
scans and `intersect_xz` against all segments; a spatial index would get it below 0.1 ms.
