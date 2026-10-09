//! `cp_citizen` — citizens and households: move-in, jobs, move-out. See
//! `architecture/system-design.md` §9.1.

pub mod grid;
pub mod model;
pub mod system;

pub use grid::CitizenGrid;
pub use model::{
    ADULT_AGE_DAYS, Citizen, CitizenId, DAYS_PER_YEAR, HOUSEHOLD_MAX, Household, HouseholdId,
    Location,
};
pub use system::{
    ARRIVAL_M_PER_TICK, BASE_DELAY, FALLBACK_DELAY, JOBLESS_LEAVE_DAYS, MOVE_IN_ONE_IN,
    MOVE_IN_STREAM, update_citizens,
};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_citizen";

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_citizen");
    }
}
