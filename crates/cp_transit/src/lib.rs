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
pub mod fleet;
pub mod goods;
pub mod intercity;
pub mod model;
pub mod plan;
pub mod riders;
pub mod rules;
pub mod run;
pub mod station;
pub mod sync;
pub mod tourists;
pub mod trains;
pub mod view;

pub use command::{MAX_LINE_NAME, StopRef, TransitCommand, TransitError, apply};
pub use fleet::{Bus, BusId, BusPhase, TransitFleet, TransitLedger};
pub use goods::{Endpoint, GOODS_BUCKET_TICKS, Goods, Haul, run_goods};
pub use intercity::{Intercity, Train, TrainPhase, run_intercity};
pub use model::{
    DEFAULT_LINE_VEHICLES, Journey, Line, LineId, LineState, LineStats, MAX_LINE_STOPS,
    MAX_LINE_VEHICLES, PALETTE_LEN, Ride, Stage, Stop, StopId, StopKind, TransitMode, TransitNet,
};
pub use plan::{Leg, LinePlan, LinePlans, PlanError, plan_line, plan_metro};
pub use riders::{choose_transit, run_riders};
pub use rules::{STOP_NODE_CLEARANCE, STOP_SNAP, STOP_SPACING, StopError, check_road_stop};
pub use run::run_lines;
pub use station::{
    add_station, connected_station, platform_ends, platform_segment, remove_station,
    station_reaches_outside, station_stop,
};
pub use sync::{rebind_stops, update_transit};
pub use tourists::{TOURISM_STREAM, Tourist, TouristPhase, Tourists, bring_by_train, run_tourists};
pub use trains::{IntercityPose, MetroTrain, intercity_pose, metro_trains};
pub use view::{LINE_PALETTE_RGB, LineRoute, ROUTE_STEP, TransitView, next_line_color};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_transit";

#[cfg(test)]
mod tests;
