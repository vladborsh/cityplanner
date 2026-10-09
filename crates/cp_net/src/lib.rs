//! `cp_net` — simulation crate skeleton. See `architecture/system-design.md` §1.

// Sim code must iterate deterministically: HashMap/HashSet are banned (see clippy.toml).
#![deny(clippy::disallowed_types)]

pub mod command;

pub use command::{NetCommand, NetError, apply};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_net";

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_net");
    }
}
