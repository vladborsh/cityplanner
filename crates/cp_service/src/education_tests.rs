//! Education: enrolment by age, reach and places, graduation.

use std::sync::Arc;

use bevy_ecs::schedule::Schedule;
use bevy_ecs::world::World;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_citizen::{CitizenGrid, CitizenId, DAYS_PER_YEAR};
use cp_content::{ContentDb, SimContent};
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

fn content(places: u32) -> Arc<ContentDb> {
    let b = |id: &str, svc: &str| {
        format!(
            r#"Building((id: "{id}", name: "{id}", zone: Industrial, level: 1, lot: (1, 1),
            households: 4, styles: ["p"], service: {svc}))"#
        )
    };
    let s = |id: &str, tier: u8, radius: f32| {
        format!(
            r#"Service((id: "{id}", name: "{id}", kind: Education, radius_m: {radius},
            capacity: {places}, tier: {tier}, site: Land, vehicles: []))"#
        )
    };
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            ("b/house.ron".into(), b("house", "None")),
            ("b/primary.ron".into(), b("primary", "Some(\"primary\")")),
            ("s/primary.ron".into(), s("primary", 1, 500.0)),
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

struct Town {
    world: World,
    schedule: Schedule,
    segs: Vec<SegmentId>,
}

fn town(places: u32) -> Town {
    let db = content(places);
    let mut world = World::new();
    world.insert_resource(Tick(0));
    world.insert_resource(SimContent(Arc::clone(&db)));
    world.init_resource::<NetGraph>();
    world.init_resource::<BuildingGrid>();
    world.init_resource::<CitizenGrid>();
    world.resource_mut::<CitizenGrid>().ensure_derived();
    ServicePlugin::init(&mut world);
    let road = db.road_id("street").unwrap();
    let def = db.road(road).clone();
    let mut segs = Vec::new();
    for k in 0..8 {
        let (a, b) = (k as f32 * 100.0, (k + 1) as f32 * 100.0);
        let curve = CubicBezier3::line(Vec3::new(a, 0.0, 0.0), Vec3::new(b, 0.0, 0.0));
        segs.push(
            world
                .resource_mut::<NetGraph>()
                .add_segment(&def, road, curve, Elevation::Ground)
                .unwrap(),
        );
    }
    let mut schedule = Schedule::default();
    schedule.add_systems(update_education);
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
            pos: Vec2::new(0.0, 5.0),
            rot: 0.0,
            access: Some(AccessPoint {
                segment: self.segs[seg],
                t: 0.5,
            }),
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        })
    }

    fn child(&mut self, home: BuildingId, years: u32, education: u8) -> CitizenId {
        let mut g = self.world.resource_mut::<CitizenGrid>();
        let h = g.add_household(
            home,
            0,
            Tick(0),
            &[(years * DAYS_PER_YEAR, education, false)],
        );
        g.household(h).unwrap().member_ids().next().unwrap()
    }

    /// A full sweep of the citizen buckets.
    fn sweep(&mut self) {
        for t in 0..256 {
            self.world.insert_resource(Tick(t));
            self.schedule.run(&mut self.world);
        }
    }

    fn school(&self, id: CitizenId) -> Option<BuildingId> {
        self.world
            .resource::<CitizenGrid>()
            .citizen(id)
            .unwrap()
            .school
    }

    fn education(&self, id: CitizenId) -> u8 {
        self.world
            .resource::<CitizenGrid>()
            .citizen(id)
            .unwrap()
            .education
    }
}

#[test]
fn a_seven_year_old_in_reach_enrols_and_one_without_a_school_does_not() {
    let mut t = town(10);
    let near = t.put("house", 1);
    let far = t.put("house", 7);
    let (a, b) = (t.child(near, 7, 0), t.child(far, 7, 0));
    t.sweep();
    assert!(
        t.school(a).is_none() && t.school(b).is_none(),
        "no school yet"
    );
    let school = t.put("primary", 0);
    t.sweep();
    assert_eq!(t.school(a), Some(school));
    assert_eq!(t.school(b), None, "700 m away is beyond the 500 m radius");
    assert_eq!(
        t.world
            .resource::<ServiceState>()
            .facility(school)
            .unwrap()
            .stored,
        1
    );
}

#[test]
fn a_school_with_one_place_takes_one_child() {
    let mut t = town(1);
    let home = t.put("house", 1);
    t.put("primary", 0);
    let kids: Vec<_> = (0..3).map(|_| t.child(home, 8, 0)).collect();
    t.sweep();
    assert_eq!(kids.iter().filter(|&&k| t.school(k).is_some()).count(), 1);
}

#[test]
fn a_pupil_who_outgrows_the_tier_graduates_with_education_one() {
    let mut t = town(10);
    let home = t.put("house", 1);
    let school = t.put("primary", 0);
    let pupil = t.child(home, 13, 0);
    t.sweep();
    assert_eq!(t.school(pupil), Some(school));
    assert_eq!(t.education(pupil), 0);
    // A year on, past the tier.
    t.world
        .resource_mut::<CitizenGrid>()
        .set_age_days(pupil, 14 * DAYS_PER_YEAR);
    t.sweep();
    assert_eq!(t.school(pupil), None);
    assert_eq!(t.education(pupil), 1);
}
