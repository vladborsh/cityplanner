//! T-196: district specialisations. In a district specialised for commercial only the
//! specialised shops spawn, outside only the ordinary ones; switching a specialisation on
//! rebuilds at most `respec_per_day` buildings a day and switching it off brings the ordinary
//! ones back; a specialised building never rises; a Tourism shop draws more tourists.

use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{
    Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Demand, Lot, place,
};
use cp_content::{ContentDb, SpecKind, ZoneType};
use cp_core::Tick;
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_district::{DistrictCommand, DistrictId};
use cp_economy::Decay;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_terrain::TileOwnership;
use cp_zone::{ZoneBlockId, ZoneCellRef, ZoneGrid};

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

fn building(id: &str, level: u8, extra: &str) -> (String, String) {
    (
        format!("b/{id}.ron"),
        format!(
            r#"Building((id: "{id}", name: "{id}", zone: ComLow, level: {level}, lot: (2, 2),
            jobs: (2, 0, 0, 0), tax_base: 1.0, styles: ["p"], {extra}))"#
        ),
    )
}

/// Ordinary shops of levels 1 and 2, and the Tourism and Organic shops (level 1).
fn content() -> Arc<ContentDb> {
    let spec = |kind: &str, weight: f32| {
        (
            format!("sp/{kind}.ron"),
            format!(
                r#"Spec((kind: {kind}, name: "{kind}", icon: "i", zones: [ComLow, ComHigh],
                tax_mul: 1.0, tourist_weight: {weight:?}, noise_add: 0.0, land_value_add: 0.0,
                goods_mul: 1.0))"#
            ),
        )
    };
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            building("shop_1", 1, ""),
            building("shop_2", 2, ""),
            building("spec_tourism_s", 1, "spec: Some(Tourism),"),
            building("spec_organic_s", 1, "spec: Some(Organic),"),
            spec("Tourism", 3.0),
            spec("Organic", 1.0),
            (
                "v/car.ron".into(),
                r#"Vehicle((id: "car", name: "car", kind: Car, length: 4.5, width: 1.8,
                max_speed_kmh: 60.0, accel: 2.0, decel: 4.0, min_gap: 2.0, mesh: "m"))"#
                    .into(),
            ),
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

/// A 576 m street along z = 3000 from x = 3000, every block painted commercial.
fn street(content: &Arc<ContentDb>) -> SimRunner {
    street_with(content, false)
}

/// [`street`]; with `outside` its west end is an `Outside` node (an entry highway).
fn street_with(content: &Arc<ContentDb>, outside: bool) -> SimRunner {
    let road = content.road_id("street").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(3);
    sim.set_content(Arc::clone(content));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(0.0);
    *sim.world_mut().resource_mut::<Decay>() = Decay::disabled();
    if outside {
        sim.world_mut().resource_mut::<NetGraph>().add_node(
            Vec3::new(3000.0, 0.0, 3000.0),
            cp_net::NodeKind::Outside,
            Elevation::Ground,
        );
    }
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
        .map(|(id, b)| (id, b.cell_count()))
        .collect();
    let mut zones = sim.world_mut().resource_mut::<ZoneGrid>();
    for (id, cells) in blocks {
        for cell in 0..cells {
            zones.set_zone(
                ZoneCellRef {
                    block: id,
                    cell: cell as u16,
                },
                ZoneType::ComLow,
            );
        }
    }
    sim.tick_once();
    sim
}

/// Puts an `Active` building of `def` at column `col` of `block`.
fn put(
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
    id
}

fn run(sim: &mut SimRunner, ticks: u64) {
    for _ in 0..ticks {
        sim.tick_once();
    }
}

fn set(sim: &mut SimRunner, cmd: DistrictCommand) {
    cp_district::apply(sim.world_mut(), &cmd).unwrap();
}

/// A district covering every cell of the street's blocks west of x = 3300.
fn west_district(sim: &mut SimRunner, spec: Option<SpecKind>) {
    let cells: Vec<_> = (90..=102)
        .flat_map(|c| (90..=97).map(move |r| (c, r)))
        .collect();
    set(
        sim,
        DistrictCommand::Create {
            name: "West".into(),
        },
    );
    set(
        sim,
        DistrictCommand::Paint {
            cells,
            district: DistrictId(1),
        },
    );
    if let Some(spec) = spec {
        set_spec(sim, Some(spec));
    }
}

fn set_spec(sim: &mut SimRunner, spec: Option<SpecKind>) {
    set(
        sim,
        DistrictCommand::SetSpec {
            district: DistrictId(1),
            zone: ZoneType::ComLow,
            spec,
        },
    );
}

/// `(ordinary, specialised)` buildings standing in the district (x < 3300) and outside it.
fn census(sim: &SimRunner, db: &ContentDb) -> [(usize, usize); 2] {
    let mut out = [(0, 0); 2];
    for (_, b) in sim.world().resource::<BuildingGrid>().iter() {
        let side = usize::from(b.pos.x >= 3300.0);
        if db.building(b.def).spec.is_some() {
            out[side].1 += 1;
        } else {
            out[side].0 += 1;
        }
    }
    out
}

#[test]
fn a_specialised_district_grows_only_its_own_shops_and_the_rest_only_ordinary_ones() {
    let db = content();
    let mut sim = street(&db);
    west_district(&mut sim, Some(SpecKind::Tourism));
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(1.0);
    run(&mut sim, 4000);
    let [west, east] = census(&sim, &db);
    assert!(west.1 >= 3 && west.0 == 0, "west: {west:?}");
    assert!(east.0 >= 3 && east.1 == 0, "east: {east:?}");
    // Only the Tourism shops, not the Organic ones.
    let organic = db.building_id("spec_organic_s").unwrap();
    assert!(
        sim.world()
            .resource::<BuildingGrid>()
            .iter()
            .all(|(_, b)| b.def != organic)
    );
}

#[test]
fn switching_a_specialisation_on_rebuilds_four_a_day_and_off_brings_the_ordinary_ones_back() {
    let db = content();
    let mut sim = street(&db);
    // Ten ordinary shops in the west, then the district.
    let blocks: Vec<ZoneBlockId> = sim
        .world()
        .resource::<ZoneGrid>()
        .blocks()
        .filter(|(_, b)| b.curve.point(0.5).x < 3300.0)
        .map(|(id, _)| id)
        .collect();
    let mut n = 0;
    'fill: for &block in &blocks {
        for col in [0u16, 2, 4] {
            put(&mut sim, &db, "shop_1", block, col);
            n += 1;
            if n == 10 {
                break 'fill;
            }
        }
    }
    assert_eq!(n, 10);
    // No spawning while the rebuild is counted.
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(0.0);
    west_district(&mut sim, None);
    let ordinary_west = |sim: &SimRunner| census(sim, &db)[0].0;
    assert_eq!(ordinary_west(&sim), 10);
    set_spec(&mut sim, Some(SpecKind::Tourism));
    let day = TICKS_PER_DAY;
    let t0 = sim.tick().0;
    let first_day_end = (t0 / day + 1) * day;
    run(&mut sim, first_day_end - t0 + 1);
    assert_eq!(ordinary_west(&sim), 6, "four rebuilt on the first day");
    run(&mut sim, day);
    assert_eq!(ordinary_west(&sim), 2, "four more on the second");
    run(&mut sim, day);
    assert_eq!(ordinary_west(&sim), 0, "the rest on the third");

    // Demand returns: the specialised shops are built in the district, and when the
    // specialisation goes they are rebuilt as ordinary ones, four a day.
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(1.0);
    run(&mut sim, 3000);
    let spec_before = census(&sim, &db)[0].1;
    assert!(spec_before >= 3, "{spec_before} specialised shops");
    set_spec(&mut sim, None);
    run(&mut sim, 12 * day);
    let [west, _] = census(&sim, &db);
    assert_eq!(west.1, 0, "no specialised shop is left");
    assert!(west.0 >= 3, "ordinary shops took their place: {west:?}");
}

#[test]
fn a_specialised_shop_never_rises_while_an_ordinary_one_does() {
    let db = content();
    let mut sim = street(&db);
    let block = sim
        .world()
        .resource::<ZoneGrid>()
        .blocks()
        .map(|(id, _)| id)
        .next()
        .unwrap();
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(0.0);
    let spec = put(&mut sim, &db, "spec_tourism_s", block, 0);
    let plain = put(&mut sim, &db, "shop_1", block, 4);
    // T-301: under a day (2048 ticks): the daily respec would take the specialised shop (it
    // stands outside a district) at the first day tick. 7 passes are 6 level checks and one more.
    run(&mut sim, 7 * 256);
    let level = |id| {
        sim.world()
            .resource::<BuildingGrid>()
            .get(id)
            .unwrap()
            .level
    };
    assert_eq!(level(spec), 1, "specialised: stays at level 1");
    assert_eq!(level(plain), 2, "ordinary: rose");
}

#[test]
fn the_same_district_gives_the_same_city() {
    let db = content();
    let build = || {
        let mut sim = street(&db);
        west_district(&mut sim, Some(SpecKind::Tourism));
        *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(1.0);
        run(&mut sim, 3000);
        sim.checksum()
    };
    assert_eq!(build(), build());
}

#[test]
fn a_tourism_shop_draws_more_tourists_than_an_organic_one_nearer_the_entry() {
    use cp_transit::Tourists;
    let db = content();
    let mut sim = street_with(&db, true);
    // Two districts, so neither shop is rebuilt: Organic (weight 1) in the west beside the
    // entry, Tourism (weight 3) in the east further along the street.
    west_district(&mut sim, Some(SpecKind::Organic));
    let east: Vec<_> = (103..=125)
        .flat_map(|c| (90..=97).map(move |r| (c, r)))
        .collect();
    set(
        &mut sim,
        DistrictCommand::Create {
            name: "East".into(),
        },
    );
    set(
        &mut sim,
        DistrictCommand::Paint {
            cells: east,
            district: DistrictId(2),
        },
    );
    set(
        &mut sim,
        DistrictCommand::SetSpec {
            district: DistrictId(2),
            zone: ZoneType::ComLow,
            spec: Some(SpecKind::Tourism),
        },
    );
    let blocks: Vec<(ZoneBlockId, f32)> = sim
        .world()
        .resource::<ZoneGrid>()
        .blocks()
        .map(|(id, b)| (id, b.curve.point(0.5).x))
        .collect();
    let at = |west: bool| {
        blocks
            .iter()
            .find(|(_, x)| (*x < 3300.0) == west)
            .unwrap()
            .0
    };
    let organic = put(&mut sim, &db, "spec_organic_s", at(true), 0);
    let touristy = put(&mut sim, &db, "spec_tourism_s", at(false), 0);
    run(&mut sim, 6 * TICKS_PER_DAY);
    let t = sim.world().resource::<Tourists>();
    let visits = |id| t.customers.get(&id).copied().unwrap_or(0);
    let (a, b) = (visits(touristy), visits(organic));
    assert!(a + b >= 8, "{} visits", a + b);
    assert!(a > b * 3 / 2, "{a} against {b}");
}
