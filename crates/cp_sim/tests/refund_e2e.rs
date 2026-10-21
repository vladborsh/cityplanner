//! BUG-097: bulldozing gives back 70 % (`demolish_refund`) of what the player paid: a placed
//! building its price, a road its length at its road's price (a part by the length taken), a
//! pipe its length; a grown house nothing, nor the road the map came with. The money is in the
//! treasury after the command and the next hour books it as the budget's refund line.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_building::{
    Building, BuildingCommand, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint,
};
use cp_content::{ContentDb, UtilityKind};
use cp_core::{CommandOutcome, Cost, Tick, Treasury};
use cp_economy::MoneyFlow;
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath, SegmentId};
use cp_sim::refund::{RefundInputs, demolition_refund, is_map_road};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_utility::{UtilityCommand, UtilityGraph};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
/// The street runs east from here.
const A: Vec2 = Vec2::new(4400.0, 4400.0);
const LEN: f32 = 160.0;

fn sim() -> (SimRunner, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(96);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    sim.tick_once();
    sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(1_000_000.0);
    (sim, db)
}

fn balance(sim: &SimRunner) -> Cost {
    sim.world().resource::<Treasury>().balance
}

/// Runs `cmd` now; its outcome.
fn run(sim: &mut SimRunner, cmd: SimCommand) -> Result<Cost, String> {
    let at = sim.tick();
    sim.push_command(cmd, at);
    sim.tick_once();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last()
        .expect("an outcome")
        .result
        .clone()
        .map_err(|e| e.to_string())
}

fn street(sim: &mut SimRunner, db: &ContentDb) {
    run(
        sim,
        SimCommand::Net(NetCommand::BuildRoad {
            road: db.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: A,
                to: A + Vec2::new(LEN, 0.0),
            },
            elevation: ElevationMode::Ground,
        }),
    )
    .expect("the street");
}

fn segments(sim: &SimRunner) -> Vec<(SegmentId, f32)> {
    sim.world()
        .resource::<NetGraph>()
        .segments()
        .map(|(id, s)| (id, s.length))
        .collect()
}

/// A building of `def` standing north of the street, as if placed or grown.
fn stand(sim: &mut SimRunner, db: &ContentDb, def: &str) -> BuildingId {
    sim.world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
            def: db.building_id(def).unwrap(),
            footprint: Footprint::Free { w: 2, d: 2 },
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos: A + Vec2::new(LEN / 2.0, -16.5),
            rot: 0.0,
            access: None,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        })
}

/// 70 % of `units` of money, in cents.
fn seventy(units: f32) -> i64 {
    (Cost::from_units(units).0 as f64 * 0.7).round() as i64
}

#[test]
fn a_placed_building_a_road_and_a_pipe_give_back_70_percent_of_what_they_cost() {
    let (mut sim, db) = sim();
    street(&mut sim, &db);
    let mut refunded = 0;

    // A police station: 9 900.00 paid, 6 930.00 back.
    let police = stand(&mut sim, &db, "police_station");
    let before = balance(&sim);
    let cost = run(
        &mut sim,
        SimCommand::Building(BuildingCommand::Demolish { building: police }),
    )
    .expect("the demolition");
    let back = balance(&sim).0 - before.0;
    assert_eq!(back, seventy(9900.0));
    assert_eq!(
        cost,
        Cost(-back),
        "the command's cost is the refund, negative"
    );
    refunded += back;

    // 100 m of pipe at 4.00/m: 280.00 back.
    let path = vec![A + Vec2::new(0.0, 60.0), A + Vec2::new(100.0, 60.0)];
    run(
        &mut sim,
        SimCommand::Utility(UtilityCommand::BuildLink {
            kind: UtilityKind::Water,
            path,
        }),
    )
    .expect("the pipe");
    let links: Vec<_> = sim
        .world()
        .resource::<UtilityGraph>()
        .links()
        .map(|(id, _)| id)
        .collect();
    let before = balance(&sim);
    for link in links {
        run(
            &mut sim,
            SimCommand::Utility(UtilityCommand::RemoveLink { link }),
        )
        .unwrap();
    }
    let back = balance(&sim).0 - before.0;
    assert!(
        (back - seventy(400.0)).abs() <= 2,
        "{back} cents for the pipe"
    );
    refunded += back;

    // The 160 m street at 4.00/m: 448.00 back, a cent per piece at most lost to rounding.
    let pieces = segments(&sim);
    let before = balance(&sim);
    for (seg, _) in &pieces {
        run(
            &mut sim,
            SimCommand::Net(NetCommand::RemoveSegment { seg: *seg }),
        )
        .unwrap();
    }
    let back = balance(&sim).0 - before.0;
    assert!(
        (back - seventy(LEN * 4.0)).abs() <= pieces.len() as i64,
        "{back} cents for the street"
    );
    refunded += back;

    // The next hour books it all as the refund line of the budget.
    for _ in 0..=cp_core::time::TICKS_PER_HOUR.ceil() as u64 {
        sim.tick_once();
    }
    assert_eq!(
        sim.world().resource::<MoneyFlow>().week_to_date.refunds,
        Cost(refunded)
    );
}

#[test]
fn a_grown_house_gives_nothing_and_a_part_of_a_road_gives_back_by_its_length() {
    let (mut sim, db) = sim();
    street(&mut sim, &db);
    let house = stand(&mut sim, &db, "house_low_1");
    let before = balance(&sim);
    assert_eq!(
        run(
            &mut sim,
            SimCommand::Building(BuildingCommand::Demolish { building: house })
        ),
        Ok(Cost::ZERO)
    );
    assert_eq!(balance(&sim), before);

    // 40 m out of the middle of the longest piece.
    let (seg, len) = segments(&sim)
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    assert!(len >= 72.0, "{len} m");
    let before = balance(&sim);
    run(
        &mut sim,
        SimCommand::Net(NetCommand::RemovePart {
            seg,
            s0: 16.0,
            s1: 56.0,
        }),
    )
    .unwrap();
    assert_eq!(balance(&sim).0 - before.0, seventy(40.0 * 4.0));
}

#[test]
fn the_road_the_map_came_with_gives_nothing() {
    let (mut sim, db) = sim();
    cp_sim::new_game::build_entry_road(sim.world_mut()).expect("the entry road");
    let map_pieces: Vec<_> = segments(&sim).into_iter().map(|(s, _)| s).collect();
    assert!(!map_pieces.is_empty());
    street(&mut sim, &db);
    let world = sim.world();
    let terrain = world.resource::<cp_terrain::SimTerrain>();
    let inputs = RefundInputs {
        content: &db,
        net: world.resource::<NetGraph>(),
        buildings: world.resource::<BuildingGrid>(),
        utilities: world.resource::<UtilityGraph>(),
        transit: None,
        meta: Some(terrain.0.meta()),
    };
    for (seg, _) in segments(&sim) {
        let remove = SimCommand::Net(NetCommand::RemoveSegment { seg });
        if map_pieces.contains(&seg) {
            assert!(is_map_road(&inputs, seg));
            assert_eq!(demolition_refund(&inputs, &remove), Cost::ZERO);
        } else {
            assert!(!is_map_road(&inputs, seg));
            assert!(demolition_refund(&inputs, &remove).0 > 0);
        }
    }
}
