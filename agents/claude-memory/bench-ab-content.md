---
name: bench-ab-content
description: "cp_bench A/B: the baseline binary reads ./assets/content, so give it its base commit's content; build the base with a detached checkout, not `git checkout master`"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 689ebb10-ab3d-4491-80af-5f63ac946b7d
  modified: 2026-10-20T19:07:09.160Z
---

For a cp_bench A/B between a branch and its base:
- build the base with `git checkout --detach <sha>`, because `git checkout master` fails in a worktree (master is checked out in the coordinator checkout);
- give the base binary its own content with `git archive <sha> assets/content | tar -x -C <dir>` and `--content <dir>/assets/content`. Otherwise new balance fields make it exit with a parse error;
- delete old `--out` JSON before each run and never send stderr to /dev/null, so a failed run cannot reprint stale numbers;
- check that the two checksums differ when the change is meant to change the sim.

**Why:** on 2026-10-20 T-280's A/B printed stale numbers twice and cost the TL about 80 minutes of waiting.

**How to apply:** use this for any bench comparison across a content or schema change. Related: [[feedback-taskpolicy-background]], [[perf-cpu-contention]].
