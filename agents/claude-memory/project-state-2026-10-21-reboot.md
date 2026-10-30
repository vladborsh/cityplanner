---
name: project-state-2026-10-21-reboot
description: Where work stood when the user rebooted on 2026-10-21 ~03:30 — what to resume first
metadata:
  type: project
---

User rebooted the Mac (system update) 2026-10-21 ~03:30. Master pushed at 915fcec0 (tasks.jsonl has the backlog).

Resume in this order:
1. Branch `tl-batch-1021` (pushed) = master + T-298, BUG-108 (23b8480e), T-299, BUG-104 cherry-picked; its FULL GATE was never finished (first run died on ENOSPC, the rerun was stopped for the reboot). Run `scripts/gate.sh` on it (touch crates/cp_app/build.rs first; my target/debug was deleted, so it builds cold), then ff master to it.
2. BUG-108 ghost-colour follow-up 6724f65b on bug-108 (pushed) — cherry-pick after the batch.
3. In progress, pushed, gates unfinished: dev-3 bug-105 2a41f0d1 (on bug-104), dev-2 t-296 326ab18a. Then dev-3: T-297, T-287b, BUG-106; dev-2: BUG-107. Architect: design/review mode (see [[feedback-architect-role]]).
4. Agents were told to WIP-commit and push their branches before the reboot; check their replies / branches.
5. Disk fills fast (3 times on 10-20/21): check `df -h /` after every merge. Waiter: orca orchestration check --run run_f4c1553db19c --wait.
