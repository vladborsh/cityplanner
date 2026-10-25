//! T-042 acceptance: levels follow land value and worker education, industry nearby lowers
//! housing, a level change keeps the lot, and buildings in trouble are abandoned after 7 days
//! and demolished after 14 without leaving the rest of the sim inconsistent.

use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Demand, Lot,
    place,
};
use cp_citizen::{CitizenGrid, TripBook};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Id, Tick};
use cp_economy::Decay;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_terrain::TileOwnership;
use cp_traffic::Agents;
use cp_zone::{ZoneBlockId, ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

fn building(id: &str, zone: &str, level: u8, lot: (u8, u8), extra: &str) -> (String, String) {
    (
        format!("b/{id}.ron"),
        format!(
            r#"Building((id: "{id}", name: "{id}", zone: {zone}, level: {level}, lot: {lot:?}, {extra}
            tax_base: 1.0, styles: ["p"]))"#
        ),
    )
}

/// Houses of levels 1-3 (lots 2x2, 2x2, 3x2) and a factory, none of which consumes water or
/// power, so no utility flag ever stops them from rising.
fn plain_content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            // No services in these towns: homes rise without them (T-146 would cap them at 2).
            (
                "balance.ron".into(),
                "Balance((res_level_services: (0, 0, 0, 0, 0)))".into(),
            ),
            building("house_1", "ResLow", 1, (2, 2), "households: 4,"),
            building("house_2", "ResLow", 2, (2, 2), "households: 6,"),
            building("house_3", "ResLow", 3, (3, 2), "households: 8,"),
            building("works", "Industrial", 1, (3, 3), "jobs: (6, 0, 0, 0),"),
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

/// One 576 m street (six 96 m pieces) along z = 3000 from x = 3000. Blocks centred left of x = 3300 are
/// residential, the others industrial.
fn street(content: &Arc<ContentDb>) -> SimRunner {
    street_zoned(content, ZoneType::ResLow)
}

/// [`street`] with the left-hand blocks painted `residential`.
fn street_zoned(content: &Arc<ContentDb>, residential: ZoneType) -> SimRunner {
    let road = content.road_id("street").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(3);
    sim.set_content(Arc::clone(content));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(0.0);
    // No water or power lines here: the levels, not the decay, are under test.
    *sim.world_mut().resource_mut::<Decay>() = Decay::disabled();
    for k in 0..6 {
        let x = 3000.0 + 96.0 * k as f32;
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(x, 0.0, 3000.0), Vec3::new(x + 96.0, 0.0, 3000.0)),
                Elevation::Ground,
            )
            .unwrap();
    }
    sim.tick_once();
    let blocks: Vec<_> = sim
        .world()
        .resource::<ZoneGrid>()
        .blocks()
        .map(|(id, b)| {
            (
                id,
                b.curve.point(0.5).x,
                b.curve.point(0.5).z,
                b.cell_count(),
            )
        })
        .collect();
    let mut zones = sim.world_mut().resource_mut::<ZoneGrid>();
    for (id, x, _, cells) in blocks {
        let zone = if x < 3300.0 {
            residential
        } else {
            ZoneType::Industrial
        };
        for cell in 0..cells {
            zones.set_zone(
                ZoneCellRef {
                    block: id,
                    cell: cell as u16,
                },
                zone,
            );
        }
    }
    sim.tick_once();
    sim
}

/// The block of `zone` whose centre is nearest to x = `x` (either side of the street).
fn block_near(sim: &SimRunner, zone: ZoneType, x: f32) -> ZoneBlockId {
    let zones = sim.world().resource::<ZoneGrid>();
    zones
        .blocks()
        .filter(|(_, b)| b.zones[0] == zone)
        .min_by(|a, b| {
            (a.1.curve.point(0.5).x - x)
                .abs()
                .total_cmp(&(b.1.curve.point(0.5).x - x).abs())
        })
        .map(|(id, _)| id)
        .expect("a block")
}

/// Puts an `Active` building of `def` at column 0 of `block`, as if it had grown there.
fn put(sim: &mut SimRunner, content: &ContentDb, def: &str, block: ZoneBlockId) -> BuildingId {
    put_at(sim, content, def, block, 0)
}

/// [`put`] at column `col`.
fn put_at(
    sim: &mut SimRunner,
    content: &ContentDb,
    def: &str,
    block: ZoneBlockId,
    col: u16,
) -> BuildingId {
    let def_id = content.building_id(def).unwrap();
    let d = content.building(def_id);
    let lot = Lot {
        block,
        col,
        w: d.lot.0,
        d: d.lot.1,
    };
    let (pos, rot, access) = {
        let w = sim.world();
        place(
            w.resource::<ZoneGrid>(),
            w.resource::<NetGraph>(),
            block,
            &lot,
        )
        .unwrap()
    };
    let cells: Vec<_> = lot.cells().collect();
    let id = sim
        .world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
            def: def_id,
            footprint: cp_building::Footprint::Lot(lot),
            level: d.level,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos,
            rot,
            access: Some(access),
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
    sim.world_mut().resource_mut::<ZoneGrid>().occupy(&cells);
    let _ = AccessPoint {
        segment: Id::new(0, 0),
        t: 0.0,
    };
    id
}

fn level(sim: &SimRunner, id: BuildingId) -> u8 {
    sim.world()
        .resource::<BuildingGrid>()
        .get(id)
        .unwrap()
        .level
}

fn run(sim: &mut SimRunner, ticks: u64) {
    for _ in 0..ticks {
        sim.tick_once();
    }
}

#[test]
fn industry_nearby_lowers_the_level_of_housing() {
    let content = plain_content();
    let mut sim = street(&content);
    let far = block_near(&sim, ZoneType::ResLow, 3048.0);
    let near = block_near(&sim, ZoneType::ResLow, 3240.0);
    let works = block_near(&sim, ZoneType::Industrial, 3336.0);
    let a = put(&mut sim, &content, "house_3", far);
    // At the end of its block, beside the works at the start of the next (T-300: industry
    // reaches 8 zone cells, 64 m).
    let b = put_at(&mut sim, &content, "house_3", near, 9);
    put(&mut sim, &content, "works", works);
    assert_eq!((level(&sim, a), level(&sim, b)), (3, 3));

    // Three days: the ground under the house comes up to the smoke (T-300).
    run(&mut sim, 3 * cp_core::time::TICKS_PER_DAY);
    let grids = sim.world().resource::<cp_economy::LandGrids>();
    let at = |id| sim.world().resource::<BuildingGrid>().get(id).unwrap().pos;
    assert!(
        grids.land_value_for_test(at(b)) < grids.land_value_for_test(at(a)),
        "the smoke costs land value"
    );
    assert_eq!(level(&sim, a), 3, "the clean one keeps its level");
    assert!(level(&sim, b) < 3, "the one beside the factory drops");
    // A lower level is rebuilt inside the same lot.
    let nb = *sim
        .world()
        .resource::<BuildingGrid>()
        .get(b)
        .unwrap()
        .lot()
        .expect("a lot");
    assert_eq!((nb.block, nb.col), (near, 9));
}

#[test]
fn a_level_up_keeps_the_lot_and_the_people() {
    let content = plain_content();
    let mut sim = street(&content);
    let block = block_near(&sim, ZoneType::ResLow, 3048.0);
    let id = put(&mut sim, &content, "house_1", block);
    sim.world_mut()
        .resource_mut::<CitizenGrid>()
        .ensure_derived();
    sim.world_mut().resource_mut::<CitizenGrid>().add_household(
        id,
        0,
        Tick(0),
        &[(30 * 365, 0, false), (3 * 365, 0, false)],
    );
    let before = *sim
        .world()
        .resource::<BuildingGrid>()
        .get(id)
        .unwrap()
        .lot()
        .expect("a lot");
    assert_eq!(level(&sim, id), 1);

    run(&mut sim, 12 * 256);
    // The clean street is worth 50: levels 1 -> 2 -> 3 (45), not 4 (55).
    let b = sim
        .world()
        .resource::<BuildingGrid>()
        .get(id)
        .unwrap()
        .clone();
    assert_eq!(b.level, 3);
    assert_eq!(content.building(b.def).level, 3);
    assert_eq!(
        (b.lot().expect("a lot").block, b.lot().expect("a lot").col),
        (before.block, before.col),
        "same anchor"
    );
    assert!(b.lot().expect("a lot").w >= before.w && b.lot().expect("a lot").d >= before.d);
    assert_eq!(b.state, BuildingState::Active);
    // Its cells, and only its cells, are occupied; the grown lot took free neighbours.
    let zones = sim.world().resource::<ZoneGrid>();
    let cells: Vec<_> = b.lot().expect("a lot").cells().collect();
    assert!(cells.iter().all(|c| zones.is_occupied(*c)));
    assert_eq!(
        zones
            .block_cells(block)
            .into_iter()
            .filter(|c| zones.is_occupied(*c))
            .count(),
        cells.len()
    );
    // The household is still home and the building is still where its lot says.
    let c = sim.world().resource::<CitizenGrid>();
    assert_eq!(c.household_count(), 1);
    assert_eq!(c.households_in(id), 1);
    let pos = b.pos;
    let (p, _, _) = place(
        zones,
        sim.world().resource::<NetGraph>(),
        b.lot().expect("a lot").block,
        b.lot().expect("a lot"),
    )
    .unwrap();
    assert_eq!(pos, p);
}

/// A small growing town without water or power lines: everything is in trouble.
#[test]
fn a_city_goes_through_abandonment_and_demolition_consistently() {
    let content = content();
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(5);
    sim.set_content(Arc::clone(&content));
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(1.0);
    *sim.world_mut().resource_mut::<Decay>() = Decay::disabled();
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        for (a, b) in [
            ((3000.0, 3000.0), (3090.0, 3000.0)),
            ((3045.0, 2940.0), (3045.0, 3060.0)),
            ((3000.0, 3090.0), (3090.0, 3090.0)),
        ] {
            g.add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1)),
                Elevation::Ground,
            )
            .unwrap();
        }
    }
    sim.tick_once();
    let kinds = [ZoneType::ResLow, ZoneType::Industrial, ZoneType::ComLow];
    {
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = grid.blocks().map(|(i, b)| (i, b.cell_count())).collect();
        for (n, (id, cells)) in ids.into_iter().enumerate() {
            for cell in 0..cells {
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    kinds[n % kinds.len()],
                );
            }
        }
    }
    run(&mut sim, 5 * TICKS_PER_DAY);
    let grown = sim.world().resource::<CitizenGrid>().citizen_count();
    assert!(grown > 20, "{grown} citizens");
    assert!(
        sim.world()
            .resource::<BuildingGrid>()
            .iter()
            .any(|(_, b)| b.flags.has(BuildingFlags::NO_WATER)),
        "no water lines: the utility flags are up"
    );

    // From now on nothing new is built, and the trouble counts. Align to a 256-tick pass.
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(0.0);
    *sim.world_mut().resource_mut::<Decay>() = Decay::default();
    while !sim.tick().0.is_multiple_of(256) {
        sim.tick_once();
    }
    let t0 = sim.tick().0;
    let total = sim.world().resource::<BuildingGrid>().len();
    let in_trouble = |sim: &SimRunner| {
        sim.world()
            .resource::<BuildingGrid>()
            .iter()
            .filter(|(_, b)| b.state == BuildingState::Abandoned)
            .count()
    };

    run(&mut sim, 35 * TICKS_PER_DAY - 300);
    assert_eq!(
        in_trouble(&sim),
        0,
        "not yet 35 days (T-282: abandon_days 7 -> 35)"
    );
    run(&mut sim, 600);
    let abandoned = in_trouble(&sim);
    assert!(
        abandoned > 0,
        "after 35 days the buildings in trouble are abandoned"
    );
    // T-295: fire_base x2, and with no fire station a house may burn down within the 35 days;
    // an abandoned one is never demolished yet (it is after demolish_days).
    let left = sim.world().resource::<BuildingGrid>().len();
    assert!(
        left + 3 >= total && left <= total,
        "none demolished yet: {left} of {total}"
    );
    // The people of abandoned homes have gone (the citizens' next update evicts them).
    run(&mut sim, 600);
    {
        let b = sim.world().resource::<BuildingGrid>();
        let c = sim.world().resource::<CitizenGrid>();
        for (_, h) in c.households() {
            assert_eq!(b.get(h.home).map(|b| b.state), Some(BuildingState::Active));
        }
    }

    // Day 70 (T-282: was 14): demolished, one pass after the buildings were first seen in trouble at the latest.
    while sim.tick().0 < t0 + 70 * TICKS_PER_DAY + 512 {
        sim.tick_once();
        let b = sim.world().resource::<BuildingGrid>();
        let c = sim.world().resource::<CitizenGrid>();
        for (_, h) in c.households() {
            assert!(b.get(h.home).is_some(), "a household of a removed building");
        }
        for (_, cit) in c.citizens() {
            assert!(
                cit.work.is_none_or(|w| b.get(w).is_some()),
                "a job in a removed building"
            );
        }
    }
    assert!(
        sim.world().resource::<BuildingGrid>().is_empty(),
        "all demolished"
    );
    assert!(sim.world().resource::<Decay>().since.is_empty());
    let zones = sim.world().resource::<ZoneGrid>();
    assert!(
        zones
            .blocks()
            .all(|(id, _)| zones.block_cells(id).iter().all(|c| !zones.is_occupied(*c)))
    );
    run(&mut sim, 600);
    assert_eq!(sim.world().resource::<CitizenGrid>().citizen_count(), 0);
    assert!(sim.world().resource::<TripBook>().is_empty());
    let a = sim.world().resource::<Agents>();
    assert_eq!(a.vehicle_count() + a.pedestrian_count(), 0);
    // And the sim keeps running.
    run(&mut sim, TICKS_PER_DAY);
}

#[test]
fn the_view_carries_land_value_and_pollution() {
    let content = plain_content();
    let mut sim = street(&content);
    run(&mut sim, 2 * 256);
    let v = sim.view();
    let land = v.overlays.land_value.as_ref().expect("land value");
    let poll = v.overlays.pollution.as_ref().expect("pollution");
    assert_eq!(land.side, cp_economy::SIDE);
    assert_eq!(poll.values.len(), cp_economy::SIDE * cp_economy::SIDE);
    // The street lifts the land around it above the bare 0.2.
    let cell = |x: f32, z: f32| (z / 64.0) as usize * land.side + (x / 64.0) as usize;
    assert!(land.values[cell(3100.0, 3000.0)] > land.values[cell(100.0, 100.0)]);
    assert_eq!(land.values[cell(100.0, 100.0)], 0.2);
    let _ = Vec2::ZERO;
}

/// Like [`plain_content`] with tall homes (ResHigh, levels 1-3) and the HighRiseBan policy (2).
fn high_content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            (
                "balance.ron".into(),
                "Balance((res_level_services: (0, 0, 0, 0, 0)))".into(),
            ),
            building("high_1", "ResHigh", 1, (2, 2), "households: 4,"),
            building("high_2", "ResHigh", 2, (2, 2), "households: 6,"),
            building("high_3", "ResHigh", 3, (3, 2), "households: 8,"),
            cp_content::test_policy_source("HighRiseBan", 2.0),
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

#[test]
fn a_high_rise_ban_stops_tall_homes_at_level_two_and_leaves_others_growing() {
    use cp_content::PolicyKind;
    use cp_district::{DistrictCommand, DistrictId};

    let content = high_content();
    let mut sim = street_zoned(&content, ZoneType::ResHigh);
    let inside = block_near(&sim, ZoneType::ResHigh, 3048.0);
    let outside = block_near(&sim, ZoneType::ResHigh, 3240.0);
    let a = put(&mut sim, &content, "high_1", inside);
    let b = put(&mut sim, &content, "high_1", outside);
    // The district covers x 2880..3104 (cells 90..=96), not the other block at x ~3240.
    let cells: Vec<_> = (90..=96)
        .flat_map(|c| (90..=97).map(move |r| (c, r)))
        .collect();
    for cmd in [
        DistrictCommand::Create {
            name: "Calm".into(),
        },
        DistrictCommand::Paint {
            cells,
            district: DistrictId(1),
        },
        DistrictCommand::SetPolicy {
            district: Some(DistrictId(1)),
            policy: PolicyKind::HighRiseBan,
            on: true,
        },
    ] {
        cp_district::apply(sim.world_mut(), &cmd).unwrap();
    }
    run(&mut sim, 12 * 256);
    // The clean street is worth 50: both would reach level 3.
    assert_eq!(level(&sim, b), 3, "outside the district it grows to 3");
    assert_eq!(level(&sim, a), 2, "inside it stops at the ban's level");
    // The ban is not a demolition: switching it off lets the building go on.
    cp_district::apply(
        sim.world_mut(),
        &DistrictCommand::SetPolicy {
            district: Some(DistrictId(1)),
            policy: PolicyKind::HighRiseBan,
            on: false,
        },
    )
    .unwrap();
    run(&mut sim, 12 * 256);
    assert_eq!(level(&sim, a), 3);
}
