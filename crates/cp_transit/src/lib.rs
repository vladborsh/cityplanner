//! `cp_transit` — public transport of M3 (`architecture/system-design.md` §16 M3 A): stops,
//! lines, the commands that edit them and the route of a line as a value derived from the
//! road network. Sits above `cp_traffic` and below `cp_economy`/`cp_sim`.
//!
//! * [`TransitNet`] — the serialized stops and lines (part of the save and the checksum).
//! * [`rules`] — where a stop may stand; pure functions shared with the tool.
//! * [`plan_line`] — the A* route of a line, leg by leg; [`LinePlans`] keeps them (derived,
//!   never saved) and [`update_transit`] re-derives them after every rebuild of the
//!   `PathGraph` and after a load.
//! * [`TransitCommand`] / [`apply`] — the player's side.

pub mod command;
pub mod model;
pub mod plan;
pub mod rules;
pub mod sync;

pub use command::{StopRef, TransitCommand, TransitError, apply};
pub use model::{
    DEFAULT_LINE_VEHICLES, Line, LineId, LineState, MAX_LINE_STOPS, MAX_LINE_VEHICLES, PALETTE_LEN,
    Stop, StopId, StopKind, TransitMode, TransitNet,
};
pub use plan::{Leg, LinePlan, LinePlans, PlanError, plan_line};
pub use rules::{STOP_NODE_CLEARANCE, STOP_SNAP, STOP_SPACING, StopError, check_road_stop};
pub use sync::{rebind_stops, update_transit};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_transit";

#[cfg(test)]
mod tests;
