//! `cp_building` — simulation crate skeleton. See `architecture/system-design.md` §1.

pub mod command;

pub use command::{BuildingCommand, BuildingError, apply};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_building";

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_building");
    }
}
