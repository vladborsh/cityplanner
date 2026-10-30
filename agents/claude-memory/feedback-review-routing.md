---
name: feedback-review-routing
description: User rule 2026-10-21 — Reviewer TL reviews only at milestone close (commit range), not per task; TL merges tasks on quick gate + own check
metadata:
  type: feedback
---

User (2026-10-21): first "мелкие визуальные правки вливать без проверяющего", then "добавь проверку лида только на майлстоуны, не каждую задачу". Also: restart agents fresh when their context is huge (~500k makes steps slow).

**Why:** the per-task review round-trip added ~1 h per task. It did catch real bugs (BUG-109 alpha fight, T-300a resume divergence, BUG-111 pick order), so per-task checks by me must stay careful.
**How to apply:** I merge every task myself after the quick gate and reading the dev's report, tests and shots (for sim/save changes read the diff of the risky parts myself: serialized fields, per-tick readers of derived state, determinism). At a milestone close send the Reviewer TL (term_b564c135, worktree .../tester) the commit range since the last milestone for one broad review. Restart an agent fresh (resume brief in its .data/<slot>/briefs) after EVERY big task when its context passes ~250k (user asked why the Architect burned 550k on T-302a: the session had carried 8 tasks). Related: [[feedback-architect-role]], [[orca-agent-handles]], [[feedback-batch-full-gate]]

2026-10-23, user: "приостанавливай, когда он выжирает столько памяти" (the architect had reached 474k context). Watch every agent's Ctx in the status line at each poll. At ~300k, have it commit its WIP and restart it fresh with a resume brief, even mid-task. Don't wait for the task to finish.
