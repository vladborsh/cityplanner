---
name: feedback-architect-role
description: User rule 2026-10-23: the Architect writes NO code, only specs for hard tasks (plus root-cause analysis when asked); all implementation goes to the devs
metadata:
  type: feedback
---

User (2026-10-23): "отдай код писать кому то другому; он только пишет спеку там где это требуется в сложных задачах". This replaces the 2026-10-21 rule, which let the Architect keep hard code (roads, traffic core).

**Why:** after his six specs were done I handed him T-318a code because he was idle. The user saw that as the Architect being slow on "a spec". Idle Architect time is fine.
**How to apply:** never assign the Architect implementation, even when he is idle and the devs are busy. Queue the code for dev-2/dev-3 instead. Architect work = specs for complex features (not for simple ones: devs work from the task text), root-cause analysis on request, design answers to devs. Related: [[feedback-review-routing]]
