//! Synthetic traffic for render benchmarks and demos: random car and pedestrian routes on the
//! current network, injected straight into the agent pools. Not part of the game: trips are
//! not in the `TripBook`, so arrivals complete nothing (`complete_trip` ignores unknown
//! trips). Deterministic from the seed it is given.

use cp_citizen::{CitizenId, TripId};
use cp_core::{SimRng, Tick};
use cp_net::NetGraph;

use crate::agents::{Agents, Pedestrian, Vehicle};
use crate::astar::{find_car_path, find_ped_path};
use crate::graph::{Access, CongestionMap, PathGraph, Side};

/// RNG stream id of the demo traffic.
const DEMO_STREAM: u64 = 0xDE30_7A1F;

/// Routes planned per call at most, so a top-up never stalls a frame.
const MAX_PER_CALL: usize = 600;

fn random_access(rng: &mut SimRng, ids: &[(cp_net::SegmentId, f32)]) -> Access {
    let (segment, length) = ids[rng.below(ids.len() as u32) as usize];
    Access {
        segment,
        s: rng.next_f32() * length,
        side: if rng.below(2) == 0 {
            Side::Left
        } else {
            Side::Right
        },
    }
}

/// Adds routed cars and pedestrians until the pools hold `cars` and `peds` (at most
/// [`MAX_PER_CALL`] per kind per call). Randomness is the `batch`-th sub-stream of `rng`; returns how many were added.
pub fn top_up(
    agents: &mut Agents,
    graph: &PathGraph,
    net: &NetGraph,
    cars: usize,
    peds: usize,
    rng: &SimRng,
    batch: u64,
) -> usize {
    let ids: Vec<_> = net.segments().map(|(id, s)| (id, s.length)).collect();
    if ids.is_empty() {
        return 0;
    }
    // An independent stream per call, derived from the sim RNG (no shared stream).
    let mut rng = rng.fork(DEMO_STREAM, Tick(batch));
    let cong = CongestionMap::default();
    let mut added = 0;
    let mut tries = 0;
    while agents.vehicle_count() < cars && added < MAX_PER_CALL && tries < MAX_PER_CALL * 4 {
        tries += 1;
        let (a, b) = (random_access(&mut rng, &ids), random_access(&mut rng, &ids));
        if let Ok(path) = find_car_path(graph, &cong, &a, &b) {
            agents.insert_demo_vehicle(Vehicle {
                trip: TripId::new(u32::MAX, 0),
                citizen: CitizenId::new(u32::MAX, 0),
                s: path.start_s,
                path,
                kind: 0,
                hop: 0,
                in_conn: false,
                spawned: false,
                speed: 0.0,
                stuck_timer: 0,
                reroute: None,
            });
            added += 1;
        }
    }
    let mut ped_added = 0;
    tries = 0;
    while agents.pedestrian_count() < peds && ped_added < MAX_PER_CALL && tries < MAX_PER_CALL * 4 {
        tries += 1;
        let (a, b) = (random_access(&mut rng, &ids), random_access(&mut rng, &ids));
        if let Ok(route) = find_ped_path(graph, &a, &b) {
            agents.insert_demo_pedestrian(Pedestrian {
                trip: TripId::new(u32::MAX, 0),
                citizen: CitizenId::new(u32::MAX, 0),
                route,
                leg: 0,
                s: 0.0,
                leg_len: 0.0,
                started: false,
                waited: 0,
                pos: bevy_math::Vec2::ZERO,
            });
            ped_added += 1;
        }
    }
    added + ped_added
}
