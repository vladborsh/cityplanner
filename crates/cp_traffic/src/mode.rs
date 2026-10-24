//! Trip mode by need (T-242): a citizen with a car walks when walking the real route on the
//! pedestrian network takes less time than driving it (congestion included) plus the time it
//! takes to get going; otherwise he drives. A missing footpath or a long detour sends people to
//! their cars, a new footpath lets them walk. Only times and penalties, no population shares.

use cp_citizen::TripMode;

use crate::astar::{WalkClass, find_car_path, find_ped_path, find_walk_path};
use crate::graph::{Access, CongestionMap, PathGraph};

/// Straight lines shorter than this (metres) are walked without looking at the routes.
pub const ALWAYS_WALK_M: f32 = 25.0;

/// Whether `citizen` owns a bike (T-322, §23.1): `share` of the citizens, by a hash of the
/// index (like [`crate::ped_speed_factor`]); no field, nothing saved.
pub fn has_bike(citizen: cp_citizen::CitizenId, share: f32) -> bool {
    /// Salt of the bike hash.
    const BIKE_SALT: u64 = 0x6269_6b65_6f77_6e73;
    share > 0.0 && cp_core::rng::hash_unit([0; 32], u64::from(citizen.index), BIKE_SALT) < share
}

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

/// Seconds the ways between two buildings take, `(walk, drive, ride)`: the real routes on the
/// pedestrian and car networks, congestion and `car_start_penalty_s` included for the drive
/// (T-243), and for a cyclist (`bike`, T-322) the ride with its start penalty when it is no
/// longer than the longest ride. A way that does not exist is `None`; all are `None` for a
/// building with no access.
pub fn way_times(
    net: &cp_net::NetGraph,
    graph: &PathGraph,
    cong: &CongestionMap,
    from: &cp_building::Building,
    to: &cp_building::Building,
    car_start_penalty_s: f32,
    bike: Option<crate::service::BikeChoice>,
) -> (Option<f32>, Option<f32>, Option<f32>) {
    let (Some(a), Some(b)) = (
        crate::agents::access_of(net, from),
        crate::agents::access_of(net, to),
    ) else {
        return (None, None, None);
    };
    (
        find_ped_path(graph, &a, &b).ok().map(|p| p.cost),
        find_car_path(graph, cong, &a, &b)
            .ok()
            .map(|p| p.cost + car_start_penalty_s),
        bike.and_then(|k| {
            let r = find_walk_path(graph, &a, &b, WalkClass::Bike(k.speeds)).ok()?;
            k.ride_s(r.cost)
        }),
    )
}

/// How long trips take, for the Traffic metrics (BUG-070): a running average of the time from
/// asking for the route to arriving, in game minutes of the movement clock, on foot and by car.
/// Derived while the game runs, never saved (it starts again after a load).
#[derive(bevy_ecs::resource::Resource, Clone, Debug, Default)]
pub struct TripTimes {
    live: std::collections::BTreeMap<cp_citizen::TripId, (u64, usize)>,
    /// Average minutes `[on foot, by car, by bike]` (T-322), `0` until a trip of that mode
    /// ended.
    pub avg_minutes: [f32; 3],
    seen: [bool; 3],
}

/// Weight of the newest trip in [`TripTimes::avg_minutes`].
const TRIP_TIME_ALPHA: f32 = 0.01;

/// Records when trips start routing and how long they took when they ended.
pub fn track_trip_times(
    tick: bevy_ecs::system::Res<cp_core::Tick>,
    book: bevy_ecs::system::Res<cp_citizen::TripBook>,
    mut times: bevy_ecs::system::ResMut<TripTimes>,
) {
    let now = tick.0;
    let mut ended: Vec<(u64, usize)> = Vec::new();
    times.live.retain(|id, (since, kind)| {
        if let Some(t) = book.get(*id) {
            // T-322: a trip becomes a ride when its route arrives.
            if t.mode == cp_citizen::TripMode::Bike {
                *kind = 2;
            }
            return true;
        }
        ended.push((now - *since, *kind));
        false
    });
    for (ticks, kind) in ended {
        let minutes = ticks as f32 * cp_core::time::MOVE_SECONDS_PER_TICK / 60.0;
        if times.seen[kind] {
            let a = times.avg_minutes[kind];
            times.avg_minutes[kind] = a + (minutes - a) * TRIP_TIME_ALPHA;
        } else {
            times.avg_minutes[kind] = minutes;
            times.seen[kind] = true;
        }
    }
    let fresh: Vec<_> = book
        .iter()
        .filter(|(id, t)| {
            matches!(t.state, cp_citizen::TripState::AwaitingPath { .. })
                && !times.live.contains_key(id)
        })
        .filter_map(|(id, t)| match t.mode {
            cp_citizen::TripMode::Walk => Some((id, 0)),
            cp_citizen::TripMode::Car => Some((id, 1)),
            cp_citizen::TripMode::Transit | cp_citizen::TripMode::Bike => None,
        })
        .collect();
    for (id, kind) in fresh {
        times.live.insert(id, (now, kind));
    }
}
