//! `cp_utility` — pipe and power-line networks. See `architecture/system-design.md` §10.
//!
//! This crate holds the network *graphs*, their validation and the `BuildLink` /
//! `RemoveLink` commands. Capacity distribution, supply flags and overlays come later
//! (T-038…); [`UtilityGraph::components`] is the BFS-ready derivation they build on.

pub mod command;
pub mod graph;
pub mod plan;

pub use command::{UtilityCommand, UtilityError, apply};
pub use graph::{
    Components, GraphError, UtilityGraph, UtilityLink, UtilityLinkId, UtilityNet, UtilityNode,
    UtilityNodeId,
};
pub use plan::{
    LinkPlan, PathProblem, PlanInputs, PlanNode, PlannedLink, PlannedSplit, apply_plan,
    plan_link_path,
};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_utility";

#[cfg(test)]
mod tests;
