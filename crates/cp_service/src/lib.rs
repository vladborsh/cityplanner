//! `cp_service` — city services of M2 (`architecture/system-design.md` §16 M2 D): garbage,
//! health, deathcare, fire, police and education buildings, and how far along the roads each
//! one reaches. Sits above `cp_building`/`cp_net` and below `cp_economy`/`cp_sim`.
//!
//! This crate holds the road coverage of every service kind ([`ServiceCoverage`]); the state
//! the service rows keep per building arrives with T-135.

pub mod coverage;

pub use coverage::{
    COVERAGE_PERIOD, ServiceCoverage, ServicePlugin, rebuild_coverage, update_coverage,
};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_service";

#[cfg(test)]
mod tests;
