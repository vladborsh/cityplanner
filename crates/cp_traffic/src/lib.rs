//! `cp_traffic` — pathfinding (and, later, vehicle/pedestrian movement). See
//! `architecture/system-design.md` §9.3.
//!
//! Dependencies: `cp_net` (lanes, junction links) only for now; `cp_citizen` (a §1 edge)
//! is not needed until trips exist.

pub mod agents;
pub mod astar;
pub mod demo;
pub mod drive;
pub mod graph;
pub mod heavy;
pub mod pose;
pub mod runs;
pub mod service;

pub use agents::{
    AgentSettings, AgentStats, Agents, KIND_KEEP, KIND_MASK, KIND_PARKED, PED_CHUNK, PED_STEP,
    Pedestrian, PedestrianId, SegmentLoad, Vehicle, VehicleId, crossing_of, move_agents,
    purge_removed_buildings, receive_paths, route_trips, update_traffic_load,
};
pub use astar::{
    CarPath, NoPath, PedPath, find_car_path, find_car_path_lanes, find_ped_path, find_rail_path,
    find_truck_path, find_truck_path_lanes,
};
pub use graph::{
    Access, CONGESTION_K, CongestionMap, LEFT_TURN_PENALTY, PED_SPEED, PathGraph, SegEnd, Side,
    U_TURN_PENALTY,
};
pub use heavy::HeavyBan;
pub use pose::{AgentFrame, AgentKey, PedestrianPose, VehiclePose, agent_key};
pub use runs::{
    RunError, RunId, RunPoint, RunState, SERVICE_GEN, ServiceRun, ServiceRuns, WALK_KIND,
    advance_service_runs, is_rail_kind, receive_service_paths, route_service_runs, run_of,
    walk_run_of,
};
pub use service::{
    Mode, PATH_DELAY_TICKS, PathNetwork, PathOutcome, PathRequest, PathResult, PathResults,
    PathService, RequestId, SavedPaths, SavedRequest, deliver_paths, update_path_network,
};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_traffic";

#[cfg(test)]
mod drive_tests;
#[cfg(test)]
mod heavy_tests;
#[cfg(test)]
mod rail_tests;
#[cfg(test)]
mod tests;
