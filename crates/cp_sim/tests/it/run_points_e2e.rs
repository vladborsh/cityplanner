//! T-154: runs between points, and vehicles that stay. A keep run stops where it was sent and
//! holds its place; `continue_run` drives it on from there; `release` frees its pool slot; a
//! run can start at the `Outside` node of an entry road.

use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_content::{ContentDb, VehicleKind};
use cp_core::curve::CubicBezier3;
use cp_core::{Id, SimLimits, Tick};
use cp_net::{Elevation, NetGraph, NodeId, NodeKind, SegmentId};
use cp_sim::SimRunner;
use cp_traffic::{Access, Agents, RunPoint, RunState, ServiceRuns, Side};

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            (
                "b/house.ron".into(),
                r#"Building((id: "house", name: "house", zone: Industrial, level: 1, lot: (1, 1),
                styles: ["p"], service: None))"#
                    .into(),
            ),
            (
                "v/truck.ron".into(),
                r#"Vehicle((id: "truck", name: "truck", kind: Truck, length: 6.0, width: 2.4,
                max_speed_kmh: 60.0, accel: 2.0, decel: 4.0, min_gap: 2.0, mesh: "m"))"#
                    .into(),
            ),
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

struct Town {
    sim: SimRunner,
    seg: SegmentId,
    outside: NodeId,
}

/// A 1 km street from an `Outside` node at x = 0.
fn town(limits: SimLimits) -> Town {
    let db = content();
    let mut sim = SimRunner::with_limits(1, limits);
    sim.set_content(Arc::clone(&db));
    let road = db.road_id("street").unwrap();
    let def = db.road(road).clone();
    let outside = sim.world_mut().resource_mut::<NetGraph>().add_node(
        Vec3::ZERO,
        NodeKind::Outside,
        Elevation::Ground,
    );
    let seg = sim
        .world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1000.0, 0.0, 0.0)),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();
    Town { sim, seg, outside }
}

impl Town {
    fn house(&mut self, at: f32) -> BuildingId {
        let db = self
            .sim
            .world()
            .resource::<cp_content::SimContent>()
            .0
            .clone();
        self.sim
            .world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(Building {
                def: db.building_id("house").unwrap(),
                footprint: Footprint::Lot(Lot {
                    block: Id::new(0, 0),
                    col: 0,
                    w: 1,
                    d: 1,
                }),
                level: 1,
                state: BuildingState::Active,
                ready_at: Tick(0),
                pos: Vec2::new(at, 5.0),
                rot: 0.0,
                access: Some(AccessPoint {
                    segment: self.seg,
                    t: at / 1000.0,
                }),
                flags: BuildingFlags(0),
                level_progress: 0.0,
            })
    }

    fn stop_at(&self, s: f32) -> RunPoint {
        RunPoint::Access(Access {
            segment: self.seg,
            s,
            side: Side::Right,
        })
    }

    fn start(&mut self, from: RunPoint, to: RunPoint, keep: bool) -> u32 {
        self.sim
            .world_mut()
            .resource_mut::<ServiceRuns>()
            .start_run(VehicleKind::Truck, from, to, keep)
    }

    fn state(&self, run: u32) -> RunState {
        self.sim
            .world()
            .resource::<ServiceRuns>()
            .get(run)
            .unwrap()
            .state
    }

    fn until(&mut self, run: u32, max: u64, f: impl Fn(RunState) -> bool) -> bool {
        for _ in 0..max {
            if f(self.state(run)) {
                return true;
            }
            self.sim.tick_once();
        }
        f(self.state(run))
    }

    /// Front-bumper position of the only vehicle, metres along its lane.
    fn only_vehicle(&self) -> (f32, f32) {
        let agents = self.sim.world().resource::<Agents>();
        let (_, v) = agents.vehicles().next().expect("a vehicle");
        (v.s, v.speed)
    }
}

fn parked(s: RunState) -> bool {
    matches!(s, RunState::Parked { .. })
}

#[test]
fn a_keep_run_stands_at_its_point_and_continue_drives_it_on() {
    let mut t = town(SimLimits::desktop());
    let run = t.start(t.stop_at(100.0), t.stop_at(500.0), true);
    assert!(t.until(run, 2000, parked), "parks at the first stop");
    // 500 m along the lane (the lane starts at the junction setback of a dead end: none).
    let (s, speed) = t.only_vehicle();
    assert!((s - 500.0).abs() < 1.0, "stands at 500 m, got {s}");
    assert_eq!(speed, 0.0);
    for _ in 0..20 {
        t.sim.tick_once();
        let (s2, v2) = t.only_vehicle();
        assert!((s2 - s).abs() < 1e-3 && v2 == 0.0, "it does not move");
        assert!(parked(t.state(run)));
    }
    // On to a stop further along.
    let to = t.stop_at(900.0);
    t.sim
        .world_mut()
        .resource_mut::<ServiceRuns>()
        .continue_run(run, to)
        .unwrap();
    assert!(!parked(t.state(run)));
    assert!(t.until(run, 2000, |s| matches!(s, RunState::Driving)));
    assert!(t.until(run, 3000, parked), "parks at the second stop");
    let (s, _) = t.only_vehicle();
    assert!((s - 900.0).abs() < 1.0, "stands at 900 m, got {s}");
}

#[test]
fn release_frees_the_slot_of_the_pool() {
    let mut t = town(SimLimits::desktop());
    let run = t.start(t.stop_at(100.0), t.stop_at(300.0), true);
    assert!(t.until(run, 2000, parked));
    assert_eq!(t.sim.world().resource::<Agents>().vehicle_count(), 1);
    t.sim
        .world_mut()
        .resource_scope(|w, mut agents: bevy_ecs::prelude::Mut<Agents>| {
            assert!(w.resource_mut::<ServiceRuns>().release(run, &mut agents));
        });
    assert_eq!(t.sim.world().resource::<Agents>().vehicle_count(), 0);
    assert!(t.sim.world().resource::<ServiceRuns>().get(run).is_none());
    // The lane is free again and the world carries on.
    for _ in 0..10 {
        t.sim.tick_once();
    }
    assert!(t.sim.world().resource::<Agents>().occupancy_is_live());
}

#[test]
fn a_run_from_the_outside_node_arrives_at_the_building() {
    let mut t = town(SimLimits::desktop());
    let house = t.house(600.0);
    let from = RunPoint::Outside(t.outside);
    let run = t.start(from, RunPoint::Building(house), false);
    assert!(t.until(run, 3000, |s| matches!(s, RunState::Arrived { .. })));
}

#[test]
fn with_the_pool_full_a_keep_run_arrives_abstractly_and_continues_from_there() {
    let limits = SimLimits {
        vehicles: 0,
        ..SimLimits::desktop()
    };
    let mut t = town(limits);
    let run = t.start(t.stop_at(100.0), t.stop_at(500.0), true);
    assert!(t.until(run, 3000, |s| matches!(s, RunState::Arrived { .. })));
    let to = t.stop_at(900.0);
    t.sim
        .world_mut()
        .resource_mut::<ServiceRuns>()
        .continue_run(run, to)
        .unwrap();
    assert!(t.until(run, 3000, |s| matches!(s, RunState::Arrived { .. })));
    assert_eq!(t.sim.world().resource::<Agents>().vehicle_count(), 0);
}
