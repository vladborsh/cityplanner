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
pub mod mode;
pub mod parking;
pub mod pose;
pub mod runs;
pub mod service;

pub use agents::{
    AgentSettings, AgentStats, Agents, EntryTraffic, KIND_KEEP, KIND_MASK, KIND_PARKED,
    KIND_TRAFFIC_REROUTE, MIN_VISIBLE_SHARE, PED_CHUNK, PED_STEP, Pedestrian, PedestrianId,
    REROUTE_CAP, REROUTE_GAIN, REROUTE_MIN_M, REROUTE_PERIOD, SegmentLoad, Vehicle, VehicleId,
    VisibleShare, crossing_of, lane_delay_now, move_agents, ped_speed_factor,
    purge_departed_citizens, purge_removed_buildings, receive_paths, reroute_by_traffic,
    route_trips, update_traffic_load, update_visible_share,
};
pub use astar::{
    CarPath, NoPath, PedPath, find_car_path, find_car_path_lanes, find_class_path,
    find_class_path_lanes, find_ped_path, find_rail_path, find_truck_path, find_truck_path_lanes,
};
pub use graph::{
    Access, CongestionMap, LEFT_TURN_PENALTY, PED_SPEED, PathGraph, SegEnd, Side, U_TURN_PENALTY,
};
pub use heavy::HeavyBan;
pub use mode::{ALWAYS_WALK_M, TripTimes, mode_by_time, track_trip_times, way_times};
pub use parking::{
    Front, PARK_RETRIES, PARK_SEARCH_M, PARKED_CAP, PARKING_PERIOD, ParkedCar, ParkedCars,
    ParkingIndex, Place, building_places, choose_place, kerb_fronts, place_access, side_index,
    slot_blocked, update_parking_index,
};
pub use pose::{
    AgentFrame, AgentKey, CROSSING_ROOM_M, DOOR_WALK_M, PULL_M, PedestrianPose, Portal,
    VehiclePose, agent_key,
};
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
