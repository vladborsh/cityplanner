//! §25 (T-329a): the door portal and sidewalk room of pedestrians in `frame()`, and the walking
//! speed spread by citizen hash.

use bevy_math::Vec2;
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint,
};
use cp_citizen::{Trip, TripBook, TripMode, TripPurpose, TripState};
use cp_core::{Id, Tick};
use cp_net::{JunctionGeometry, NetGraph, SegmentId};

use super::*;
use crate::tests::{acc, sim};
use crate::{
    CROSSING_ROOM_M, PathNetwork, PedestrianPose, Portal, ServiceRuns, VehiclePose, find_ped_path,
};

/// A free-standing 1×1 building right of the +x road `seg`, its access at curve `t`.
fn building(grid: &mut BuildingGrid, seg: SegmentId, t: f32, pos: Vec2) -> BuildingId {
    grid.insert(Building {
        def: cp_content::BuildingDefId(0),
        footprint: Footprint::Free { w: 1, d: 1 },
        level: 1,
        state: BuildingState::Active,
        ready_at: Tick(0),
        pos,
        rot: 0.0,
        access: Some(AccessPoint { segment: seg, t }),
        flags: BuildingFlags::default(),
        level_progress: 0.0,
    })
}

fn walker(trip: TripId, route: PedPath, leg: u32, s: f32, leg_len: f32) -> Pedestrian {
    Pedestrian {
        trip,
        citizen: Id::new(0, 0),
        route,
        leg,
        s,
        leg_len,
        started: true,
        waited: 0,
        pos: Vec2::ZERO,
        bike: false,
        cache: LegCache::default(),
    }
}

struct Town {
    net: NetGraph,
    geo: JunctionGeometry,
    graph: std::sync::Arc<crate::PathGraph>,
    buildings: BuildingGrid,
    trips: TripBook,
    trip: TripId,
    home: BuildingId,
    work: BuildingId,
    /// Home → work along the right sidewalk of two segments (one junction corner between).
    route: PedPath,
    /// Home → a point mid-way on the second segment (a stop).
    to_stop: PedPath,
}

fn town() -> Town {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    let b = s.seg((1100.0, 1000.0), (1200.0, 1000.0));
    // Right of travel along +x is +z.
    let mut buildings = BuildingGrid::default();
    let home = building(&mut buildings, a, 0.2, Vec2::new(1020.0, 1015.0));
    let work = building(&mut buildings, b, 0.8, Vec2::new(1180.0, 1015.0));
    s.tick();
    let net = s.world.resource::<NetGraph>().clone();
    let geo = s.world.resource::<JunctionGeometry>().clone();
    let graph = s.world.resource::<PathNetwork>().graph().clone();
    let access = |b: BuildingId| access_of(&net, buildings.get(b).unwrap()).unwrap();
    let route = find_ped_path(&graph, &access(home), &access(work)).unwrap();
    let to_stop = find_ped_path(&graph, &access(home), &acc(b, 50.0, Side::Right)).unwrap();
    let mut trips = TripBook::default();
    let trip = trips
        .add(Trip {
            citizen: Id::new(0, 0),
            from: home,
            to: work,
            purpose: TripPurpose::Work,
            mode: TripMode::Walk,
            state: TripState::Agent,
            started: Tick(0),
            park: Default::default(),
        })
        .unwrap();
    Town {
        net,
        geo,
        graph,
        buildings,
        trips,
        trip,
        home,
        work,
        route,
        to_stop,
    }
}

fn pose(t: &Town, p: Pedestrian) -> PedestrianPose {
    let mut agents = Agents::new();
    agents.insert_demo_pedestrian(p);
    let f = agents.frame(
        &t.graph,
        &t.net,
        &t.geo,
        &t.trips,
        &ServiceRuns::default(),
        &t.buildings,
    );
    f.pedestrians[0]
}

#[test]
fn a_walker_leaves_its_door_and_enters_the_next_within_ten_metres() {
    let t = town();
    let n = t.route.states.len() as u32;
    assert!(n > 0, "the route turns a corner");
    let at = |leg, s, len| pose(&t, walker(t.trip, t.route.clone(), leg, s, len)).portal;
    assert_eq!(
        at(0, 5.0, 60.0),
        Some(Portal {
            building: t.home,
            progress: 0.5,
            leaving: true
        })
    );
    let entering = at(n, 46.0, 50.0).unwrap();
    assert_eq!((entering.building, entering.leaving), (t.work, false));
    assert!((entering.progress - 0.6).abs() < 1e-5, "{entering:?}");
    // Mid-way on the first and on the last leg, and on the legs between: no door.
    assert_eq!(at(0, 30.0, 60.0), None);
    assert_eq!(at(n, 20.0, 50.0), None);
    if n > 1 {
        assert_eq!(at(1, 1.0, 20.0), None);
    }
    // Not yet on the first leg: not in the frame at all, no door either.
    let mut waiting = walker(t.trip, t.route.clone(), 0, 0.0, 0.0);
    waiting.started = false;
    let mut agents = Agents::new();
    agents.insert_demo_pedestrian(waiting);
    let f = agents.frame(
        &t.graph,
        &t.net,
        &t.geo,
        &t.trips,
        &ServiceRuns::default(),
        &t.buildings,
    );
    assert!(f.pedestrians.is_empty());
}

#[test]
fn a_walk_to_a_stop_has_its_door_only_at_home() {
    let t = town();
    let n = t.to_stop.states.len() as u32;
    let at = |leg, s, len| pose(&t, walker(t.trip, t.to_stop.clone(), leg, s, len)).portal;
    assert_eq!(
        at(0, 2.0, 60.0).map(|p| (p.building, p.leaving)),
        Some((t.home, true))
    );
    assert_eq!(at(n, 48.0, 50.0), None, "no door at the stop");
}

#[test]
fn a_short_walk_between_two_doors_splits_at_half_way() {
    let mut t = town();
    // A neighbour 12 m along the same sidewalk: the doors take 6 m each.
    let seg = t.buildings.get(t.home).unwrap().access().unwrap().segment;
    let next = building(&mut t.buildings, seg, 0.32, Vec2::new(1032.0, 1015.0));
    let access = |b: BuildingId| access_of(&t.net, t.buildings.get(b).unwrap()).unwrap();
    let route = find_ped_path(&t.graph, &access(t.home), &access(next)).unwrap();
    assert!(route.states.is_empty());
    let len = (route.goal_s - route.start_s).abs();
    assert!((len - 12.0).abs() < 0.5, "{len}");
    let trip = t
        .trips
        .add(Trip {
            citizen: Id::new(1, 0),
            from: t.home,
            to: next,
            purpose: TripPurpose::Shop,
            mode: TripMode::Walk,
            state: TripState::Agent,
            started: Tick(0),
            park: Default::default(),
        })
        .unwrap();
    let at = |s: f32| {
        pose(&t, walker(trip, route.clone(), 0, s, len))
            .portal
            .unwrap()
    };
    let p = at(len * 0.25);
    assert_eq!((p.building, p.leaving), (t.home, true));
    assert!((p.progress - 0.5).abs() < 1e-4, "{p:?}");
    let p = at(len * 0.75);
    assert_eq!((p.building, p.leaving), (next, false));
    assert!((p.progress - 0.5).abs() < 1e-4, "{p:?}");
}

#[test]
fn room_is_half_the_free_sidewalk_and_little_on_corners() {
    let t = town();
    let lane = t.net.lane(t.route.start).unwrap();
    let room = pose(&t, walker(t.trip, t.route.clone(), 0, 30.0, 60.0)).room;
    assert!(
        (room - (lane.width - 1.2) * 0.5).abs() < 1e-5,
        "{room} on {}",
        lane.width
    );
    assert!(room > CROSSING_ROOM_M);
    let states = &t.route.states;
    let corner = (1..states.len())
        .find(|&i| states[i - 1].0 != states[i].0)
        .expect("a corner leg");
    let room = pose(&t, walker(t.trip, t.route.clone(), corner as u32, 1.0, 5.0)).room;
    assert_eq!(room, CROSSING_ROOM_M);
}

/// 1 000 walkers on one 200 m sidewalk arrive between 200/1,61 and 200/1,19 s, spread over the
/// whole range, each at the speed of its citizen's hash.
#[test]
fn walking_speeds_spread_by_citizen() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1300.0, 1000.0));
    s.tick();
    let net = s.world.resource::<NetGraph>().clone();
    let geo = s.world.resource::<JunctionGeometry>().clone();
    let pnet = s.world.resource::<PathNetwork>();
    let graph = pnet.graph().clone();
    let route = find_ped_path(
        &graph,
        &acc(a, 50.0, Side::Right),
        &acc(a, 250.0, Side::Right),
    )
    .unwrap();
    assert!(route.states.is_empty());
    let ctx = Ctx {
        net: &net,
        graph: &graph,
        geo: &geo,
        tick: 0,
        generation: pnet.generation(),
        bike: crate::astar::BikeSpeeds::of_balance(&cp_content::Balance::default()),
    };
    let mut secs = Vec::new();
    for i in 0..1000u32 {
        let mut p = walker(TripId::new(0, 0), route.clone(), 0, 0.0, 0.0);
        p.started = false;
        p.citizen = Id::new(i, 0);
        let mut ticks = 0u32;
        while matches!(step_pedestrian(&mut p, &ctx), Step::Moving) {
            ticks += 1;
            assert!(ticks < 1000);
        }
        let len = p.leg_len;
        assert!((len - 200.0).abs() < 1.0, "{len}");
        // Arrived on tick `ticks + 1`: the walk took between `ticks` and `ticks + 1` steps.
        let speed = ped_speed_factor(p.citizen) * PED_SPEED;
        assert!((1.19..=1.61).contains(&speed), "{speed}");
        assert!(
            (ticks as f32 * PED_STEP..=(ticks + 1) as f32 * PED_STEP)
                .contains(&(len / ped_speed_factor(p.citizen))),
            "citizen {i}: {ticks} ticks for {len} m"
        );
        secs.push(len / speed);
    }
    let (lo, hi) = secs
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &s| (lo.min(s), hi.max(s)));
    assert!((200.0 / 1.61 - 1.0..200.0 / 1.58).contains(&lo), "{lo}");
    assert!((200.0 / 1.22..=200.0 / 1.19 + 1.0).contains(&hi), "{hi}");
    // Spread, not two clusters: every fifth of the speed range holds 10 % or more.
    let mut bins = [0u32; 5];
    for i in 0..1000u32 {
        let f = (ped_speed_factor(Id::new(i, 0)) - 0.85) / 0.3;
        bins[((f * 5.0) as usize).min(4)] += 1;
    }
    assert!(bins.iter().all(|&b| b >= 100), "{bins:?}");
    // The same citizen walks the same speed every time.
    assert_eq!(
        ped_speed_factor(Id::new(7, 3)),
        ped_speed_factor(Id::new(7, 3))
    );
}

fn car(trip: TripId, path: CarPath, hop: u16, s: f32, spawned: bool) -> Vehicle {
    Vehicle {
        trip,
        citizen: Id::new(0, 0),
        path,
        kind: 0,
        hop,
        in_conn: false,
        spawned,
        s,
        speed: 5.0,
        stuck_timer: 0,
        reroute: None,
    }
}

fn car_route(t: &Town) -> CarPath {
    let access = |b: BuildingId| access_of(&t.net, t.buildings.get(b).unwrap()).unwrap();
    crate::find_car_path(
        &t.graph,
        &crate::CongestionMap::default(),
        &access(t.home),
        &access(t.work),
    )
    .unwrap()
}

fn car_pose(t: &Town, runs: &ServiceRuns, v: Vehicle) -> Option<VehiclePose> {
    let mut agents = Agents::new();
    agents.insert_demo_vehicle(v);
    let f = agents.frame(&t.graph, &t.net, &t.geo, &t.trips, runs, &t.buildings);
    f.vehicles.first().copied()
}

/// §25.5 item 1 (cars): a car waiting to pull out is in the frame at its yard (progress 0), one
/// 8 m into its first lane is half way out, one 4 m before its goal is 0.75 in, one mid-route has
/// no yard.
#[test]
fn a_car_pulls_out_of_its_yard_and_into_the_next() {
    let t = town();
    let path = car_route(&t);
    let n = path.steps.len() as u16;
    let runs = ServiceRuns::default();
    let at = |hop, s, spawned| {
        car_pose(&t, &runs, car(t.trip, path.clone(), hop, s, spawned)).map(|v| v.portal)
    };
    let waiting = car_pose(&t, &runs, car(t.trip, path.clone(), 0, 0.0, false)).unwrap();
    assert_eq!(
        waiting.portal,
        Some(Portal {
            building: t.home,
            progress: 0.0,
            leaving: true
        })
    );
    assert_eq!(waiting.speed, 0.0);
    let out = at(0, path.start_s + 8.0, true).unwrap().unwrap();
    assert_eq!((out.building, out.leaving), (t.home, true));
    assert!((out.progress - 0.5).abs() < 1e-5, "{out:?}");
    let last_s = path.goal_s - 4.0;
    let inn = at(n, last_s, true).unwrap().unwrap();
    assert_eq!((inn.building, inn.leaving), (t.work, false));
    assert!((inn.progress - 0.75).abs() < 1e-5, "{inn:?}");
    assert_eq!(at(0, path.start_s + 30.0, true).unwrap(), None);
    if n > 1 {
        assert_eq!(at(1, 5.0, true).unwrap(), None);
    }
}

/// A run ends at a building's yard only when that end is a building; a waiting car without a
/// yard (to a stop, or a trip that is gone) stays out of the frame as before.
#[test]
fn runs_have_yards_only_at_buildings() {
    let t = town();
    let path = car_route(&t);
    let mut runs = ServiceRuns::default();
    let run = runs.start(cp_content::VehicleKind::Car, t.home, t.work);
    let trip = TripId::new(run, crate::SERVICE_GEN);
    let v = car_pose(
        &t,
        &runs,
        car(trip, path.clone(), 0, path.start_s + 4.0, true),
    )
    .unwrap();
    assert_eq!(
        v.portal.map(|p| (p.building, p.leaving)),
        Some((t.home, true))
    );
    // An unknown run: no yard; waiting, not drawn.
    let lost = TripId::new(run + 1, crate::SERVICE_GEN);
    assert_eq!(
        car_pose(
            &t,
            &runs,
            car(lost, path.clone(), 0, path.start_s + 4.0, true)
        )
        .unwrap()
        .portal,
        None
    );
    assert_eq!(
        car_pose(&t, &runs, car(lost, path.clone(), 0, 0.0, false)),
        None
    );
    // A parked car has none.
    let mut parked = car(t.trip, path, 0, 0.0, true);
    parked.kind |= KIND_PARKED;
    let p = car_pose(&t, &runs, parked);
    assert!(p.is_none_or(|p| p.portal.is_none()));
}
