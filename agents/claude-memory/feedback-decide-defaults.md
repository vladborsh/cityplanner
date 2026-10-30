---
name: feedback-decide-defaults
description: "When the user is away, TL picks the most fitting default for decisions; hard design questions go to the Architect"
metadata:
  node_type: memory
  type: feedback
  originSessionId: f92be386-2f35-4788-b563-f85aca353413
  modified: 2026-10-16T22:12:58.279Z
---

User (2026-10-16, going to sleep): "if some tasks need my answer/decision, pick the most fitting default, or if it's very hard, hand it to the Architect".

**Why:** the user wants work to keep flowing overnight instead of blocking on them.
**How to apply:** don't park tasks waiting for the user; choose the conventional default and log the choice (task row / report to the user in the morning). Send genuinely hard design calls to a short-lived Opus Architect (max 3 agents total). Still never break AGENTS.md hard rules (secrets, determinism, saves, licences) on the user's behalf. Related: [[feedback-features-over-testing]]
