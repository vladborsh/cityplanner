//! T-150: buying tiles — neighbour only, priced by ring, limited by the milestones; a replay
//! of the same commands gives the same checksum.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use cp_content::ContentDb;
use cp_core::{CommandError, CommandOutcome, Cost, Treasury};
use cp_sim::{Progress, SimCommand, SimRunner};
use cp_terrain::{TerrainCommand, TileOwnership};

fn sim() -> SimRunner {
    let content = Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    );
    let mut sim = SimRunner::new(5);
    sim.set_content(content);
    sim.world_mut().insert_resource(TileOwnership::only((2, 2)));
    sim.world_mut().insert_resource(Progress::START);
    sim
}

fn buy(sim: &mut SimRunner, tile: (u8, u8)) -> Result<Cost, CommandError> {
    sim.push_command(
        SimCommand::Terrain(TerrainCommand::BuyTile { tile }),
        sim.tick(),
    );
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last().unwrap().result.clone()
}

fn script(sim: &mut SimRunner) {
    buy(sim, (3, 2)).unwrap();
    assert!(buy(sim, (0, 0)).is_err());
    buy(sim, (3, 3)).unwrap();
    assert!(buy(sim, (4, 4)).is_err());
    for _ in 0..8 {
        sim.tick_once();
    }
}

#[test]
fn a_neighbour_is_bought_for_its_ring_price_and_far_or_over_limit_tiles_are_refused() {
    let mut s = sim();
    let before = s.world().resource::<Treasury>().balance;
    assert_eq!(buy(&mut s, (3, 2)), Ok(Cost::from_units(5000.0)));
    assert_eq!(
        s.world().resource::<Treasury>().balance,
        Cost(before.0 - Cost::from_units(5000.0).0)
    );
    let owned = *s.world().resource::<TileOwnership>();
    assert!(owned.is_open(3, 2) && owned.open_count() == 2);
    // Not next to an owned tile; already owned.
    assert!(matches!(
        buy(&mut s, (0, 0)),
        Err(CommandError::Rejected { .. })
    ));
    assert!(matches!(
        buy(&mut s, (3, 2)),
        Err(CommandError::Rejected { .. })
    ));
    // A diagonal-ring-2 tile next to the new one costs the second ring.
    assert_eq!(buy(&mut s, (4, 2)), Ok(Cost::from_units(10000.0)));
    // Milestone 0 allows 1 + 2 tiles: the next is refused until the milestone rises.
    assert!(matches!(
        buy(&mut s, (4, 3)),
        Err(CommandError::Rejected { .. })
    ));
    s.world_mut().insert_resource(Progress { reached: 1 });
    buy(&mut s, (4, 3)).unwrap();
}

#[test]
fn a_replay_of_the_purchases_has_the_same_checksum() {
    let (mut a, mut b) = (sim(), sim());
    script(&mut a);
    script(&mut b);
    assert_eq!(a.checksum(), b.checksum());
    let mut c = sim();
    for _ in 0..8 {
        c.tick_once();
    }
    assert_ne!(a.checksum(), c.checksum());
}
