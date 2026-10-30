---
name: landing-order-format-order
description: TL rule 2026-10-24 — branches land in save-format order; each dev takes the next format number at rebase time
metadata:
  node_type: memory
  type: project
  originSessionId: 8ebc2748-f01c-4184-9f3c-3d91bccd0bba
  modified: 2026-10-24T19:46:55.983Z
---

From 2026-10-24 the TL lands branches in save-format order (landing order = format order). Example: T-323a (79) → T-327 (80) → T-320a/b/c (81). A dev with a format bump waits for the TL's "rebase now", then rebases onto master, sets the assigned FORMAT_VERSION, regenerates and repins fixtures, and re-gates.

**Why:** parallel branches each bumping the save version kept colliding on rebase (format numbers, fixture pins).
**How to apply:** don't rebase a format-bumping branch early; take the format number the TL assigns. Related: [[cityplanner-delivery-setup]]
