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
            boost: &[],
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

#[test]
fn a_grey_deal_makes_its_factory_pollute_more() {
    let db = db();
    let mut grid = BuildingGrid::default();
    let id = place(&mut grid, &db, "ind_1", at_cell(0.0));
    let mut e = Emissions::default();
    e.update(&EmissionInputs {
        content: &db,
        buildings: &grid,
        net: &NetGraph::default(),
        load: &SegmentLoad::default(),
        boost: &[(id, 0.5)],
    });
    let src = e.pollution_src[cell_of(at_cell(0.0))];
    assert!((src - 75.0).abs() < 1e-3, "50 × 1.5: {src}");
}

/// A loaded highway along `z = 6000` and the segment's id; `trees` set on it by the caller.
fn loud_highway(db: &ContentDb, trees: bool) -> (NetGraph, SegmentLoad) {
    let mut net = NetGraph::default();
    let id = db.road_id("highway_1w_2l").unwrap();
    let seg = net
        .add_segment(
            db.road(id),
            id,
            CubicBezier3::line(
                Vec3::new(4850.0, 0.0, 6000.0),
                Vec3::new(5150.0, 0.0, 6000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    net.set_trees(seg, trees).unwrap();
    let mut ema = vec![0.0; seg.index as usize + 1];
    ema[seg.index as usize] = 1.0;
    (
        net,
        SegmentLoad {
            ema,
            ..Default::default()
        },
    )
}

#[test]
fn a_tree_row_cuts_the_noise_of_a_highway_near_it_and_not_beyond_its_reach() {
    let db = db();
    let (bare, load) = loud_highway(&db, false);
    let (green, _) = loud_highway(&db, true);
    let at = |dz: f32| Vec2::new(5000.0, 6000.0 + dz);
    let mut a = Emissions::default();
    let mut b = Emissions::default();
    step(&mut a, &db, &BuildingGrid::default(), &bare, &load, 1);
    step(&mut b, &db, &BuildingGrid::default(), &green, &load, 1);
    // 8 m from the row (a sample point of it, every 32 m): 0.7 of the bare value (§24.4 item 1).
    let (n0, n1) = (a.noise_at(at(8.0)), b.noise_at(at(8.0)));
    assert!(
        n0 > 0.0 && (0.55..0.75).contains(&(n1 / n0)),
        "{n0} -> {n1}"
    );
    // 48 m away: the row's reach (32 m) is over, the noise is as it was.
    assert_eq!(a.noise_at(at(48.0)), b.noise_at(at(48.0)));
    // The row taken away (the bare net again): the next pass gives the bare value back.
    step(&mut b, &db, &BuildingGrid::default(), &bare, &load, 1);
    assert_eq!(b.noise_at(at(8.0)), n0);
}

#[test]
fn a_tree_row_cuts_the_pollution_source_and_the_ground_follows_slowly() {
    let db = db();
    let id = db.road_id("basic_2l").unwrap();
    let mut grid = BuildingGrid::default();
    let pos = at_cell(0.0);
    place(&mut grid, &db, "ind_1", pos);
    let mut net = NetGraph::default();
    let seg = net
        .add_segment(
            db.road(id),
            id,
            CubicBezier3::line(
                Vec3::new(pos.x, 0.0, pos.y),
                Vec3::new(pos.x + 200.0, 0.0, pos.y),
            ),
            Elevation::Ground,
        )
        .unwrap();
    let load = SegmentLoad::default();
    let here = cell_of(pos);
    let mut e = Emissions::default();
    step(&mut e, &db, &grid, &net, &load, 3 * DAY);
    let before = e.pollution_src[here];
    let ground = e.ground[here];
    assert!(before > 0.0 && ground > 0);
    net.set_trees(seg, true).unwrap();
    e.rebuild_fields(&EmissionInputs {
        content: &db,
        buildings: &grid,
        net: &net,
        load: &load,
        boost: &[],
    });
    // On the row the source is cut by the full 25 %; the ground has not moved yet.
    let cut = e.pollution_src[here];
    assert!((cut - before * 0.75).abs() < 1e-3, "{before} -> {cut}");
    assert_eq!(e.ground[here], ground);
    step(&mut e, &db, &grid, &net, &load, 1);
    assert!(e.ground[here] < ground && f32::from(e.ground[here]) > cut * 100.0);
}

/// Land value of a ground road along `z` for each `(z, trees)`, read at `at`.
fn land_at(db: &ContentDb, rows: &[(f32, bool)], at: Vec2) -> f32 {
    let id = db.road_id("basic_2l").unwrap();
    let mut net = NetGraph::default();
    for &(z, trees) in rows {
        let seg = net
            .add_segment(
                db.road(id),
                id,
                CubicBezier3::line(Vec3::new(1000.0, 0.0, z), Vec3::new(1300.0, 0.0, z)),
                Elevation::Ground,
            )
            .unwrap();
        net.set_trees(seg, trees).unwrap();
    }
    let grid = BuildingGrid::default();
    let mut ema = Emissions::default();
    ema.update(&EmissionInputs {
        content: db,
        buildings: &grid,
        net: &net,
        load: &SegmentLoad::default(),
        boost: &[],
    });
    crate::LandGrids::compute(&crate::LandInputs {
        content: db,
        buildings: &grid,
        net: &net,
        utilities: &cp_utility::UtilityGraph::default(),
        emissions: &ema,
        services: None,
        parks: None,
    })
    .value_at(at)
}

#[test]
fn a_tree_row_lifts_the_land_beside_it_and_two_rows_do_not_add() {
    let db = db();
    // A cell centre (64 m cells) on the road line.
    let on = Vec2::new(1184.0, 1056.0);
    let bare = land_at(&db, &[(1056.0, false)], on);
    let one = land_at(&db, &[(1056.0, true)], on);
    let tree_land = db.balance().tree_land;
    // The sample points of the row are a few metres from the cell centre: nearly all of +6.
    assert!(
        one - bare > 0.8 * tree_land && one - bare <= tree_land + 1e-3,
        "+6 on the row: {bare} -> {one}"
    );
    // A second row 40 m off: the maximum counts, not the sum.
    let two = land_at(&db, &[(1056.0, true), (1096.0, true)], on);
    assert!(two - bare < tree_land + 1e-3, "{bare} -> {two}");
    assert!(two >= one - 1e-3);
}
