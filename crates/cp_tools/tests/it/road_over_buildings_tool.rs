//! BUG-096 in the tools: the road ghost tints exactly the buildings the click demolishes; a
//! click over a protected building (a police station) sends nothing and holds a question whose
//! yes (`NetDemolish`) takes the station down with the road.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_building::{Building, BuildingFlags, BuildingGrid, BuildingState, Footprint};
use cp_content::ContentDb;
use cp_core::{CommandOutcome, Cost, Tick, Treasury};
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::{RoadMode, RoadTool, compute_ghost_alt, resolve_cursor, road_click};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const A: Vec2 = Vec2::new(4400.0, 4400.0);

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

#[test]
fn the_ghost_shows_what_the_click_demolishes_and_a_police_station_asks_first() {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(97);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    sim.tick_once();
    sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(1_000_000_000.0);
    let road = db.road_id("basic_2l").unwrap();
    run(
        &mut sim,
        SimCommand::Net(NetCommand::BuildRoad {
            road,
            path: RoadPath::Straight {
                from: A,
                to: A + Vec2::new(160.0, 0.0),
            },
            elevation: ElevationMode::Ground,
        }),
    )
    .unwrap();
    // A station south of the street, where a cross street from its middle will run.
    let police = sim
        .world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
            def: db.building_id("police_station").unwrap(),
            footprint: Footprint::Free { w: 2, d: 2 },
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos: A + Vec2::new(80.0, 40.0),
            rot: 0.0,
            access: None,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
    sim.tick_once();
    let view = sim.view();
    let mut tool = RoadTool {
        mode: RoadMode::Straight,
        road: Some(road),
        ..Default::default()
    };
    let start = A + Vec2::new(80.0, 0.0);
    let q = resolve_cursor(&view, &tool, start, false).unwrap().cursor;
    let _ = tool.click(q);
    let end = A + Vec2::new(80.0, 80.0);
    let ghost = compute_ghost_alt(&view, &tool, end, false).unwrap();
    assert!(ghost.valid, "{:?}", ghost.errors);
    assert_eq!(ghost.demolish, vec![police], "the ghost tints the station");
    let plan = ghost.plan.clone().unwrap();
    let cmd = SimCommand::Net(NetCommand::BuildPlan { plan });
    let ask = road_click(Some(&view), vec![cmd.clone()]).expect_err("a question");
    assert_eq!(
        ask.names,
        vec![
            db.building(db.building_id("police_station").unwrap())
                .name
                .clone()
        ]
    );
    assert!(matches!(ask.commands[..], [SimCommand::NetDemolish(_)]));
    drop(view);
    // A plain command is refused by the sim, the confirmed one takes the station down.
    assert!(run(&mut sim, cmd).unwrap_err().contains("would demolish"));
    assert!(sim.world().resource::<BuildingGrid>().get(police).is_some());
    run(&mut sim, ask.commands[0].clone()).expect("the confirmed road");
    assert!(sim.world().resource::<BuildingGrid>().get(police).is_none());
}
