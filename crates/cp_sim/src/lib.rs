//! `cp_sim` — simulation crate skeleton. See `architecture/system-design.md` §1.

pub mod checksum;
pub mod command;
pub mod phase;
pub mod plugin;
pub mod runner;
pub mod view;

pub use checksum::{Fnv1a, SimChecksum};
pub use command::{SimCommand, dispatch_commands, init_command_resources};
pub use phase::SimPhase;
pub use plugin::{SimBuilder, SimPlugin};
pub use runner::{MAX_TICKS_PER_STEP, SimRunner, run_headless, run_headless_with};
pub use view::{ArenaFill, ArenaGauges, SimStats, SimTerrain, SimView};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_sim";

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_sim");
    }
}
