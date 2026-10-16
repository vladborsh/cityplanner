//! Crime: piling up without a station, held down by one in reach, shared by daily capacity.

use std::sync::Arc;

use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule};
use bevy_ecs::world::World;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_content::{ContentDb, SimContent};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Id, Tick};
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_service::{
    ServicePlugin, ServiceProblems, ServiceState, reset_daily_capacity, update_coverage,
};

use crate::land::LandGrids;
use crate::police::update_police;

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

fn content(capacity: u32) -> Arc<ContentDb> {
    let b = |id: &str, zone: &str, svc: &str| {
        format!(
            r#"Building((id: "{id}", name: "{id}", zone: {zone}, level: 1, lot: (1, 1),
            styles: ["p"], service: {svc}))"#
        )
    };
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            ("b/house.ron".into(), b("house", "ResLow", "None")),
            (
                "b/post.ron".into(),
                b("post", "Industrial", "Some(\"post\")"),
            ),
            (
                "s/post.ron".into(),
                format!(
                    r#"Service((id: "post", name: "post", kind: Police, radius_m: 500.0,
                    capacity: {capacity}, site: Land, vehicles: []))"#
                ),
            ),
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

struct Town {
    world: World,
    schedule: Schedule,
    segs: Vec<SegmentId>,
}

fn town(capacity: u32) -> Town {
    let db = content(capacity);
    let mut world = World::new();
    world.insert_resource(Tick(0));
    world.insert_resource(SimContent(Arc::clone(&db)));
    world.init_resource::<NetGraph>();
    world.init_resource::<BuildingGrid>();
    world.init_resource::<LandGrids>();
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
    let mut schedule = Schedule::default();
    schedule.add_systems((reset_daily_capacity, update_coverage, update_police).chain());
    Town {
        world,
        schedule,
        segs,
    }
}

impl Town {
    fn put(&mut self, def: &str, seg: usize) -> BuildingId {
        let db = self.world.resource::<SimContent>().0.clone();
        self.world.resource_mut::<BuildingGrid>().insert(Building {
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
            pos: Vec2::new(seg as f32 * 100.0 + 50.0, 5.0),
            rot: 0.0,
            access: Some(AccessPoint {
                segment: self.segs[seg],
                t: 0.5,
            }),
            flags: BuildingFlags(0),
            level_progress: 0.0,
        })
    }

    fn run(&mut self, from: u64, to: u64) {
        for t in from..to {
            self.world.insert_resource(Tick(t));
            self.schedule.run(&mut self.world);
        }
    }

    fn crime(&self, id: BuildingId) -> u16 {
        self.world
            .resource::<ServiceState>()
            .get(id)
            .map_or(0, |s| s.crime)
    }

    fn flagged(&self, id: BuildingId) -> bool {
        self.world
            .resource::<ServiceState>()
            .problems_of(id)
            .has(ServiceProblems::CRIME)
    }

    fn used(&self, station: BuildingId) -> u32 {
        self.world
            .resource::<ServiceState>()
            .facility(station)
            .map_or(0, |f| f.used_today)
    }
}

#[test]
fn without_a_station_a_house_has_crime_after_a_few_days_and_a_station_in_reach_prevents_it() {
    let mut bare = town(200);
    let house = bare.put("house", 0);
    bare.run(0, TICKS_PER_DAY);
    // Cheap land (value 20): 20 × (1 + 0.8) = 36 a day.
    assert_eq!(bare.crime(house), 36);
    assert!(!bare.flagged(house));
    bare.run(TICKS_PER_DAY, 4 * TICKS_PER_DAY);
    assert!(bare.flagged(house), "crime {}", bare.crime(house));

    let mut policed = town(200);
    let house = policed.put("house", 0);
    policed.put("post", 1);
    policed.run(0, 6 * TICKS_PER_DAY);
    assert!(!policed.flagged(house));
    assert!(policed.crime(house) < 20, "crime {}", policed.crime(house));
}

#[test]
fn a_small_capacity_spills_over_to_the_next_station() {
    let mut t = town(5);
    let houses: Vec<_> = (0..3).map(|_| t.put("house", 1)).collect();
    let near = t.put("post", 1);
    let far = t.put("post", 3);
    t.run(0, TICKS_PER_DAY - 1);
    assert_eq!(t.used(near), 5, "the nearest station works to its capacity");
    assert!(t.used(far) > 0, "the rest goes to the next one");
    assert!(houses.iter().all(|&h| !t.flagged(h)));
}
