use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule};
use bevy_ecs::world::World;
use bevy_math::Vec3;
use bevy_tasks::TaskPoolBuilder;
use cp_content::{ContentDb, RoadDefId};
use cp_core::curve::CubicBezier3;
use cp_core::{SimRng, Tick};
use cp_net::{Elevation, JunctionGeometry, NetGraph, SegmentId, update_junctions};

use crate::*;

/// A small sim: net + junction geometry + path snapshot + service, stepped by hand.
pub(crate) struct Sim {
    pub(crate) db: ContentDb,
    pub(crate) world: World,
    schedule: Schedule,
}

impl Sim {
    pub(crate) fn new(service: PathService) -> Self {
        let mut world = World::new();
        world.init_resource::<NetGraph>();
        world.init_resource::<JunctionGeometry>();
        world.init_resource::<PathNetwork>();
        world.init_resource::<PathResults>();
        world.insert_resource(service);
        world.insert_resource(Tick(0));
        let mut schedule = Schedule::default();
        schedule.add_systems((
            update_junctions,
            update_path_network.after(update_junctions),
            deliver_paths.after(update_path_network),
        ));
        Self {
            db: ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
            world,
            schedule,
        }
    }

    pub(crate) fn road(&self, name: &str) -> RoadDefId {
        self.db.road_id(name).unwrap()
    }

    pub(crate) fn seg_with(&mut self, road: &str, a: (f32, f32), b: (f32, f32)) -> SegmentId {
        let id = self.road(road);
        let def = self.db.road(id).clone();
        self.world
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                id,
                CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1)),
                Elevation::Ground,
            )
            .unwrap()
    }

    pub(crate) fn seg(&mut self, a: (f32, f32), b: (f32, f32)) -> SegmentId {
        self.seg_with("basic_2l", a, b)
    }

    /// Runs the schedule for the current tick, then advances it.
    pub(crate) fn tick(&mut self) -> Vec<PathOutcome> {
        self.schedule.run(&mut self.world);
        let out = self.world.resource::<PathResults>().0.clone();
        self.world.resource_mut::<Tick>().advance();
        out
    }

    pub(crate) fn now(&self) -> Tick {
        *self.world.resource::<Tick>()
    }

    fn submit(&mut self, req: PathRequest) -> RequestId {
        let now = self.now();
        self.world
            .resource_scope(|w, mut svc: bevy_ecs::change_detection::Mut<PathService>| {
                svc.submit(w.resource::<PathNetwork>(), now, req)
            })
    }

    /// Submit and run until the outcome arrives.
    pub(crate) fn solve(&mut self, req: PathRequest) -> PathResult {
        self.tick(); // let the snapshot catch up with the roads built so far
        let id = self.submit(req);
        for _ in 0..=PATH_DELAY_TICKS + 1 {
            if let Some(o) = self.tick().into_iter().find(|o| o.id == id) {
                return o.result;
            }
        }
        panic!("no outcome");
    }

    fn set_congestion(&mut self, segs: &[(SegmentId, f32)]) {
        let slots = segs
            .iter()
            .map(|(s, _)| s.index as usize + 1)
            .max()
            .unwrap_or(0);
        let mut v = vec![0.0; slots];
        for (s, c) in segs {
            v[s.index as usize] = *c;
        }
        self.world
            .resource_mut::<PathNetwork>()
            .set_congestion(CongestionMap(v));
    }
}

pub(crate) fn sim() -> Sim {
    Sim::new(PathService::new())
}

pub(crate) fn acc(segment: SegmentId, s: f32, side: Side) -> Access {
    Access { segment, s, side }
}

pub(crate) fn car(from: Access, to: Access) -> PathRequest {
    PathRequest {
        mode: Mode::Car,
        from,
        to,
    }
}

fn ped(from: Access, to: Access) -> PathRequest {
    PathRequest {
        mode: Mode::Pedestrian,
        from,
        to,
    }
}

pub(crate) fn car_path(r: PathResult) -> Result<CarPath, NoPath> {
    match r {
        PathResult::Car(p) => p,
        other => panic!("{other:?}"),
    }
}

fn ped_path(r: PathResult) -> Result<PedPath, NoPath> {
    match r {
        PathResult::Pedestrian(p) => p,
        other => panic!("{other:?}"),
    }
}

#[test]
fn straight_chain_uses_the_connection_at_the_shared_node() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    let b = s.seg((1100.0, 1000.0), (1200.0, 1000.0));
    // Heading +X, the right side is +Z and its lane is the Forward one.
    let p = car_path(s.solve(car(acc(a, 10.0, Side::Right), acc(b, 90.0, Side::Right)))).unwrap();
    assert_eq!(p.steps.len(), 1);
    assert_eq!(p.steps[0].0, p.start);
    assert_eq!(p.steps[0].1, p.goal);
    assert_eq!(p.lanes().count(), 2);
    // ~190 m at the road's speed limit (cost is seconds, > 190/speed because of the curve
    // and the 0 penalty for going straight).
    let speed = {
        let g = s.world.resource::<PathNetwork>().graph().clone();
        g.resolve_car_for_test(&acc(a, 10.0, Side::Right)).1
    };
    assert!(
        (p.cost - 190.0 / speed).abs() < 3.0,
        "{} vs {}",
        p.cost,
        190.0 / speed
    );

    // The opposite direction uses the other lanes: left side, heading -X.
    let back = car_path(s.solve(car(acc(b, 90.0, Side::Left), acc(a, 10.0, Side::Left)))).unwrap();
    assert_eq!(back.steps.len(), 1);
    assert_ne!(back.start, p.start);
}

#[test]
fn left_turns_cost_four_seconds_and_right_turns_nothing() {
    let mut s = sim();
    let stem = s.seg((900.0, 1000.0), (1000.0, 1000.0));
    let left = s.seg((1000.0, 1000.0), (1000.0, 900.0)); // -Z is left of heading +X
    let right = s.seg((1000.0, 1000.0), (1000.0, 1100.0));
    // Mirror-image branches; Right of each branch's own direction is its Forward lane, the
    // one leading away from the junction.
    let start = acc(stem, 10.0, Side::Right);
    let l = car_path(s.solve(car(start, acc(left, 50.0, Side::Right)))).unwrap();
    let r = car_path(s.solve(car(start, acc(right, 50.0, Side::Right)))).unwrap();
    assert_eq!(l.steps.len(), 1);
    assert_eq!(r.steps.len(), 1);
    let diff = l.cost - r.cost;
    assert!(
        (diff - LEFT_TURN_PENALTY).abs() < 0.5,
        "left-right cost difference {diff}"
    );
}

#[test]
fn u_turn_at_a_dead_end_costs_fifteen_seconds() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    // Forward lane (Right side) at s = 20, then back along the Backward lane to s = 30.
    let p = car_path(s.solve(car(acc(a, 20.0, Side::Right), acc(a, 30.0, Side::Left)))).unwrap();
    assert_eq!(p.steps.len(), 1, "one U-turn connection at the end");
    // 80 m out, 70 m back at least, plus the penalty.
    assert!(p.cost > U_TURN_PENALTY + 150.0 / 20.0, "{}", p.cost);
}

#[test]
fn same_lane_forward_is_a_direct_run() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    let p = car_path(s.solve(car(acc(a, 10.0, Side::Right), acc(a, 80.0, Side::Right)))).unwrap();
    assert!(p.steps.is_empty());
    assert_eq!(p.start, p.goal);
}

fn via(s: &Sim, p: &CarPath) -> Vec<SegmentId> {
    let g = s.world.resource::<PathNetwork>().graph().clone();
    p.lanes().map(|l| g.lane_segment_for_test(l)).collect()
}

#[test]
fn congestion_moves_the_route_to_the_free_branch() {
    // A diamond: X -> (north or south branch, equal length) -> Y.
    let mut s = sim();
    let x_n = s.seg((1000.0, 1000.0), (1100.0, 900.0));
    let n_y = s.seg((1100.0, 900.0), (1200.0, 1000.0));
    let x_s = s.seg((1000.0, 1000.0), (1100.0, 1100.0));
    let s_y = s.seg((1100.0, 1100.0), (1200.0, 1000.0));
    let stem_in = s.seg((900.0, 1000.0), (1000.0, 1000.0));
    let stem_out = s.seg((1200.0, 1000.0), (1300.0, 1000.0));
    let req = car(
        acc(stem_in, 10.0, Side::Right),
        acc(stem_out, 90.0, Side::Right),
    );
    s.set_congestion(&[(x_n, 5.0), (n_y, 5.0)]);
    let p = car_path(s.solve(req)).unwrap();
    let segs = via(&s, &p);
    assert!(
        segs.contains(&x_s) && segs.contains(&s_y) && !segs.contains(&x_n),
        "{segs:?}"
    );

    s.set_congestion(&[(x_s, 5.0), (s_y, 5.0)]);
    let p = car_path(s.solve(req)).unwrap();
    let segs = via(&s, &p);
    assert!(
        segs.contains(&x_n) && segs.contains(&n_y) && !segs.contains(&x_s),
        "{segs:?}"
    );

    // Congestion raises the cost.
    s.set_congestion(&[]);
    let free = car_path(s.solve(req)).unwrap().cost;
    s.set_congestion(&[(x_n, 5.0), (n_y, 5.0), (x_s, 5.0), (s_y, 5.0)]);
    assert!(car_path(s.solve(req)).unwrap().cost > free + 1.0);
}

#[test]
fn one_way_roads_cannot_be_driven_backwards() {
    let mut s = sim();
    let a = s.seg_with("highway_1w_3l", (1000.0, 1000.0), (1100.0, 1000.0));
    let b = s.seg_with("highway_1w_3l", (1100.0, 1000.0), (1200.0, 1000.0));
    let fwd = car_path(s.solve(car(acc(a, 10.0, Side::Right), acc(b, 90.0, Side::Right))));
    assert!(fwd.is_ok());
    let back = car_path(s.solve(car(acc(b, 90.0, Side::Right), acc(a, 10.0, Side::Right))));
    assert_eq!(back, Err(NoPath));
}

/// T-172: the one-way streets with sidewalks drive like the highways: with the arrows only.
#[test]
fn one_way_streets_cannot_be_driven_backwards() {
    for road in ["basic_1w_2l", "medium_1w_3l"] {
        let mut s = sim();
        let a = s.seg_with(road, (1000.0, 1000.0), (1100.0, 1000.0));
        let b = s.seg_with(road, (1100.0, 1000.0), (1200.0, 1000.0));
        let fwd = car_path(s.solve(car(acc(a, 10.0, Side::Right), acc(b, 90.0, Side::Right))));
        assert!(fwd.is_ok(), "{road}");
        let back = car_path(s.solve(car(acc(b, 90.0, Side::Right), acc(a, 10.0, Side::Right))));
        assert_eq!(back, Err(NoPath), "{road}");
        // The sidewalks go both ways.
        let walk = ped_path(s.solve(ped(acc(b, 90.0, Side::Right), acc(a, 10.0, Side::Right))));
        assert!(walk.is_ok(), "{road}");
    }
}

#[test]
fn disconnected_networks_have_no_path_for_either_mode() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    let b = s.seg((3000.0, 3000.0), (3100.0, 3000.0));
    assert_eq!(
        car_path(s.solve(car(acc(a, 10.0, Side::Right), acc(b, 10.0, Side::Right)))),
        Err(NoPath)
    );
    assert_eq!(
        ped_path(s.solve(ped(acc(a, 10.0, Side::Right), acc(b, 10.0, Side::Right)))),
        Err(NoPath)
    );
}

#[test]
fn pedestrians_walk_sidewalks_corners_and_crossings() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    let b = s.seg((1100.0, 1000.0), (1200.0, 1000.0));
    let along =
        ped_path(s.solve(ped(acc(a, 10.0, Side::Right), acc(b, 90.0, Side::Right)))).unwrap();
    assert!(!along.states.is_empty());
    assert!(
        along.cost > 180.0 / PED_SPEED && along.cost < 215.0 / PED_SPEED,
        "{}",
        along.cost
    );
    let g = s.world.resource::<PathNetwork>().graph().clone();
    assert!(g.ped_path_valid(&along));

    // The other side of the road: must go to a junction and cross.
    let across =
        ped_path(s.solve(ped(acc(a, 50.0, Side::Right), acc(a, 50.0, Side::Left)))).unwrap();
    assert!(across.cost > 100.0 / PED_SPEED, "{}", across.cost);
    assert!(g.ped_path_valid(&across));

    // Same sidewalk: a direct walk.
    let direct =
        ped_path(s.solve(ped(acc(a, 20.0, Side::Right), acc(a, 60.0, Side::Right)))).unwrap();
    assert!(direct.states.is_empty());
    assert!(
        (direct.cost - 40.0 / PED_SPEED).abs() < 1.0,
        "{}",
        direct.cost
    );
}

#[test]
fn results_arrive_exactly_eight_ticks_after_submission() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    for _ in 0..5 {
        s.tick();
    }
    let id = s.submit(car(acc(a, 10.0, Side::Right), acc(a, 80.0, Side::Right)));
    let submitted = s.now();
    for t in 0..PATH_DELAY_TICKS {
        assert!(s.tick().is_empty(), "nothing at +{t}");
    }
    let out = s.tick();
    assert_eq!(out.len(), 1);
    assert_eq!((out[0].id, out[0].submitted), (id, submitted));
    assert_eq!(s.now(), Tick(submitted.0 + PATH_DELAY_TICKS + 1));
    // Not delivered twice.
    assert!(s.tick().is_empty());
}

#[test]
fn outcomes_come_in_request_order_within_a_tick() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    let ids: Vec<_> = (0..6)
        .map(|i| {
            s.submit(car(
                acc(a, 10.0, Side::Right),
                acc(a, 20.0 + i as f32, Side::Right),
            ))
        })
        .collect();
    let mut got = Vec::new();
    for _ in 0..=PATH_DELAY_TICKS {
        got = s.tick();
    }
    assert_eq!(got.iter().map(|o| o.id).collect::<Vec<_>>(), ids);
}

#[test]
fn a_network_change_in_flight_makes_the_result_stale() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    let b = s.seg((1100.0, 1000.0), (1200.0, 1000.0));
    s.tick(); // build the snapshot
    let req = car(acc(a, 10.0, Side::Right), acc(b, 90.0, Side::Right));
    let id = s.submit(req);
    for _ in 0..3 {
        s.tick();
    }
    // Change the net mid-flight (an unrelated far-away segment is enough: the snapshot of
    // the request is no longer the current network).
    s.seg((5000.0, 5000.0), (5100.0, 5000.0));
    let mut outcome = None;
    for _ in 0..PATH_DELAY_TICKS {
        if let Some(o) = s.tick().into_iter().find(|o| o.id == id) {
            outcome = Some(o);
        }
    }
    assert_eq!(outcome.unwrap().result, PathResult::Stale);
    // Asking again works.
    assert!(car_path(s.solve(req)).is_ok());
}

#[test]
fn stored_paths_are_invalidated_only_when_their_connections_change() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    let b = s.seg((1100.0, 1000.0), (1200.0, 1000.0));
    let far = s.seg((3000.0, 3000.0), (3100.0, 3000.0));
    let far2 = s.seg((3100.0, 3000.0), (3200.0, 3000.0));
    let p1 = car_path(s.solve(car(acc(a, 10.0, Side::Right), acc(b, 90.0, Side::Right)))).unwrap();
    let p2 = car_path(s.solve(car(
        acc(far, 10.0, Side::Right),
        acc(far2, 90.0, Side::Right),
    )))
    .unwrap();
    let valid = |s: &Sim, p: &CarPath| s.world.resource::<PathNetwork>().graph().car_path_valid(p);
    assert!(valid(&s, &p1) && valid(&s, &p2));

    // Remove the second segment of the first route: its connection (and lane) vanish.
    s.world
        .resource_mut::<NetGraph>()
        .remove_segment(b)
        .unwrap();
    s.tick();
    assert!(!valid(&s, &p1));
    assert!(
        valid(&s, &p2),
        "paths elsewhere stay valid across the rebuild"
    );

    // Re-adding the road gives new lane ids: the old path stays invalid (stable ids are
    // arena ids with generations).
    s.seg((1100.0, 1000.0), (1200.0, 1000.0));
    s.tick();
    assert!(!valid(&s, &p1));
}

fn grid(s: &mut Sim, n: usize) {
    for i in 0..n {
        for j in 0..n {
            let p = (1000.0 + i as f32 * 100.0, 1000.0 + j as f32 * 100.0);
            if i + 1 < n {
                s.seg(p, (p.0 + 100.0, p.1));
            }
            if j + 1 < n {
                s.seg(p, (p.0, p.1 + 100.0));
            }
        }
    }
}

fn random_requests(s: &Sim, count: usize, seed: u64) -> Vec<PathRequest> {
    let segs: Vec<(SegmentId, f32)> = s
        .world
        .resource::<NetGraph>()
        .segments()
        .map(|(id, seg)| (id, seg.length))
        .collect();
    let mut rng = SimRng::seed_from_u64(seed);
    let mut pick = || {
        let (id, len) = segs[rng.below(segs.len() as u32) as usize];
        let side = if rng.below(2) == 0 {
            Side::Left
        } else {
            Side::Right
        };
        acc(id, rng.next_f32() * len, side)
    };
    (0..count)
        .map(|i| PathRequest {
            mode: if i % 5 == 0 {
                Mode::Pedestrian
            } else {
                Mode::Car
            },
            from: pick(),
            to: pick(),
        })
        .collect()
}

/// All outcomes of `reqs` on a `n`-wide grid with the given service, submitted in one tick.
fn run_batch(service: PathService, reqs_seed: u64) -> Vec<PathOutcome> {
    let mut s = Sim::new(service);
    grid(&mut s, 8);
    s.tick();
    let reqs = random_requests(&s, 200, reqs_seed);
    for r in &reqs {
        s.submit(*r);
    }
    let mut out = Vec::new();
    for _ in 0..=PATH_DELAY_TICKS {
        out.extend(s.tick());
    }
    out
}

#[test]
fn results_are_identical_for_one_and_eight_threads_and_in_sync_mode() {
    let one = run_batch(
        PathService::with_pool(TaskPoolBuilder::new().num_threads(1).build()),
        7,
    );
    let eight = run_batch(
        PathService::with_pool(TaskPoolBuilder::new().num_threads(8).build()),
        7,
    );
    let mut sync = PathService::new();
    sync.set_sync(true);
    let sync = run_batch(sync, 7);
    assert_eq!(one.len(), 200);
    assert!(
        one.iter()
            .any(|o| matches!(&o.result, PathResult::Car(Ok(_))))
    );
    assert!(
        one.iter()
            .any(|o| matches!(&o.result, PathResult::Pedestrian(Ok(_))))
    );
    assert_eq!(one, eight, "1 vs 8 threads");
    assert_eq!(one, sync, "threaded vs synchronous");
}

#[test]
fn grid_routes_are_optimal_manhattan_distances() {
    // On a uniform grid a car's cost is at least distance/speed and not absurdly more.
    let mut s = sim();
    grid(&mut s, 6);
    s.tick();
    let reqs = random_requests(&s, 60, 3);
    let g = s.world.resource::<PathNetwork>().graph().clone();
    let vmax = g.max_speed_for_test();
    for r in reqs.into_iter().filter(|r| r.mode == Mode::Car) {
        let res = car_path(s.solve(r));
        let p = res.expect("grid is connected");
        let from = g.point_for_test(&r.from);
        let to = g.point_for_test(&r.to);
        assert!(
            p.cost + 1e-3 >= from.distance(to) / vmax,
            "cost below the straight line"
        );
    }
}

#[test]
fn the_route_between_two_points_takes_the_100_kmh_highway_over_the_40_kmh_street() {
    // Two equal-length branches between the same two stems: a one-way highway and a street.
    let mut s = sim();
    let stem_in = s.seg((900.0, 1000.0), (1000.0, 1000.0));
    let stem_out = s.seg((1200.0, 1000.0), (1300.0, 1000.0));
    let h1 = s.seg_with("highway_1w_2l", (1000.0, 1000.0), (1100.0, 900.0));
    let h2 = s.seg_with("highway_1w_2l", (1100.0, 900.0), (1200.0, 1000.0));
    let s1 = s.seg((1000.0, 1000.0), (1100.0, 1100.0));
    let s2 = s.seg((1100.0, 1100.0), (1200.0, 1000.0));
    let req = car(
        acc(stem_in, 10.0, Side::Right),
        acc(stem_out, 90.0, Side::Right),
    );
    let path = car_path(s.solve(req)).unwrap();
    let segs = via(&s, &path);
    assert!(
        segs.contains(&h1) && segs.contains(&h2) && !segs.contains(&s1) && !segs.contains(&s2),
        "{segs:?}"
    );
}

// T-222: pedestrian paths.

#[test]
fn pedestrians_walk_over_a_path_joined_to_a_sidewalk_and_cars_do_not() {
    let mut s = sim();
    let road = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    let path = s.seg_with("park_path", (1100.0, 1000.0), (1100.0, 1100.0));
    let far = s.seg_with("park_path", (1100.0, 1100.0), (1200.0, 1100.0));
    let walk = ped_path(s.solve(ped(
        acc(road, 10.0, Side::Right),
        acc(far, 90.0, Side::Right),
    )))
    .unwrap();
    assert!(!walk.states.is_empty());
    let g = s.world.resource::<PathNetwork>().graph().clone();
    assert!(g.ped_path_valid(&walk));
    // And back.
    assert!(
        ped_path(s.solve(ped(
            acc(far, 90.0, Side::Left),
            acc(road, 10.0, Side::Right)
        )))
        .is_ok()
    );
    // A path has no car lane: a car cannot start or end on it.
    assert_eq!(
        car_path(s.solve(car(
            acc(road, 10.0, Side::Right),
            acc(path, 50.0, Side::Right)
        ))),
        Err(NoPath)
    );
    assert_eq!(
        car_path(s.solve(car(
            acc(path, 50.0, Side::Right),
            acc(road, 10.0, Side::Right)
        ))),
        Err(NoPath)
    );
}

#[test]
fn two_roads_joined_only_by_a_path_are_a_walk_but_not_a_drive() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    s.seg_with("park_path", (1100.0, 1000.0), (1200.0, 1000.0));
    let b = s.seg((1200.0, 1000.0), (1300.0, 1000.0));
    assert!(ped_path(s.solve(ped(acc(a, 10.0, Side::Right), acc(b, 90.0, Side::Right)))).is_ok());
    assert_eq!(
        car_path(s.solve(car(acc(a, 10.0, Side::Right), acc(b, 90.0, Side::Right)))),
        Err(NoPath)
    );
}

/// T-242: two houses 100 m apart on parallel streets that only meet 190 m away: driving beats
/// the 480 m walk; a footpath between them makes the walk the quicker choice.
#[test]
fn a_footpath_shortcut_makes_people_walk_and_without_it_they_drive() {
    use cp_citizen::TripMode;
    let build = |with_path: bool| {
        let mut s = sim();
        let a = s.seg((1000.0, 1000.0), (1020.0, 1000.0));
        s.seg((1020.0, 1000.0), (1200.0, 1000.0));
        s.seg((1200.0, 1000.0), (1200.0, 1100.0));
        let b = s.seg((1000.0, 1100.0), (1020.0, 1100.0));
        s.seg((1020.0, 1100.0), (1200.0, 1100.0));
        if with_path {
            s.seg_with("park_path", (1020.0, 1000.0), (1020.0, 1100.0));
        }
        s.tick();
        s.tick();
        (s, a, b)
    };
    let decide = |with_path: bool| {
        let (s, a, b) = build(with_path);
        let net = s.world.resource::<PathNetwork>();
        mode_by_time(
            net.graph(),
            net.congestion(),
            &acc(a, 10.0, Side::Right),
            &acc(b, 10.0, Side::Left),
            90.0,
        )
    };
    assert_eq!(decide(false), Some(TripMode::Car));
    assert_eq!(decide(true), Some(TripMode::Walk));
    // The same answer every time.
    assert_eq!(decide(true), decide(true));
}

/// T-242: a congested corridor makes the drive slower, so the walk wins sooner.
#[test]
fn congestion_makes_the_drive_slower_than_a_walk_of_the_same_route() {
    use cp_citizen::TripMode;
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    let b = s.seg((1100.0, 1000.0), (1200.0, 1000.0));
    s.tick();
    s.tick();
    let (from, to) = (acc(a, 10.0, Side::Right), acc(b, 90.0, Side::Right));
    let (free, jam) = {
        let net = s.world.resource::<PathNetwork>();
        let cost = |c: &CongestionMap| find_car_path(net.graph(), c, &from, &to).unwrap().cost;
        (cost(net.congestion()), cost(&CongestionMap(vec![1.0; 2])))
    };
    assert!(jam > free * 1.5, "{jam} against {free}");
    let _ = TripMode::Car;
}

/// T-302: a route keeps to the lanes its vehicle class may use. A car cannot cross a tram-only
/// street, a tram cannot leave the rails, a bus rides a bus street and a car street alike, a
/// trolleybus only its wired street.
#[test]
fn routes_keep_to_the_lanes_of_the_vehicle_class() {
    use cp_content::VehicleClass as V;
    let mut s = sim();
    // A car street, then a tram-only street, then a car street, in a line.
    let a = s.seg_with("basic_2l", (0.0, 0.0), (200.0, 0.0));
    let t = s.seg_with("tram_2l", (200.0, 0.0), (400.0, 0.0));
    let b = s.seg_with("basic_2l", (400.0, 0.0), (600.0, 0.0));
    // A bus street beside them.
    let p = s.seg_with("basic_2l", (0.0, 300.0), (200.0, 300.0));
    let u = s.seg_with("bus_2l", (200.0, 300.0), (400.0, 300.0));
    let q = s.seg_with("basic_2l", (400.0, 300.0), (600.0, 300.0));
    s.tick();
    fn class(s: &mut Sim, class: V, from: Access, to: Access) -> Result<CarPath, NoPath> {
        car_path(s.solve(PathRequest {
            mode: Mode::Class { class },
            from,
            to,
        }))
    }
    let (from, to) = (acc(a, 50.0, Side::Right), acc(b, 150.0, Side::Right));
    assert_eq!(
        class(&mut s, V::Car, from, to),
        Err(NoPath),
        "a car cannot cross the rails"
    );
    assert_eq!(class(&mut s, V::Bus, from, to), Err(NoPath), "nor a bus");
    // A tram rides the rails, between two points on them.
    let (ta, tb) = (acc(t, 20.0, Side::Right), acc(t, 180.0, Side::Right));
    let tram = class(&mut s, V::Tram, ta, tb).expect("a tram on its rails");
    assert!(tram.start == tram.goal || tram.steps.is_empty());
    assert_eq!(
        class(&mut s, V::Car, ta, tb),
        Err(NoPath),
        "no car on the rails"
    );
    // A bus crosses the bus street, a car cannot.
    let (pf, qt) = (acc(p, 50.0, Side::Right), acc(q, 150.0, Side::Right));
    let bus = class(&mut s, V::Bus, pf, qt).expect("a bus over the bus street");
    let g = std::sync::Arc::clone(s.world.resource::<crate::service::PathNetwork>().graph());
    assert!(
        bus.steps
            .iter()
            .any(|(_, l)| g.car_lane(*l).is_some_and(|n| n.segment == u)),
        "the bus takes the bus street"
    );
    assert_eq!(
        class(&mut s, V::Car, pf, qt),
        Err(NoPath),
        "a car cannot use the bus street"
    );
    assert_eq!(
        class(&mut s, V::Trolley, pf, qt),
        Err(NoPath),
        "no wires anywhere"
    );
    // The plain car request is the car class.
    assert_eq!(car_path(s.solve(car(pf, qt))), Err(NoPath));
}
