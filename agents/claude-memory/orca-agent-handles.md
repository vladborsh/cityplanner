---
name: orca-agent-handles
description: Current Orca terminal handles of the cityplanner agents (change after every reboot) and how to relaunch them
metadata:
  type: reference
---

After the 2026-10-21 reboot (handles die with Orca's PTYs):
- Architect (Opus for code since 2026-10-22; switch to Fable with /model fable ONLY for specs/hard root-cause, user rule): term_ab707c76-7389-473a-9df3-2f775b7e560e, worktree .../architect, resume .data/architect/briefs/RESUME-1021c.md
- dev-3 (Opus, fresh session 2026-10-21 ~08:10): term_64d24fb1-332b-4107-9abc-8eeb3310e68c, worktree .../dev-3-2, resume .data/dev-3/briefs/RESUME-dev3.md
- Reviewer TL (Opus, 2026-10-21, user: 'проверки разработчиков отдай тех лиду'): term_b564c135-01fe-417c-b323-c3a0db4c8afb, worktree .../tester (branch reviewer), role .data/reviewer/briefs/ROLE.md. Reviews only at milestone close (user rule 2026-10-21).
- dev-2 (Sonnet, fresh 2026-10-23): term_294565c5-d9c4-42d0-892c-77362beee930, worktree .../dev-2, resume .data/dev-2/briefs/RESUME-dev2c.md
- dev-4 (UI developer; model per agents/roles/software-engineer.md = Sonnet, Opus was a mistake 2026-10-29; created 2026-10-23 on user request "исправления интерфейса отдай новому разработчику"): term_e8c9198b-b0eb-4b7e-bb5b-0e7384dc894f, worktree .../dev-4 (orca worktree create --name dev-4), target ../.cargo-target/dev-4 (already in sccache basedirs), data .data/dev-4, start brief .data/dev-4/briefs/START-dev4.md. Owns all UI fixes (cp_ui/cp_app).
- Tech Lead (coordinator, 2026-10-23): term_46dc2c9d-1694-49ee-8f18-c4458360d016 (seen as sender in the run inbox). Architect reports go `--to run:run_f4c1553db19c`; `--type report` is rejected as 'Invalid input', send without --type.

Relaunch: `orca terminal create --worktree path:<worktree> --title X --command "claude --dangerously-skip-permissions --model opus --continue"`. `--continue` can pick a stale old session (dev-2 resumed an Oct 11 one) — check the terminal and restart fresh with a resume brief if so. Tell each agent its new handle. Related: [[project-state-2026-10-21-reboot]]
