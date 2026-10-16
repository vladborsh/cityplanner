//! T-040 acceptance: purchases go through the treasury. A build with too little money is an
//! `Err` that changes nothing and costs nothing; one that fits is charged exactly its cost.
//! The weekly settlement runs on the sim tick, replays bit for bit and survives a save.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingCommand, BuildingGrid};
use cp_content::{ContentDb, ServiceKind, UtilityKind, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_WEEK;
use cp_core::treasury::DEBT_FLOOR;
use cp_core::{CommandError, CommandLog, CommandOutcome, Cost, Tick, Treasury};
use cp_economy::{EconomyCommand, EconomyState};
use cp_net::{Elevation, ElevationMode, NetCommand, NetGraph, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_utility::{UtilityCommand, UtilityGraph};
use cp_zone::ZoneGrid;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

/// A flat map with the first tile owned and content loaded.
fn sim() -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain);
    sim.set_content(content());
    sim
}

fn balance(sim: &SimRunner) -> Cost {
    sim.world().resource::<Treasury>().balance
}

fn set_balance(sim: &mut SimRunner, c: Cost) {
    sim.world_mut().resource_mut::<Treasury>().balance = c;
}

/// Runs `cmd` at the current tick without advancing it; returns its outcome.
fn run(sim: &mut SimRunner, cmd: SimCommand) -> Result<Cost, CommandError> {
    sim.push_command(cmd, sim.tick());
    sim.apply_due_commands();
    sim.world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .last()
        .unwrap()
        .result
}

fn road_cmd(len: f32) -> SimCommand {
    road_at(4600.0, len)
}

fn road_at(z: f32, len: f32) -> SimCommand {
    let db = content();
    SimCommand::Net(NetCommand::BuildRoad {
        road: db.road_id("basic_2l").unwrap(),
        path: RoadPath::Straight {
            from: Vec2::new(4600.0, z),
            to: Vec2::new(4600.0 + len, z),
        },
        elevation: ElevationMode::Ground,
    })
}

#[test]
fn a_road_without_money_is_an_error_that_changes_nothing() {
    let mut sim = sim();
    // 100 m of basic street costs 1000.00; 500.00 of headroom is not enough.
    set_balance(&mut sim, Cost(DEBT_FLOOR.0 + 50_000));
    let err = run(&mut sim, road_cmd(100.0)).unwrap_err();
    assert_eq!(
        err,
        CommandError::InsufficientFunds {
            needed: Cost(100_000),
            available: Cost(50_000)
        }
    );
    assert_eq!(sim.world().resource::<NetGraph>().segments().count(), 0);
    assert_eq!(balance(&sim), Cost(DEBT_FLOOR.0 + 50_000));
    assert!(sim.world().resource::<CommandLog<SimCommand>>().is_empty());

    // Exactly the headroom is enough: the balance ends on the floor.
    set_balance(&mut sim, Cost(DEBT_FLOOR.0 + 100_000));
    assert_eq!(run(&mut sim, road_cmd(100.0)), Ok(Cost(100_000)));
    assert_eq!(balance(&sim), DEBT_FLOOR);
    assert!(sim.world().resource::<NetGraph>().segments().count() > 0);
    assert_eq!(sim.world().resource::<CommandLog<SimCommand>>().len(), 1);
    // And now nothing that costs anything, but free commands still work.
    assert!(matches!(
        run(&mut sim, road_at(4800.0, 50.0)),
        Err(CommandError::InsufficientFunds { .. })
    ));
    let at = sim.tick();
    assert_eq!(
        run(
            &mut sim,
            SimCommand::Economy(EconomyCommand::SetTaxRate {
                zone: ZoneType::ResLow,
                pct: 12
            })
        ),
        Ok(Cost::ZERO)
    );
    assert_eq!(sim.tick(), at);
}

#[test]
fn a_road_is_charged_its_cost_once() {
    let mut sim = sim();
    let before = balance(&sim);
    assert_eq!(before, Cost(7_000_000));
    let cost = run(&mut sim, road_cmd(100.0)).unwrap();
    assert_eq!(cost, Cost(100_000));
    assert_eq!(balance(&sim), Cost(before.0 - 100_000));
}

#[test]
fn a_service_building_costs_its_price_and_is_refused_without_money() {
    let db = content();
    let def = db.road_id("basic_2l").unwrap();
    let road = db.road(def).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(&db));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &road,
            def,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3200.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();
    let at = {
        let zones = sim.world().resource::<ZoneGrid>();
        let (id, _) = zones.blocks().next().unwrap();
        zones
            .cell_footprint(cp_zone::ZoneCellRef {
                block: id,
                cell: 21,
            })
            .unwrap()
            .c
    };
    let place = SimCommand::Building(BuildingCommand::Place {
        def: db.building_id("wind_turbine").unwrap(),
        pos: at,
        rot: 0.0,
    });

    set_balance(&mut sim, Cost(-1_900_000)); // 1000.00 of headroom, the turbine is 6000.00
    assert!(matches!(
        run(&mut sim, place.clone()),
        Err(CommandError::InsufficientFunds {
            needed: Cost(600_000),
            available: Cost(100_000)
        })
    ));
    assert!(sim.world().resource::<BuildingGrid>().is_empty());
    assert_eq!(balance(&sim), Cost(-1_900_000));
    assert!(
        !sim.world()
            .resource::<ZoneGrid>()
            .is_occupied(cp_zone::ZoneCellRef {
                block: sim
                    .world()
                    .resource::<ZoneGrid>()
                    .blocks()
                    .next()
                    .unwrap()
                    .0,
                cell: 21
            })
    );

    set_balance(&mut sim, Cost(7_000_000));
    assert_eq!(run(&mut sim, place), Ok(Cost(600_000)));
    assert_eq!(balance(&sim), Cost(6_400_000));
    assert_eq!(sim.world().resource::<BuildingGrid>().len(), 1);
}

#[test]
fn a_pipe_without_money_is_refused() {
    let mut sim = sim();
    let build = SimCommand::Utility(UtilityCommand::BuildLink {
        kind: UtilityKind::Water,
        path: vec![Vec2::new(4600.0, 4600.0), Vec2::new(4700.0, 4600.0)],
    });
    // 100 m of pipe at 4.00/m = 400.00.
    set_balance(&mut sim, Cost(DEBT_FLOOR.0 + 39_999));
    assert!(matches!(
        run(&mut sim, build.clone()),
        Err(CommandError::InsufficientFunds { .. })
    ));
    assert_eq!(sim.world().resource::<UtilityGraph>().link_count(), 0);
    set_balance(&mut sim, Cost(DEBT_FLOOR.0 + 40_000));
    assert_eq!(run(&mut sim, build), Ok(Cost(40_000)));
    assert_eq!(balance(&sim), DEBT_FLOOR);
}

#[test]
fn the_week_settles_on_the_tick_and_replays_identically() {
    let build = || {
        let mut sim = sim();
        run(&mut sim, road_cmd(160.0)).unwrap();
        sim.push_command(
            SimCommand::Economy(EconomyCommand::SetBudget {
                service: ServiceKind::Roads,
                pct: 150,
            }),
            Tick(10),
        );
        sim
    };
    let (mut a, mut b) = (build(), build());
    let start = balance(&a);
    // BUG-040: upkeep is booked every game hour; between two hour boundaries the balance
    // stands still.
    let mut hour_end = start;
    while a.tick().0 < TICKS_PER_WEEK {
        a.tick_once();
        b.tick_once();
        let t = a.tick().0;
        if cp_economy::game_hour(t - 1) != cp_economy::game_hour(t.saturating_sub(2)) {
            hour_end = balance(&a);
            assert!(hour_end.0 < start.0, "an hour was booked");
        } else {
            assert_eq!(balance(&a), hour_end, "nothing between two hour boundaries");
        }
    }
    // Tick TICKS_PER_WEEK itself has not run yet (the counter is the next tick).
    a.tick_once();
    b.tick_once();
    // 160 m × 0.1/m (basic street upkeep) × 150% = 24.00 over the 168 hourly shares.
    assert_eq!(balance(&a), Cost(start.0 - 2_400));
    let r = a.world().resource::<EconomyState>().last_week;
    assert_eq!((r.week, r.road_upkeep), (1, Cost(2_400)));
    assert_eq!(a.checksum(), b.checksum());
}
