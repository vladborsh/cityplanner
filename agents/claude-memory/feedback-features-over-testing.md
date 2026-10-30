---
name: feedback-features-over-testing
description: "User wants feature velocity — less manual testing, fewer autotests (gate only at merge), little balancing; build M2 services"
metadata:
  node_type: memory
  type: feedback
  originSessionId: f92be386-2f35-4788-b563-f85aca353413
  modified: 2026-10-15T19:51:56.243Z
---

User (2026-10-15): "less manual testing, fewer autotests, only at merge to master, less balancing on every sneeze, focus on features — right now we only have road and building simulation, an empty city; we still need schools, garbage, police, fire, hospitals…".

**Why:** a day of polish/acceptance work left the game feature-poor; the user values visible gameplay over verification depth.

**How to apply:** one full gate per task, at merge only; specs ask for a few core headless tests, no exhaustive matrices, no rasters/benchmarks unless perf is the point; no balance-analyst runs per feature (batch balancing later); no Tester acceptance rounds unless the user asks; keep sim determinism/save rules (AGENTS.md) — those are not optional. Prioritise E-002 service features. Related: [[ui-icons-square-uniform]]
