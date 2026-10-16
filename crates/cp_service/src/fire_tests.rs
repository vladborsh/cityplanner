//! Fire: an unattended fire burns the building down and it is demolished; a station in reach
//! puts it out; engines are limited; fire spreads to a neighbour.

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
use cp_core::time::{TICKS_PER_DAY, TICKS_PER_HOUR};
use cp_core::{Id, SimRng, Tick};
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_zone::ZoneGrid;

use super::*;

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

/// Fires are lit by hand (`fire_base` 0); `extinguish` hours of work per engine, `spread` per hour.
fn content(engines: u32, extinguish: f32, spread: f32) -> Arc<ContentDb> {
    let b = |id: &str, svc: &str| {
        format!(
            r#"Building((id: "{id}", name: "{id}", zone: Industrial, level: 1, lot: (1, 1),
            styles: ["p"], service: {svc}))"#
        )
    };
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            ("b/house.ron".into(), b("house", "None")),
            ("b/station.ron".into(), b("station", "Some(\"station\")")),
            (
                "s/station.ron".into(),
                format!(
                    r#"Service((id: "station", name: "station", kind: Fire, radius_m: 500.0,
                    capacity: {engines}, site: Land, vehicles: []))"#
                ),
            ),
            (
                "balance.ron".into(),
                format!(
                    "Balance((fire_base: 0.0, fire_extinguish_hours: {extinguish}, fire_spread: {spread}))"
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

fn town(engines: u32, extinguish: f32, spread: f32) -> Town {
    let db = content(engines, extinguish, spread);
    let mut world = World::new();
    world.insert_resource(Tick(0));
    world.insert_resource(SimRng::from_seed([7; 32]));
    world.insert_resource(SimContent(Arc::clone(&db)));
    world.init_resource::<NetGraph>();
    world.init_resource::<BuildingGrid>();
    world.init_resource::<CitizenGrid>();
    world.init_resource::<ZoneGrid>();
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
    schedule.add_systems((reset_daily_capacity, update_coverage, update_fire).chain());
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

    fn light(&mut self, id: BuildingId) {
        let st = &mut *self.world.resource_mut::<ServiceState>();
        let s = st.get_mut(id);
        s.fire = Some(Fire {
            since: Tick(0),
            out_at: None,
            station: None,
        });
        s.problems.set(ServiceProblems::FIRE, true);
    }

    fn run(&mut self, from: u64, to: u64) {
        for t in from..to {
            self.world.insert_resource(Tick(t));
            self.schedule.run(&mut self.world);
        }
    }

    fn state(&self, id: BuildingId) -> Option<BuildingState> {
        self.world
            .resource::<BuildingGrid>()
            .get(id)
            .map(|b| b.state)
    }

    fn burning(&self, id: BuildingId) -> bool {
        self.world
            .resource::<ServiceState>()
            .problems_of(id)
            .has(ServiceProblems::FIRE)
    }

    fn engines_out(&self, station: BuildingId) -> u32 {
        self.world
            .resource::<ServiceState>()
            .facility(station)
            .map_or(0, |f| f.stored)
    }
}

const HOURS_8: u64 = (8.0 * TICKS_PER_HOUR) as u64;

#[test]
fn without_a_station_a_burning_house_burns_down_and_is_then_demolished() {
    let mut t = town(3, 2.0, 0.0);
    let house = t.put("house", 0);
    t.light(house);
    t.run(0, HOURS_8);
    assert_eq!(t.state(house), Some(BuildingState::Burned));
    assert!(!t.burning(house), "a burned building is not burning");
    t.run(HOURS_8, 3 * TICKS_PER_DAY);
    assert_eq!(t.state(house), None, "demolished after burned_days");
}

#[test]
fn a_station_in_reach_puts_the_fire_out_and_gets_its_engine_back() {
    let mut t = town(3, 2.0, 0.0);
    let house = t.put("house", 0);
    let station = t.put("station", 1);
    t.light(house);
    t.run(0, 128);
    assert!(t.burning(house));
    assert_eq!(t.engines_out(station), 1, "an engine is on its way");
    t.run(128, HOURS_8);
    assert_eq!(t.state(house), Some(BuildingState::Active));
    assert!(!t.burning(house));
    assert_eq!(t.engines_out(station), 0);
}

#[test]
fn two_engines_for_three_fires_leave_the_third_to_burn_down() {
    // Work takes 20 hours, so no engine is back before the unattended fire has burned out.
    let mut t = town(2, 20.0, 0.0);
    let houses: Vec<_> = (0..3).map(|_| t.put("house", 0)).collect();
    let station = t.put("station", 1);
    for &h in &houses {
        t.light(h);
    }
    t.run(0, HOURS_8);
    let burned = houses
        .iter()
        .filter(|&&h| t.state(h) == Some(BuildingState::Burned))
        .count();
    assert_eq!(burned, 1);
    assert_eq!(t.engines_out(station), 2);
    assert_eq!(
        t.state(houses[2]),
        Some(BuildingState::Burned),
        "last in id order"
    );
}

#[test]
fn a_burning_house_ignites_its_neighbour_when_the_spread_chance_is_one() {
    let mut t = town(3, 2.0, 1.0);
    let a = t.put("house", 0);
    let b = t.put("house", 0);
    t.light(a);
    t.run(0, 256);
    assert!(t.burning(b));
    // A neighbour out of reach stays safe.
    let mut far = town(3, 2.0, 1.0);
    let a = far.put("house", 0);
    let c = far.put("house", 3);
    far.light(a);
    far.run(0, 256);
    assert!(!far.burning(c));
}
