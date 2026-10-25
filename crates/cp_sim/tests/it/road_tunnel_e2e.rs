//! T-173: road tunnels. A street dives under the river of `river_valley` with a ramp at each end
//! (portals on dry ground, at most the road's 12 %), cars can drive through, a tunnel too short to
//! reach its depth is refused, and the tunnel part is never zoned.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::CommandOutcome;
use cp_core::Cost;
use cp_core::treasury::Treasury;
use cp_net::{Elevation, ElevationMode, NetCommand, NetGraph, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_zone::ZoneGrid;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn sim() -> (SimRunner, Arc<ContentDb>, Arc<Terrain>) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/river_valley")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::clone(&t));
    sim.set_content(Arc::clone(&db));
    sim.world_mut()
        .insert_resource(TileOwnership::m1_around(t.meta().start_tile));
    sim.world_mut().resource_mut::<Treasury>().balance = Cost(1_000_000_000);
    sim.tick_once();
    (sim, db, t)
}

fn build(
    sim: &mut SimRunner,
    db: &ContentDb,
    road: &str,
    from: Vec2,
    to: Vec2,
    elevation: ElevationMode,
) -> CommandOutcome {
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: db.road_id(road).unwrap(),
            path: RoadPath::Straight { from, to },
            elevation,
        }),
        sim.tick(),
    );
    sim.tick_once();
    let mut out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    sim.tick_once();
    out.pop().expect("an outcome")
}

/// An east-west line across the river: dry for 150 m at both ends, water between.
fn crossing(t: &Terrain) -> (Vec2, Vec2) {
    let (lo, hi) = (4300.0f32, 6700.0f32);
    for k in 0..60 {
        let z = 4300.0 + k as f32 * 25.0;
        let wet: Vec<f32> = (0..)
            .map(|i| lo + i as f32 * 4.0)
            .take_while(|x| *x <= hi)
            .filter(|x| t.is_water(Vec2::new(*x, z)))
            .collect();
        let (Some(&first), Some(&last)) = (wet.first(), wet.last()) else {
            continue;
        };
        let (a, b) = (Vec2::new(first - 160.0, z), Vec2::new(last + 160.0, z));
        if a.x > lo && b.x < hi && (b.x - a.x) < 700.0 {
            return (a, b);
        }
    }
    panic!("no crossing found");
}

#[test]
fn a_street_dives_under_the_river_with_a_ramp_at_each_end_and_cars_drive_through() {
    let (mut sim, db, t) = sim();
    let (a, b) = crossing(&t);
    let out = build(&mut sim, &db, "basic_2l", a, b, ElevationMode::Tunnel(12.0));
    assert!(out.result.is_ok(), "{:?}", out.result);
    let net = sim.world().resource::<NetGraph>();
    let segs: Vec<_> = net.segments().collect();
    assert!(segs.len() >= 4, "{} pieces", segs.len());
    assert!(segs.iter().all(|(_, s)| s.elevation == Elevation::Tunnel));
    // The ends are portals on the surface, joinable by ground roads; the middle is deep.
    let ground_nodes: Vec<_> = net
        .nodes()
        .filter(|(_, n)| n.elevation == Elevation::Ground)
        .collect();
    assert_eq!(ground_nodes.len(), 2, "two portals");
    for (_, n) in &ground_nodes {
        let h = t.height(Vec2::new(n.pos.x, n.pos.z));
        assert!((n.pos.y - h).abs() < 0.1, "a portal is on the ground");
    }
    let deepest = net
        .nodes()
        .map(|(_, n)| t.height(Vec2::new(n.pos.x, n.pos.z)) - n.pos.y)
        .fold(0.0f32, f32::max);
    assert!(deepest > 11.0, "the middle reaches its depth: {deepest}");
    // No piece is steeper than the street's 12 %.
    for (_, s) in &segs {
        let run = Vec2::new(s.curve.p3.x - s.curve.p0.x, s.curve.p3.z - s.curve.p0.z).length();
        assert!(((s.curve.p3.y - s.curve.p0.y) / run).abs() <= 0.12 + 1e-3);
    }
    // Cars drive through: a path exists from one end to the other.
    let first = segs
        .iter()
        .min_by(|x, y| x.1.curve.p0.x.total_cmp(&y.1.curve.p0.x))
        .unwrap()
        .0;
    let last = segs
        .iter()
        .max_by(|x, y| x.1.curve.p3.x.total_cmp(&y.1.curve.p3.x))
        .unwrap()
        .0;
    let pnet = sim.world().resource::<cp_traffic::PathNetwork>();
    let at = |segment, s| cp_traffic::Access {
        segment,
        s,
        side: cp_traffic::Side::Right,
    };
    let there = cp_traffic::find_car_path(
        pnet.graph(),
        &Default::default(),
        &at(first, 4.0),
        &at(last, net.segment(last).unwrap().length - 4.0),
    );
    assert!(there.is_ok(), "no way through the tunnel");
    // The tunnel part is not zoned.
    let zones = sim.world().resource::<ZoneGrid>();
    assert!(
        segs.iter()
            .all(|(id, _)| zones.blocks_of(*id) == [None, None])
    );
}

#[test]
fn a_tunnel_too_short_to_reach_its_depth_is_refused_and_a_highway_needs_more_run() {
    let (mut sim, db, t) = sim();
    // Dry ground west of the river: 250 m ending where the crossing starts.
    let a = crossing(&t).0 - Vec2::new(250.0, 0.0);
    // 120 m cannot dip 12 m at 12 %.
    let out = build(
        &mut sim,
        &db,
        "basic_2l",
        a,
        a + Vec2::new(120.0, 0.0),
        ElevationMode::Tunnel(12.0),
    );
    assert!(out.result.is_err(), "a ramp steeper than 12 % is refused");
    // A highway ramps at 6 %: 250 m is too short for it, though a street gets there.
    let street = build(
        &mut sim,
        &db,
        "basic_2l",
        a,
        a + Vec2::new(250.0, 0.0),
        ElevationMode::Tunnel(12.0),
    );
    assert!(street.result.is_ok(), "{:?}", street.result);
    let (mut sim, db, t) = self::sim();
    let a = crossing(&t).0 - Vec2::new(250.0, 0.0);
    let highway = build(
        &mut sim,
        &db,
        "highway_1w_2l",
        a,
        a + Vec2::new(250.0, 0.0),
        ElevationMode::Tunnel(12.0),
    );
    assert!(highway.result.is_err(), "a 6 % ramp needs more run");
}
