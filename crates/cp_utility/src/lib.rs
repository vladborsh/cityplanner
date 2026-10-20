//! `cp_utility` — pipe and power-line networks. See `architecture/system-design.md` §10.
//!
//! This crate holds the network *graphs*, their validation, the `BuildLink` / `RemoveLink`
//! commands and the supply computation ([`supply`]): capacity distribution and the
//! `NO_WATER` / `NO_SEWAGE` / `NO_POWER` building flags.

pub mod command;
pub mod graph;
pub mod plan;
pub mod supply;

pub use command::{UtilityCommand, UtilityError, apply};
pub use graph::{
    Components, GraphError, UtilityGraph, UtilityLink, UtilityLinkId, UtilityNet, UtilityNode,
    UtilityNodeId,
};
pub use plan::{
    LinkPlan, PathProblem, PlanInputs, PlanNode, PlannedLink, PlannedSplit, apply_plan,
    plan_link_path,
};
pub use supply::{
    AttachCache, Coverage, PowerPool, PowerPools, SUPPLY_PERIOD, compute_supply,
    compute_supply_cached, compute_supply_pools, compute_supply_with, reach_m, update_supply,
};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_utility";

#[cfg(test)]
mod tests;

#[cfg(test)]
mod supply_tests;
