---
name: rebuild-release-after-merge
description: The user plays target/release/cp_desktop in the coordinator checkout; rebuild it after merging anything they will test
metadata:
  type: feedback
---

2026-10-20: the user tested T-278 (1-cell road step) right after I reported it merged and saw the old behaviour — target/release/cp_desktop was built 07:24, the merge landed 07:31.

**Why:** the user runs the coordinator's release binary; a merge alone doesn't update what they play.
**How to apply:** after merging a user-visible change, run `cargo build --release -p cp_desktop` in the coordinator checkout (CARGO_BUILD_JOBS=6) before saying "можно проверять", and mention the build time. Related: [[feedback-roads-whole-cells]]
