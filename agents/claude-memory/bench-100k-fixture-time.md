---
name: bench-100k-fixture-time
description: "cp_bench fixture --pop 100000 takes ~1.5-2 h wall at background QoS on a loaded Mac (README says 80 s); start it first, in the background"
metadata:
  node_type: memory
  type: project
  originSessionId: 511ed037-16b8-409a-bcbd-f58d37f6ca4f
  modified: 2026-10-22T03:01:15.741Z
---

Generating `fixtures/saves/bench-100k.sav` (`cp_bench fixture --pop 100000`) took 2 h 08 min (base)
and 1 h 50 min (after) on 2026-10-21 with `taskpolicy -b` while other agents ran gates (load 15-24).
bench/README.md still says "about 80 s", which is long out of date. The 10k fixture takes ~5 min.

**Why:** a bench-100k before/after (asked for in perf-sensitive tasks like T-291/T-305) is gated by
this, not by the build.

**How to apply:** kick off the base fixture generation (base binary + `git archive` content, see
[[bench-ab-content]]) as the very first background job of the task, before writing code; generate
the after fixture as soon as the release binary builds; run both benches interleaved at the end.

2026-10-23 measurement: the release cp_bench fixture --pop 100000 took ~9.5 min (10:38 to 10:47), running alongside a full gate on a moderately loaded Mac. Use "~10 min" as the estimate. The 2 h figure was a pathological case: background QoS plus load ~40.
