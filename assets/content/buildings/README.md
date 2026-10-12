# Building definitions (M1)

Every zone type has levels and **two variants per level** (`<name>_<level>` and
`<name>_<level>b`), with lot sizes from 1×1 to 4×4 cells (frontage × depth; one cell is 8 m).

| Prefix | Zone | Levels |
|---|---|---|
| `house_low_N` | ResLow | 1–5 |
| `apt_high_N` | ResHigh | 1–5 |
| `com_low_N` | ComLow | 1–3 |
| `com_high_N` | ComHigh | 1–3 |
| `ind_N` | Industrial | 1–3 |
| `office_N` | Office | 1–3 |

Service buildings (`water_pump_station` on water, `sewage_outflow_station` at the shore,
`coal_power_plant`, `wind_turbine`; `ServiceDef.capacity` is their supply per game day, `site` where
they may stand) carry `service: Some(..)`, a `cost` (what the player pays to place one) and an
`upkeep_week` (per game week at a 100% budget; both in currency units) and are never spawned by
the zone system.

**All numbers below the lot size are PLACEHOLDERS for balance tuning** (T-028):
households / jobs per education level, consumption (water, sewage, power per game day),
`tax_base`, floors and roofs. They only need to be plausible and monotone in the level.
`house_low_1` is the original M1 sample and is kept as is.
