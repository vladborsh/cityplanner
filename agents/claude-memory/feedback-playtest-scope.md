---
name: feedback-playtest-scope
description: user rule 2026-10-21 — no playtests for visual/UI/render tasks; sim tasks one seed only if the brief demands; no balance tuning
metadata:
  node_type: memory
  type: feedback
  originSessionId: 2357bd05-f628-41e4-8d2f-707f61992181
  modified: 2026-10-21T18:13:06.350Z
---

User rule (relayed by the TL, 2026-10-21):
- Visual/UI/render tasks: NO playtest at all, only shots + unit tests.
- Sim features: unit/headless tests; a playtest only if the brief demands it, and then ONE seed, at the end.
- Rebalancing is postponed (BUG-114 parked): don't tune balance knobs unless the task is about balance.

**Why:** feature velocity; playtests cost CPU on the user's machine and time (see [[feedback-features-over-testing]], [[perf-cpu-contention]]).

**How to apply:** this replaces the older 3-seed A/B habit ([[playtest-seed-ab]]) unless a brief explicitly asks for more seeds.
