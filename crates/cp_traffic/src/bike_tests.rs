//! Bicycles over the walking graph (T-322, §23.6 items 1 and 2).

use cp_net::{NetGraph, SegmentId};

use crate::tests::{Sim, acc, sim};
use crate::*;

/// The cyclist's speeds of the shipped balance.
fn speeds(s: &Sim) -> BikeSpeeds {
    BikeSpeeds::of_balance(s.db.balance())
}

fn choice(s: &Sim) -> BikeChoice {
    BikeChoice::of_balance(s.db.balance(), 1.0)
}

/// A 20 m lead street, a 600 m street of `road`, a 20 m lead street: the two leads' access
/// points, 640 m apart along the sidewalks.
fn corridor(road: &str) -> (Sim, SegmentId, SegmentId) {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1020.0, 1000.0));
    s.seg_with(road, (1020.0, 1000.0), (1620.0, 1000.0));
    let b = s.seg((1620.0, 1000.0), (1640.0, 1000.0));
    s.tick();
    s.tick();
    (s, a, b)
}

fn route(s: &Sim, a: SegmentId, b: SegmentId, class: WalkClass) -> PedPath {
    let net = s.world.resource::<PathNetwork>();
    find_walk_path(
        net.graph(),
        &acc(a, 10.0, Side::Right),
        &acc(b, 10.0, Side::Right),
        class,
    )
    .unwrap()
}

/// §23.6 item 1: a walker takes the same time beside a bike lane as beside a plain street and
/// never sets foot on the bike lane; a cyclist rides the bike lane about 2.2 times faster than
/// the sidewalk of the plain street, and the walker's search is `find_ped_path` itself.
#[test]
fn a_cyclist_rides_the_bike_lane_and_a_walker_never_does() {
    let (plain, pa, pb) = corridor("basic_2l");
    let (bike, ba, bb) = corridor("basic_2l_bike");
    let walk_plain = route(&plain, pa, pb, WalkClass::Walk);
    let walk_bike = route(&bike, ba, bb, WalkClass::Walk);
    // The same within the corners' metres (the narrower sidewalks sit further out).
    assert!(
        (walk_plain.cost - walk_bike.cost).abs() < walk_plain.cost * 0.03,
        "{} vs {}",
        walk_plain.cost,
        walk_bike.cost
    );
    let net = bike.world.resource::<NetGraph>();
    let kind = |l: cp_net::LaneId| net.lane(l).unwrap().kind;
    assert!(
        walk_bike
            .states
            .iter()
            .all(|&(l, _)| kind(l) == cp_content::LaneKind::Pedestrian)
    );
    {
        let g = bike.world.resource::<PathNetwork>();
        let p = find_ped_path(
            g.graph(),
            &acc(ba, 10.0, Side::Right),
            &acc(bb, 10.0, Side::Right),
        )
        .unwrap();
        assert_eq!(p, walk_bike);
    }
    let ride_plain = route(&plain, pa, pb, WalkClass::Bike(speeds(&plain)));
    let ride_bike = route(&bike, ba, bb, WalkClass::Bike(speeds(&bike)));
    assert!(
        ride_bike
            .states
            .iter()
            .any(|&(l, _)| kind(l) == cp_content::LaneKind::Bike),
        "rides the bike lane"
    );
    let ratio = ride_plain.cost / ride_bike.cost;
    assert!(
        (1.8..2.4).contains(&ratio),
        "{} / {} = {ratio}",
        ride_plain.cost,
        ride_bike.cost
    );
    // And riding beats walking: the sidewalk at 2.5 m/s against 1.4.
    assert!(ride_plain.cost < walk_plain.cost / 1.6);
}

/// A bike path between two roads joins their sidewalks for cyclists only.
#[test]
fn a_bike_path_is_a_ride_and_no_walk() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1100.0, 1000.0));
    let b = s.seg((1000.0, 1300.0), (1100.0, 1300.0));
    s.seg_with("bike_path", (1100.0, 1000.0), (1100.0, 1300.0));
    s.tick();
    s.tick();
    let net = s.world.resource::<PathNetwork>();
    let (from, to) = (acc(a, 50.0, Side::Right), acc(b, 50.0, Side::Left));
    assert!(find_ped_path(net.graph(), &from, &to).is_err());
    let ride = find_walk_path(net.graph(), &from, &to, WalkClass::Bike(speeds(&s))).unwrap();
    // 300 m of bike path at 5.5 m/s and the bits of sidewalk at either end.
    assert!(
        ride.cost > 300.0 / 5.5 && ride.cost < 300.0 / 5.5 + 60.0,
        "{}",
        ride.cost
    );
}

fn decide(s: &mut Sim, a: SegmentId, b: SegmentId, car: bool, bike: BikeChoice) -> PathResult {
    s.solve(PathRequest {
        mode: Mode::ByTimeBike {
            car_start_penalty_bits: if car { 90.0f32.to_bits() } else { u32::MAX },
            bike,
            fallback_car: car,
            drive_to: None,
            walk_tail_bits: 0,
        },
        from: acc(a, 10.0, Side::Right),
        to: acc(b, 10.0, Side::Right),
    })
}

/// §23.6 item 2: 640 m without a car is a ride; 8 km with a car on a free road the drive, with
/// the car route jammed the ride; past `bike_max_min` no ride at all.
#[test]
fn the_ride_wins_when_it_is_the_quickest_and_not_too_long() {
    let (mut s, a, b) = corridor("basic_2l_bike");
    let k = choice(&s);
    assert!(matches!(
        decide(&mut s, a, b, false, k),
        PathResult::Bike(Ok(_))
    ));
    // A long way: 8 km of street with bike lanes.
    let mut long = sim();
    let la = long.seg((1000.0, 1000.0), (1020.0, 1000.0));
    let m1 = long.seg_with("basic_2l_bike", (1020.0, 1000.0), (5020.0, 1000.0));
    let m2 = long.seg_with("basic_2l_bike", (5020.0, 1000.0), (9020.0, 1000.0));
    let lb = long.seg((9020.0, 1000.0), (9040.0, 1000.0));
    long.tick();
    long.tick();
    let k = choice(&long);
    // With a car on a free road the drive wins.
    assert!(matches!(
        decide(&mut long, la, lb, true, k),
        PathResult::Car(Ok(_))
    ));
    // The same car on a jammed road (half an hour lost on each half): the ride.
    long.set_congestion(&[(m1, 1800.0), (m2, 1800.0)]);
    assert!(matches!(
        decide(&mut long, la, lb, true, k),
        PathResult::Bike(Ok(_))
    ));
    long.set_congestion(&[]);
    // Without one: 8 km at 5.5 m/s is about 24 min, a ride.
    assert!(matches!(
        decide(&mut long, la, lb, false, k),
        PathResult::Bike(Ok(_))
    ));
    // At a ceiling of 10 min it is too long to ride: walk.
    let short_max = BikeChoice {
        max_s_bits: 600.0f32.to_bits(),
        ..k
    };
    assert!(matches!(
        decide(&mut long, la, lb, false, short_max),
        PathResult::Pedestrian(Ok(_))
    ));
}
