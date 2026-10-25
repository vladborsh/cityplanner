//! T-155: a walking run is a pedestrian of the shared pool that goes from point to point and
//! reports `Arrived` to its run; it never completes a trip. With the pool full the walk is
//! abstract; a demolished building at either end fails it.

use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingCommand, BuildingFlags, BuildingGrid, BuildingId, BuildingState,
    Footprint, Lot,
};
use cp_citizen::TripBook;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::{Id, SimLimits, Tick};
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_sim::{SimCommand, SimRunner};
use cp_traffic::{Access, Agents, RunPoint, RunState, ServiceRuns, Side};

const ROAD: &str = r#"Road((id: "street", name: "street", width: 14.0,
    lanes: [
        (kind: Pedestrian, dir: Forward, width: 2.5, offset: 4.75),
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
        (kind: Pedestrian, dir: Forward, width: 2.5, offset: -4.75),
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
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

struct Town {
    sim: SimRunner,
    /// Two 500 m streets end to end.
    segs: [SegmentId; 2],
}

fn town(limits: SimLimits) -> Town {
    let db = content();
    let mut sim = SimRunner::with_limits(1, limits);
    sim.set_content(Arc::clone(&db));
    let road = db.road_id("street").unwrap();
    let def = db.road(road).clone();
    let mut add = |a: f32, b: f32| {
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(a, 0.0, 0.0), Vec3::new(b, 0.0, 0.0)),
                Elevation::Ground,
            )
            .unwrap()
    };
    let segs = [add(0.0, 500.0), add(500.0, 1000.0)];
    sim.tick_once();
    Town { sim, segs }
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
                pos: Vec2::new(at, 9.0),
                rot: 0.0,
                access: Some(AccessPoint {
                    segment: self.segs[0],
                    t: at / 500.0,
                }),
                flags: BuildingFlags(0),
                level_progress: 0.0,
            })
    }

    fn stop(&self, seg: usize, s: f32) -> RunPoint {
        RunPoint::Access(Access {
            segment: self.segs[seg],
            s,
            side: Side::Left,
        })
    }

    fn walk(&mut self, from: RunPoint, to: RunPoint) -> u32 {
        self.sim
            .world_mut()
            .resource_mut::<ServiceRuns>()
            .start_walk(from, to)
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
}

fn arrived(s: RunState) -> bool {
    matches!(s, RunState::Arrived { .. })
}

#[test]
fn a_walk_from_a_house_to_a_stop_on_the_next_street_arrives_and_leaves_the_trips_alone() {
    let mut t = town(SimLimits::desktop());
    let house = t.house(100.0);
    let (trips, walked) = {
        let w = t.sim.world();
        (
            w.resource::<TripBook>().len(),
            w.resource::<Agents>().stats.walked,
        )
    };
    let run = t.walk(RunPoint::Building(house), t.stop(1, 200.0));
    assert!(t.until(run, 500, |s| matches!(s, RunState::Driving)));
    assert_eq!(t.sim.world().resource::<Agents>().pedestrian_count(), 1);
    assert!(t.until(run, 5000, arrived), "the walker arrives");
    let w = t.sim.world();
    assert_eq!(
        w.resource::<Agents>().pedestrian_count(),
        0,
        "its slot is free"
    );
    assert_eq!(w.resource::<TripBook>().len(), trips, "no trip was touched");
    assert_eq!(w.resource::<Agents>().stats.walked, walked + 1);
}

#[test]
fn with_the_pedestrian_pool_full_the_walk_arrives_after_its_walking_time() {
    let limits = SimLimits {
        pedestrians: 0,
        ..SimLimits::desktop()
    };
    let mut t = town(limits);
    let run = t.walk(t.stop(0, 100.0), t.stop(1, 400.0));
    assert!(t.until(run, 5000, arrived));
    assert_eq!(t.sim.world().resource::<Agents>().pedestrian_count(), 0);
}

#[test]
fn demolishing_the_house_on_the_way_fails_the_walk_without_a_panic() {
    let mut t = town(SimLimits::desktop());
    let house = t.house(100.0);
    let run = t.walk(RunPoint::Building(house), t.stop(1, 400.0));
    assert!(t.until(run, 500, |s| matches!(s, RunState::Driving)));
    for _ in 0..20 {
        t.sim.tick_once();
    }
    let at = Tick(t.sim.tick().0 + 1);
    t.sim.push_command(
        SimCommand::Building(BuildingCommand::Demolish { building: house }),
        at,
    );
    assert!(t.until(run, 50, |s| s == RunState::Failed));
    assert_eq!(t.sim.world().resource::<Agents>().pedestrian_count(), 0);
    for _ in 0..50 {
        t.sim.tick_once();
    }
}
