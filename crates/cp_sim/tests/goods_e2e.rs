//! T-169: goods. A factory with workers makes stock, a truck carries a load to the shop with the
//! least stock, and a shop that stays empty gets `NO_GOODS` (which abandons nothing while the
//! balance switch is off). Numbers come from a test `balance.ron`.

use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_citizen::CitizenGrid;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::{Id, Tick};
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_traffic::ServiceRuns;
use cp_transit::Goods;

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

fn building(zone: &str, id: &str) -> (String, String) {
    (
        format!("b/{id}.ron"),
        format!(
            r#"Building((id: "{id}", name: "{id}", zone: {zone}, level: 1, lot: (1, 1),
            styles: ["p"], service: None))"#
        ),
    )
}

fn content() -> Arc<ContentDb> {
    let mut src = vec![
        ("r/street.ron".into(), ROAD.into()),
        ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
        (
            "v/truck.ron".into(),
            r#"Vehicle((id: "truck", name: "truck", kind: Truck, length: 8.0, width: 2.5,
            max_speed_kmh: 60.0, accel: 1.2, decel: 4.0, min_gap: 2.0, mesh: "m"))"#
                .into(),
        ),
        (
            "balance.ron".into(),
            "Balance((goods_per_worker: 10.0, goods_per_job: 1.0, goods_truck_load: 20.0))".into(),
        ),
    ];
    src.extend([
        building("Industrial", "factory"),
        building("ComLow", "shop"),
    ]);
    Arc::new(ContentDb::from_sources(src).unwrap_or_else(|e| panic!("{e}")))
}

struct Town {
    sim: SimRunner,
    factory: BuildingId,
    shops: Vec<BuildingId>,
}

/// A 1 km street with a factory at x = 100 and shops at the given x; `workers` work in the
/// factory.
fn town(shops_at: &[f32], workers: usize) -> Town {
    let db = content();
    let mut sim = SimRunner::new(3);
    sim.set_content(Arc::clone(&db));
    let road = db.road_id("street").unwrap();
    let def = db.road(road).clone();
    let seg = sim
        .world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(Vec3::ZERO, Vec3::new(1000.0, 0.0, 0.0)),
            Elevation::Ground,
        )
        .unwrap();
    let put = |sim: &mut SimRunner, name: &str, at: f32| {
        sim.world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(Building {
                def: db.building_id(name).unwrap(),
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
                    segment: seg,
                    t: at / 1000.0,
                }),
                flags: BuildingFlags(0),
                level_progress: 0.0,
            })
    };
    let factory = put(&mut sim, "factory", 100.0);
    let shops = shops_at.iter().map(|&x| put(&mut sim, "shop", x)).collect();
    sim.tick_once();
    if workers > 0 {
        let mut c = sim.world_mut().resource_mut::<CitizenGrid>();
        let hid = c.add_household(factory, 0, Tick(0), &vec![(9000, 0, false); workers]);
        let members: Vec<_> = c
            .household(hid)
            .unwrap()
            .members
            .iter()
            .flatten()
            .copied()
            .collect();
        for m in members {
            c.set_job(m, factory, 0);
        }
    }
    Town {
        sim,
        factory,
        shops,
    }
}

fn flags(t: &Town, b: BuildingId) -> BuildingFlags {
    t.sim
        .world()
        .resource::<BuildingGrid>()
        .get(b)
        .unwrap()
        .flags
}

#[test]
fn a_truck_brings_the_factorys_goods_to_the_shop() {
    let mut t = town(&[600.0], 2);
    let mut left_factory = false;
    for _ in 0..4096 * 4 {
        t.sim.tick_once();
        let g = t.sim.world().resource::<Goods>();
        if !g.hauls.is_empty() {
            left_factory = true;
        }
        if left_factory && g.hauls.is_empty() && g.stock_of(t.shops[0]) > 0.0 {
            break;
        }
    }
    let g = t.sim.world().resource::<Goods>();
    assert!(left_factory, "a truck set out");
    assert!(g.stock_of(t.shops[0]) > 0.0, "the shop has stock");
    assert!(
        t.sim.world().resource::<ServiceRuns>().is_empty(),
        "the run is released"
    );
    assert!(!flags(&t, t.shops[0]).has(BuildingFlags::NO_GOODS));
}

#[test]
fn the_shop_with_least_stock_gets_the_truck() {
    let mut t = town(&[400.0, 700.0], 2);
    let (a, b) = (t.shops[0], t.shops[1]);
    t.sim
        .world_mut()
        .resource_mut::<Goods>()
        .stock
        .insert(a, 50.0);
    for _ in 0..4096 * 3 {
        t.sim.tick_once();
        if let Some(h) = t.sim.world().resource::<Goods>().hauls.values().next() {
            assert_eq!(h.to, b);
            return;
        }
    }
    panic!("no truck set out");
}

#[test]
fn a_shop_without_a_factory_gets_no_goods_but_is_not_abandoned() {
    let mut t = town(&[600.0], 0);
    for _ in 0..4096 * 3 {
        t.sim.tick_once();
    }
    assert!(flags(&t, t.shops[0]).has(BuildingFlags::NO_GOODS));
    let b = t
        .sim
        .world()
        .resource::<BuildingGrid>()
        .get(t.shops[0])
        .unwrap();
    assert_eq!(b.state, BuildingState::Active);
    let _ = t.factory;
}

#[test]
fn demolishing_the_shop_with_a_truck_on_the_road_fails_the_run_without_a_panic() {
    let mut t = town(&[900.0], 2);
    for _ in 0..4096 * 4 {
        t.sim.tick_once();
        if !t.sim.world().resource::<Goods>().hauls.is_empty() {
            break;
        }
    }
    assert!(!t.sim.world().resource::<Goods>().hauls.is_empty());
    let shop = t.shops[0];
    t.sim
        .world_mut()
        .resource_mut::<BuildingGrid>()
        .remove(shop);
    for _ in 0..4096 {
        t.sim.tick_once();
    }
    assert!(t.sim.world().resource::<Goods>().hauls.is_empty());
    assert!(t.sim.world().resource::<ServiceRuns>().is_empty());
}

#[test]
fn goods_are_deterministic() {
    let run = || {
        let mut t = town(&[400.0, 700.0], 2);
        for _ in 0..4096 * 4 {
            t.sim.tick_once();
        }
        t.sim.checksum()
    };
    assert_eq!(run(), run());
}
