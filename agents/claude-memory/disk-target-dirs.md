---
name: disk-target-dirs
description: "Per-slot cargo target dirs grow to 40-75 GB each and filled the 460 GB disk on 2026-10-11; clean closed slots' dirs"
metadata:
  node_type: memory
  type: project
  originSessionId: f92be386-2f35-4788-b563-f85aca353413
  modified: 2026-10-11T21:27:00.319Z
---

On 2026-10-11 the disk hit 4 GB free (dev-2's build failed with disk-full). Cause: per-slot `../.cargo-target/<slot>` dirs under ~/orca/workspaces/cityplanner/.cargo-target were 40-75 GB each (debug + release + wasm + gate). Deleting the dirs of closed slots (tech-artist, dev-3, devops, architect) freed ~125 GB.

**Why:** every slot keeps a full separate build cache; with 7 slots that is ~300 GB.
**How to apply:** check `df -h /Users` when starting a work session; delete target dirs only of slots whose agents are closed (use literal absolute paths in rm — the safety check blocks `rm -rf "$VAR/..."`). Machine also has 24 GB RAM: run at most 3 agents with CARGO_BUILD_JOBS=4. Related: [[cityplanner-delivery-setup]]

2026-10-12: with 3 active devs the disk fell to 10 GB free again within ~6 h (dev-1 102 GB, dev-2 79 GB, dev-3 45 GB). Check `df -h /Users` at every merge approval; in merge briefs have devs delete `$CARGO_TARGET_DIR/{debug,release}/incremental` after their merge has LANDED (not after a failed ff-merge attempt: that forces a full rebuild and the dev loses the master race again — seen 2026-10-12, dev-2 needed 3+ attempts).

2026-10-15: again 38 GB free after a day with 2 devs (dev-1 148 GB, dev-2 96 GB) — even 2 slots fill the disk in ~8 h. Rule given to devs: after each MERGE, if your own target dir > ~40 GB, rm -rf it (only yours). Delete idle slots' dirs (tester/balance/dev-3) as soon as their agents close.

2026-10-15 (TL rule): after each merge, if your slot target dir is above ~40 GB, rm -rf it (dev-1 was 148 GB). Freed disk: 291 GB free after.

2026-10-16: also look for stray `<worktree>/target` dirs (builds run without CARGO_TARGET_DIR, e.g. after a context clear): dev-1/target 36 GB + dev-2/target 33 GB, untouched for a day, deleted (+62 GB). Check `du -sh ~/orca/workspaces/cityplanner/*/target`.

2026-10-17: with 3 devs + the TL full-gate dir the disk hit 100 % (dev-1 92, dev-2 101, dev-3 82, full-gate 36 GB) and a dev build failed. Check `df -h /Users` and the slot dirs at least every hour; delete the full-gate target after each batch run; remind devs after every merge (they forget).

2026-10-19: disk hit 1.5 GB free with only 2 devs + idle architect (dev-2 126 GB of which incremental 46 GB, dev-3 76, architect 50+19 stale). The batch full gate's doctest step died with "No space left on device" after all nextest tests passed (looked like a red gate, was not). Before launching any full gate run `df -h /`; if < 40 GB free, delete idle agents' target dirs and the busy slot's debug/incremental first.


2026-10-23 MISTAKE: never delete files from a slot target by mtime (`find deps -mtime +1 -delete`). Cargo keeps the old mtime on dependency rlibs it does not rebuild. 1300 live rlibs vanished while their fingerprints stayed, and the architect's T-318a gate broke at link time. The only safe options: rm -rf the slot's whole target (or debug/) while the agent is idle between tasks, or ask the agent to do it after its merge.

2026-10-24: with 4 agents (dev-2/3/4 plus mine) the disk went from 52 to 5.9 GB free in ~2 h. dev-2 and dev-4 were at 86-87 GB each. Run df at EVERY poll. Below 30 GB, delete my target/debug and the debug dir of any slot with no cargo running (check with ps, path-scoped). Fix proposed to the user: debug = line-tables-only, one test binary per crate.
