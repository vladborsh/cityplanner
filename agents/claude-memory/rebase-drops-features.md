---
name: rebase-drops-features
description: "Workers squashing with `git reset --soft master` after master moved silently delete other merged features; require a deletion check in every rebase/merge brief"
metadata:
  node_type: memory
  type: feedback
  originSessionId: f92be386-2f35-4788-b563-f85aca353413
  modified: 2026-10-09T22:51:49.962Z
---

On 2026-10-09 dev-1 squashed T-027 with `git reset --soft master` while its tree was still based on an older master, so the commit deleted the just-merged T-043 overlays (~800 lines) and reverted Tech Lead rows in tasks.jsonl. Caught in review via `git diff --stat`.

**Why:** reset --soft onto a newer master records the old tree, which reverts everything merged in between.
**How to apply:** in every merge/rebase brief, require `git diff master...HEAD --diff-filter=D --name-only` (empty unless intended) and no tasks.jsonl change, and check it myself in review (look for unexpected deletions in the stat). Merges also run one at a time when memory is tight (24 GB machine, CARGO_BUILD_JOBS=4). Related: [[cityplanner-delivery-setup]]
