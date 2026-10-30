---
name: feedback-cp-desktop-hidden
description: every agent cp_desktop run (shots/tests) must pass --hidden (plus --ui for panels) under an outer perl alarm timeout; no visible windows
metadata:
  type: feedback
---

Never launch cp_desktop without `--hidden`; the user saw game windows pop up from dev-2's shot runs (2026-10-22). Use `--hidden --ui` for UI shots and wrap with `perl -e 'alarm 120; exec @ARGV' --` (macOS has no `timeout`). See apps/desktop/README.md "Agents and scripts".

**Why:** visible windows interrupt the user playing on the same Mac.
**How to apply:** check any shot command/script for `--hidden` before running.
