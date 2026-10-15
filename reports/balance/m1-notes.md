# M1 balance run — partial notes (stopped by user decision)

Status: driver written (`balance_m1_driver.rs`, copy of an uncommitted `crates/cp_sim/examples/balance_m1.rs`; run as
`cargo run --release -p cp_sim --example balance_m1 -- <map> <seed> <days> <out-prefix> [content-dir] [funds]`).
Runs finished but were NOT analysed into a report (CSV under `.data/balance/runs/`, gitignored). Master 6967ee8.

Early observations (flatlands seeds 11/22, river_valley seed 33, new-game funds 70 000, default 9% taxes):
- Flatlands has no water on the owned tiles: pump/outflow impossible, player must buy tower (5k) + treatment plant (25k).
- Pump/outflow need zoning cells of a road beside the shore; a road straight to the shore is not enough (needed a shore leg).
- river_valley start area is steep (road max slope 0.12): many lattice streets rejected, heading had to be rotated.
- Population stalls at ~300-500 by day 5 and stays flat for 45 days.
- Weekly budget is deeply negative: income ~200 vs upkeep ~3 700 at ~400 residents (tax_base ~10-130 per building x 9%);
  balance hits the -20 000 floor by day ~30 and keeps falling (weekly upkeep ignores the floor).
- Painted area is exhausted; expansion is unaffordable, so no further growth.
- Demand R goes strongly negative (-50) with ~18% unemployment while C/I stay small positive.
Tuning direction (unvalidated): raise tax_base roughly 10-20x or cut service upkeep; not tested.
