//! Deathcare: bodies wait at home with DEAD until a cemetery or crematorium in reach takes them.

use std::sync::Arc;

use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule};
use bevy_ecs::world::World;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_content::{ContentDb, SimContent};
use cp_core::curve::CubicBezier3;
use cp_core::{Id, Tick};
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_service::{
    ServicePlugin, ServiceProblems, ServiceState, reset_daily_capacity, update_coverage,
};

use crate::deathcare::update_deathcare;
use crate::land::LandGrids;
use cp_citizen::CitizenGrid;

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

fn content(capacity: u32, storage: u32) -> Arc<ContentDb> {
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
                "b/grave.ron".into(),
                b("grave", "Industrial", "Some(\"grave\")"),
            ),
            (
                "s/grave.ron".into(),
                format!(
                    r#"Service((id: "grave", name: "grave", kind: Deathcare, radius_m: 500.0,
                    capacity: {capacity}, storage: {storage}, site: Land, vehicles: []))"#
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

fn town(capacity: u32, storage: u32) -> Town {
    let db = content(capacity, storage);
    let mut world = World::new();
    world.insert_resource(Tick(0));
    world.insert_resource(SimContent(Arc::clone(&db)));
    world.init_resource::<NetGraph>();
    world.init_resource::<BuildingGrid>();
    world.init_resource::<LandGrids>();
    world.init_resource::<CitizenGrid>();
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
    schedule.add_systems((reset_daily_capacity, update_coverage, update_deathcare).chain());
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

    fn bury(&mut self, home: BuildingId, n: u16) {
        self.world
            .resource_mut::<CitizenGrid>()
            .dead
            .insert(home, n);
    }

    fn waiting(&self, home: BuildingId) -> u16 {
        self.world
            .resource::<CitizenGrid>()
            .dead
            .get(&home)
            .copied()
            .unwrap_or(0)
    }

    fn stored(&self, grave: BuildingId) -> u32 {
        self.world
            .resource::<ServiceState>()
            .facility(grave)
            .map_or(0, |f| f.stored)
    }

    fn flagged(&self, id: BuildingId) -> bool {
        self.world
            .resource::<ServiceState>()
            .problems_of(id)
            .has(ServiceProblems::DEAD)
    }
}

#[test]
fn bodies_without_a_cemetery_keep_dead_and_one_in_reach_takes_them() {
    let mut bare = town(20, 100);
    let house = bare.put("house", 0);
    bare.bury(house, 2);
    bare.run(0, 128);
    assert!(bare.flagged(house));
    assert_eq!(bare.waiting(house), 2);

    let mut served = town(20, 100);
    let house = served.put("house", 0);
    let grave = served.put("grave", 1);
    served.bury(house, 2);
    served.run(0, 128);
    assert!(!served.flagged(house));
    assert_eq!(served.waiting(house), 0);
    assert_eq!(served.stored(grave), 2);
}

#[test]
fn a_full_cemetery_takes_no_more_and_a_crematorium_without_power_does_not_work() {
    let mut t = town(20, 3);
    let house = t.put("house", 0);
    let grave = t.put("grave", 1);
    t.bury(house, 5);
    t.run(0, 128);
    assert_eq!(t.stored(grave), 3, "up to its storage");
    assert_eq!(t.waiting(house), 2);
    assert!(t.flagged(house));

    let mut dark = town(20, 100);
    let house = dark.put("house", 0);
    let grave = dark.put("grave", 1);
    dark.world
        .resource_mut::<BuildingGrid>()
        .get_mut(grave)
        .unwrap()
        .flags = BuildingFlags(BuildingFlags::NO_POWER);
    dark.bury(house, 2);
    dark.run(0, 128);
    assert_eq!(dark.waiting(house), 2);
    assert!(dark.flagged(house));
}
