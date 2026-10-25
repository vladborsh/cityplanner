//! T-149: a building a milestone unlocks is refused until the population reaches it; the
//! milestone never drops again.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_ecs::system::RunSystemOnce;
use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingCommand, BuildingGrid};
use cp_citizen::CitizenGrid;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandOutcome, Tick};
use cp_net::{Elevation, NetGraph};
use cp_sim::progress::update_progress;
use cp_sim::{PROGRESS_PERIOD, Progress, SimCommand, SimRunner};
use cp_terrain::TileOwnership;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn world(content: &Arc<ContentDb>) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(content));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3200.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();
    sim
}

fn spot(sim: &SimRunner, col: u16) -> Vec2 {
    let zones = sim.world().resource::<ZoneGrid>();
    let (id, _) = zones.blocks().next().unwrap();
    zones
        .cell_footprint(ZoneCellRef {
            block: id,
            cell: col * 4 + 1,
        })
        .unwrap()
        .c
}

fn place(sim: &mut SimRunner, name: &str, col: u16) -> Result<(), CommandError> {
    let def = sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .building_id(name)
        .unwrap();
    let pos = spot(sim, col);
    sim.push_command(
        SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 }),
        sim.tick(),
    );
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last().unwrap().result.clone().map(|_| ())
}

#[test]
fn a_locked_building_is_refused_until_its_milestone_is_reached_and_the_milestone_never_drops() {
    let c = content();
    let mut sim = world(&c);
    sim.world_mut().insert_resource(Progress::START);
    // Before: the clinic waits for a milestone.
    assert!(matches!(
        place(&mut sim, "clinic", 3),
        Err(CommandError::Locked { milestone, population: 300 }) if milestone == "Village"
    ));
    // A building of the first milestone is open from the start.
    place(&mut sim, "coal_power_plant", 8).unwrap();
    let home = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .next()
        .unwrap()
        .0;

    // 600 residents; the next pass (every 256 ticks) reaches the milestone.
    {
        let now = sim.world().resource::<Tick>().0;
        let members = vec![(30 * cp_citizen::DAYS_PER_YEAR, 1, false); 6];
        let mut g = sim.world_mut().resource_mut::<CitizenGrid>();
        for _ in 0..100 {
            g.add_household(home, 0, Tick(now), &members);
        }
    }
    // Run the pass itself on a pass tick: the sim would evict homeless residents first.
    *sim.world_mut().resource_mut::<Tick>() = Tick(PROGRESS_PERIOD);
    sim.world_mut().run_system_once(update_progress).unwrap();
    assert!(sim.world().resource::<CitizenGrid>().citizen_count() >= 500);
    assert_eq!(sim.world().resource::<Progress>().reached, 1);
    place(&mut sim, "clinic", 3).unwrap();
    assert!(matches!(
        place(&mut sim, "university", 14),
        Err(CommandError::Locked { .. })
    ));

    // The residents leave; the milestone stays.
    sim.world_mut().insert_resource(CitizenGrid::default());
    *sim.world_mut().resource_mut::<Tick>() = Tick(2 * PROGRESS_PERIOD);
    sim.world_mut().run_system_once(update_progress).unwrap();
    assert_eq!(sim.world().resource::<Progress>().reached, 1);
}

/// T-283: a policy waits for its milestone like a building; Hamlet's policy is open from the
/// start; switching a policy off is never refused.
#[test]
fn a_locked_policy_is_refused_until_its_milestone() {
    use cp_content::PolicyKind;
    use cp_district::DistrictCommand;
    let c = content();
    let mut sim = world(&c);
    let set = |sim: &mut SimRunner, policy, on| {
        sim.push_command(
            SimCommand::District(DistrictCommand::SetPolicy {
                district: None,
                policy,
                on,
            }),
            sim.tick(),
        );
        sim.apply_due_commands();
        let out: Vec<CommandOutcome> = sim
            .world_mut()
            .resource_mut::<Messages<CommandOutcome>>()
            .drain()
            .collect();
        out.last().unwrap().result.clone().map(|_| ())
    };
    sim.world_mut().insert_resource(Progress::START);
    set(&mut sim, PolicyKind::PowerSaving, true).unwrap();
    assert!(matches!(
        set(&mut sim, PolicyKind::NeighborhoodWatch, true),
        Err(CommandError::Locked { milestone, population: 700 }) if milestone == "Large Village"
    ));
    set(&mut sim, PolicyKind::NeighborhoodWatch, false).unwrap();
    sim.world_mut().insert_resource(Progress::at(2));
    set(&mut sim, PolicyKind::NeighborhoodWatch, true).unwrap();
}
