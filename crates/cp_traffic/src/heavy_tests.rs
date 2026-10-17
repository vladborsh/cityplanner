//! The heavy traffic ban (T-195): trucks go round a banned district when there is a way round,
//! cars do not, a truck bound for the district goes in, and repainting the district changes the
//! route.

use std::sync::Arc;

use cp_content::{ContentDb, PolicyKind, SimContent};
use cp_district::{CityPolicies, DistrictCommand, DistrictId, DistrictMap, Districts};

use crate::tests::{Sim, acc, car, car_path, sim};
use crate::*;

fn truck(from: Access, to: Access) -> PathRequest {
    PathRequest {
        mode: Mode::Truck,
        from,
        to,
    }
}

struct Diamond {
    s: Sim,
    stem_in: cp_net::SegmentId,
    stem_out: cp_net::SegmentId,
    x_n: cp_net::SegmentId,
    n_y: cp_net::SegmentId,
    x_s: cp_net::SegmentId,
}

/// X -> north branch (short, through the district) or south branch (longer) -> Y.
fn diamond() -> Diamond {
    let mut s = sim();
    let content =
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap();
    s.world.insert_resource(SimContent(Arc::new(content)));
    s.world.init_resource::<DistrictMap>();
    s.world.init_resource::<Districts>();
    s.world.init_resource::<CityPolicies>();
    let x_n = s.seg((1000.0, 1000.0), (1100.0, 900.0));
    let n_y = s.seg((1100.0, 900.0), (1200.0, 1000.0));
    let x_s = s.seg((1000.0, 1000.0), (1100.0, 1300.0));
    let _s_y = s.seg((1100.0, 1300.0), (1200.0, 1000.0));
    let stem_in = s.seg((900.0, 1000.0), (1000.0, 1000.0));
    let stem_out = s.seg((1200.0, 1000.0), (1300.0, 1000.0));
    Diamond {
        s,
        stem_in,
        stem_out,
        x_n,
        n_y,
        x_s,
    }
}

fn via(s: &Sim, p: &CarPath) -> Vec<cp_net::SegmentId> {
    let g = s.world.resource::<PathNetwork>().graph().clone();
    p.lanes().map(|l| g.lane_segment_for_test(l)).collect()
}

/// A district over the north branch (its midpoints are near (1050, 950) and (1150, 950)) with
/// the ban switched on; the south branch's midpoints are near y = 1150, outside it.
fn ban_north(s: &mut Sim) {
    let cells: Vec<_> = (32..=35)
        .flat_map(|c| (28..=30).map(move |r| (c, r)))
        .collect();
    for cmd in [
        DistrictCommand::Create {
            name: "Quiet".into(),
        },
        DistrictCommand::Paint {
            cells,
            district: DistrictId(1),
        },
        DistrictCommand::SetPolicy {
            district: Some(DistrictId(1)),
            policy: PolicyKind::HeavyTrafficBan,
            on: true,
        },
    ] {
        cp_district::apply(&mut s.world, &cmd).unwrap();
    }
}

#[test]
fn a_truck_between_points_outside_goes_round_the_district_and_a_car_goes_through() {
    let mut d = diamond();
    let (from, to) = (
        acc(d.stem_in, 10.0, Side::Right),
        acc(d.stem_out, 90.0, Side::Right),
    );
    // Without the policy everybody takes the short north branch.
    let p = car_path(d.s.solve(truck(from, to))).unwrap();
    assert!(via(&d.s, &p).contains(&d.x_n));

    ban_north(&mut d.s);
    let car_p = car_path(d.s.solve(car(from, to))).unwrap();
    assert!(
        via(&d.s, &car_p).contains(&d.x_n),
        "a car still takes the short way"
    );
    let truck_p = car_path(d.s.solve(truck(from, to))).unwrap();
    let segs = via(&d.s, &truck_p);
    assert!(
        segs.contains(&d.x_s) && !segs.contains(&d.x_n) && !segs.contains(&d.n_y),
        "{segs:?}"
    );
    // It is a cost, not a wall: the detour is the dearer trip in plain seconds.
    assert!(truck_p.cost > car_p.cost);

    // The same request again gives the same route (a pure function of the snapshot).
    let again = car_path(d.s.solve(truck(from, to))).unwrap();
    assert_eq!(again, truck_p);
}

#[test]
fn a_truck_bound_for_the_district_drives_in() {
    let mut d = diamond();
    ban_north(&mut d.s);
    let to = acc(d.n_y, 50.0, Side::Right);
    let p = car_path(d.s.solve(truck(acc(d.stem_in, 10.0, Side::Right), to))).unwrap();
    assert!(via(&d.s, &p).contains(&d.x_n), "{:?}", via(&d.s, &p));
}

#[test]
fn without_a_way_round_the_truck_drives_straight_through() {
    // Only the district road remains between the two stems: remove the south branch.
    let mut s = sim();
    let content =
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap();
    s.world.insert_resource(SimContent(Arc::new(content)));
    s.world.init_resource::<DistrictMap>();
    s.world.init_resource::<Districts>();
    s.world.init_resource::<CityPolicies>();
    let x_n = s.seg((1000.0, 1000.0), (1100.0, 900.0));
    s.seg((1100.0, 900.0), (1200.0, 1000.0));
    let stem_in = s.seg((900.0, 1000.0), (1000.0, 1000.0));
    let stem_out = s.seg((1200.0, 1000.0), (1300.0, 1000.0));
    ban_north(&mut s);
    let p = car_path(s.solve(truck(
        acc(stem_in, 10.0, Side::Right),
        acc(stem_out, 90.0, Side::Right),
    )))
    .unwrap();
    assert!(via(&s, &p).contains(&x_n));
}

#[test]
fn repainting_the_district_changes_the_route_and_so_does_a_city_wide_ban() {
    let mut d = diamond();
    let (from, to) = (
        acc(d.stem_in, 10.0, Side::Right),
        acc(d.stem_out, 90.0, Side::Right),
    );
    ban_north(&mut d.s);
    let p = car_path(d.s.solve(truck(from, to))).unwrap();
    assert!(via(&d.s, &p).contains(&d.x_s));
    // The district is erased from the map: the short way is open again.
    let cells: Vec<_> = (32..=35)
        .flat_map(|c| (28..=30).map(move |r| (c, r)))
        .collect();
    cp_district::apply(&mut d.s.world, &DistrictCommand::Erase { cells }).unwrap();
    let p = car_path(d.s.solve(truck(from, to))).unwrap();
    assert!(via(&d.s, &p).contains(&d.x_n));
    // The whole city bans trucks: every district is banned, but both ends are in the same
    // (no) district, so the plain short way costs the same as the long one's multiplier: a
    // uniform multiplier changes nothing and the short way stays.
    cp_district::apply(
        &mut d.s.world,
        &DistrictCommand::SetPolicy {
            district: None,
            policy: PolicyKind::HeavyTrafficBan,
            on: true,
        },
    )
    .unwrap();
    let p = car_path(d.s.solve(truck(from, to))).unwrap();
    assert!(via(&d.s, &p).contains(&d.x_n));
}
