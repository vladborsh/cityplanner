---
name: orca-ask-timeout
description: "Worker `ask` questions time out after ~15 min; answers that need a user decision arrive too late and the worker proceeds with its own default"
metadata:
  node_type: memory
  type: feedback
  originSessionId: f92be386-2f35-4788-b563-f85aca353413
  modified: 2026-10-09T19:00:46.006Z
---

On 2026-10-09 tech-artist asked (via `orca orchestration ask`) how to handle a bevy_egui WebGL2 blocker; I escalated to the user, and by the time I replied the worker's ask had timed out, so it shipped its fallback (option B) instead of the user's choice (A). devops had the same 15-min timeout on another question.

**Why:** worker asks have a ~15 min timeout; a reply after that isn't consumed unless the worker resumes the ask.
**How to apply:** when a worker question needs a user decision, immediately send a short holding note (terminal send: "decision pending with the user — don't proceed with a default, pick other work") before asking the user; then deliver the decision as a new dispatch, not only as a `reply`. Related: [[orca-dispatch-brief-files]]
