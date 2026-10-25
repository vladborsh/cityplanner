//! T-321b (§22.7 item 6): a save in the middle of a day, with guests in their rooms and on an
//! outing, loads into the same state: two more days give the checksum of the run that never
//! stopped.

use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Id, Tick};
use cp_net::{Elevation, NetGraph, NodeId, NodeKind, SegmentId};
use cp_save::{load_into_runner, save_runner};
use cp_sim::SimRunner;
use cp_transit::{HotelState, Tourists};

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

/// A night in the test hotel, currency units.
const NIGHT: f32 = 40.0;

/// Shops, a hotel of `rooms`, a car, and the tourism knobs `tourism` (the flow knobs too).
fn content(rooms: u16, balance: &str) -> Arc<ContentDb> {
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            (
                "b/shop.ron".into(),
                r#"Building((id: "shop", name: "shop", zone: ComLow, level: 1, lot: (1, 1),
                styles: ["p"], service: None))"#
                    .into(),
            ),
            (
                "b/hotel.ron".into(),
                format!(
                    r#"Building((id: "hotel", name: "hotel", zone: Industrial, level: 1,
                    lot: (2, 2), styles: ["p"], hotel: Some((rooms: {rooms}, stars: 2,
                    price_night: {NIGHT:?}))))"#
                ),
            ),
            (
                "v/car.ron".into(),
                r#"Vehicle((id: "car", name: "car", kind: Car, length: 4.5, width: 1.8,
                max_speed_kmh: 60.0, accel: 2.0, decel: 4.0, min_gap: 2.0, mesh: "m"))"#
                    .into(),
            ),
            ("balance.ron".into(), format!("Balance(({balance}))")),
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

/// A 1 km street from an `Outside` node with ten shops and the hotel in the middle.
fn town(db: &Arc<ContentDb>) -> (SimRunner, BuildingId, NodeId) {
    let mut sim = SimRunner::new(7);
    sim.set_content(Arc::clone(db));
    let road = db.road_id("street").unwrap();
    let def = db.road(road).clone();
    let entry = sim.world_mut().resource_mut::<NetGraph>().add_node(
        Vec3::ZERO,
        NodeKind::Outside,
        Elevation::Ground,
    );
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
    for i in 0..10 {
        put(&mut sim, db, "shop", seg, 150.0 + 80.0 * i as f32);
    }
    let hotel = put(&mut sim, db, "hotel", seg, 520.0);
    sim.tick_once();
    (sim, hotel, entry)
}

fn put(sim: &mut SimRunner, db: &ContentDb, def: &str, seg: SegmentId, at: f32) -> BuildingId {
    sim.world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
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
            pos: Vec2::new(at, 5.0),
            rot: 0.0,
            access: Some(AccessPoint {
                segment: seg,
                t: at / 1000.0,
            }),
            flags: BuildingFlags(0),
            level_progress: 0.0,
        })
}

fn days(sim: &mut SimRunner, n: u64) {
    for _ in 0..n * TICKS_PER_DAY {
        sim.tick_once();
    }
}

#[test]
fn a_save_with_guests_in_and_out_loads_into_the_same_run() {
    let db = content(20, "tourism: (stay_share: 1.0, walk_m: 0.0)");
    let (mut a, hotel, _) = town(&db);
    days(&mut a, 1);
    // On through the second day until guests are both in their rooms and out on an outing.
    let both = |sim: &SimRunner| {
        let t = sim.world().resource::<Tourists>();
        !t.lodged.is_empty() && t.visits.values().any(|v| v.stay.is_some())
    };
    while !both(&a) {
        a.tick_once();
        assert!(a.tick().0 < 2 * TICKS_PER_DAY, "never a guest out");
    }
    assert!(a.world().resource::<HotelState>().slots[&hotel].guests > 0);
    let bytes = save_runner(&a, db.content_hash(), "test").unwrap();
    let mut b = SimRunner::new(7);
    b.set_content(Arc::clone(&db));
    load_into_runner(&mut b, &bytes, db.content_hash()).unwrap();
    assert_eq!(b.checksum(), a.checksum());
    days(&mut a, 2);
    days(&mut b, 2);
    assert_eq!(b.checksum(), a.checksum());
}
