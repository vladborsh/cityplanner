//! BUG-117 (folded into T-306): cars coming in from the map's edge use every lane of the entry
//! carriageway and enter at the road's speed. Before: every run from an `Outside` node started
//! on the lowest-id lane (the second lane of the two-lane highway stayed empty, the user saw one
//! standing queue) and from standstill behind a bumper gap, one car per 7 ticks per lane.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_content::{ContentDb, VehicleKind};
use cp_core::SimLimits;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, LaneId, NetGraph, SegmentId};
use cp_sim::SimRunner;
use cp_terrain::Terrain;
use cp_traffic::{Access, Agents, RunPoint, RunState, ServiceRuns, Side};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// The shipped content with every tourist driving and no cap on the entry queue: the test
/// watches the lanes themselves.
fn content() -> Arc<ContentDb> {
    let root = Path::new(ROOT).join("../../assets/content");
    let db = ContentDb::load_dir_with_balance(root, |text| {
        text.lines()
            .map(|l| {
                let t = l.trim_start();
                if t.starts_with("tourist_visible_share:") {
                    "    tourist_visible_share: 1.0,".to_string()
                } else if t.starts_with("spawn_queue_cap:") {
                    "    spawn_queue_cap: 100000,".to_string()
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
    .unwrap();
    Arc::new(db)
}

/// A new game on flatlands with a street across the stub end; returns the sim, the entry
/// `Outside` node's run point, the inbound carriageway's first piece and a goal on the street.
fn town() -> (SimRunner, RunPoint, SegmentId, RunPoint) {
    let db = content();
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(Arc::clone(&terrain));
    sim.set_content(Arc::clone(&db));
    sim.new_game().unwrap();
    let e = terrain.meta().entry_road.clone().unwrap();
    let (end, dir) = (e.stub_end(), e.direction());
    let side = Vec2::new(-dir.y, dir.x);
    let y = |p: Vec2| Vec3::new(p.x, terrain.height(p), p.y);
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let street = {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let s = g
            .add_segment(
                &def,
                road,
                CubicBezier3::line(y(end), y(end - side * 300.0)),
                Elevation::Ground,
            )
            .unwrap();
        g.add_segment(
            &def,
            road,
            CubicBezier3::line(y(end), y(end + side * 300.0)),
            Elevation::Ground,
        )
        .unwrap();
        s
    };
    sim.tick_once();
    let (entries, _) = cp_net::outside_ways(sim.world().resource::<NetGraph>(), &db);
    let (node, _) = entries[0];
    let inbound = *sim
        .world()
        .resource::<NetGraph>()
        .node(node)
        .unwrap()
        .segments
        .first()
        .unwrap();
    let goal = RunPoint::Access(Access {
        segment: street,
        s: 250.0,
        side: Side::Right,
    });
    (sim, RunPoint::Outside(node), inbound, goal)
}

/// Releases the runs that got there or failed.
fn collect(sim: &mut SimRunner) {
    let w = sim.world_mut();
    let done: Vec<_> = w
        .resource::<ServiceRuns>()
        .iter()
        .filter(|(_, r)| matches!(r.state, RunState::Arrived { .. } | RunState::Failed))
        .map(|(id, _)| id)
        .collect();
    w.resource_scope::<ServiceRuns, _>(|w, mut runs| {
        let mut agents = w.resource_mut::<Agents>();
        for id in done {
            runs.release(id, &mut agents);
        }
    });
}

struct Flood {
    /// Cars that got on the road, by the lane they started on.
    on_road_by_lane: BTreeMap<LaneId, u32>,
    /// Speed of the first car on the road at its first tick, m/s.
    first_speed: f32,
    checksum: u64,
}

/// One car run from the entry every other tick for `ticks` ticks.
fn flood(ticks: u64) -> Flood {
    let (mut sim, entry, inbound, goal) = town();
    let mut seen: BTreeSet<cp_traffic::VehicleId> = BTreeSet::new();
    let mut on_road_by_lane: BTreeMap<LaneId, u32> = BTreeMap::new();
    let mut first_speed: Option<f32> = None;
    for t in 0..ticks {
        if t % 2 == 0 {
            sim.world_mut().resource_mut::<ServiceRuns>().start_run(
                VehicleKind::Car,
                entry,
                goal,
                false,
            );
        }
        sim.tick_once();
        collect(&mut sim);
        let w = sim.world();
        let net = w.resource::<NetGraph>();
        for (id, v) in w.resource::<Agents>().vehicles() {
            if !v.spawned || !seen.insert(id) {
                continue;
            }
            // A car that just got on the road at the entry.
            if net.lane(v.path.start).is_some_and(|l| l.segment == inbound) {
                *on_road_by_lane.entry(v.path.start).or_insert(0) += 1;
                first_speed.get_or_insert(v.speed);
            }
        }
    }
    Flood {
        on_road_by_lane,
        first_speed: first_speed.unwrap_or(0.0),
        checksum: sim.checksum(),
    }
}

#[test]
fn cars_from_the_outside_use_both_entry_lanes_and_enter_at_speed() {
    let f = flood(400);
    let total: u32 = f.on_road_by_lane.values().sum();
    assert_eq!(f.on_road_by_lane.len(), 2, "{:?}", f.on_road_by_lane);
    for (lane, n) in &f.on_road_by_lane {
        assert!(
            *n * 100 >= total * 35,
            "lane {lane:?} carries {n} of {total}: {:?}",
            f.on_road_by_lane
        );
    }
    // 1 car per 7 ticks on one lane would be 57; two lanes at road speed give far more.
    assert!(total >= 120, "{total} cars got on the road in 400 ticks");
    assert!(
        f.first_speed >= 20.0,
        "the first car enters at road speed, not from standstill: {} m/s",
        f.first_speed
    );
}

#[test]
fn the_entry_flood_is_deterministic() {
    assert_eq!(flood(200).checksum, flood(200).checksum);
}
