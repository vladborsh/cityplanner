//! `cp_zone` — simulation crate skeleton. See `architecture/system-design.md` §1.

pub mod command;

pub use command::{ZoneCommand, ZoneError, apply};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_zone";

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_zone");
    }
}
