---
name: sim-state-read-between-passes
description: "A field a per-tick sim system reads, but a periodic pass builds, must be saved state; a serde(skip) cache makes a loaded game diverge"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 689ebb10-ab3d-4491-80af-5f63ac946b7d
  modified: 2026-10-21T14:08:38.616Z
---

If a periodic pass (e.g. `update_city` every 256 ticks) builds a field that another system reads every tick (e.g. health reading noise), that field must be saved state and must be in the checksum. A `serde(skip)` field comes back as zeros after a load, so the per-tick RNG rolls differ from the continuous run.

Rebuilding the field on restore is not exact either: its inputs change between the pass and the save (e.g. highway `load.ema`, buildings that finish).

**Why:** the Reviewer rejected T-300a (2026-10-21) because `Emissions.noise` was `serde(skip)`. The fix was to save it as u16 hundredths, sparse. T-300b's per-home water dirt follows the same rule.

**How to apply:** for every new derived grid or map, check who reads it and when. Prove it with a test that saves mid-cycle, then compares the field right after the load and the checksum 256 ticks later, with the relevant knob boosted (`crates/cp_save/tests/emissions_resume.rs`). Related: [[playtest-seed-ab]].
