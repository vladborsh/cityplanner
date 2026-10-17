//! `cp_district` — the district map: a 32 m brush grid over the whole map, named and coloured
//! districts, and per-zone-type tax overrides (`architecture/system-design.md` §16 M2 (B)).
//!
//! The crate sits **below** `cp_economy`: districts are data that systems read (tax, policies).
//! Systems that apply a policy effect (`cp_service`, `cp_utility`, `cp_building`, `cp_transit`,
//! `cp_traffic`, `cp_economy`) depend on this crate, which itself depends only on `cp_core`,
//! `cp_content` and `cp_terrain`. It holds two resources, both saved and checksummed:
//!
//! * [`DistrictMap`] — one `u8` per 32 m cell, `0` = no district, `n` = [`DistrictId`]`(n)`;
//! * [`Districts`] — the district records; district `n` is `list[n - 1]`, `None` = a free slot;
//! * [`CityPolicies`] — the policies switched on for the whole city (T-190); a district's own
//!   are in its record, and [`policy_at`] answers city ∨ district for a position.
//!
//! Player input is [`DistrictCommand`]; validation is the pure functions in [`command`].
//! New command variants are only appended (save stability rule).

pub mod command;
pub mod map;

pub use command::{DistrictCommand, DistrictError, TAX_MAX, TAX_MIN, apply};
pub use map::{
    CELL_M, Cell, CityPolicies, District, DistrictId, DistrictMap, Districts, MAX_DISTRICTS,
    NAME_MAX, PALETTE_LEN, PALETTE_RGB, SIDE, district_at, policy_at, zone_slot,
};

#[cfg(test)]
mod tests;
