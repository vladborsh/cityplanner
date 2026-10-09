//! `cp_content` — RON content schemas, loader and [`ContentDb`].
//! See `architecture/system-design.md` §4, §6, §8, §12.

// Sim code must iterate deterministically: HashMap/HashSet are banned (see clippy.toml).
#![deny(clippy::disallowed_types)]

pub mod db;
pub mod defs;
pub mod error;
pub mod kinds;
mod validate;

pub use db::{ContentDb, DEFAULT_CONTENT_ROOT};
pub use defs::*;
pub use error::{ContentError, ContentErrors};
pub use kinds::{ServiceKind, UtilityKind, ZoneType};
