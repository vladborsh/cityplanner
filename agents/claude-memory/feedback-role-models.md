---
name: feedback-role-models
description: "Agent models come from agents/roles/<role>.md (devs = Sonnet), not from memory notes; read the role file before every worker launch"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 29d59d9f-ce58-4d12-aa0f-dd2bbf61b0f9
  modified: 2026-10-29T10:58:55.918Z
---

On 2026-10-29 I launched dev-4 on T-335 with `--model opus` because [[orca-agent-handles]] said "dev-4 (Opus)". The user asked: «ты папку с ролями агентов смотрел? там четко модели прописаны». `agents/roles/*.md` has a `## Model` section in every role: software-engineer, tester, devops, tech-artist, integration and balance are Sonnet; tech-lead and researcher are Opus; architect is Fable. I restarted the worker on `--model sonnet`.

**Why:** the role files are the user's source of truth. Model notes in memory are old exceptions and go stale.
**How to apply:** before any `worker-start` or `terminal create` for an agent, read `agents/roles/<role>.md` `## Model` and pass the matching `--model`. If a memory note disagrees with the role file, the role file wins; ask the user before deviating.
