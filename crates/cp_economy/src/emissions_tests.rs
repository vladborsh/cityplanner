//! T-300 acceptance (§11.1): the falloff of a source, the ground rising and falling, which
//! buildings are sources, and road noise from highways only. Real content.

use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_traffic::SegmentLoad;

use crate::emissions::{CELL_E, EmissionInputs, Emissions, cell_of, emission_of};

/// Passes of `update_city` in a game day.
const DAY: u32 = (cp_core::time::TICKS_PER_DAY / crate::CITY_PERIOD) as u32;

fn db() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap_or_else(|e| panic!("{e}")),
    )
}

fn place(grid: &mut BuildingGrid, db: &ContentDb, def: &str, pos: Vec2) -> BuildingId {
    let def = db.building_id(def).unwrap_or_else(|| panic!("no {def}"));
    let d = db.building(def);
    grid.insert(Building {
        def,
        footprint: Footprint::Lot(Lot {
            block: cp_core::Id::new(0, 0),
            col: 0,
            w: d.lot.0,
            d: d.lot.1,
        }),
        level: d.level,
        state: BuildingState::Active,
        ready_at: cp_core::Tick(0),
        pos,
        rot: 0.0,
        access: None,
        flags: BuildingFlags::default(),
        level_progress: 0.0,
    })
}

fn step(
    e: &mut Emissions,
    db: &ContentDb,
    grid: &BuildingGrid,
    net: &NetGraph,
    load: &SegmentLoad,
    n: u32,
) {
    for _ in 0..n {
        e.update(&EmissionInputs {
            content: db,
            buildings: grid,
            net,
            load,
        });
    }
}

/// The centre of a 16 m cell near the middle of the map, `dx` cells east.
fn at_cell(dx: f32) -> Vec2 {
    Vec2::new(4992.0 + CELL_E * 0.5 + dx * CELL_E, 4992.0 + CELL_E * 0.5)
}

#[test]
fn a_source_falls_off_linearly_to_its_radius() {
    let db = db();
    let mut grid = BuildingGrid::default();
    // Zone industry: 50 within 8 zone cells (64 m).
    place(&mut grid, &db, "ind_1", at_cell(0.0));
    let mut e = Emissions::default();
    step(
        &mut e,
        &db,
        &grid,
        &NetGraph::default(),
        &SegmentLoad::default(),
        1,
    );
    let src = |zone_cells: f32| e.pollution_src[cell_of(at_cell(zone_cells / 2.0))];
    assert!((src(0.0) - 50.0).abs() < 1e-3, "{}", src(0.0));
    assert!((src(4.0) - 25.0).abs() < 1e-3, "{}", src(4.0));
    assert_eq!(src(8.0), 0.0);
    // Noise the same way (60 / 8).
    assert!((e.noise_at(at_cell(1.0)) - 45.0).abs() < 1e-3);
}

#[test]
fn the_ground_comes_up_in_three_days_and_is_gone_ten_days_after_the_source() {
    let db = db();
    let mut grid = BuildingGrid::default();
    // A coal plant: 90 (100 takes ten days to go, so a weaker source is gone sooner).
    let id = place(&mut grid, &db, "coal_power_plant", at_cell(0.0));
    let (net, load) = (NetGraph::default(), SegmentLoad::default());
    let here = cell_of(at_cell(0.0));
    let mut e = Emissions::default();
    step(&mut e, &db, &grid, &net, &load, DAY);
    let day1 = f32::from(e.ground[here]) / 100.0;
    assert!(day1 < 45.0, "under half after a day: {day1}");
    step(&mut e, &db, &grid, &net, &load, 2 * DAY);
    let day3 = f32::from(e.ground[here]) / 100.0;
    assert!(
        day3 >= 0.95 * 90.0,
        "at the source after three days: {day3}"
    );
    // The factory goes: the ground stays a while.
    grid.remove(id);
    step(&mut e, &db, &grid, &net, &load, 5 * DAY);
    assert!(e.ground[here] > 0, "five days on");
    assert_eq!(e.pollution_src[here], 0.0);
    step(&mut e, &db, &grid, &net, &load, 5 * DAY);
    assert_eq!(e.ground[here], 0, "ten days on");
}

#[test]
fn only_the_listed_buildings_pollute_or_make_noise() {
    let db = db();
    let of = |id: &str| emission_of(db.building(db.building_id(id).unwrap()), &db);
    for quiet in [
        "elementary_school",
        "police_station",
        "house_low_1",
        "office_1",
        "water_pump_station",
    ] {
        assert_eq!(of(quiet), (None, None), "{quiet}");
    }
    for loud in [
        "stadium",
        "train_station",
        "wind_turbine",
        "com_high_1",
        "com_low_1",
    ] {
        assert!(of(loud).1.is_some(), "{loud} makes noise");
        assert!(of(loud).0.is_none(), "{loud} does not pollute");
    }
    for dirty in ["ind_1", "landfill", "incinerator", "coal_power_plant"] {
        assert!(of(dirty).0.is_some(), "{dirty} pollutes");
    }
    assert_eq!(
        of("stadium").1.map(|e| (e.strength, e.radius_cells)),
        Some((90.0, 8))
    );
}

#[test]
fn a_loaded_highway_is_loud_and_a_loaded_street_is_not() {
    let db = db();
    let mut net = NetGraph::default();
    let mut add = |road: &str, z: f32| {
        let id = db.road_id(road).unwrap();
        net.add_segment(
            db.road(id),
            id,
            CubicBezier3::line(Vec3::new(4800.0, 0.0, z), Vec3::new(5200.0, 0.0, z)),
            Elevation::Ground,
        )
        .unwrap()
    };
    let street = add("basic_2l", 4000.0);
    let highway = add("highway_1w_2l", 6000.0);
    let mut ema = vec![0.0; street.index.max(highway.index) as usize + 1];
    ema[street.index as usize] = 1.0;
    ema[highway.index as usize] = 1.0;
    let load = SegmentLoad {
        ema,
        ..Default::default()
    };
    let mut e = Emissions::default();
    step(&mut e, &db, &BuildingGrid::default(), &net, &load, 1);
    assert_eq!(e.noise_at(Vec2::new(5000.0, 4000.0)), 0.0);
    assert!(e.noise_at(Vec2::new(5000.0, 6000.0)) > 30.0);
}
