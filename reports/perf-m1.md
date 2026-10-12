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
