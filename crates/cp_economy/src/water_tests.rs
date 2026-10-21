//! T-300 step 2 acceptance: a pump on polluted ground. Exactly the homes of its water pool get
//! BAD_WATER and fall ill more often; a home on another pump's pool gets neither; once the ground
//! has cleared, so has the flag. The supply, city and health systems run in the sim's order
//! (Utilities before Economy).

use std::sync::Arc;

use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule};
use bevy_ecs::world::World;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, SimContent};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Id, SimRng, Tick};
use cp_service::{ServicePlugin, ServiceProblems, ServiceState, update_coverage};
use cp_traffic::Agents;
use cp_utility::{PowerPools, UtilityGraph, UtilityNet, WaterPools, update_supply};
use cp_zone::ZoneGrid;

use crate::emissions::cell_of;
use crate::{Decay, Emissions, LandGrids, WorkerShortage, update_city, update_health};

fn content() -> Arc<ContentDb> {
    let b = |id: &str, svc: &str, cons: &str| {
        format!(
            r#"Building((id: "{id}", name: "{id}", zone: ResLow, level: 1, lot: (1, 1),
            consumption: {cons}, households: 4, styles: ["p"], service: {svc}))"#
        )
    };
    Arc::new(
        ContentDb::from_sources(vec![
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            ("b/house.ron".into(), b("house", "None", "{Water: 1.0}")),
            ("b/pump.ron".into(), b("pump", "Some(\"pump\")", "{}")),
            (
                "s/pump.ron".into(),
                r#"Service((id: "pump", name: "pump", kind: Water, radius_m: 100.0,
                capacity: 1000, vehicles: []))"#
                    .into(),
            ),
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

struct Town {
    world: World,
    schedule: Schedule,
    tick: u64,
}

fn put(world: &mut World, db: &ContentDb, def: &str, x: f32) -> BuildingId {
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
        pos: Vec2::new(5000.0 + x, 5010.0),
        rot: 0.0,
        access: None,
        flags: BuildingFlags(0),
        level_progress: 0.0,
    })
}

/// Two pipes 400 m apart, a pump and two homes on each.
fn town() -> (Town, [BuildingId; 2], [BuildingId; 4]) {
    let db = content();
    let mut world = World::new();
    world.insert_resource(Tick(0));
    world.insert_resource(SimContent(Arc::clone(&db)));
    world.insert_resource(SimRng::from_seed([3; 32]));
    for r in [
        |w: &mut World| w.init_resource::<cp_net::NetGraph>(),
        |w: &mut World| w.init_resource::<BuildingGrid>(),
        |w: &mut World| w.init_resource::<CitizenGrid>(),
        |w: &mut World| w.init_resource::<UtilityGraph>(),
        |w: &mut World| w.init_resource::<Agents>(),
        |w: &mut World| w.init_resource::<ZoneGrid>(),
        |w: &mut World| w.init_resource::<LandGrids>(),
        |w: &mut World| w.init_resource::<Emissions>(),
        |w: &mut World| w.init_resource::<Decay>(),
        |w: &mut World| w.init_resource::<WorkerShortage>(),
        |w: &mut World| w.init_resource::<PowerPools>(),
        |w: &mut World| w.init_resource::<WaterPools>(),
    ] {
        r(&mut world);
    }
    ServicePlugin::init(&mut world);
    {
        let mut g = world.resource_mut::<UtilityGraph>();
        for x0 in [0.0, 400.0] {
            let p = |x: f32| Vec3::new(5000.0 + x, 0.0, 5000.0);
            let a = g.add_node(UtilityNet::Pipe, p(x0));
            let b = g.add_node(UtilityNet::Pipe, p(x0 + 100.0));
            g.add_link(a, b, CubicBezier3::line(p(x0), p(x0 + 100.0)))
                .unwrap();
        }
    }
    let pumps = [
        put(&mut world, &db, "pump", 0.0),
        put(&mut world, &db, "pump", 400.0),
    ];
    let homes = [
        put(&mut world, &db, "house", 50.0),
        put(&mut world, &db, "house", 80.0),
        put(&mut world, &db, "house", 450.0),
        put(&mut world, &db, "house", 480.0),
    ];
    let age = 30 * cp_citizen::DAYS_PER_YEAR;
    {
        let mut g = world.resource_mut::<CitizenGrid>();
        g.ensure_derived();
        for home in homes {
            for _ in 0..50 {
                g.add_household(home, 0, Tick(0), &[(age, 1, false); 4]);
            }
        }
    }
    let mut schedule = Schedule::default();
    schedule.add_systems((update_supply, update_city, update_coverage, update_health).chain());
    (
        Town {
            world,
            schedule,
            tick: 0,
        },
        pumps,
        homes,
    )
}

impl Town {
    fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.world.insert_resource(Tick(self.tick));
            self.schedule.run(&mut self.world);
            self.tick += 1;
        }
    }

    fn bad(&self, id: BuildingId) -> bool {
        self.world
            .resource::<ServiceState>()
            .problems_of(id)
            .has(ServiceProblems::BAD_WATER)
    }

    fn sick(&self, home: BuildingId) -> usize {
        self.world
            .resource::<CitizenGrid>()
            .citizens()
            .filter(|(_, c)| c.home == Some(home) && c.sick_since.is_some())
            .count()
    }
}

#[test]
fn the_homes_of_a_pump_on_polluted_ground_get_bad_water_and_fall_ill() {
    let (mut t, pumps, homes) = town();
    let at = t
        .world
        .resource::<BuildingGrid>()
        .get(pumps[0])
        .unwrap()
        .pos;
    // Ground of 60 under the west pump (a factory stood there), nothing at the homes.
    t.world.resource_mut::<Emissions>().ground[cell_of(at)] = 6_000;
    t.run(3 * TICKS_PER_DAY);
    assert!(t.bad(homes[0]) && t.bad(homes[1]), "the west pool");
    assert!(!t.bad(homes[2]) && !t.bad(homes[3]), "the east pool");
    assert!(!t.bad(pumps[0]), "a pump drinks nothing");
    let west = t.sick(homes[0]) + t.sick(homes[1]);
    let east = t.sick(homes[2]) + t.sick(homes[3]);
    assert!(west > 2 * east + 5, "west {west}, east {east}");

    // Without a source the ground clears within its decay days, and the flag goes with it.
    let decay = t
        .world
        .resource::<SimContent>()
        .0
        .balance()
        .pollution_decay_days;
    t.run((decay * TICKS_PER_DAY as f32) as u64);
    assert_eq!(t.world.resource::<Emissions>().ground[cell_of(at)], 0);
    assert!(!t.bad(homes[0]) && !t.bad(homes[1]));
}
