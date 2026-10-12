//! `cp_core` — ids and arenas, tick and game time, `SimRng`, `SimLimits`, generic command bus.
//! See `architecture/system-design.md` §2–§3.

pub mod arena;
pub mod command;
pub mod curve;
pub mod limits;
pub mod rng;
pub mod time;
pub mod treasury;

pub use arena::{Arena, Id};
pub use command::{
    CommandError, CommandId, CommandLog, CommandOutcome, CommandQueue, Cost, LogEntry,
};
pub use limits::{MAX_COORD, SimLimits, sane_coord, sane_point};
pub use rng::SimRng;
pub use time::{GameTime, SimSpeed, Tick};
pub use treasury::{InsufficientFunds, Treasury};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_core";

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_core");
    }
}
