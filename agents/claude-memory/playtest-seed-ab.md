---
name: playtest-seed-ab
description: "Judge traffic/behaviour changes by the playtest on 3+ seeds (PLAYTEST_SEED), not one run: BUG-102 won on seed 1 and lost on seeds 2-3"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 689ebb10-ab3d-4491-80af-5f63ac946b7d
  modified: 2026-10-21T05:26:30.222Z
---

To judge a sim behaviour change by `cargo test --release -p cp_app --test playtest -- --ignored --nocapture`, run master against the variant(s) on 3 or more seeds, each with `PLAYTEST_SEED=n` (added 2026-10-20; default 1). Build one test binary per variant with `cargo test --no-run` and copy it, then run them in parallel; the results don't depend on thread count. A constant set to a no-op value (for example `f32::INFINITY`) makes the "master" variant from the same branch.

**Why:** the playtest is one deterministic but chaotic run. The BUG-102 yield-gap rule beat master on seed 1 (309 → 207 stuck) and lost badly on seeds 2 and 3 (267 → 516–630), so the change was dropped (TL, 2026-10-20). The TL's acceptance rule: lowest total stuck, no gridlock burst that master lacks, and better than master on every seed.

**How to apply:** for any drive, priority or spawn change, report a seed × variant table (stuck total, worst day, full-beyond total and worst day, reasons). Related: [[feedback-taskpolicy-background]], [[bench-ab-content]].
