---
name: feedback-visual-bug-acceptance
description: "Accept a visual bug fix only if the after-shot from the user's camera no longer shows what the user complained about; a dev's \"left: ...\" that is the core complaint means not done"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 658cadbb-3a19-4ad5-9f5e-58b5c8951eae
  modified: 2026-10-27T11:40:47.633Z
---

2026-10-27 BUG-122: I merged dev-2's fix (kinks smoothed) and reported "fixed", though the report said "left: still one node / V" and dev-2's own after-entry.png looked like before-entry.png at the junction. The user replied «так и шо ты исправил? всё ещё рубленое соединение» with a new screenshot.

**Why:** the user judges by the picture; a passing geometry test ("full width at J") did not match what was on screen.
**How to apply:** before merging a visual fix, put the after-shot next to the user's screenshot (same camera) and check each point of the complaint. If the "left/not done" part is the user's main complaint, send it back (format bumps are allowed while saves aren't kept) instead of merging and calling it fixed. Say plainly what's still wrong. Related: [[feedback-roads-whole-cells]], [[rebuild-release-after-merge]]
