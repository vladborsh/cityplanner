//! Health: the sick of a home without a clinic keep SICK, a clinic in reach treats them by its daily capacity.

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

use crate::health::update_health;
use crate::land::LandGrids;
use cp_citizen::{CitizenGrid, CitizenId};
use cp_core::SimRng;

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
                "b/clinic.ron".into(),
                b("clinic", "Industrial", "Some(\"clinic\")"),
            ),
            (
                "s/clinic.ron".into(),
                format!(
                    r#"Service((id: "clinic", name: "clinic", kind: Health, radius_m: 500.0,
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
    world.init_resource::<CitizenGrid>();
    world.insert_resource(SimRng::from_seed([7; 32]));
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
    schedule.add_systems((reset_daily_capacity, update_coverage, update_health).chain());
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

    /// Moves a household of `n` sick adults into `home`.
    fn sicken(&mut self, home: BuildingId, n: usize) -> Vec<CitizenId> {
        let mut g = self.world.resource_mut::<CitizenGrid>();
        g.ensure_derived();
        let age = 30 * cp_citizen::DAYS_PER_YEAR;
        g.add_household(home, 0, Tick(0), &vec![(age, 1, false); n]);
        let ids: Vec<_> = g.citizens().map(|(id, _)| id).collect();
        for &id in &ids {
            g.set_sick(id, Some(Tick(0)));
        }
        ids
    }

    fn sick_count(&self) -> usize {
        self.world
            .resource::<CitizenGrid>()
            .citizens()
            .filter(|(_, c)| c.sick_since.is_some())
            .count()
    }

    fn flagged(&self, id: BuildingId) -> bool {
        self.world
            .resource::<ServiceState>()
            .problems_of(id)
            .has(ServiceProblems::SICK)
    }
}

#[test]
fn the_sick_of_a_home_without_a_clinic_make_it_sick_and_a_clinic_in_reach_cures_them() {
    let mut bare = town(20);
    let house = bare.put("house", 0);
    bare.sicken(house, 2);
    bare.run(0, 128);
    assert!(bare.flagged(house));
    assert_eq!(bare.sick_count(), 2);

    let mut cared = town(20);
    let house = cared.put("house", 0);
    cared.put("clinic", 1);
    cared.sicken(house, 2);
    cared.run(0, 128);
    assert!(!cared.flagged(house));
    assert_eq!(cared.sick_count(), 0);
}

#[test]
fn a_clinic_of_capacity_one_cures_one_of_three_a_day() {
    let mut t = town(1);
    let house = t.put("house", 0);
    t.put("clinic", 1);
    t.sicken(house, 3);
    t.run(0, TICKS_PER_DAY - 1);
    assert_eq!(t.sick_count(), 2);
    assert!(t.flagged(house));
    t.run(TICKS_PER_DAY - 1, 2 * TICKS_PER_DAY - 1);
    assert_eq!(t.sick_count(), 1);
}
