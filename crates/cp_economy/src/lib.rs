//! `cp_economy` — treasury rules, taxes, upkeep and the weekly settlement. See
//! `architecture/system-design.md` §11.
//!
//! The [`Treasury`](cp_core::Treasury) itself lives in `cp_core` (the command paths of the
//! road, building and utility crates charge it); this crate owns the settings
//! ([`EconomyState`]: tax rates, budgets, last report), the `SetTaxRate`/`SetBudget` commands
//! and [`settle_week`].

pub mod command;
pub mod demand;
pub mod growth;
pub mod health;
pub mod land;
pub mod police;
pub mod state;
pub mod week;

pub use command::{EconomyCommand, EconomyError, apply};
pub use demand::{DemandAcc, smooth, update_demand};
pub use growth::{CITY_PERIOD, Decay, WorkerShortage, level_for, update_city};
pub use health::{HEALTH_STREAM, update_health};
pub use land::{CELL_M, LandGrids, LandInputs, SIDE};
pub use police::update_police;
pub use state::{
    BUDGET_DEFAULT, BUDGET_MAX, BUDGET_MIN, BUDGETED, Budgets, DistrictIncome, EconomyState, Loan,
    TAX_DEFAULT, TAX_MAX, TAX_MIN, TAXED_ZONES, TaxRates, WeekReport,
};
pub use week::{WeekInputs, segment_upkeep, settle_week, week_detail, week_report};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_economy";

#[cfg(test)]
mod health_tests;
#[cfg(test)]
mod police_tests;
#[cfg(test)]
mod tests;
