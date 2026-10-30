---
name: feedback-desktop-hidden
description: "every cp_desktop run by an agent (shots, demos, tests) must pass --hidden; visible windows pop up on the user's screen"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 2357bd05-f628-41e4-8d2f-707f61992181
  modified: 2026-10-22T08:04:35.684Z
---

On 2026-10-22 the TL relayed that the user saw game windows popping up; dev-3's T-303b shots had run `cp_desktop --screenshot ...` without `--hidden`.

**Why:** the user plays and works on the same Mac; a visible window steals focus and screen (see [[perf-cpu-contention]], [[feedback-taskpolicy-background]]).

**How to apply:** always `cp_desktop --hidden ...` (renders off-screen, 1600x900) for shots, demos and any scripted run; never launch it windowed unless the user asks to play. Note `--hidden` with `--ui` keeps the UI in the shot.
