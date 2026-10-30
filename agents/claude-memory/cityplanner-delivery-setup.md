---
name: cityplanner-delivery-setup
description: "How the M0 delivery loop is wired (Orca slots, merge path, GitHub remote) as set up on 2026-10-09"
metadata:
  node_type: memory
  type: project
  originSessionId: f92be386-2f35-4788-b563-f85aca353413
  modified: 2026-10-09T19:28:41.995Z
---

Set up 2026-10-09 by the Tech Lead session:
- Rust installed via rustup into ~ (user's choice); `~/.zshenv` sources cargo env.
- GitHub remote: private `vladborsh/cityplanner`, default branch `master` (user chose "create via gh"). Tasks saying "main" mean master.
- Orca slots are worktrees under `~/orca/workspaces/cityplanner/<slot>` (dev-1, dev-2, devops, tech-artist; dev-3 later), agent claude --model sonnet.
- Merge path: master is checked out in the coordinator checkout, so after approval the developer rebases, runs green, then `git -C <coord checkout> merge --ff-only <branch>`; Tech Lead then pushes master. Tech Lead avoids committing to master while merges are in flight (see "Merge churn" below); ff-only merges work fine with an uncommitted, untouched-by-them tasks.jsonl.

**Why:** single-machine setup; ff-only into the coordinator checkout avoids "branch checked out elsewhere" refusals.
**How to apply:** reuse this path for every approval; see [[orca-dispatch-brief-files]] for dispatch hygiene.

Merge churn (observed 2026-10-09): each Tech Lead tasks.jsonl commit moves master and forces in-flight ff merges to rebase + re-run the full suite (minutes each); several merges in parallel can loop. While merges are in flight, batch backlog edits uncommitted and commit once after they land (ff-only merges are fine with a dirty, untouched tasks.jsonl).
Exception: a merge whose branch itself touches tasks.jsonl (Architect rulings, Tester bug rows) cannot fast-forward over an uncommitted tasks.jsonl — commit the backlog before dispatching such merges.
Since T-058 (2026-10-09): merges go through `scripts/ff-merge.sh <branch>` (runs scripts/gate.sh, refuses on a dirty coordinator tree or if master moves during the ~100 s gate). So: commit backlog edits immediately (dirty tree blocks), but avoid committing while a merge is running (any master move forces a rerun). Batch backlog commits between merges.
