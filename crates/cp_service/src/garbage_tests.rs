//! Garbage: piling up, collection within the daily capacity, full stores, a dead incinerator.

use std::sync::Arc;

use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule};
use bevy_ecs::world::World;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, SimContent};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
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

/// `dump(capacity, storage)` is a landfill-like service; `burn` has no store.
fn content(capacity: u32, storage: u32) -> Arc<ContentDb> {
    let b = |id: &str, households: u32, svc: &str| {
        format!(
            r#"Building((id: "{id}", name: "{id}", zone: Industrial, level: 1, lot: (1, 1),
            households: {households}, styles: ["p"], service: {svc}))"#
        )
    };
    let s = |id: &str, capacity: u32, storage: u32| {
        format!(
            r#"Service((id: "{id}", name: "{id}", kind: Garbage, radius_m: 500.0,
            capacity: {capacity}, storage: {storage}, site: Land, vehicles: []))"#
        )
    };
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            // The numbers these tests are written for, not the tuned game defaults.
            (
                "balance.ron".into(),
                "Balance((garbage_household: 25.0, garbage_job: 4.0, garbage_problem: 100.0))"
                    .into(),
            ),
            ("b/house.ron".into(), b("house", 4, "None")),
            ("b/dump.ron".into(), b("dump", 0, "Some(\"dump\")")),
            ("b/burn.ron".into(), b("burn", 0, "Some(\"burn\")")),
            ("s/dump.ron".into(), s("dump", capacity, storage)),
            ("s/burn.ron".into(), s("burn", capacity, 0)),
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
    schedule.add_systems((reset_daily_capacity, update_coverage, update_garbage).chain());
    Town {
        world,
        schedule,
        segs,
    }
}

impl Town {
    fn put(&mut self, def: &str, seg: usize, flags: u8) -> BuildingId {
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

    fn garbage(&self, id: BuildingId) -> u16 {
        self.world
            .resource::<ServiceState>()
            .get(id)
            .map_or(0, |s| s.garbage)
    }

    fn flagged(&self, id: BuildingId) -> bool {
        self.world
            .resource::<ServiceState>()
            .problems_of(id)
            .has(ServiceProblems::GARBAGE)
    }

    fn facility(&self, id: BuildingId) -> Facility {
        self.world
            .resource::<ServiceState>()
            .facility(id)
            .copied()
            .unwrap_or_default()
    }
}

const TWO_DAYS: u64 = 2 * TICKS_PER_DAY;

#[test]
fn without_a_landfill_a_house_has_garbage_after_two_days() {
    let mut t = town(400, 20000);
    let house = t.put("house", 0, 0);
    t.run(0, TWO_DAYS);
    // One household makes 25 a day.
    assert_eq!(t.garbage(house), 50);
    assert!(!t.flagged(house), "50 is below the threshold of 100");
    t.run(TWO_DAYS, 4 * TICKS_PER_DAY);
    assert_eq!(t.garbage(house), 100);
    assert!(t.flagged(house));
}

#[test]
fn a_landfill_in_reach_takes_the_garbage_away_and_clears_the_flag() {
    let mut t = town(400, 20000);
    let house = t.put("house", 0, 0);
    let dump = t.put("dump", 1, 0);
    // Pile up past the threshold first, with the landfill out of the way.
    t.world
        .resource_mut::<ServiceState>()
        .get_mut(house)
        .garbage = 500;
    t.world
        .resource_mut::<ServiceState>()
        .get_mut(house)
        .problems
        .set(ServiceProblems::GARBAGE, true);
    t.run(0, TWO_DAYS);
    assert_eq!(t.garbage(house), 0);
    assert!(!t.flagged(house));
    assert_eq!(t.facility(dump).stored, 550);
}

#[test]
fn a_capacity_of_one_a_day_serves_one_unit_in_total() {
    let mut t = town(1, 20000);
    let houses: Vec<_> = (0..3).map(|k| t.put("house", k, 0)).collect();
    let dump = t.put("dump", 3, 0);
    t.run(0, TICKS_PER_DAY - 1);
    // Whoever came first got the one unit; the others are untouched.
    assert_eq!(t.facility(dump).used_today, 1);
    let made: u16 = houses.iter().map(|&h| t.garbage(h)).sum();
    assert_eq!(made, 3 * 25 - 1);
}

#[test]
fn a_full_landfill_takes_no_more() {
    let mut t = town(400, 30);
    let house = t.put("house", 0, 0);
    let dump = t.put("dump", 1, 0);
    t.run(0, TWO_DAYS);
    assert_eq!(t.facility(dump).stored, 30);
    assert_eq!(t.garbage(house), 50 - 30);
}

#[test]
fn an_incinerator_without_power_does_not_work() {
    let mut t = town(800, 0);
    let house = t.put("house", 0, 0);
    let burn = t.put("burn", 1, BuildingFlags::NO_POWER);
    t.run(0, TWO_DAYS);
    assert_eq!(t.garbage(house), 50);
    assert_eq!(t.facility(burn).used_today, 0);
    // The power comes back: it burns what it can reach.
    t.world
        .resource_mut::<BuildingGrid>()
        .get_mut(burn)
        .unwrap()
        .flags = BuildingFlags(0);
    t.run(TWO_DAYS, 3 * TICKS_PER_DAY);
    assert_eq!(t.garbage(house), 0);
    assert_eq!(t.facility(burn).used_today, 75);
}
