---
name: feedback-scoped-pkill
description: "never pkill by script/tool name (quick-gate, cargo nextest) — it kills other slots' and the TL's gates; kill by own PID or own target-dir path"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 2357bd05-f628-41e4-8d2f-707f61992181
  modified: 2026-10-21T19:05:20.699Z
---

On 2026-10-21 dev-3 ran `pkill -f 'quick-gate.sh'` and killed the TL's quick gate in the coordinator checkout (exit 144).

**Why:** all slots and the TL run the same scripts and cargo commands on one machine; a name pattern matches everyone's.

**How to apply:** stop only your own work: prefer the PID you started (`$!`, or the nohup job), else a pattern containing your own path, e.g. `pkill -f 'cargo-target/dev-3'` / your worktree path. Never `pkill -f quick-gate`, `gate.sh`, `cargo nextest`, `cargo`. Killing cargo mid-build can also leave a broken rlib (undefined `_anon...llvm` symbols at link); fix with `cargo clean -p <crate>` (see [[long-commands-nohup]]).
