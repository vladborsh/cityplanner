---
name: feedback-roads-whole-cells
description: "User rule — roads are built by visible whole cells, never metres or half cells; accept road work only via a sloppy-cursor player-flow test"
metadata:
  node_type: memory
  type: feedback
  originSessionId: f92be386-2f35-4788-b563-f85aca353413
  modified: 2026-10-19T21:54:58.502Z
---

User (2026-10-19, angry after a day of road fixes): "строю по визуальным клеткам, а не по метрам; какие полклетки — делай аппроксимацию". With snaps on, every street end is quantized so the frontage has a WHOLE number of cells from the cross street's edge; closing streets land on the neighbour's cell line; the length label is an integer equal to the cells visible after the click; no metres in the road UI. Only Alt (snaps off) may give non-whole blocks.

**Why:** the day's road redesign passed idealized-point sweeps but the user's simple 8x8/8x16 blocks still came out crooked (label 8 = 7 visible, 87° closing street, staircase inside a closed block).
**How to apply:** never report a road/grid fix as done until a player-flow test (real tool code, sloppy cursor ±3 m/±2°, 8x8/8x16/closed blocks/closing from a dead end) is green, and check a screenshot of the user's own layout myself before telling them. Related: [[feedback-decide-defaults]]

2026-10-20: user again: "мне все равно на метры, главное чтобы не мешало ставить постройки в углах". When reporting to the user, speak in cells and in-game effects only (can I build there? how many cells?), never metres/radii, and don't ask the user metric tuning questions — decide them yourself.

2026-10-20 CONFIRMED: after T-275 (spec v2: cell model, sections 10 cells between road centres, roads 2/4 cells wide) the user said roads/blocks are "чётче и лучше, углы магнитятся супер, секции ложатся идеально — всего-то надо было разложить математику по клеткам". The approach that worked: one owner (Architect) end to end, a grilled spec in the user's own unit (cells), sloppy-cursor player-flow tests, my own screenshot check before reporting. Next refinement T-278: 1-cell step, 10-marks as weak guides.
