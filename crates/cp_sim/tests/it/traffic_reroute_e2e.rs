//! T-315 (§18.6): routes by the measured delay per lane, and cars that ask for a quicker route
//! on the way: when a lane ahead of them gets slow (a "hot" lane at the refresh of the delays)
//! and in their turn every `REROUTE_PERIOD` ticks, taking it only when it is quicker by
//! `REROUTE_GAIN`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::ContentDb;
use cp_core::SimRng;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, LaneId, NetGraph, SegmentId};
use cp_sim::SimRunner;
use cp_traffic::{Agents, PathNetwork, REROUTE_MIN_M, demo};

const ORIGIN: f32 = 2000.0;
/// Lines of the lattice each way; 90 m pieces, so routes run up to ~2.3 km.
const LINES: usize = 14;
const PIECE: f32 = 90.0;

/// A 14×14-line lattice of 90 m streets with `cars` demo cars on it, spawned and driving.
fn lattice(cars: usize) -> SimRunner {
    let content = Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    );
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(5);
    sim.set_content(Arc::clone(&content));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let p = |x: f32, z: f32| Vec3::new(ORIGIN + x, 0.0, ORIGIN + z);
        for line in 0..LINES {
            let o = line as f32 * PIECE;
            for piece in 0..LINES - 1 {
                let a = piece as f32 * PIECE;
                for (from, to) in [(p(a, o), p(a + PIECE, o)), (p(o, a), p(o, a + PIECE))] {
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

fn segment_of(sim: &SimRunner, lane: LaneId) -> Option<SegmentId> {
    sim.world()
        .resource::<NetGraph>()
        .lane(lane)
        .map(|l| l.segment)
}

/// Segments still ahead of each car (not counting the lane it is on).
fn ahead(sim: &SimRunner) -> BTreeMap<SegmentId, usize> {
    let mut m = BTreeMap::new();
    for (_, v) in sim.world().resource::<Agents>().vehicles() {
        let segs: BTreeSet<_> = v
            .path
            .lanes()
            .skip(usize::from(v.hop) + 1)
            .filter_map(|l| segment_of(sim, l))
            .collect();
        for s in segs {
            *m.entry(s).or_default() += 1;
        }
    }
    m
}

/// Puts `seconds` of measured delay on every vehicle lane of `seg`, as if a queue stood there.
fn slow_down(sim: &mut SimRunner, seg: SegmentId, seconds: f32) {
    let lanes: Vec<LaneId> = sim
        .world()
        .resource::<NetGraph>()
        .segment(seg)
        .unwrap()
        .lanes
        .to_vec();
    let mut agents = sim.world_mut().resource_mut::<Agents>();
    for l in lanes {
        let i = l.index as usize;
        if agents.load.lane_delay.len() <= i {
            agents.load.lane_delay.resize(i + 1, 0.0);
        }
        agents.load.lane_delay[i] = seconds;
    }
}

/// The busiest segment ahead whose lanes are not the start of anybody's route now.
fn busiest_ahead(sim: &SimRunner) -> (SegmentId, usize) {
    let here: BTreeSet<_> = sim
        .world()
        .resource::<Agents>()
        .vehicles()
        .filter_map(|(_, v)| segment_of(sim, v.lane()))
        .collect();
    ahead(sim)
        .into_iter()
        .filter(|(s, _)| !here.contains(s))
        .max_by_key(|(_, n)| *n)
        .expect("a segment ahead of the cars")
}

/// Metres of road still ahead of `v` (whole lanes after the one it is on).
fn road_ahead(sim: &SimRunner, v: &cp_traffic::Vehicle) -> f32 {
    let net = sim.world().resource::<NetGraph>();
    v.path
        .lanes()
        .skip(usize::from(v.hop) + 1)
        .filter_map(|l| net.lane(l).and_then(|l| net.segment(l.segment)))
        .map(|s| s.length)
        .sum()
}

/// Cars with `seg` still ahead and enough road left to ask for another route.
fn heading_for(sim: &SimRunner, seg: SegmentId) -> BTreeSet<cp_traffic::VehicleId> {
    sim.world()
        .resource::<Agents>()
        .vehicles()
        .filter(|(_, v)| {
            road_ahead(sim, v) > REROUTE_MIN_M + 200.0
                && v.path
                    .lanes()
                    .skip(usize::from(v.hop) + 1)
                    .any(|l| segment_of(sim, l) == Some(seg))
        })
        .map(|(id, _)| id)
        .collect()
}

#[test]
fn a_lane_that_turns_slow_ahead_sends_the_cars_round_it() {
    let mut sim = lattice(400);
    let (slow, _) = busiest_ahead(&sim);
    let before = heading_for(&sim, slow);
    assert!(
        before.len() >= 6,
        "{} cars with a long way to go still have to drive the segment",
        before.len()
    );
    let start = sim.world().resource::<Agents>().stats.clone();
    // Ten minutes lost on it: the next refresh of the delays marks its lanes hot.
    for _ in 0..48 {
        slow_down(&mut sim, slow, 600.0);
        sim.tick_once();
    }
    let st = sim.world().resource::<Agents>().stats.clone();
    let taken = st.traffic_rerouted - start.traffic_rerouted;
    let still: BTreeSet<_> = heading_for(&sim, slow)
        .intersection(&before)
        .copied()
        .collect();
    assert!(taken > 0, "cars took quicker routes: {st:?}");
    assert!(
        still.len() * 3 <= before.len(),
        "{} of {} cars still head for the slow segment ({taken} re-routed)",
        still.len(),
        before.len()
    );
    assert_eq!(st.reroute_failed, start.reroute_failed, "nobody lost");
}

#[test]
fn on_free_roads_the_turns_keep_the_routes() {
    let mut sim = lattice(120);
    for _ in 0..400 {
        sim.tick_once();
    }
    let st = sim.world().resource::<Agents>().stats.clone();
    assert!(st.traffic_kept > 0, "cars asked in their turn: {st:?}");
    assert!(
        st.traffic_rerouted * 4 <= st.traffic_kept,
        "a free lattice gives few quicker routes: {st:?}"
    );
}

#[test]
fn routes_only_ask_with_enough_road_ahead() {
    // A sanity bound for the test lattice: its long diagonals are longer than the threshold.
    let side = (LINES - 1) as f32 * PIECE;
    assert!(2.0 * side > REROUTE_MIN_M * 1.2);
}

#[test]
fn the_traffic_reroutes_are_deterministic() {
    let run = || {
        let mut sim = lattice(120);
        let (slow, _) = busiest_ahead(&sim);
        for _ in 0..300 {
            slow_down(&mut sim, slow, 300.0);
            sim.tick_once();
        }
        let st = sim.world().resource::<Agents>().stats.clone();
        (sim.checksum(), st.traffic_rerouted, st.traffic_kept)
    };
    let a = run();
    assert!(a.1 + a.2 > 0, "the run re-routed: {a:?}");
    assert_eq!(a, run());
}
