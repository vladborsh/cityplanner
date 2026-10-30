---
name: feedback-batch-full-gate
description: User approved (2026-10-17) quick gate per merge (affected crates) and full gate on master in batches run by the TL
metadata:
  node_type: memory
  type: feedback
  originSessionId: f92be386-2f35-4788-b563-f85aca353413
  modified: 2026-10-17T17:23:21.154Z
---

User (2026-10-17): "don't rerun all tests and checks on every merge, it's too long; run them after a batch of tasks". Agreed design (T-239): ff-merge runs a quick gate (fmt + clippy/nextest on changed crates and their reverse deps); the TL runs the full gate on master (scripts/full-gate-master.sh, own target dir) after ~4-5 merges / every ~2 h / before closing a milestone; red full gate = merges stop until fixed.

**Why:** each full gate took 20-30 min and parallel gates raced ("master moved" reruns), stalling devs for hours.
**How to apply:** after T-239 lands, keep a merge counter and launch the full gate in the background on schedule; never skip it before a milestone close; keep the determinism test in the full gate. Related: [[feedback-features-over-testing]], [[feedback-separate-green-steps]]
