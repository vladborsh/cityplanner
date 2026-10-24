//! Mail (T-247): piling up, collection by a post office, the sorting backlog, locked, vans.

use std::sync::Arc;

use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule};
use bevy_ecs::world::World;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, ServiceKind, ServiceUnlocks, SimContent, VehicleKind};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Id, Tick};
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_traffic::{RunPoint, RunState, ServiceRuns};

use super::*;

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

/// A post office (collects `office` a day, `vans` vans) and a sorting facility (sorts `sorting`).
fn content(office: u32, vans: u32, sorting: u32) -> Arc<ContentDb> {
    let b = |id: &str, households: u32, svc: &str| {
        format!(
            r#"Building((id: "{id}", name: "{id}", zone: Industrial, level: 1, lot: (1, 1),
            households: {households}, styles: ["p"], service: {svc}))"#
        )
    };
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            cp_content::test_policy_source("Recycling", 0.3),
            (
                "balance.ron".into(),
                "Balance((mail_household: 20.0, mail_job: 0.0, mail_van_min: 10.0,
                    mail_problem: 50.0, mail_post_sort: 100.0, mail_backlog_days: 2.0))"
                    .into(),
            ),
            ("b/house.ron".into(), b("house", 4, "None")),
            ("b/post.ron".into(), b("post", 0, "Some(\"post\")")),
            ("b/sort.ron".into(), b("sort", 0, "Some(\"sort\")")),
            (
                "s/post.ron".into(),
                format!(
                    r#"Service((id: "post", name: "post", kind: Post, radius_m: 500.0,
                    capacity: {office}, storage: {vans}, tier: 1, site: Land, vehicles: []))"#
                ),
            ),
            (
                "s/sort.ron".into(),
                format!(
                    r#"Service((id: "sort", name: "sort", kind: Post, radius_m: 0.0,
                    capacity: {sorting}, tier: 2, site: Land, vehicles: []))"#
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

fn town(office: u32, vans: u32, sorting: u32) -> Town {
    let db = content(office, vans, sorting);
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
    schedule.add_systems(
        (
            reset_daily_capacity,
            update_coverage,
            update_mail_sorting,
            update_mail,
        )
            .chain(),
    );
    Town {
        world,
        schedule,
        segs,
    }
}

impl Town {
    fn put(&mut self, def: &str, seg: usize, flags: u16) -> BuildingId {
        let db = self.world.resource::<SimContent>().0.clone();
        let id = self.world.resource_mut::<BuildingGrid>().insert(Building {
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
            flags: BuildingFlags(flags),
            level_progress: 0.0,
        });
        if def == "house" {
            self.world.resource_mut::<CitizenGrid>().add_household(
                id,
                0,
                Tick(0),
                &[(9000, 1, false)],
            );
        }
        id
    }

    /// Runs the ticks `from..to`.
    fn run(&mut self, from: u64, to: u64) {
        for t in from..to {
            self.world.insert_resource(Tick(t));
            self.schedule.run(&mut self.world);
        }
    }

    fn mail(&self, id: BuildingId) -> u16 {
        self.world.resource::<MailState>().pile(id)
    }

    fn flagged(&self, id: BuildingId) -> bool {
        self.world
            .resource::<ServiceState>()
            .problems_of(id)
            .has(ServiceProblems::MAIL)
    }

    fn state(&self) -> MailState {
        self.world.resource::<MailState>().clone()
    }

    fn facility(&self, id: BuildingId) -> Facility {
        self.world
            .resource::<ServiceState>()
            .facility(id)
            .copied()
            .unwrap_or_default()
    }
}

const DAY: u64 = TICKS_PER_DAY;

#[test]
fn a_post_office_in_reach_collects_the_mail_and_there_is_no_problem() {
    let mut t = town(400, 0, 1);
    let house = t.put("house", 0, 0);
    let post = t.put("post", 1, 0);
    t.run(0, 4 * DAY);
    // 20 a day made, picked up as it passes 10: never near the threshold of 50.
    assert!(t.mail(house) < 10, "{}", t.mail(house));
    assert!(!t.flagged(house));
    let m = t.state();
    assert!(
        m.yesterday.made >= 19 && m.yesterday.made <= 21,
        "{:?}",
        m.yesterday
    );
    assert!(m.yesterday.collected >= 18, "{:?}", m.yesterday);
    assert_eq!(m.yesterday.collected, m.yesterday.delivered);
    assert!(m.yesterday.sorted >= 18, "the office sorts its 100 a day");
    assert!(m.unsorted < 20);
    assert_eq!(m.sort_capacity, 100);
    assert!(t.facility(post).used_today > 0);
}

#[test]
fn without_a_post_office_the_mail_piles_up_into_a_problem() {
    let mut t = town(400, 0, 1);
    let house = t.put("house", 0, 0);
    t.run(0, 2 * DAY);
    assert_eq!(t.mail(house), 40);
    assert!(!t.flagged(house), "40 is below the threshold of 50");
    t.run(2 * DAY, 4 * DAY);
    assert_eq!(t.mail(house), 80);
    assert!(t.flagged(house));
    assert_eq!(t.world.resource::<ServiceState>().summary().counts[5], 1);
}

#[test]
fn a_big_town_without_a_sorting_facility_gets_a_backlog_and_stops_being_collected() {
    // Ten houses make 200 a day; the one office sorts 100.
    let mut t = town(400, 0, 1000);
    let houses: Vec<_> = (0..10).map(|k| t.put("house", k % 4, 0)).collect();
    let post = t.put("post", 1, 0);
    let sort = t.put("sort", 2, BuildingFlags::NO_POWER);
    t.run(0, 6 * DAY);
    let m = t.state();
    assert_eq!(m.sort_capacity, 100, "the facility has no power");
    assert!(m.backlog_full(2.0), "{} unsorted", m.unsorted);
    assert!(
        houses.iter().any(|&h| t.flagged(h)),
        "mail stops being collected"
    );
    // The power comes back: the backlog is sorted away and the collecting goes on.
    t.world
        .resource_mut::<BuildingGrid>()
        .get_mut(sort)
        .unwrap()
        .flags = BuildingFlags(0);
    t.run(6 * DAY, 10 * DAY);
    let m = t.state();
    assert_eq!(m.sort_capacity, 1100);
    assert!(!m.backlog_full(2.0));
    assert!(m.unsorted < 300, "{}", m.unsorted);
    assert!(houses.iter().all(|&h| !t.flagged(h)));
    assert!(t.facility(post).used_today > 0);
}

#[test]
fn a_sorting_facility_keeps_a_big_town_clear() {
    let mut t = town(400, 0, 1000);
    let houses: Vec<_> = (0..10).map(|k| t.put("house", k % 4, 0)).collect();
    let _post = t.put("post", 1, 0);
    let _sort = t.put("sort", 2, 0);
    t.run(0, 6 * DAY);
    assert!(!t.state().backlog_full(2.0));
    assert!(houses.iter().all(|&h| !t.flagged(h)));
}

#[test]
fn while_the_post_office_is_locked_there_is_no_mail_and_no_problem() {
    let mut t = town(400, 0, 1);
    let house = t.put("house", 0, 0);
    let mut unlocks = ServiceUnlocks::default();
    unlocks.set(ServiceKind::Post, false);
    t.world.insert_resource(unlocks);
    t.run(0, 4 * DAY);
    assert_eq!(t.mail(house), 0);
    assert!(!t.flagged(house));
    assert_eq!(t.state().today.made, 0);
    // Open: the mail starts.
    t.world.insert_resource(ServiceUnlocks::default());
    t.run(4 * DAY, 8 * DAY);
    assert!(t.flagged(house));
}

#[test]
fn a_demolished_building_loses_its_mail() {
    let mut t = town(400, 0, 1);
    let house = t.put("house", 0, 0);
    t.run(0, DAY);
    assert!(t.mail(house) > 0);
    t.world.resource_mut::<MailState>().remove(house);
    assert_eq!(t.mail(house), 0);
}

#[test]
fn with_runs_a_van_is_sent_and_the_mail_is_taken_when_it_arrives() {
    let mut t = town(400, 1, 1);
    t.world.init_resource::<ServiceRuns>();
    let house = t.put("house", 0, 0);
    let other = t.put("house", 1, 0);
    let post = t.put("post", 1, 0);
    // Past the first passes: 20 a day reach the 10 for a van.
    t.run(0, DAY);
    let runs = t.world.resource::<ServiceRuns>();
    let (run, r) = runs
        .find(VehicleKind::Mail, house)
        .expect("a van for the house");
    assert_eq!(r.from, RunPoint::Building(post));
    // One van only: the other house waits.
    assert!(runs.find(VehicleKind::Mail, other).is_none());
    assert_eq!(t.facility(post).stored, 1);
    let held = t.mail(house);
    assert!(held >= 10);
    // Nothing is taken while the van drives.
    assert_eq!(t.state().today.collected, 0);
    t.world
        .resource_mut::<ServiceRuns>()
        .set_state(run, RunState::Arrived { at: Tick(DAY) });
    t.run(DAY, DAY + 64);
    assert!(t.mail(house) < held);
    assert!(t.state().today.collected >= u32::from(held));
    assert!(
        t.world
            .resource::<ServiceRuns>()
            .find(VehicleKind::Mail, house)
            .is_none()
    );
}

#[test]
fn the_same_town_twice_gives_the_same_mail() {
    let run = || {
        let mut t = town(400, 2, 1000);
        t.world.init_resource::<ServiceRuns>();
        for k in 0..8 {
            t.put("house", k % 4, 0);
        }
        t.put("post", 1, 0);
        t.put("sort", 2, 0);
        t.run(0, 3 * DAY);
        (t.state(), t.world.resource::<ServiceState>().clone())
    };
    assert_eq!(run(), run());
}
