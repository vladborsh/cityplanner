//! Content vocabulary shared by RON definitions and commands (`system-design.md` §4).
//!
//! These enums are serialised by variant number (postcard) in saves: `#[repr(u8)]`,
//! explicit discriminants, **append-only** — never reorder or remove a variant.

use serde::{Deserialize, Serialize};

/// Zoning category of a cell or building. `None` erases zoning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ZoneType {
    None = 0,
    ResLow = 1,
    ResHigh = 2,
    ComLow = 3,
    ComHigh = 4,
    Industrial = 5,
    Office = 6,
}

/// Utility network kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum UtilityKind {
    Water = 0,
    Sewage = 1,
    Power = 2,
}

/// Budgeted service. M1 stub: extended in M2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ServiceKind {
    Water = 0,
    Power = 1,
    Roads = 2,
}
