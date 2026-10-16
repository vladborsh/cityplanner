//! `SimCommand` — the sum of all domain commands — and the dispatcher
//! (`architecture/system-design.md` §4, level 3).
//!
//! Save stability: variants are only appended, never reordered or removed.

use bevy_ecs::message::Messages;
use bevy_ecs::world::World;
use cp_building::BuildingCommand;
use cp_core::{CommandError, CommandLog, CommandOutcome, CommandQueue, Cost, SimSpeed, Tick};
use cp_district::DistrictCommand;
use cp_economy::EconomyCommand;
use cp_net::NetCommand;
use cp_utility::UtilityCommand;
use cp_zone::ZoneCommand;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SimCommand {
    SetSpeed {
        speed: SimSpeed,
    },
    Net(NetCommand),
    Zone(ZoneCommand),
    Building(BuildingCommand),
    Utility(UtilityCommand),
    Economy(EconomyCommand),
    /// Debug and demo commands: logged and replayed like any other, never sent by the game UI.
    Debug(DebugCommand),
    District(DistrictCommand),
    Terrain(cp_terrain::TerrainCommand),
    // M3+: Transit, ...
}

/// Commands for demos, benchmarks and tests (see [`SimCommand::Debug`]). Variants are only
/// appended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DebugCommand {
    /// Adds randomly routed cars and pedestrians (synthetic traffic, not tied to any citizen
    /// or trip) until the pools hold `cars` and `peds`, at most 600 of each kind per command
    /// (`cp_traffic::demo`). The randomness is the sub-stream of the sim RNG named by the
    /// tick the command runs at, so a replay of the log adds the same agents.
    TopUpTraffic { cars: u32, peds: u32 },
}

impl SimCommand {
    /// Replaces the road and building definition ids inside the command (a save loaded against
    /// content whose definitions sit at other indices). Every variant is listed on purpose: a
    /// new command that carries a definition id has to be handled here.
    pub fn remap_defs(
        &mut self,
        road: &dyn Fn(cp_content::RoadDefId) -> cp_content::RoadDefId,
        building: &dyn Fn(cp_content::BuildingDefId) -> cp_content::BuildingDefId,
    ) {
        match self {
            SimCommand::Net(NetCommand::BuildRoad { road: r, .. })
            | SimCommand::Net(NetCommand::UpgradeSegment { road: r, .. }) => *r = road(*r),
            SimCommand::Building(BuildingCommand::Place { def, .. }) => *def = building(*def),
            SimCommand::SetSpeed { .. }
            | SimCommand::Net(NetCommand::RemoveSegment { .. })
            | SimCommand::Net(NetCommand::RemovePart { .. })
            | SimCommand::Net(NetCommand::ToggleTrafficLight { .. })
            | SimCommand::Building(BuildingCommand::Demolish { .. })
            | SimCommand::Zone(_)
            | SimCommand::Utility(_)
            | SimCommand::Economy(_)
            | SimCommand::District(_)
            | SimCommand::Terrain(_)
            | SimCommand::Debug(_) => {}
        }
    }
}

fn rejected(domain: &'static str, e: impl std::fmt::Display) -> CommandError {
    CommandError::Rejected {
        domain,
        reason: e.to_string(),
    }
}

/// [`rejected`], except that a shortfall in the treasury becomes the typed
/// [`CommandError::InsufficientFunds`].
fn rejected_or_funds(
    domain: &'static str,
    e: impl std::fmt::Display,
    funds: Option<cp_core::InsufficientFunds>,
) -> CommandError {
    funds.map_or_else(|| rejected(domain, e), CommandError::from)
}

/// Applies one command. The `match` has no wildcard: a new variant won't compile until
/// it is routed here.
fn apply(world: &mut World, cmd: &SimCommand) -> Result<Cost, CommandError> {
    match cmd {
        SimCommand::SetSpeed { speed } => {
            world.insert_resource(*speed);
            Ok(Cost::ZERO)
        }
        SimCommand::Net(c) => cp_net::apply(world, c)
            .map_err(|e| rejected_or_funds("net", &e, e.insufficient_funds())),
        SimCommand::Zone(c) => cp_zone::apply(world, c).map_err(|e| rejected("zone", e)),
        SimCommand::Building(c) => {
            if let (BuildingCommand::Place { def, .. }, Some(content)) =
                (c, world.get_resource::<cp_content::SimContent>())
            {
                let progress = world.get_resource::<crate::Progress>().copied();
                crate::progress::check_unlocked(&progress.unwrap_or_default(), &content.0, *def)?;
            }
            cp_building::apply(world, c)
                .map_err(|e| rejected_or_funds("building", &e, e.insufficient_funds()))
        }
        SimCommand::Utility(c) => cp_utility::apply(world, c)
            .map_err(|e| rejected_or_funds("utility", &e, e.insufficient_funds())),
        SimCommand::Economy(c) => cp_economy::apply(world, c).map_err(|e| rejected("economy", e)),
        SimCommand::District(c) => {
            cp_district::apply(world, c).map_err(|e| rejected("district", e))
        }
        SimCommand::Terrain(c) => crate::tiles::apply(world, c),
        SimCommand::Debug(c) => apply_debug(world, c),
    }
}

fn apply_debug(world: &mut World, cmd: &DebugCommand) -> Result<Cost, CommandError> {
    match cmd {
        DebugCommand::TopUpTraffic { cars, peds } => {
            let tick = world.get_resource::<Tick>().copied().unwrap_or_default();
            world.resource_scope(
                |world, mut agents: bevy_ecs::prelude::Mut<cp_traffic::Agents>| {
                    let (Some(pnet), Some(net), Some(rng)) = (
                        world.get_resource::<cp_traffic::PathNetwork>(),
                        world.get_resource::<cp_net::NetGraph>(),
                        world.get_resource::<cp_core::SimRng>(),
                    ) else {
                        return Err(rejected("debug", "no network to route on"));
                    };
                    let added = cp_traffic::demo::top_up(
                        &mut agents,
                        pnet.graph(),
                        net,
                        *cars as usize,
                        *peds as usize,
                        rng,
                        tick.0,
                    );
                    if added == 0 {
                        return Err(rejected("debug", "nothing added"));
                    }
                    Ok(Cost::ZERO)
                },
            )
        }
    }
}

/// Inserts the command resources (queue, log, outcome messages, speed) if missing.
pub fn init_command_resources(world: &mut World) {
    world.init_resource::<CommandQueue<SimCommand>>();
    world.init_resource::<CommandLog<SimCommand>>();
    world.init_resource::<Messages<CommandOutcome>>();
    world.init_resource::<SimSpeed>();
    world.init_resource::<Tick>();
}

/// The `Commands`-phase dispatcher: an exclusive system. Drains everything due at the
/// current [`Tick`] in `(tick, CommandId)` order, applies it, appends **successfully
/// applied** commands to the [`CommandLog`] (rejected ones changed nothing, so replay
/// doesn't need them) and writes a [`CommandOutcome`] for every command.
/// The domains check the treasury and charge it themselves (the change and its price are one
/// step: a rejected command costs nothing); the returned [`Cost`] is what they charged.
pub fn dispatch_commands(world: &mut World) {
    let tick = *world.resource::<Tick>();
    let due = world
        .resource_mut::<CommandQueue<SimCommand>>()
        .drain_due(tick);
    for (_, id, cmd) in due {
        let result = apply(world, &cmd);
        if result.is_ok() {
            world
                .resource_mut::<CommandLog<SimCommand>>()
                .push(tick, id, cmd);
        }
        world
            .resource_mut::<Messages<CommandOutcome>>()
            .write(CommandOutcome { id, result });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ecs::schedule::Schedule;

    fn world() -> (World, Schedule) {
        let mut world = World::new();
        init_command_resources(&mut world);
        let mut schedule = Schedule::default();
        schedule.add_systems(dispatch_commands);
        (world, schedule)
    }

    fn outcomes(world: &mut World) -> Vec<CommandOutcome> {
        world
            .resource_mut::<Messages<CommandOutcome>>()
            .drain()
            .collect()
    }

    #[test]
    fn set_speed_goes_through_dispatcher_and_log() {
        let (mut world, mut schedule) = world();
        let t = Tick(4);
        let id = world.resource_mut::<CommandQueue<SimCommand>>().push(
            SimCommand::SetSpeed {
                speed: SimSpeed::X3,
            },
            t,
        );

        // Not due yet.
        world.insert_resource(Tick(3));
        schedule.run(&mut world);
        assert!(outcomes(&mut world).is_empty());
        assert_eq!(*world.resource::<SimSpeed>(), SimSpeed::X1);

        world.insert_resource(t);
        schedule.run(&mut world);
        assert_eq!(*world.resource::<SimSpeed>(), SimSpeed::X3);
        assert_eq!(
            outcomes(&mut world),
            vec![CommandOutcome {
                id,
                result: Ok(Cost::ZERO)
            }]
        );
        let log = world.resource::<CommandLog<SimCommand>>();
        assert_eq!(log.len(), 1);
        let e = &log.entries()[0];
        assert_eq!((e.tick, e.id), (t, id));
        assert_eq!(
            e.command,
            SimCommand::SetSpeed {
                speed: SimSpeed::X3
            }
        );
    }

    #[test]
    fn same_tick_commands_apply_in_arrival_order() {
        let (mut world, mut schedule) = world();
        let mut q = world.resource_mut::<CommandQueue<SimCommand>>();
        q.push(
            SimCommand::SetSpeed {
                speed: SimSpeed::X2,
            },
            Tick(1),
        );
        q.push(
            SimCommand::SetSpeed {
                speed: SimSpeed::Pause,
            },
            Tick(1),
        );
        world.insert_resource(Tick(1));
        schedule.run(&mut world);
        assert_eq!(*world.resource::<SimSpeed>(), SimSpeed::Pause);
        let log = world.resource::<CommandLog<SimCommand>>();
        assert_eq!(log.len(), 2);
        assert!(log.entries()[0].id < log.entries()[1].id);
    }

    #[test]
    fn sim_command_and_log_serde_round_trip() {
        let (mut world, mut schedule) = world();
        world.resource_mut::<CommandQueue<SimCommand>>().push(
            SimCommand::SetSpeed {
                speed: SimSpeed::X2,
            },
            Tick(0),
        );
        schedule.run(&mut world);
        let log = world.resource::<CommandLog<SimCommand>>();
        let bytes = postcard::to_stdvec(log).unwrap();
        let back: CommandLog<SimCommand> = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(&back, log);
    }
}
