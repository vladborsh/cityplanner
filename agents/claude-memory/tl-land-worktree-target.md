---
name: tl-land-worktree-target
description: Temp "land" worktrees must not share the coordinator target dir — cp_app's build.rs embeds content by absolute path and goes stale
metadata:
  type: feedback
---

2026-10-20: the TL full gate failed with 4 cp_app scene tests panicking at `road_id("basic_2l")` — not a code bug. I had run quick gates in temp worktrees (.data/coord/tl/land) with CARGO_TARGET_DIR=<coord>/target; cp_app's build.rs embeds assets/content via include_str! of absolute paths, so the cached build-script output pointed at the deleted worktree. `touch crates/cp_app/build.rs` fixed it.

**Why:** shared target + build scripts with absolute paths = stale embedded content.
**How to apply:** for temp land worktrees use a separate target (`../.cargo-target/tl-land`) or touch crates/cp_app/build.rs before the coordinator's full gate. Related: [[feedback-batch-full-gate]]
