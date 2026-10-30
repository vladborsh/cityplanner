---
name: feedback-waiter-always-on
description: The TL must always have a background orchestration waiter running; on 2026-10-23 none ran and three READY reports sat unseen for ~8 h
metadata:
  type: feedback
---

On 2026-10-23 I landed BUG-121 at 14:20 and started only a release-build waiter, with no orchestration waiter. dev-3 (T-324), dev-2 (T-313) and dev-4 (T-317f) reported READY at ~15:00. I saw the reports only at 23:11, when the user asked "ну что там?", and all the agents sat idle all evening.

**Why:** I am re-invoked only by background-task notifications or by the user. With no waiter running, nothing wakes me.
**How to apply:** every turn that ends with agents working must leave `orca orchestration check --run run_f4c1553db19c --wait --timeout-ms 1500000 --json > .data/coord/tl/wait.out` running in the background. A build or gate waiter is not enough. When that waiter times out, poll the terminals and restart it. Related: [[orca-terminal-reports]], [[feedback-review-routing]]
