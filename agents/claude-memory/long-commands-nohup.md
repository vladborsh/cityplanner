---
name: long-commands-nohup
description: "In Orca sessions, foreground Bash builds/tests die (exit 144) when a mid-turn TL message arrives and background tasks are killed at 10 min; run long cargo work with nohup + a log sentinel and poll with until loops"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 24f82677-822f-47ec-8340-38b2b1c15a08
  modified: 2026-10-21T17:36:12.628Z
---

Long cargo commands (nextest, release builds, playtests) run as a foreground Bash call were killed with exit 144 whenever the TL sent a mid-turn message, and `run_in_background` tasks were killed at the 600 s limit (seen 2026-10-21, architect slot).

**Why:** the harness interrupts the running tool call to deliver the message; detached processes survive.

**How to apply:** `nohup sh -c '... > log 2>&1; echo DONE_EXIT=$? >> log' >/dev/null 2>&1 &`, then wait with `until grep -q DONE_EXIT log; do sleep 10; done` (foreground, re-run after a 600 s timeout). Never pipe cargo output when checking green; read the sentinel. Related: [[feedback-taskpolicy-background]] (prefix cargo with `taskpolicy -b`).

**Set the slot's env inside the nohup script (2026-10-23).** The Bash tool's shell does not carry `CARGO_TARGET_DIR`; a `nohup sh -c 'cargo …'` without it built into `<worktree>/target/` (15 GB, cold engine rebuild, 51 min gate). Write the script with `export CARGO_TARGET_DIR=<abs>/.cargo-target/<slot> CARGO_BUILD_JOBS=4 CITYPLANNER_DATA_DIR=.data/<slot>` at the top. Also never chain a gate after `git rebase` with `;` — a conflict leaves the tree mid-rebase and the gate runs on it; use `&&` or check status first.
