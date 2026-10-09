//! `cp_core` — ids and arenas, tick and game time, `SimRng`, `SimLimits`, generic command bus.
//! See `architecture/system-design.md` §2–§3.

// Sim code must iterate deterministically: HashMap/HashSet are banned (see clippy.toml).
#![deny(clippy::disallowed_types)]

pub mod arena;
pub mod command;
pub mod curve;
pub mod limits;
pub mod rng;
pub mod time;

pub use arena::{Arena, Id};
pub use command::{
    CommandError, CommandId, CommandLog, CommandOutcome, CommandQueue, Cost, LogEntry,
};
pub use limits::SimLimits;
pub use rng::SimRng;
pub use time::{GameTime, SimSpeed, Tick};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_core";

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_core");
    }
}
