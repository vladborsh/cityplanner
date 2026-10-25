//! T-151: fire engines and garbage trucks are vehicles that drive on the roads. The fire goes
//! out `fire_extinguish_hours` after the engine arrives; a truck takes the garbage when it
//! arrives; with the vehicle pool full the trip is abstract and ends after its travel time.

use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_content::{ContentDb, VehicleKind};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_HOUR;
use cp_core::{Id, SimLimits, Tick};
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_service::{Fire, ServiceProblems, ServiceState};
use cp_sim::SimRunner;
use cp_traffic::{Agents, RunState, ServiceRuns};

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

fn content() -> Arc<ContentDb> {
    let b = |id: &str, svc: &str| {
        format!(
            r#"Building((id: "{id}", name: "{id}", zone: Industrial, level: 1, lot: (1, 1),
            styles: ["p"], service: {svc}))"#
        )
    };
    let vehicle = |id: &str, kind: &str| {
        format!(
            r#"Vehicle((id: "{id}", name: "{id}", kind: {kind}, length: 6.0, width: 2.4,
            max_speed_kmh: 60.0, accel: 2.0, decel: 4.0, min_gap: 2.0, mesh: "m"))"#
        )
    };
    let svc = |id: &str, kind: &str, extra: &str| {
        format!(
            r#"Service((id: "{id}", name: "{id}", kind: {kind}, radius_m: 2000.0,
            capacity: 400, {extra} site: Land, vehicles: []))"#
        )
    };
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            ("b/house.ron".into(), b("house", "None")),
            ("b/station.ron".into(), b("station", "Some(\"station\")")),
            ("b/landfill.ron".into(), b("landfill", "Some(\"landfill\")")),
            ("s/station.ron".into(), svc("station", "Fire", "")),
            (
                "s/landfill.ron".into(),
                svc("landfill", "Garbage", "storage: 5000,"),
            ),
            ("v/engine.ron".into(), vehicle("engine", "Emergency")),
            ("v/truck.ron".into(), vehicle("truck", "Truck")),
            (
                "balance.ron".into(),
                "Balance((fire_base: 0.0, fire_extinguish_hours: 2.0, fire_spread: 0.0, \
                 garbage_household: 0.0, garbage_job: 0.0, garbage_truck_min: 50.0))"
                    .into(),
            ),
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

struct Town {
    sim: SimRunner,
    seg: SegmentId,
}

/// A 1 km street; `at` places things along it (metres).
fn town(limits: SimLimits) -> Town {
    let db = content();
    let mut sim = SimRunner::with_limits(1, limits);
    sim.set_content(Arc::clone(&db));
    let road = db.road_id("street").unwrap();
    let def = db.road(road).clone();
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
    Town { sim, seg }
}

impl Town {
    fn put(&mut self, def: &str, at: f32) -> BuildingId {
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
                def: db.building_id(def).unwrap(),
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

    fn light(&mut self, id: BuildingId) {
        let mut st = self.sim.world_mut().resource_mut::<ServiceState>();
        let s = st.get_mut(id);
        s.fire = Some(Fire {
            since: Tick(0),
            out_at: None,
            station: None,
        });
        s.problems.set(ServiceProblems::FIRE, true);
    }

    fn fire(&self, id: BuildingId) -> Option<Fire> {
        self.sim
            .world()
            .resource::<ServiceState>()
            .get(id)
            .and_then(|s| s.fire)
    }

    fn vehicles_of(&self, kind: VehicleKind) -> usize {
        self.sim
            .world()
            .resource::<Agents>()
            .vehicles()
            .filter(|(_, v)| v.kind == kind as u8)
            .count()
    }

    /// The tick a run of `kind` is seen `Arrived`, running at most `max` ticks.
    fn run_until_arrival(&mut self, kind: VehicleKind, max: u64) -> Option<(u64, u64)> {
        let (mut driven, mut arrived) = (0u64, None);
        for _ in 0..max {
            self.sim.tick_once();
            driven += self.vehicles_of(kind) as u64;
            let runs = self.sim.world().resource::<ServiceRuns>();
            if let Some((_, r)) = runs.iter().find(|(_, r)| r.kind == kind as u8)
                && let RunState::Arrived { at } = r.state
            {
                arrived = Some(at.0);
                break;
            }
        }
        arrived.map(|a| (a, driven))
    }
}

const WORK: u64 = (2.0 * TICKS_PER_HOUR).round() as u64;

#[test]
fn the_fire_goes_out_two_hours_after_the_engine_drives_in() {
    let mut t = town(SimLimits::desktop());
    t.put("station", 50.0);
    let house = t.put("house", 900.0);
    t.light(house);
    let (at, driven) = t
        .run_until_arrival(VehicleKind::Emergency, 6 * TICKS_PER_HOUR as u64)
        .expect("the engine arrives");
    assert!(driven > 0, "the engine was a vehicle on the road");
    assert!(at > 30, "850 m take time: arrived at {at}");
    // The next service pass starts the extinguishing from the arrival tick.
    for _ in 0..64 {
        t.sim.tick_once();
        if let Some(f) = t.fire(house)
            && f.out_at.is_some()
        {
            break;
        }
    }
    assert_eq!(t.fire(house).and_then(|f| f.out_at), Some(Tick(at + WORK)));
    for _ in 0..(WORK + 64) {
        t.sim.tick_once();
    }
    assert!(t.fire(house).is_none(), "the fire is out");
    assert_eq!(
        t.sim
            .world()
            .resource::<ServiceState>()
            .facility(BuildingId::new(0, 0))
            .map(|f| f.stored),
        Some(0)
    );
}

#[test]
fn a_farther_fire_gets_its_engine_later() {
    let arrival = |at: f32| {
        let mut t = town(SimLimits::desktop());
        t.put("station", 50.0);
        let house = t.put("house", at);
        t.light(house);
        t.run_until_arrival(VehicleKind::Emergency, 6 * TICKS_PER_HOUR as u64)
            .expect("arrives")
            .0
    };
    assert!(arrival(300.0) < arrival(900.0));
}

#[test]
fn with_the_vehicle_pool_full_the_trip_is_abstract_and_still_ends_the_fire() {
    let mut t = town(SimLimits {
        vehicles: 0,
        ..SimLimits::desktop()
    });
    t.put("station", 50.0);
    let house = t.put("house", 900.0);
    t.light(house);
    let (at, driven) = t
        .run_until_arrival(VehicleKind::Emergency, 6 * TICKS_PER_HOUR as u64)
        .expect("the abstract trip ends");
    assert_eq!(driven, 0, "no vehicle was on the road");
    for _ in 0..(WORK + 128) {
        t.sim.tick_once();
    }
    assert!(
        t.fire(house).is_none(),
        "out after the abstract arrival at {at}"
    );
}

#[test]
fn a_garbage_truck_takes_the_garbage_when_it_arrives() {
    let mut t = town(SimLimits::desktop());
    let dump = t.put("landfill", 50.0);
    let house = t.put("house", 900.0);
    t.sim
        .world_mut()
        .resource_mut::<ServiceState>()
        .get_mut(house)
        .garbage = 80;
    let garbage = |t: &Town| {
        t.sim
            .world()
            .resource::<ServiceState>()
            .get(house)
            .map_or(0, |s| s.garbage)
    };
    let mut seen_truck = false;
    let mut at_truck_start = None;
    for tick in 0..(6 * TICKS_PER_HOUR as u64) {
        t.sim.tick_once();
        if t.vehicles_of(VehicleKind::Truck) > 0 {
            seen_truck = true;
            at_truck_start.get_or_insert(tick);
            assert_eq!(garbage(&t), 80, "nothing is taken while the truck drives");
        }
        if garbage(&t) == 0 {
            break;
        }
    }
    assert!(seen_truck, "a truck drove");
    assert_eq!(garbage(&t), 0, "the garbage was taken");
    let stored = t
        .sim
        .world()
        .resource::<ServiceState>()
        .facility(dump)
        .map_or(0, |f| f.stored);
    assert_eq!(stored, 80);
}

#[test]
fn service_runs_are_deterministic() {
    let run = || {
        let mut t = town(SimLimits::desktop());
        t.put("station", 50.0);
        let house = t.put("house", 700.0);
        t.light(house);
        for _ in 0..3000 {
            t.sim.tick_once();
        }
        t.sim.checksum()
    };
    assert_eq!(run(), run());
}
