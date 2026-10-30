---
name: feedback-separate-green-steps
description: Run each green check as its own command and check its exit status before any ff-merge into master
metadata:
  type: feedback
---

Never chain the green checks and the `git merge --ff-only` into one shell command; run each check separately and look at its exit status first.

**Why:** On T-037 a chained command ff-merged to master even though `cargo test --workspace` had failed to compile (apps/bench `map_checksum_e2e.rs`, a pre-existing master-red from T-023 changing `cp_app::build_sim`). The Tech Lead then declared master red and froze merges.

**How to apply:** In the cityplanner merge flow, one command per step (fmt, clippy, test, check-sim-deps, check-isolated-clippy, wasm build), confirm each passed (no `error`, test count shown), only then the ff-merge. Also confirm the test total is non-empty: an empty `total` means the run failed.
