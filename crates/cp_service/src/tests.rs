//! Coverage along the roads: the decay with road distance and the rebuild after a demolition.

use std::sync::Arc;

use bevy_ecs::schedule::Schedule;
use bevy_ecs::world::World;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_content::{ContentDb, ServiceKind, SimContent};
use cp_core::curve::CubicBezier3;
use cp_core::{Id, Tick};
use cp_net::{Elevation, NetGraph, SegmentId};

use super::*;

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

fn content() -> Arc<ContentDb> {
    let building = r#"Building((id: "clinic", name: "clinic", zone: Industrial, level: 1, lot: (1, 1),
        styles: ["p"], service: Some("clinic")))"#;
    // A Health service reaching 300 m along the roads.
    let service = r#"Service((id: "clinic", name: "clinic", kind: Health, radius_m: 300.0,
        capacity: 10, site: Land, vehicles: []))"#;
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            ("b/clinic.ron".into(), building.into()),
            (
                "b/house.ron".into(),
                r#"Building((id: "house", name: "house", zone: ResLow, level: 1, lot: (1, 1),
                households: 4, styles: ["p"], service: None))"#
                    .into(),
            ),
            ("s/clinic.ron".into(), service.into()),
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

/// A straight street of four 100 m segments, and a clinic at the start of the first.
fn town() -> (World, Schedule, BuildingId, Vec<SegmentId>) {
    let db = content();
    let mut world = World::new();
    world.insert_resource(Tick(0));
    world.insert_resource(SimContent(Arc::clone(&db)));
    world.init_resource::<NetGraph>();
    world.init_resource::<BuildingGrid>();
    ServicePlugin::init(&mut world);
    let road = db.road_id("street").unwrap();
    let def = db.road(road).clone();
    let mut segs = Vec::new();
    for k in 0..4 {
        let (a, b) = (k as f32 * 100.0, (k + 1) as f32 * 100.0);
        let curve = CubicBezier3::line(Vec3::new(a, 0.0, 0.0), Vec3::new(b, 0.0, 0.0));
        let id = world
            .resource_mut::<NetGraph>()
            .add_segment(&def, road, curve, Elevation::Ground)
            .unwrap();
        segs.push(id);
    }
    let clinic = world.resource_mut::<BuildingGrid>().insert(Building {
        def: db.building_id("clinic").unwrap(),
        footprint: Footprint::Lot(Lot {
            block: Id::new(0, 0),
            col: 0,
            w: 1,
            d: 1,
        }),
        level: 1,
        state: BuildingState::Active,
        ready_at: Tick(0),
        pos: Vec2::new(0.0, 5.0),
        rot: 0.0,
        access: Some(AccessPoint {
            segment: segs[0],
            t: 0.0,
        }),
        flags: BuildingFlags::default(),
        level_progress: 0.0,
    });
    let mut schedule = Schedule::default();
    schedule.add_systems(update_coverage);
    (world, schedule, clinic, segs)
}

fn run_at(world: &mut World, schedule: &mut Schedule, tick: u64) {
    world.insert_resource(Tick(tick));
    schedule.run(world);
}

#[test]
fn coverage_falls_off_along_the_street_and_ends_at_the_radius() {
    let (mut world, mut schedule, clinic, segs) = town();
    // Not computed between the 64-tick boundaries.
    run_at(&mut world, &mut schedule, 1);
    assert_eq!(world.resource::<ServiceCoverage>().generation, 0);
    run_at(&mut world, &mut schedule, 64);
    let cov = world.resource::<ServiceCoverage>();
    let at: Vec<f32> = segs
        .iter()
        .map(|&s| cov.on_segment(ServiceKind::Health, s))
        .collect();
    // The segments start 0, 100, 200 and 300 m from the clinic: 1, 2/3, 1/3, 0.
    for (got, want) in at.iter().zip([1.0, 2.0 / 3.0, 1.0 / 3.0, 0.0]) {
        assert!((got - want).abs() < 1e-3, "{at:?}");
    }
    // Other kinds are not covered; the building reads the value on its access segment.
    assert_eq!(cov.on_segment(ServiceKind::Fire, segs[0]), 0.0);
    let b = world.resource::<BuildingGrid>().get(clinic).unwrap();
    assert_eq!(cov.coverage_of(ServiceKind::Health, b), 1.0);
}

#[test]
fn a_demolished_building_stops_covering_at_the_next_recompute() {
    let (mut world, mut schedule, clinic, segs) = town();
    run_at(&mut world, &mut schedule, 64);
    world.resource_mut::<BuildingGrid>().remove(clinic);
    run_at(&mut world, &mut schedule, 100);
    assert!(
        world
            .resource::<ServiceCoverage>()
            .on_segment(ServiceKind::Health, segs[0])
            > 0.0
    );
    run_at(&mut world, &mut schedule, 128);
    let cov = world.resource::<ServiceCoverage>();
    assert!(
        segs.iter()
            .all(|&s| cov.on_segment(ServiceKind::Health, s) == 0.0)
    );
}

/// A building of `def` on segment `seg` at `t`.
fn put(world: &mut World, def: &str, seg: SegmentId, t: f32) -> BuildingId {
    let db = world.resource::<SimContent>().0.clone();
    world.resource_mut::<BuildingGrid>().insert(Building {
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
        pos: Vec2::new(0.0, 5.0),
        rot: 0.0,
        access: Some(AccessPoint { segment: seg, t }),
        flags: BuildingFlags::default(),
        level_progress: 0.0,
    })
}

#[test]
fn the_nearest_facility_with_room_wins_and_a_full_one_is_skipped() {
    let (mut world, _, near, segs) = town();
    // A second clinic at the far end and a house at 250 m: the far clinic is 150 m away, the
    // first one 250 m.
    let far = put(&mut world, "clinic", segs[3], 0.5);
    let house = put(&mut world, "house", segs[2], 0.5);
    let ask = |world: &World| {
        let db = world.resource::<SimContent>().0.clone();
        nearest_facility(
            world.resource::<NetGraph>(),
            world.resource::<BuildingGrid>(),
            &db,
            world.resource::<ServiceState>(),
            ServiceKind::Health,
            house,
        )
    };
    assert_eq!(ask(&world), Some(far));
    // The far clinic is used up for today (capacity 10): the next one is chosen.
    world
        .resource_mut::<ServiceState>()
        .facility_mut(far)
        .used_today = 10;
    assert_eq!(ask(&world), Some(near));
    // A new day starts with room again.
    let mut day = Schedule::default();
    day.add_systems(reset_daily_capacity);
    run_at(&mut world, &mut day, cp_core::time::TICKS_PER_DAY);
    assert_eq!(ask(&world), Some(far));
    // Nothing of another kind.
    let db = world.resource::<SimContent>().0.clone();
    assert_eq!(
        nearest_facility(
            world.resource::<NetGraph>(),
            world.resource::<BuildingGrid>(),
            &db,
            world.resource::<ServiceState>(),
            ServiceKind::Fire,
            house
        ),
        None
    );
}

#[test]
fn demolishing_a_building_drops_its_service_records() {
    let (mut world, _, clinic, _) = town();
    {
        let mut st = world.resource_mut::<ServiceState>();
        st.get_mut(clinic).garbage = 7;
        st.facility_mut(clinic).stored = 3;
    }
    world.resource_mut::<BuildingGrid>().remove(clinic);
    let mut purge = Schedule::default();
    purge.add_systems(purge_removed_services);
    purge.run(&mut world);
    let st = world.resource::<ServiceState>();
    assert!(st.get(clinic).is_none() && st.facility(clinic).is_none());
}
