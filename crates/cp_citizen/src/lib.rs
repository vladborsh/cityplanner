//! `cp_citizen` — citizens and households: move-in, jobs, move-out. See
//! `architecture/system-design.md` §9.1.

pub mod grid;
pub mod model;
pub mod routine;
pub mod system;
pub mod trips;

pub use grid::CitizenGrid;
pub use model::{
    ADULT_AGE_DAYS, Citizen, CitizenId, DAYS_PER_YEAR, HOUSEHOLD_MAX, Household, HouseholdId,
    Location,
};
pub use routine::{
    CAR_SPEED_MPS, ROUTINE_STREAM, RoutineSettings, Shift, WALK_SPEED_MPS, cancel_trip,
    choose_mode, complete_trip, hour_of_day, shift_for, travel_ticks, update_routine,
};
pub use system::{
    ARRIVAL_SPEED_MPS, BASE_DELAY, FALLBACK_DELAY, JOBLESS_LEAVE_DAYS, MOVE_IN_ONE_IN,
    MOVE_IN_STREAM, update_citizens,
};
pub use trips::{Trip, TripBook, TripId, TripMode, TripPurpose, TripState};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_citizen";

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_citizen");
    }
}
