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
use crate::{CROSSING_ROOM_M, PathNetwork, PedestrianPose, Portal, find_ped_path};

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
    let f = agents.frame(&t.graph, &t.net, &t.geo, &t.trips, &t.buildings);
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
    let f = agents.frame(&t.graph, &t.net, &t.geo, &t.trips, &t.buildings);
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
    assert!(lo >= 200.0 / 1.61 - 1.0 && lo < 200.0 / 1.58, "{lo}");
    assert!(hi <= 200.0 / 1.19 + 1.0 && hi > 200.0 / 1.22, "{hi}");
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
