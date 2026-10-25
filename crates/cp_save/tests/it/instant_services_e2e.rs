//! T-175: a service building is `Active` the moment it is placed (no construction stage) and
//! serves in the same tick; replaying the same commands gives the same checksum.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_building::{BuildingCommand, BuildingGrid, BuildingState};
use cp_content::ContentDb;
use cp_core::{CommandOutcome, SimLimits};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn placed() -> (SimRunner, u64) {
    let c = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let tower = c.building_id("water_tower").unwrap();
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(Arc::new(
        Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap(),
    ));
    sim.set_content(c);
    sim.world_mut().insert_resource(TileOwnership::only((2, 2)));
    sim.push_command(
        SimCommand::Building(BuildingCommand::Place {
            def: tower,
            pos: Vec2::new(5100.0, 5100.0),
            rot: 0.0,
        }),
        sim.tick(),
    );
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    assert!(out.last().unwrap().result.is_ok(), "{:?}", out.last());
    sim.tick_once();
    let sum = sim.checksum();
    (sim, sum)
}

#[test]
fn a_placed_service_is_active_at_once_and_replays_identically() {
    let (sim, a) = placed();
    let g = sim.world().resource::<BuildingGrid>();
    let (_, b) = g.iter().next().unwrap();
    assert_eq!(b.state, BuildingState::Active);
    let (_, b2) = placed();
    assert_eq!(a, b2);
}
