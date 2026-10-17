//! BUG-052 probe: weekly income against upkeep of grown lattice cities. Prints a table
//! (`cargo test -p cp_sim --test econ_probe -- --nocapture --ignored`); asserts nothing.

use std::path::Path;
use std::sync::Arc;

use cp_content::ContentDb;
use cp_economy::EconomyState;
use cp_sim::scenario::{GridCityConfig, grid_city, population};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
#[ignore = "prints a balance table; run by hand"]
fn weekly_income_against_upkeep_by_population() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    println!("pop  income  roads  bldgs  utils  ratio");
    for target in [50usize, 100, 300, 1000, 3000] {
        let cfg = GridCityConfig::default();
        let (mut sim, _) = grid_city(target, 1, Arc::clone(&terrain), Arc::clone(&content), &cfg);
        for _ in 0..(cp_core::time::TICKS_PER_DAY * 8) {
            sim.tick_once();
        }
        let r = sim.world().resource::<EconomyState>().last_week;
        let (inc, up) = (r.income().0 as f64 / 100.0, r.upkeep().0 as f64 / 100.0);
        println!(
            "{:5} {:8.0} {:7.0} {:7.0} {:7.0} {:6.2}",
            population(&sim),
            inc,
            r.road_upkeep.0 as f64 / 100.0,
            r.building_upkeep.0 as f64 / 100.0,
            r.utility_upkeep.0 as f64 / 100.0,
            inc / up.max(1.0)
        );
    }
}

/// A new game with the player's starter set: a street from the entry stub, a water tower, a
/// wind turbine and a sewage plant with their pipe and line, every cell zoned. Prints the
/// treasury, population and the weekly flows day by day.
#[test]
#[ignore = "prints a balance table; run by hand"]
fn starter_city_cash_flow() {
    use bevy_math::Vec2;
    use cp_building::BuildingCommand;
    use cp_net::{ElevationMode, NetCommand, RoadPath};
    use cp_sim::SimCommand;
    use cp_zone::ZoneCommand;
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = cp_sim::SimRunner::new(1);
    sim.set_terrain(Arc::clone(&t));
    sim.set_content(Arc::clone(&content));
    sim.new_game().unwrap();
    sim.tick_once();
    let money =
        |s: &cp_sim::SimRunner| s.world().resource::<cp_core::Treasury>().balance.0 as f64 / 100.0;
    let start = money(&sim);
    let stub = t.meta().entry_road.as_ref().unwrap().stub_end();
    let (tx, tz) = t.meta().start_tile;
    let centre = Vec2::new(
        (tx as f32 + 0.5) * cp_terrain::TILE_SIZE,
        (tz as f32 + 0.5) * cp_terrain::TILE_SIZE,
    );
    let dir = (centre - stub).normalize();
    let street = content.road_id("basic_2l").unwrap();
    let send = |sim: &mut cp_sim::SimRunner, c: SimCommand| {
        let tick = sim.tick();
        sim.push_command(c, tick);
        sim.tick_once();
        sim.tick_once();
        let out: Vec<_> = sim
            .world_mut()
            .resource_mut::<bevy_ecs::message::Messages<cp_core::CommandOutcome>>()
            .drain()
            .collect();
        if let Some(Err(e)) = out.last().map(|o| o.result.clone()) {
            println!("  rejected: {e}");
        }
    };
    let n = 6;
    for k in 0..n {
        send(
            &mut sim,
            SimCommand::Net(NetCommand::BuildRoad {
                road: street,
                path: RoadPath::Straight {
                    from: stub + dir * (160.0 * k as f32),
                    to: stub + dir * (160.0 * (k + 1) as f32),
                },
                elevation: ElevationMode::Ground,
            }),
        );
    }
    let after_road = money(&sim);
    let side = dir.perp();
    let mid = stub + dir * 480.0;
    for (name, off) in [
        ("water_tower", 90.0f32),
        ("wind_turbine", -90.0),
        ("sewage_treatment_plant", 140.0),
    ] {
        let def = content.building_id(name).unwrap();
        let pos = mid + side * off;
        send(
            &mut sim,
            SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 }),
        );
        if name != "sewage_treatment_plant" {
            let kind = if name == "water_tower" {
                cp_content::UtilityKind::Water
            } else {
                cp_content::UtilityKind::Power
            };
            send(
                &mut sim,
                SimCommand::Utility(cp_utility::UtilityCommand::BuildLink {
                    kind,
                    path: vec![pos, mid + side * 12.0],
                }),
            );
        } else {
            send(
                &mut sim,
                SimCommand::Utility(cp_utility::UtilityCommand::BuildLink {
                    kind: cp_content::UtilityKind::Water,
                    path: vec![pos, mid + side * 12.0],
                }),
            );
        }
    }
    let after_services = money(&sim);
    // Zone every cell: low residential, with a block of shops and industry.
    let cells: Vec<_> = sim
        .view()
        .zones
        .cells_within(mid, 600.0)
        .into_iter()
        .filter(|c| sim.view().zones.is_valid(*c))
        .collect();
    for (i, chunk) in cells.chunks(cells.len() / 3 + 1).enumerate() {
        let zone = [
            cp_content::ZoneType::ResLow,
            cp_content::ZoneType::ComLow,
            cp_content::ZoneType::Industrial,
        ][i % 3];
        send(
            &mut sim,
            SimCommand::Zone(ZoneCommand::PaintZones {
                cells: chunk.to_vec(),
                zone,
            }),
        );
    }
    println!(
        "start {start:.0}; after street {after_road:.0} (street cost {:.0}); after services {after_services:.0} (services {:.0}); zoned {} cells",
        start - after_road,
        after_road - after_services,
        cells.len()
    );
    println!("day  balance  pop  wk_income wk_upkeep");
    for day in 1..=42u64 {
        for _ in 0..cp_core::time::TICKS_PER_DAY {
            sim.tick_once();
        }
        if day % 3 == 0 {
            let r = sim.world().resource::<EconomyState>().last_week;
            println!(
                "{day:3} {:9.0} {:4} {:9.0} {:9.0} (roads {:.0} bldg {:.0} util {:.0})",
                money(&sim),
                population(&sim),
                r.income().0 as f64 / 100.0,
                (r.upkeep().0) as f64 / 100.0,
                r.road_upkeep.0 as f64 / 100.0,
                r.building_upkeep.0 as f64 / 100.0,
                r.utility_upkeep.0 as f64 / 100.0
            );
        }
    }
}

/// BUG-052: the entry highway and rail are built free and cost no upkeep; a fresh city books
/// next to no road upkeep until the player builds a road.
#[test]
fn the_entry_roads_cost_no_upkeep() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = cp_sim::SimRunner::new(1);
    sim.set_terrain(Arc::clone(&t));
    sim.set_content(Arc::clone(&content));
    sim.new_game().unwrap();
    for _ in 0..cp_core::time::TICKS_PER_DAY * 8 {
        sim.tick_once();
    }
    let view = sim.view();
    assert!(view.net.segments().count() >= 3, "the entry roads exist");
    let r = sim.world().resource::<EconomyState>().last_week;
    assert!(r.week >= 1, "a week was settled");
    // Only the stub (the player's, a few hundred metres) is paid for; the two highways, some 15
    // km, cost about 4 000 a week before this fix.
    assert!(
        r.road_upkeep < cp_core::Cost::from_units(200.0),
        "road upkeep {:?}",
        r.road_upkeep
    );
}
