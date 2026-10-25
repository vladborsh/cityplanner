//! T-321b (§22.7 items 2 to 4): hotel guests. A lodger sleeps N nights, makes N outings (N
//! more visits to the shops), pays N nights and leaves; a full hotel turns tourists away and one
//! without power takes nobody; a demolished hotel leaves no guest or visit behind.

use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Cost, Id, Tick};
use cp_net::{Elevation, NetGraph, NodeId, NodeKind, SegmentId};
use cp_sim::SimRunner;
use cp_transit::{HotelState, Lodged, Stay, TouristPhase, Tourists, TransitLedger};

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

fn customers(sim: &SimRunner) -> u32 {
    sim.world().resource::<Tourists>().customers.values().sum()
}

fn guests(sim: &SimRunner, hotel: BuildingId) -> u16 {
    sim.world().resource::<HotelState>().slots[&hotel].guests
}

/// No tourists come by themselves: only the lodger the test puts in.
/// The test street has no pavement: the guests drive to every goal.
const NO_FLOW: &str = "tourism_base: 0.0, tourism_per_com: 0.0, tourism: (walk_m: 0.0)";

/// §22.7 item 2: one guest of three nights makes three outings (three more shop visits),
/// pays three nights into the ledger and leaves; the room is free again.
#[test]
fn a_guest_sleeps_goes_out_each_day_pays_and_leaves() {
    let db = content(20, NO_FLOW);
    let (mut sim, hotel, entry) = town(&db);
    let nights = 3;
    let stay = Stay {
        hotel,
        check_out: Tick((nights * 24 + 11) * TICKS_PER_DAY / 24),
        by_train: false,
    };
    {
        let w = sim.world_mut();
        w.resource_mut::<HotelState>()
            .slots
            .get_mut(&hotel)
            .unwrap()
            .guests = 1;
        w.resource_mut::<Tourists>()
            .lodged
            .insert(Tick(10 * TICKS_PER_DAY / 24), vec![Lodged { stay, entry }]);
    }
    days(&mut sim, nights);
    assert_eq!(guests(&sim, hotel), 1, "still in on the last night");
    assert_eq!(customers(&sim), nights as u32, "an outing a day");
    let paid = sim.world().resource::<TransitLedger>().hotels;
    assert_eq!(paid, Cost::scaled(NIGHT, nights as f64));
    let slot = sim.world().resource::<HotelState>().slots[&hotel];
    assert_eq!(slot.nights_week, nights as u32);
    assert_eq!(slot.revenue_week, paid);
    days(&mut sim, 1);
    assert_eq!(guests(&sim, hotel), 0, "checked out");
    let t = sim.world().resource::<Tourists>();
    assert!(t.lodged.is_empty());
    assert!(t.visits.values().all(|v| v.stay.is_none()));
    assert_eq!(
        customers(&sim),
        nights as u32,
        "no outing after the check-out"
    );
}

/// §22.7 item 3: with every new tourist wanting a room, a hotel of two rooms fills, the rest
/// stay for the day and are counted as turned away; a hotel without power takes nobody.
#[test]
fn a_full_hotel_turns_tourists_away_and_one_without_power_takes_nobody() {
    let db = content(2, "tourism: (stay_share: 1.0)");
    let (mut sim, hotel, _) = town(&db);
    days(&mut sim, 1);
    let t = sim.world().resource::<Tourists>();
    assert!(guests(&sim, hotel) <= 2);
    assert!(guests(&sim, hotel) > 0, "the hotel took guests");
    assert!(t.turned_away_week > 0, "{}", t.turned_away_week);
    assert!(t.visits_week > 0, "the others visit for the day");

    let (mut dark, hotel, _) = town(&db);
    // The supply pass sets the flags each tick: keep the hotel dark as a city without power.
    for _ in 0..TICKS_PER_DAY {
        if let Some(b) = dark
            .world_mut()
            .resource_mut::<BuildingGrid>()
            .get_mut(hotel)
        {
            b.flags = BuildingFlags(BuildingFlags::NO_POWER);
        }
        dark.tick_once();
    }
    assert_eq!(guests(&dark, hotel), 0);
    assert!(dark.world().resource::<Tourists>().turned_away_week > 0);
}

/// §22.7 item 4: the hotel is demolished with guests in it and on the way: no lodger and no
/// visit refers to it afterwards, and the same run gives the same checksum.
#[test]
fn a_demolished_hotel_leaves_no_guest_behind() {
    let run = || {
        let db = content(20, "tourism: (stay_share: 1.0)");
        let (mut sim, hotel, _) = town(&db);
        days(&mut sim, 1);
        assert!(guests(&sim, hotel) > 0);
        sim.world_mut().resource_mut::<BuildingGrid>().remove(hotel);
        days(&mut sim, 1);
        let t = sim.world().resource::<Tourists>();
        assert!(t.lodged.values().flatten().all(|l| l.stay.hotel != hotel));
        assert!(t.visits.values().all(|v| {
            v.stay.is_none_or(|s| s.hotel != hotel)
                && v.goal != hotel
                && v.phase != TouristPhase::Outing { back_to: hotel }
        }));
        assert!(
            !sim.world()
                .resource::<HotelState>()
                .slots
                .contains_key(&hotel)
        );
        sim.checksum()
    };
    assert_eq!(run(), run());
}
