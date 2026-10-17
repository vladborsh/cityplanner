//! `cp_content` — RON content schemas, loader and [`ContentDb`].
//! See `architecture/system-design.md` §4, §6, §8, §12.

pub mod db;
pub mod defs;
pub mod error;
pub mod kinds;
mod validate;

pub use db::{
    ContentDb, DEFAULT_CONTENT_ROOT, DefKind, PLACEHOLDER_INDEX, SimContent, TEST_STYLE_RON,
    test_policy_source,
};
pub use defs::*;
pub use error::{ContentError, ContentErrors};
pub use kinds::{PolicyKind, PolicyScope, ServiceKind, ServiceSite, UtilityKind, ZoneType};
