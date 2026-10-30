---
name: perf-cpu-contention
description: "Game stutter during playtests came from agents' builds on the same Mac; CP_LOW_PRIORITY=1 and game QoS fix it; benchmark only when load is low"
metadata:
  node_type: memory
  type: project
  originSessionId: 678ae7a9-384b-4ee0-b50b-155cee9e5e5b
  modified: 2026-10-20T15:37:58.604Z
---

T-281 (2026-10-20): the user's "lags when the camera moves" at ~700 residents was CPU contention from agent cargo builds/tests (load avg 30-90) on the same M4 Pro, not game code. On a quiet machine the game ran at 120 Hz, p95 9.6 ms. 16 busy processes took it to 76 fps; at background QoS (`taskpolicy -b`) the same load left it at 117.

**Why:** all agents and the user share one machine; frame-time benches under load are noise. In a chrome trace under load, trivial systems show ~10 ms OS-preemption quanta.

**How to apply:** while the user plays, agents run the scripts with `CP_LOW_PRIORITY=1`. Measure perf only when `uptime` load is < ~8. The hidden `--bench` has a ~6.4 ms frame floor, so use a real window with `--vsync` for pacing. Related: [[feedback-taskpolicy-background]], [[disk-target-dirs]].
