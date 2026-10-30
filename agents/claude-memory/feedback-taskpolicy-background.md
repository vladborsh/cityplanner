---
name: feedback-taskpolicy-background
description: "default is NORMAL priority (TL 2026-10-24); use taskpolicy -b / CP_LOW_PRIORITY=1 only when the TL says the user is playing"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 689ebb10-ab3d-4491-80af-5f63ac946b7d
  modified: 2026-10-20T20:59:09.735Z
---

**Default (TL, 2026-10-24): normal priority.** Use background QoS only when the TL says the user is playing; then: `export CP_LOW_PRIORITY=1` for `scripts/gate.sh`, `scripts/quick-gate.sh` and `scripts/ff-merge.sh` (they re-exec under `taskpolicy -b`; merged in T-281, master 1316b5ee). Prefix every other heavy command with `taskpolicy -b`: cargo build/test/nextest/clippy, cp_bench, and cp_desktop runs (benches and screenshot shots too).

**Toggle:** the TL switches this on and off. On 2026-10-20 at 12:15 the TL said the user is not playing: use normal priority (no CP_LOW_PRIORITY, no taskpolicy -b; CARGO_BUILD_JOBS=4 stays). A bench A/B is the exception: keep it under taskpolicy -b while other agents are building. Background QoS returns when the TL says the user plays again; on such a switch, stop a normal-priority gate and restart it with CP_LOW_PRIORITY=1. While the user plays: no benches until the TL says so. **FULL PAUSE** (2026-10-20, the game lagged even at low priority): start no builds, tests or game runs at all; only read and edit code. The TL SIGSTOPs running processes and later SIGCONTs them; never kill or restart frozen commands. The pause ends when the TL says so; frozen processes are resumed at normal priority.

**Why:** the user plays the game on the same Mac while agents work; our builds made the game stutter (TL, 2026-10-20). Slower builds are accepted.

**How to apply:** `CP_LOW_PRIORITY=1 scripts/quick-gate.sh master`, `taskpolicy -b cargo ...`, `taskpolicy -b ../.cargo-target/<slot>/release/cp_desktop --hidden ...` (TL standing order, 2026-10-20). Benches under background QoS are slower and noisier, so compare only A/B runs taken under the same policy, alternating. During a TL "quiet window" start nothing heavy at all. Background and measurements: [[perf-cpu-contention]]. Related: [[feedback-batch-full-gate]], [[disk-target-dirs]].
