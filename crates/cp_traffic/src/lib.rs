//! `cp_traffic` — pathfinding (and, later, vehicle/pedestrian movement). See
//! `architecture/system-design.md` §9.3.
//!
//! Dependencies: `cp_net` (lanes, junction links) only for now; `cp_citizen` (a §1 edge)
//! is not needed until trips exist.

pub mod agents;
pub mod astar;
pub mod graph;
pub mod service;

pub use agents::{
    AgentSettings, AgentStats, Agents, PED_CHUNK, PED_STEP, Pedestrian, PedestrianId, Vehicle,
    VehicleId, crossing_of, move_agents, receive_paths, route_trips,
};
pub use astar::{CarPath, NoPath, PedPath, find_car_path, find_ped_path};
pub use graph::{
    Access, CONGESTION_K, CongestionMap, LEFT_TURN_PENALTY, PED_SPEED, PathGraph, SegEnd, Side,
    U_TURN_PENALTY,
};
pub use service::{
    Mode, PATH_DELAY_TICKS, PathNetwork, PathOutcome, PathRequest, PathResult, PathResults,
    PathService, RequestId, SavedPaths, SavedRequest, deliver_paths, update_path_network,
};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_traffic";

#[cfg(test)]
mod tests;
