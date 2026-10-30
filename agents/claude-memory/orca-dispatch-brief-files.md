---
name: orca-dispatch-brief-files
description: "Long Orca task specs can arrive truncated to worker agents; put full briefs in files in the slot's gitignored .data dir"
metadata:
  node_type: memory
  type: feedback
  originSessionId: f92be386-2f35-4788-b563-f85aca353413
  modified: 2026-10-09T15:50:43.794Z
---

When dispatching via `orca orchestration task-create` + `worker-start`, a ~3.5 KB spec reached the devops worker "cut off mid-sentence" (2026-10-09, T-002); it missed the Tech Lead notes (push to origin, guard location) and under-delivered.

**Why:** the injected preamble + spec is pasted into the agent TUI and long text can be truncated; the stored spec in Orca was intact.

**How to apply:** write the full brief to `<worktree>/.data/<slot>/briefs/<task>.md` (gitignored, inside the worker's own worktree) and keep the Orca spec short: a pointer to that file plus a 2–3 line summary. Related: [[cityplanner-delivery-setup]]
