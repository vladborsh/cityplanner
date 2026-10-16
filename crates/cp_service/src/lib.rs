//! `cp_service` — city services of M2 (`architecture/system-design.md` §16 M2 D): garbage,
//! health, deathcare, fire, police and education buildings, and how far along the roads each
//! one reaches. Sits above `cp_building`/`cp_net` and below `cp_economy`/`cp_sim`.
//!
//! This crate holds the road coverage of every service kind ([`ServiceCoverage`]), the
//! serialized per-building state the service rows fill ([`ServiceState`]) and the daily
//! capacity helper [`nearest_facility`].

pub mod coverage;
pub mod education;
pub mod facility;
pub mod fire;
pub mod garbage;
pub mod state;

pub use coverage::{
    COVERAGE_PERIOD, RoadGraph, ServiceCoverage, ServicePlugin, rebuild_coverage, update_coverage,
};
pub use education::update_education;
pub use facility::{
    nearest_facility, nearest_facility_with, purge_removed_services, reset_daily_capacity,
};
pub use fire::update_fire;
pub use garbage::update_garbage;
pub use state::{BuildingServices, Facility, Fire, ServiceProblems, ServiceState, ServiceSummary};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_service";

#[cfg(test)]
mod education_tests;
#[cfg(test)]
mod fire_tests;
#[cfg(test)]
mod garbage_tests;
#[cfg(test)]
mod tests;
