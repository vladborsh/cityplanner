---
name: feedback-no-checkout-during-gate
description: Never `git checkout` in the coordinator checkout while a gate runs there — it silently tests other code; happened twice on 2026-10-21
metadata:
  type: feedback
---

On 2026-10-21 I switched the coordinator checkout from tl-land to master (to commit tasks.jsonl) while scripts/quick-gate.sh was running there, twice. The gate then compiled/tested a mix of trees; I had to kill and rerun it (~30 min lost each time).

**Why:** cargo reads the working tree live.
**How to apply:** while a gate runs on tl-land, do NOT checkout; edit tasks.jsonl in place and commit it on tl-land (it merges with the batch), or wait until the gate ends. Check `pgrep -f quick-gate.sh` / `gate.sh` before any checkout. Related: [[tl-land-worktree-target]]
