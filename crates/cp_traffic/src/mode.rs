//! Trip mode by need (T-242): a citizen with a car walks when walking the real route on the
//! pedestrian network takes less time than driving it (congestion included) plus the time it
//! takes to get going; otherwise he drives. A missing footpath or a long detour sends people to
//! their cars, a new footpath lets them walk. Only times and penalties, no population shares.

use cp_citizen::TripMode;

use crate::astar::{find_car_path, find_ped_path};
use crate::graph::{Access, CongestionMap, PathGraph};

/// Straight lines shorter than this (metres) are walked without looking at the routes.
pub const ALWAYS_WALK_M: f32 = 25.0;

/// Walk or drive between two access points: the shorter of the walking time and the driving time
/// plus `car_start_penalty_s`. A route that does not exist for one mode leaves the other; `None`
/// when neither exists (the caller keeps the mode it had).
pub fn mode_by_time(
    graph: &PathGraph,
    cong: &CongestionMap,
    from: &Access,
    to: &Access,
    car_start_penalty_s: f32,
) -> Option<TripMode> {
    let walk = find_ped_path(graph, from, to).ok().map(|p| p.cost);
    let drive = find_car_path(graph, cong, from, to)
        .ok()
        .map(|p| p.cost + car_start_penalty_s);
    match (walk, drive) {
        (Some(w), Some(d)) => Some(if w <= d {
            TripMode::Walk
        } else {
            TripMode::Car
        }),
        (Some(_), None) => Some(TripMode::Walk),
        (None, Some(_)) => Some(TripMode::Car),
        (None, None) => None,
    }
}
