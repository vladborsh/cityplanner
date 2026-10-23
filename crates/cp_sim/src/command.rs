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
    /// M3 (T-153): lines and stops.
    Transit(cp_transit::TransitCommand),
    /// M5 (T-214): warehouse modes.
    Industry(cp_industry::IndustryCommand),
    /// M6 (T-225): the entrance fee of a park area.
    Park(cp_park::ParkCommand),
    /// BUG-096: a road command the player confirmed over protected buildings (a service, a
    /// unique building, a park): those under the new road are demolished with the grown ones.
    /// Plain [`SimCommand::Net`] demolishes only grown buildings and refuses over a protected one.
    NetDemolish(NetCommand),
    // M3+: ...
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
            | SimCommand::Net(NetCommand::UpgradeSegment { road: r, .. })
            | SimCommand::NetDemolish(NetCommand::BuildRoad { road: r, .. })
            | SimCommand::NetDemolish(NetCommand::UpgradeSegment { road: r, .. })
            | SimCommand::Net(NetCommand::UpgradePart { road: r, .. })
            | SimCommand::NetDemolish(NetCommand::UpgradePart { road: r, .. }) => *r = road(*r),
            SimCommand::Net(NetCommand::BuildPlan { plan })
            | SimCommand::NetDemolish(NetCommand::BuildPlan { plan }) => {
                plan.road = road(plan.road)
            }
            SimCommand::Building(BuildingCommand::Place { def, .. }) => *def = building(*def),
            SimCommand::SetSpeed { .. }
            | SimCommand::Net(NetCommand::RemoveSegment { .. })
            | SimCommand::Net(NetCommand::RemovePart { .. })
            | SimCommand::Net(NetCommand::FlipSegment { .. })
            | SimCommand::Net(NetCommand::ToggleTrafficLight { .. })
            | SimCommand::Net(NetCommand::SetSign { .. })
            | SimCommand::Net(NetCommand::SetLaneRule { .. })
            | SimCommand::NetDemolish(NetCommand::RemoveSegment { .. })
            | SimCommand::NetDemolish(NetCommand::RemovePart { .. })
            | SimCommand::NetDemolish(NetCommand::FlipSegment { .. })
            | SimCommand::NetDemolish(NetCommand::ToggleTrafficLight { .. })
            | SimCommand::NetDemolish(NetCommand::SetSign { .. })
            | SimCommand::NetDemolish(NetCommand::SetLaneRule { .. })
            | SimCommand::Building(BuildingCommand::Demolish { .. })
            | SimCommand::Building(BuildingCommand::Move { .. })
            | SimCommand::Zone(_)
            | SimCommand::Utility(_)
            | SimCommand::Economy(_)
            | SimCommand::District(_)
            | SimCommand::Terrain(_)
            | SimCommand::Transit(_)
            | SimCommand::Industry(_)
            | SimCommand::Park(_)
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

/// Applies one command and pays back what it demolished (BUG-097, [`crate::refund`]): the
/// refund is worked out before the command runs, while what it removes still stands, and paid
/// only if it succeeds. The cost returned is what the command charged minus the refund.
fn apply(world: &mut World, cmd: &SimCommand) -> Result<Cost, CommandError> {
    let refund = refund_of(world, cmd);
    let cost = apply_command(world, cmd)?;
    if refund.0 > 0 {
        cp_core::treasury::charge_funds(world, Cost(-refund.0));
        if let Some(mut booked) = world.get_resource_mut::<cp_economy::DemolitionRefunds>() {
            booked.0 += refund;
        }
    }
    Ok(Cost(cost.0 - refund.0))
}

/// What `cmd` would give back if it ran now: [`crate::refund::demolition_refund`], plus, for a
/// road laid over buildings (BUG-096), the refund of each building it takes (a grown one
/// gives nothing). Zero without content.
fn refund_of(world: &World, cmd: &SimCommand) -> Cost {
    let (Some(content), Some(net), Some(buildings), Some(utilities)) = (
        world.get_resource::<cp_content::SimContent>(),
        world.get_resource::<cp_net::NetGraph>(),
        world.get_resource::<cp_building::BuildingGrid>(),
        world.get_resource::<cp_utility::UtilityGraph>(),
    ) else {
        return Cost::ZERO;
    };
    let inputs = crate::refund::RefundInputs {
        content: &content.0,
        net,
        buildings,
        utilities,
        transit: world.get_resource::<cp_transit::TransitNet>(),
        meta: world
            .get_resource::<cp_terrain::SimTerrain>()
            .map(|t| t.0.meta()),
    };
    let mut refund = crate::refund::demolition_refund(&inputs, cmd);
    if let SimCommand::Net(c) | SimCommand::NetDemolish(c) = cmd
        && matches!(
            c,
            NetCommand::BuildRoad { .. } | NetCommand::BuildPlan { .. }
        )
    {
        for building in buildings_under(world, c) {
            refund += crate::refund::building_refund(&inputs, building);
        }
    }
    refund
}

/// The commands themselves. The `match` has no wildcard: a new variant won't compile until
/// it is routed here.
fn apply_command(world: &mut World, cmd: &SimCommand) -> Result<Cost, CommandError> {
    match cmd {
        SimCommand::SetSpeed { speed } => {
            world.insert_resource(*speed);
            Ok(Cost::ZERO)
        }
        SimCommand::Net(c) => apply_net(world, c, false),
        SimCommand::NetDemolish(c) => apply_net(world, c, true),
        SimCommand::Zone(c) => {
            if let (ZoneCommand::PaintZones { zone, .. }, Some(content)) =
                (c, world.get_resource::<cp_content::SimContent>())
            {
                let progress = world
                    .get_resource::<crate::Progress>()
                    .cloned()
                    .unwrap_or_default();
                crate::progress::check_zone_unlocked(&progress, &content.0, *zone)?;
            }
            let done = cp_zone::apply(world, c);
            if done.is_ok() {
                demolish_unzoned(world, c);
            }
            done.map_err(|e| rejected("zone", e))
        }
        SimCommand::Building(c) => {
            if let (BuildingCommand::Place { def, .. }, Some(content)) =
                (c, world.get_resource::<cp_content::SimContent>())
            {
                let progress = world
                    .get_resource::<crate::Progress>()
                    .cloned()
                    .unwrap_or_default();
                let inputs = unlock_inputs(world, &content.0);
                crate::progress::check_unlocked(&progress, &content.0, *def, &inputs)?;
            }
            apply_building(world, c)
                .map_err(|e| rejected_or_funds("building", &e, e.insufficient_funds()))
        }
        SimCommand::Utility(c) => cp_utility::apply(world, c)
            .map_err(|e| rejected_or_funds("utility", &e, e.insufficient_funds())),
        SimCommand::Economy(c) => cp_economy::apply(world, c).map_err(|e| rejected("economy", e)),
        SimCommand::District(c) => {
            // Only switching a policy ON is locked: a locked one can never have been on, and an
            // old save's policy must still be switchable off.
            if let (
                cp_district::DistrictCommand::SetPolicy {
                    policy, on: true, ..
                },
                Some(content),
            ) = (c, world.get_resource::<cp_content::SimContent>())
            {
                let progress = world
                    .get_resource::<crate::Progress>()
                    .cloned()
                    .unwrap_or_default();
                crate::progress::check_policy_unlocked(&progress, &content.0, *policy)?;
            }
            cp_district::apply(world, c).map_err(|e| rejected("district", e))
        }
        SimCommand::Terrain(c) => crate::tiles::apply(world, c),
        SimCommand::Transit(c) => cp_transit::apply(world, c)
            .map_err(|e| rejected_or_funds("transit", &e, e.insufficient_funds())),
        SimCommand::Industry(c) => {
            cp_industry::apply(world, c).map_err(|e| rejected("industry", e))
        }
        SimCommand::Park(c) => cp_park::apply(world, c).map_err(|e| rejected("park", e)),
        SimCommand::Debug(c) => apply_debug(world, c),
    }
}

/// A road command (BUG-096): the buildings under the road it lays go down with it. A grown one
/// always; a protected one (a service, a unique building, a park, see
/// [`cp_building::is_growable`]) only with the player's consent (`demolish`), else the command is
/// refused and nothing changes. The road is applied first; the buildings are demolished after it
/// in id order, the way a `Demolish` command takes them (a station takes its platform along).
fn apply_net(world: &mut World, c: &NetCommand, demolish: bool) -> Result<Cost, CommandError> {
    if let Some(seg) = platform_target(c)
        && world
            .get_resource::<cp_transit::TransitNet>()
            .is_some_and(|t| is_platform(t, seg))
    {
        return Err(rejected(
            "net",
            "the platform of a station goes with its building",
        ));
    }
    let hits = buildings_under(world, c);
    if !demolish
        && let Some(content) = world.get_resource::<cp_content::SimContent>()
        && let Some(grid) = world.get_resource::<cp_building::BuildingGrid>()
    {
        let names: Vec<String> = hits
            .iter()
            .filter_map(|id| grid.get(*id))
            .filter(|b| !cp_building::is_growable(content.0.building(b.def)))
            .map(|b| content.0.building(b.def).name.clone())
            .collect();
        if !names.is_empty() {
            return Err(rejected(
                "net",
                format!("would demolish: {}", names.join(", ")),
            ));
        }
    }
    let cost = cp_net::apply(world, c)
        .map_err(|e| rejected_or_funds("net", &e, e.insufficient_funds()))?;
    for building in hits {
        // A building the road already took (a station's platform) is gone: nothing to do.
        let _ = apply_building(world, &BuildingCommand::Demolish { building });
    }
    Ok(cost)
}

/// The buildings the road command `c` would stand on ([`cp_net::command_strips`],
/// [`cp_building::buildings_under_road`]); none without a map, content or buildings.
pub fn buildings_under(world: &World, c: &NetCommand) -> Vec<cp_building::BuildingId> {
    let (Some(net), Some(terrain), Some(tiles), Some(content), Some(grid)) = (
        world.get_resource::<cp_net::NetGraph>(),
        world.get_resource::<cp_terrain::SimTerrain>(),
        world.get_resource::<cp_terrain::TileOwnership>(),
        world.get_resource::<cp_content::SimContent>(),
        world.get_resource::<cp_building::BuildingGrid>(),
    ) else {
        return Vec::new();
    };
    let strips = cp_net::command_strips(net, &terrain.0, tiles, &content.0, c);
    cp_building::buildings_under_road(grid, &strips)
}

/// The segment a net command would remove or change.
fn platform_target(c: &NetCommand) -> Option<cp_net::SegmentId> {
    match c {
        NetCommand::RemoveSegment { seg }
        | NetCommand::RemovePart { seg, .. }
        | NetCommand::UpgradeSegment { seg, .. }
        | NetCommand::UpgradePart { seg, .. }
        | NetCommand::FlipSegment { seg } => Some(*seg),
        NetCommand::BuildRoad { .. }
        | NetCommand::BuildPlan { .. }
        | NetCommand::ToggleTrafficLight { .. }
        | NetCommand::SetSign { .. }
        | NetCommand::SetLaneRule { .. } => None,
    }
}

fn is_platform(transit: &cp_transit::TransitNet, seg: cp_net::SegmentId) -> bool {
    transit
        .stops
        .iter()
        .any(|(_, st)| matches!(st.kind, cp_transit::StopKind::Station { seg: s, .. } if s == seg))
}

/// A building command with the part of it that lives in the network: a station comes with a
/// platform and a stop (T-162), and leaves with them.
fn apply_building(
    world: &mut World,
    c: &BuildingCommand,
) -> Result<Cost, cp_building::BuildingError> {
    let (cost, placed) = cp_building::apply_placed(world, c)?;
    if let Some(placed) = placed {
        if let Some(seg) = placed.platform {
            cp_transit::add_station(world, placed.building, seg);
        }
        rehome_evicted(world, &placed.demolished);
    }
    if let BuildingCommand::Demolish { building } = c
        && let Some(seg) = cp_transit::remove_station(world, *building)
        && let Some(mut net) = world.get_resource_mut::<cp_net::NetGraph>()
    {
        // The platform may have gone with a part of the network already.
        let _ = net.remove_segment(seg);
    }
    Ok(cost)
}

/// T-252: the households of the zoned houses a service was placed over move to vacant homes
/// now; the rest leave with the purge of removed buildings in the `Citizens` phase, which also
/// takes the jobs of the demolished workplaces.
fn rehome_evicted(world: &mut World, demolished: &[(cp_building::BuildingId, bevy_math::Vec2)]) {
    if demolished.is_empty() || !world.contains_resource::<cp_citizen::CitizenGrid>() {
        return;
    }
    let Some(content) = world
        .get_resource::<cp_content::SimContent>()
        .map(|c| c.0.clone())
    else {
        return;
    };
    world.resource_scope(
        |world, mut grid: bevy_ecs::prelude::Mut<cp_citizen::CitizenGrid>| {
            if let Some(buildings) = world.get_resource::<cp_building::BuildingGrid>() {
                cp_citizen::rehome_evicted(&mut grid, buildings, &content, demolished);
            }
        },
    );
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

/// What the unlock conditions of the unique buildings read, as of now.
pub(crate) fn unlock_inputs(
    world: &World,
    db: &cp_content::ContentDb,
) -> crate::progress::UnlockInputs {
    crate::progress::UnlockInputs::gather(
        db,
        world
            .get_resource::<cp_citizen::CitizenGrid>()
            .map_or(0, |c| c.citizen_count() as u32),
        world
            .get_resource::<cp_building::BuildingGrid>()
            .map_or(&Default::default(), |b| b),
        world
            .get_resource::<cp_district::Districts>()
            .map_or(0, |d| d.iter().count() as u32),
        world
            .get_resource::<cp_transit::Tourists>()
            .map_or(0, |t| t.visits_week),
    )
}

/// BUG-053: after a zone brush, the zoned buildings that no longer stand on their zone are
/// demolished at once, the way a bulldozed one is (no refund; people leave on their next update).
fn demolish_unzoned(world: &mut World, cmd: &cp_zone::ZoneCommand) {
    let cp_zone::ZoneCommand::PaintZones { cells, .. } = cmd;
    let Some(content) = world
        .get_resource::<cp_content::SimContent>()
        .map(|c| c.0.clone())
    else {
        return;
    };
    let gone = {
        let (Some(buildings), Some(zones)) = (
            world.get_resource::<cp_building::BuildingGrid>(),
            world.get_resource::<cp_zone::ZoneGrid>(),
        ) else {
            return;
        };
        cp_building::zone_mismatches(buildings, zones, &content, cells)
    };
    for id in gone {
        world.resource_scope(
            |world, mut buildings: bevy_ecs::prelude::Mut<cp_building::BuildingGrid>| {
                let mut zones = world.resource_mut::<cp_zone::ZoneGrid>();
                cp_building::demolish_building(&mut buildings, &mut zones, id);
            },
        );
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
