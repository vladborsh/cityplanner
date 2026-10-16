//! The track sub-graph (T-161): trains find their way through a tunnel, cars never see it.

use bevy_math::Vec3;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph, SegmentId};

use crate::tests::{Sim, acc, car, car_path, sim};
use crate::*;

fn tunnel_seg(sim: &mut Sim, a: (f32, f32), b: (f32, f32)) -> SegmentId {
    let id = sim.road("metro_track");
    let def = sim.db.road(id).clone();
    sim.world
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            id,
            CubicBezier3::line(Vec3::new(a.0, -12.0, a.1), Vec3::new(b.0, -12.0, b.1)),
            Elevation::Tunnel,
        )
        .unwrap()
}

fn rail(from: Access, to: Access) -> PathRequest {
    PathRequest {
        mode: Mode::Rail,
        from,
        to,
    }
}

#[test]
fn trains_route_through_a_tunnel_and_cars_do_not_see_it() {
    let mut s = sim();
    let (t1, t2, t3) = (
        tunnel_seg(&mut s, (0.0, 0.0), (200.0, 0.0)),
        tunnel_seg(&mut s, (200.0, 0.0), (400.0, 0.0)),
        tunnel_seg(&mut s, (400.0, 0.0), (600.0, 0.0)),
    );
    let (r1, r2) = (
        s.seg((0.0, 100.0), (200.0, 100.0)),
        s.seg((200.0, 100.0), (400.0, 100.0)),
    );

    // On the right-hand lanes the train runs start to end through all three pieces.
    let PathResult::Rail(Ok(p)) = s.solve(rail(
        acc(t1, 50.0, Side::Right),
        acc(t3, 150.0, Side::Right),
    )) else {
        panic!("no rail route")
    };
    let lanes: Vec<_> = p.lanes().collect();
    assert_eq!(lanes.len(), 3);
    {
        let net = s.world.resource::<NetGraph>();
        let segs: Vec<_> = lanes
            .iter()
            .map(|l| net.lane(*l).unwrap().segment)
            .collect();
        assert_eq!(segs, [t1, t2, t3]);
        assert!(
            lanes
                .iter()
                .all(|l| net.lane(*l).unwrap().kind == cp_content::LaneKind::Rail)
        );
    }
    // 80 km/h: 500 m take about 22 s.
    assert!((15.0..40.0).contains(&p.cost), "{}", p.cost);
    // The left-hand lanes run the other way.
    let back = s.solve(rail(acc(t3, 100.0, Side::Left), acc(t1, 100.0, Side::Left)));
    assert!(matches!(back, PathResult::Rail(Ok(_))), "{back:?}");
    // Right-hand lanes do not run backwards.
    let wrong = s.solve(rail(
        acc(t3, 100.0, Side::Right),
        acc(t1, 100.0, Side::Right),
    ));
    assert!(matches!(wrong, PathResult::Rail(Err(NoPath))), "{wrong:?}");

    // Cars: no route on the metro, and a road route is unaffected.
    let c = car_path(s.solve(car(acc(t1, 50.0, Side::Right), acc(t3, 150.0, Side::Right))));
    assert_eq!(c, Err(NoPath));
    assert!(
        car_path(s.solve(car(acc(r1, 20.0, Side::Right), acc(r2, 150.0, Side::Right)))).is_ok()
    );
    // And a train has no route on a road.
    let on_road = s.solve(rail(
        acc(r1, 20.0, Side::Right),
        acc(r2, 150.0, Side::Right),
    ));
    assert!(
        matches!(on_road, PathResult::Rail(Err(NoPath))),
        "{on_road:?}"
    );
    // Nor do walkers use the track.
    let walk = s.solve(PathRequest {
        mode: Mode::Pedestrian,
        from: acc(t1, 50.0, Side::Right),
        to: acc(t2, 50.0, Side::Right),
    });
    assert!(
        matches!(walk, PathResult::Pedestrian(Err(NoPath))),
        "{walk:?}"
    );
}
