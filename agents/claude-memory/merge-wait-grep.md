---
name: merge-wait-grep
description: Staged-merge briefs must wait on the merge commit subject '^T-0xx:', not a bare grep; TL tasks commits mention task ids
metadata:
  type: feedback
---

On 2026-10-14 my tasks.jsonl commit subject listed "T-077/T-094/T-086", so devs waiting with `git log master --oneline | grep T-077` saw a false "landed" and started ff-merge gates in parallel.

**Why:** a bare grep matches task ids anywhere in any subject, including Tech Lead bookkeeping commits.
**How to apply:** in staged merge briefs use `git -C <coord> log master --format=%s -15 | grep -E '^T-0xx:'`. Related: [[cityplanner-delivery-setup]], [[rebase-drops-features]]

2026-10-16: two devs deadlocked ~30 min, each polling for the other's merge while no gate ran (one waited for a gate that had long finished). On every TICK: if master did not move and `pgrep -f '^bash scripts/gate.sh'` is empty while a dev is "waiting", tell one dev to merge now and the other to poll for that dev's `^T-0xx` subject. Also devs idle at "waiting for confirmation" after a merge — tell them to start the next queued task without asking.
