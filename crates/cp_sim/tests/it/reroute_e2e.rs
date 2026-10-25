//! T-066: a car whose route is cut ahead of it asks for a new one from where it is, keeps to
//! its lane meanwhile and arrives; only cars whose own lane (or destination) is gone vanish.

use std::collections::BTreeSet;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::ContentDb;
use cp_core::SimRng;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_sim::SimRunner;
use cp_traffic::{Agents, PathNetwork, demo};

const ORIGIN: f32 = 2000.0;

/// A 4×4-line lattice of 90 m streets with `cars` demo cars on it, spawned and driving.
fn lattice(cars: usize) -> SimRunner {
    let content = Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    );
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(3);
    sim.set_content(Arc::clone(&content));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let p = |x: f32, z: f32| Vec3::new(ORIGIN + x, 0.0, ORIGIN + z);
        for line in 0..4 {
            let o = line as f32 * 90.0;
            for piece in 0..3 {
                let a = piece as f32 * 90.0;
                for (from, to) in [(p(a, o), p(a + 90.0, o)), (p(o, a), p(o, a + 90.0))] {
                    g.add_segment(&def, road, CubicBezier3::line(from, to), Elevation::Ground)
                        .unwrap();
                }
            }
        }
    }
    sim.tick_once();
    sim.tick_once();
    sim.world_mut()
        .resource_scope(|w, mut agents: bevy_ecs::change_detection::Mut<Agents>| {
            let net = w.resource::<NetGraph>();
            let graph = Arc::clone(w.resource::<PathNetwork>().graph());
            let rng = w.resource::<SimRng>();
            demo::top_up(&mut agents, &graph, net, cars, 0, rng, 0);
        });
    for _ in 0..10 {
        sim.tick_once();
    }
    sim
}

fn segment_of(sim: &SimRunner, lane: cp_net::LaneId) -> Option<SegmentId> {
    sim.world()
        .resource::<NetGraph>()
        .lane(lane)
        .map(|l| l.segment)
}

#[test]
fn cutting_the_road_ahead_reroutes_the_cars_and_only_lost_lanes_remove_any() {
    let mut sim = lattice(100);
    let total = sim.world().resource::<Agents>().vehicle_count();
    assert!(total >= 90, "cars on the road: {total}");

    // Cars that would have to vanish if `seg` were cut: standing on it now (or in a junction
    // connection that leaves it), or heading for a lane on it.
    let doomed_by = |sim: &SimRunner, seg: SegmentId| -> BTreeSet<_> {
        sim.world()
            .resource::<Agents>()
            .vehicles()
            .filter(|(_, v)| {
                segment_of(sim, v.lane()) == Some(seg)
                    || segment_of(sim, v.path.goal) == Some(seg)
                    || (v.in_conn
                        && v.path
                            .steps
                            .get(usize::from(v.hop))
                            .is_some_and(|s| segment_of(sim, s.1) == Some(seg)))
            })
            .map(|(id, _)| id)
            .collect()
    };
    // The segment the most cars still have to drive on, among those few cars stand on.
    let mut ahead: std::collections::BTreeMap<SegmentId, usize> = Default::default();
    for (_, v) in sim.world().resource::<Agents>().vehicles() {
        let from = usize::from(v.hop) + 1;
        let segs: BTreeSet<_> = v
            .path
            .lanes()
            .skip(from)
            .filter_map(|l| segment_of(&sim, l))
            .collect();
        for s in segs {
            *ahead.entry(s).or_default() += 1;
        }
    }
    let (cut, n_ahead) = ahead
        .iter()
        .filter(|(s, _)| doomed_by(&sim, **s).len() * 25 <= total)
        .max_by_key(|(_, n)| **n)
        .map(|(s, n)| (*s, *n))
        .expect("a busy segment with few cars on it");
    assert!(
        n_ahead >= 6,
        "{n_ahead} cars still have to use the cut segment"
    );
    let doomed = doomed_by(&sim, cut);
    let before = sim.world().resource::<Agents>().stats.clone();

    sim.world_mut()
        .resource_mut::<NetGraph>()
        .remove_segment(cut)
        .unwrap();

    for _ in 0..4000 {
        sim.tick_once();
    }
    let agents = sim.world().resource::<Agents>();
    let st = &agents.stats;
    assert_eq!(agents.vehicle_count(), 0, "everybody got somewhere");
    let arrived = st.driven - before.driven;
    assert!(
        arrived as usize * 10 >= total * 9,
        "{arrived} of {total} arrived physically"
    );
    assert!(st.rerouted > 0, "cars asked for and got new routes: {st:?}");
    // Nobody is removed for the road changing under their route, only for losing their own
    // lane or destination.
    let removed = (st.invalid_vehicles - before.invalid_vehicles) + st.reroute_failed;
    assert!(
        removed as usize <= doomed.len(),
        "{removed} removed, {} had lost their lane or goal",
        doomed.len()
    );
    assert_eq!(st.despawned_stuck, before.despawned_stuck, "no stuck cars");
    assert_eq!(st.reroute_failed, 0, "the lattice stays connected");
}

#[test]
fn the_run_is_deterministic() {
    let run = || {
        let mut sim = lattice(60);
        let cut = sim
            .world()
            .resource::<NetGraph>()
            .segments()
            .nth(7)
            .map(|(id, _)| id)
            .unwrap();
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .remove_segment(cut)
            .unwrap();
        for _ in 0..600 {
            sim.tick_once();
        }
        sim.checksum()
    };
    assert_eq!(run(), run());
}
