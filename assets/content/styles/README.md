# Building styles (T-111 … T-113, `system-design.md` §8.2)

One `Style((...))` per file. A style is the *look* of a building: `massing` (volume shape),
`roof`, `floors` and `footprint_fill` ranges, `palette` (alternatives for walls, roof and trim, one
glass colour), `facade` and `details`. `BuildingDef.styles` lists the styles a def may use; which
one a building gets, and its floors, lot fill and colours inside the style, is a hash of its
`BuildingId` (`cp_render::building::variant`) — not sim state, not in saves.

## The rule

* **At least five looks per zone type**, named `<zone>_<look>` (`res_low_`, `res_high_`, `com_low_`,
  `com_high_`, `industrial_`, `office_`): different massing, roof, palette, facade and details.
* **Height bands.** A style owns its floor range, but the height of every zone grows with the
  level (a level-5 apartment block is 20 floors, level 1 is 5), and vertex cost grows with floors.
  So all styles come in bands, suffix `_a`/`_b`/`_c` (e.g. `res_high_podium_tower_c`): the same
  look at a floor range that fits the levels using that band (`res_low`, `com_low` and
  `industrial` have two bands, the tall zones three). Each band has four looks; a level's two
  defs take two each.
* **Every (zone type, level) draws from at least four different styles** across its two defs
  (`<name>_N` and `<name>_Nb`, two styles each), and each zone type has at least five different
  massing × roof × palette combinations. `content_e2e` tests both.
* Service buildings keep one `legacy_*` one-box style until their silhouettes (T-114).
* Details cost vertices (`near` only): balconies are capped at 16, a tank is 41 vertices, a fence
  is one board per cell. The §14 budget counts the average over the city (near ≤ 1.5×, mid and far
  ≤ 1.2× the plain-box buildings): `cargo test -p cp_render --release --test building_budget --
  --ignored --nocapture` measures a grid city.
