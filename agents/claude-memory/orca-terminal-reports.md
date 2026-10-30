---
name: orca-terminal-reports
description: Devs sometimes report only in their terminal, not via orchestration; poll terminals (orca terminal read --limit 400) when the waiter is quiet
metadata:
  type: feedback
---

On 2026-10-18 three devs (dev-1 plan, dev-2 BUG-074/076, dev-3 T-250/T-252) finished and reported only in their terminals; the orchestration waiter stayed silent and they waited 1-2 h.

**Why:** after /clear or compaction workers forget the orchestration reporting command.
**How to apply:** when the waiter is quiet >30 min or the user asks for status, read each dev terminal (`orca terminal read --terminal <h> --limit 400 --json`) and check `git branch --sort=-committerdate`; remind devs in every dispatch to report via orca orchestration. Related: [[orca-ask-timeout]]
