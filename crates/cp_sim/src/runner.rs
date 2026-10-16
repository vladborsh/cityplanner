//! `SimRunner`: the sim's own `World` + `Schedule`, tick stepping and speed accumulator.
//!
//! **One tick**, in order: (1) rotate `Messages<CommandOutcome>`; (2) run the schedule
//! `Commands → Network → … → Stats` (then the periodic checksum); (3) increment `Tick`.
//! So during tick `T` every system sees `Tick(T)`; commands due at `T` or earlier are
//! applied in phase `Commands`; a client that wants a command to run next sends it at
//! `current Tick + 1`.
//!
//! **Outcome messages** written in tick `T` are readable by any cursor until the
//! rotation at the start of tick `T + 2`: i.e. by systems in tick `T`, by the caller
//! after tick `T`, and during/after tick `T + 1`.
//!
//! **Speed:** `step(dt)` takes frame time as an argument (no wall clock in the sim).
//! The accumulator gains `dt × speed multiplier` (×1 = 20 ticks/s, one tick = 50 ms) in
//! integer nanoseconds. At most [`MAX_TICKS_PER_STEP`] ticks run per call; if more are
//! owed the surplus is dropped (game time slows down) rather than carried.

use std::time::Duration;

use bevy_ecs::message::Messages;
use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule, SystemSet};
use bevy_ecs::world::{Mut, World};
use bevy_tasks::{ComputeTaskPool, TaskPool};
use cp_core::time::TICK_MILLIS;
use cp_core::{CommandId, CommandOutcome, CommandQueue, SimLimits, SimRng, SimSpeed, Tick};
use cp_net::{JunctionGeometry, NetGraph};
use cp_terrain::TileOwnership;

use crate::checksum::{SimChecksum, update_checksum};
use crate::command::DebugCommand;
use crate::command::{SimCommand, dispatch_commands, init_command_resources};
use crate::phase::SimPhase;
use crate::plugin::{SimBuilder, SimPlugin};

pub const MAX_TICKS_PER_STEP: u32 = 4;

const TICK_NANOS: u128 = TICK_MILLIS as u128 * 1_000_000;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ChecksumSet;

pub struct SimRunner {
    world: World,
    schedule: Schedule,
    /// Owed sim time in "×1-nanoseconds" (real nanos × speed multiplier).
    accumulator: u128,
    /// `SimView::net` cache: `(junction version, node count, segment count, snapshot)`, so
    /// `view()` clones the graph only when it changed.
    pub(crate) net_cache: std::sync::Mutex<crate::view::NetCache>,
    /// `SimView::overlays.zones` cache, keyed by a fingerprint of the zone state.
    pub(crate) zone_cache: std::sync::Mutex<crate::overlay::ZoneCache>,
    /// `SimView::buildings` cache, keyed by the grid's revision.
    pub(crate) buildings_cache: std::sync::Mutex<crate::view::BuildingsCache>,
    pub(crate) district_cache: std::sync::Mutex<crate::view::DistrictCache>,
    /// `SimView::zones` cache, keyed by `ZoneGrid::revision`.
    pub(crate) zone_grid_cache: std::sync::Mutex<crate::view::ZoneCache>,
    /// `SimView::utilities` cache, keyed by a fingerprint of the links.
    pub(crate) utilities_cache: std::sync::Mutex<crate::view::UtilitiesCache>,
    /// `SimView::agents` cache, keyed by the tick.
    pub(crate) agents_cache: std::sync::Mutex<crate::view::AgentCache>,
    /// `SimView::overlays` land value and pollution, keyed by the grids' revision.
    pub(crate) land_cache: std::sync::Mutex<crate::overlay::LandCache>,
    /// `SimView::city` cache, keyed by `CityStats::generation`.
    /// `SimView::service` cache, keyed by `ServiceCoverage::generation`.
    pub(crate) service_cache: std::sync::Mutex<crate::view::ServiceCache>,
    /// `SimView::service_state` cache, keyed by `ServiceState::revision`.
    pub(crate) service_state_cache: std::sync::Mutex<crate::view::ServiceStateCache>,
    pub(crate) city_cache: std::sync::Mutex<crate::view::CityCache>,
}

fn multiplier(speed: SimSpeed) -> u128 {
    match speed {
        SimSpeed::Pause => 0,
        SimSpeed::X1 => 1,
        SimSpeed::X2 => 2,
        SimSpeed::X3 => 3,
    }
}

impl SimRunner {
    /// A fresh sim with the given seed, desktop limits, speed ×1 and tick 0.
    pub fn new(seed: u64) -> Self {
        Self::with_limits(seed, SimLimits::desktop())
    }

    /// Like [`SimRunner::new`] with explicit capacities, e.g. `SimLimits::wasm()` for the
    /// browser build (system-design §3, §15).
    pub fn with_limits(seed: u64, limits: SimLimits) -> Self {
        // `par_iter` in sim systems needs the global compute pool; initialise it here so
        // headless runs (tests, bench) work. Idempotent; apps that set it up first win.
        ComputeTaskPool::get_or_init(TaskPool::default);
        let mut world = World::new();
        init_command_resources(&mut world);
        world.insert_resource(SimRng::seed_from_u64(seed));
        world.insert_resource(limits);
        // Nothing owned until a map is set (`set_terrain`) or a save is restored; keeps
        // the checksum and `SimView` readers valid on a terrain-less sim.
        world.insert_resource(TileOwnership::none());
        world.init_resource::<SimChecksum>();
        world.init_resource::<NetGraph>();
        world.init_resource::<JunctionGeometry>();
        world.init_resource::<cp_net::EntryReach>();
        world.init_resource::<cp_zone::ZoneGrid>();
        world.init_resource::<cp_building::BuildingGrid>();
        world.init_resource::<cp_building::Demand>();
        world.init_resource::<cp_utility::UtilityGraph>();
        world.init_resource::<cp_citizen::CitizenGrid>();
        world.init_resource::<cp_traffic::PathNetwork>();
        world.init_resource::<cp_traffic::PathService>();
        world.init_resource::<cp_traffic::PathResults>();
        world.init_resource::<cp_citizen::TripBook>();
        world.init_resource::<cp_citizen::RoutineSettings>();
        world.init_resource::<cp_traffic::Agents>();
        world.init_resource::<cp_traffic::AgentSettings>();
        world.init_resource::<cp_core::Treasury>();
        world.init_resource::<cp_district::DistrictMap>();
        world.init_resource::<cp_district::Districts>();
        cp_service::ServicePlugin::init(&mut world);
        world.init_resource::<cp_economy::EconomyState>();
        world.init_resource::<cp_economy::DistrictIncome>();
        world.init_resource::<cp_economy::DemandAcc>();
        world.init_resource::<cp_economy::LandGrids>();
        world.init_resource::<cp_economy::Decay>();
        world.init_resource::<cp_economy::WorkerShortage>();
        world
            .resource_mut::<SimChecksum>()
            .register("net", crate::checksum::net_part);
        world
            .resource_mut::<SimChecksum>()
            .register("zone", crate::checksum::zone_part);
        world
            .resource_mut::<SimChecksum>()
            .register("paths", crate::checksum::paths_part);
        world
            .resource_mut::<SimChecksum>()
            .register("buildings", crate::checksum::building_part);
        world
            .resource_mut::<SimChecksum>()
            .register("utilities", crate::checksum::utility_part);
        world
            .resource_mut::<SimChecksum>()
            .register("citizens", crate::checksum::citizen_part);
        world
            .resource_mut::<SimChecksum>()
            .register("trips", crate::checksum::trip_part);
        world
            .resource_mut::<SimChecksum>()
            .register("agents", crate::checksum::agent_part);
        world
            .resource_mut::<SimChecksum>()
            .register("economy", crate::checksum::economy_part);
        world
            .resource_mut::<SimChecksum>()
            .register("districts", crate::checksum::district_part);
        world
            .resource_mut::<SimChecksum>()
            .register("services", crate::checksum::service_part);
        world.init_resource::<crate::view::ArenaGauges>();
        let building_cap = world.resource::<SimLimits>().buildings;
        let limits = *world.resource::<SimLimits>();
        let mut gauges = world.resource_mut::<crate::view::ArenaGauges>();
        gauges.register("buildings", building_cap, |w| {
            w.resource::<cp_building::BuildingGrid>().len()
        });
        gauges.register("citizens", limits.citizens, |w| {
            w.resource::<cp_citizen::CitizenGrid>().citizen_count()
        });
        gauges.register("pedestrians", limits.pedestrians, |w| {
            w.resource::<cp_traffic::Agents>().pedestrian_count()
        });
        gauges.register("vehicles", limits.vehicles, |w| {
            w.resource::<cp_traffic::Agents>().vehicle_count()
        });
        gauges.register("trips", limits.citizens, |w| {
            w.resource::<cp_citizen::TripBook>().len()
        });
        gauges.register("households", limits.households, |w| {
            w.resource::<cp_citizen::CitizenGrid>().household_count()
        });

        let mut schedule = Schedule::default();
        use SimPhase::*;
        schedule.configure_sets((
            (
                Commands, Network, Agents, Paths, Citizens, Buildings, Utilities, Economy,
                Services, Stats,
            )
                .chain(),
            ChecksumSet.after(Stats),
        ));
        schedule.add_systems(dispatch_commands.in_set(SimPhase::Commands));
        schedule.add_systems(cp_net::update_junctions.in_set(SimPhase::Network));
        schedule.add_systems(
            cp_net::update_entry_reach
                .in_set(SimPhase::Network)
                .after(cp_net::update_junctions),
        );
        // After the net's own Network-phase updates (see `cp_zone::update_zones`).
        schedule.add_systems(
            cp_zone::update_zones
                .in_set(SimPhase::Network)
                .after(cp_net::update_junctions),
        );
        schedule.add_systems(cp_building::update_buildings.in_set(SimPhase::Buildings));
        // Demolished buildings: drop the households, jobs, trips and agents that pointed at
        // them before the citizens act.
        schedule.add_systems(
            cp_traffic::purge_removed_buildings
                .in_set(SimPhase::Citizens)
                .before(cp_citizen::update_citizens),
        );
        // The service state of those buildings goes first: the purge above drains the list.
        schedule.add_systems(
            cp_service::purge_removed_services
                .in_set(SimPhase::Citizens)
                .before(cp_traffic::purge_removed_buildings),
        );
        schedule.add_systems(cp_utility::update_supply.in_set(SimPhase::Utilities));
        schedule.add_systems(cp_service::update_coverage.in_set(SimPhase::Services));
        schedule.add_systems(cp_service::reset_daily_capacity.in_set(SimPhase::Services));
        // After the reset and the coverage rebuild: it reads both.
        schedule.add_systems(
            cp_service::update_garbage
                .in_set(SimPhase::Services)
                .after(cp_service::update_coverage)
                .after(cp_service::reset_daily_capacity),
        );
        schedule.add_systems(
            cp_service::update_fire
                .in_set(SimPhase::Services)
                .after(cp_service::update_garbage)
                .after(cp_service::update_coverage)
                .after(cp_service::reset_daily_capacity),
        );
        schedule.add_systems(cp_economy::settle_week.in_set(SimPhase::Economy));
        schedule.add_systems(cp_economy::update_demand.in_set(SimPhase::Economy));
        schedule.add_systems(cp_economy::update_city.in_set(SimPhase::Economy));
        schedule.add_systems(
            cp_economy::update_police
                .in_set(SimPhase::Economy)
                .after(cp_economy::update_city),
        );
        schedule.add_systems(cp_citizen::update_citizens.in_set(SimPhase::Citizens));
        // Path snapshot after the junction links it is built from; results are applied in
        // the Paths phase (system-design §2: requests from tick t land exactly at t + 8).
        schedule.add_systems(
            cp_traffic::update_path_network
                .in_set(SimPhase::Network)
                .after(cp_net::update_junctions),
        );
        schedule.add_systems(cp_traffic::deliver_paths.in_set(SimPhase::Paths));
        schedule.add_systems(
            cp_traffic::receive_paths
                .in_set(SimPhase::Paths)
                .after(cp_traffic::deliver_paths),
        );
        // Agents move first, then freshly planned trips ask for routes.
        schedule.add_systems(
            (cp_traffic::move_agents, cp_traffic::route_trips)
                .chain()
                .in_set(SimPhase::Agents),
        );
        schedule.add_systems(cp_traffic::update_traffic_load.in_set(SimPhase::Stats));
        // Derived summary for the UI: after the load statistics it reads, never read by the sim.
        world.init_resource::<crate::stats::CityStats>();
        schedule.add_systems(
            crate::stats::update_city_stats
                .in_set(SimPhase::Stats)
                .after(cp_traffic::update_traffic_load),
        );
        schedule.add_systems(
            cp_citizen::update_routine
                .in_set(SimPhase::Citizens)
                .after(cp_citizen::update_citizens),
        );
        schedule.add_systems(update_checksum.in_set(ChecksumSet));
        Self {
            world,
            schedule,
            accumulator: 0,
            net_cache: Default::default(),
            zone_cache: Default::default(),
            buildings_cache: Default::default(),
            district_cache: Default::default(),
            zone_grid_cache: Default::default(),
            utilities_cache: Default::default(),
            agents_cache: Default::default(),
            land_cache: Default::default(),
            service_cache: Default::default(),
            service_state_cache: Default::default(),
            city_cache: Default::default(),
        }
    }

    pub fn add_plugin(&mut self, plugin: &impl SimPlugin) -> &mut Self {
        plugin.build(&mut SimBuilder {
            world: &mut self.world,
            schedule: &mut self.schedule,
        });
        self
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// How far real time is into the tick that follows the current one, `0.0..1.0`: the
    /// leftover of the speed accumulator. Clients use it to interpolate agent poses.
    pub fn tick_fraction(&self) -> f32 {
        (self.accumulator as f64 / TICK_NANOS as f64).min(0.999) as f32
    }

    pub fn tick(&self) -> Tick {
        *self.world.resource::<Tick>()
    }

    pub fn speed(&self) -> SimSpeed {
        *self.world.resource::<SimSpeed>()
    }

    /// Queues a command for execution at `tick`.
    pub fn push_command(&mut self, command: SimCommand, tick: Tick) -> CommandId {
        self.world
            .resource_mut::<CommandQueue<SimCommand>>()
            .push(command, tick)
    }

    /// Applies every command already due at the current tick without running a tick.
    ///
    /// Needed while paused: a command sent at the current tick (e.g. `SetSpeed` to resume)
    /// would otherwise wait for a tick that never comes. Equivalent to the start of the
    /// next tick's `Commands` phase (nothing runs before it), so replaying the log gives
    /// the same state. Outcomes are readable like any others.
    pub fn apply_due_commands(&mut self) {
        dispatch_commands(&mut self.world);
        // No tick follows while paused, so the `Network` phase does not run: refresh the
        // geometry clients draw (derived, never saved or hashed; the dirty set stays for the
        // next tick) so a road built while paused is trimmed and has its junction (BUG-023).
        self.world
            .resource_scope(|world, mut geo: Mut<JunctionGeometry>| {
                cp_net::refresh_geometry(world.resource::<NetGraph>(), &mut geo);
            });
    }

    /// Cars and pedestrians on the road right now.
    pub fn agent_counts(&self) -> (usize, usize) {
        let a = self.world.resource::<cp_traffic::Agents>();
        (a.vehicle_count(), a.pedestrian_count())
    }

    /// Tops the synthetic traffic up to `cars` and `peds` (see
    /// [`DebugCommand::TopUpTraffic`]) with a logged command applied right now (a command
    /// that adds nothing is rejected and not logged).
    pub fn top_up_traffic(&mut self, cars: u32, peds: u32) {
        let now = self.tick();
        self.push_command(
            SimCommand::Debug(DebugCommand::TopUpTraffic { cars, peds }),
            now,
        );
        self.apply_due_commands();
    }

    /// Runs exactly one tick, ignoring speed and the accumulator.
    pub fn tick_once(&mut self) {
        self.world
            .resource_mut::<Messages<CommandOutcome>>()
            .update();
        self.schedule.run(&mut self.world);
        self.world.resource_mut::<Tick>().advance();
    }

    /// Advances by `dt` of frame time at the current speed, stopping as soon as a command
    /// pauses the sim (`system-design.md` §2 "Пауза"). While paused it still applies
    /// commands due at the current tick, so a queued `SetSpeed` can unpause. Returns ticks executed
    /// (at most [`MAX_TICKS_PER_STEP`]; surplus time is dropped).
    pub fn step(&mut self, dt: Duration) -> u32 {
        let mut mult = multiplier(self.speed());
        if mult == 0 {
            // Paused: no tick runs, but commands due now (e.g. `SetSpeed` to resume) must
            // still go through the command layer (BUG-005). Same as the start of the next
            // tick's `Commands` phase, so replay from the log matches.
            self.apply_due_commands();
            mult = multiplier(self.speed());
            if mult == 0 {
                self.accumulator = 0;
                return 0;
            }
        }
        self.accumulator += dt.as_nanos() * mult;
        let owed = self.accumulator / TICK_NANOS;
        let n = if owed > u128::from(MAX_TICKS_PER_STEP) {
            self.accumulator = 0;
            MAX_TICKS_PER_STEP
        } else {
            self.accumulator -= owed * TICK_NANOS;
            owed as u32
        };
        // The speed is re-read before every tick: once a command made it `Pause`, the rest
        // of this call does not run and the accumulator is dropped, so a pause costs at most
        // the tick that applied it. Switching between running speeds does not change `n`
        // (it was fixed from the speed at entry).
        let mut ran = 0;
        for _ in 0..n {
            if self.speed() == SimSpeed::Pause {
                self.accumulator = 0;
                break;
            }
            self.tick_once();
            ran += 1;
        }
        if self.speed() == SimSpeed::Pause {
            self.accumulator = 0;
        }
        ran
    }

    /// Checksum of the current state (see `checksum` module).
    pub fn checksum(&mut self) -> u64 {
        self.world.resource_scope(
            |world, mut cs: bevy_ecs::change_detection::Mut<SimChecksum>| cs.compute(world),
        )
    }
}

/// Runs a fresh sim for `ticks` ticks with `commands` pre-queued and returns the final
/// checksum. For tests and `apps/bench`.
pub fn run_headless(seed: u64, commands: &[(Tick, SimCommand)], ticks: u64) -> u64 {
    run_headless_with(seed, commands, ticks, |_| {})
}

/// Like [`run_headless`], with a hook to add plugins before the first tick.
pub fn run_headless_with(
    seed: u64,
    commands: &[(Tick, SimCommand)],
    ticks: u64,
    setup: impl FnOnce(&mut SimRunner),
) -> u64 {
    let mut sim = SimRunner::new(seed);
    setup(&mut sim);
    for (tick, cmd) in commands {
        sim.push_command(cmd.clone(), *tick);
    }
    for _ in 0..ticks {
        sim.tick_once();
    }
    sim.checksum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ecs::message::MessageCursor;
    use cp_core::CommandLog;
    use std::sync::Arc;

    const FRAME: Duration = Duration::from_millis(50);

    fn run_frames(speed: SimSpeed, frames: u32, dt: Duration) -> u64 {
        let mut sim = SimRunner::new(1);
        sim.world_mut().insert_resource(speed);
        (0..frames).map(|_| u64::from(sim.step(dt))).sum()
    }

    /// A sim with a road content set and terrain, and a T of two `basic_2l` roads queued for
    /// the current tick (the second ends on the middle of the first).
    fn sim_with_t() -> SimRunner {
        use bevy_math::Vec2;
        use cp_net::{ElevationMode, NetCommand, RoadPath};
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
        let db = Arc::new(cp_content::ContentDb::load_dir(format!("{dir}/content")).unwrap());
        let terrain = Arc::new(
            cp_terrain::Terrain::load(&std::path::Path::new(dir).join("maps/flatlands")).unwrap(),
        );
        let mut sim = SimRunner::new(1);
        sim.set_terrain(terrain);
        sim.set_content(Arc::clone(&db));
        let road = db.road_id("basic_2l").unwrap();
        let at = sim.tick();
        for (from, to) in [
            (Vec2::new(4600.0, 4700.0), Vec2::new(4800.0, 4700.0)),
            (Vec2::new(4700.0, 4600.0), Vec2::new(4700.0, 4700.0)),
        ] {
            sim.push_command(
                SimCommand::Net(NetCommand::BuildRoad {
                    road,
                    path: RoadPath::Straight { from, to },
                    elevation: ElevationMode::Ground,
                }),
                at,
            );
        }
        sim
    }

    /// BUG-023/024: a road built while paused (commands applied, no tick) is trimmed and has
    /// its junction polygon in the view straight away, and the next tick changes neither the
    /// geometry nor the state.
    #[test]
    fn paused_build_road_gives_trimmed_segments_and_a_junction_polygon() {
        let mut paused = sim_with_t();
        paused.apply_due_commands();
        let v = paused.view();
        assert!(v.net.segment_count() >= 4);
        assert_eq!(v.junctions.len(), v.net.node_count());
        let t = v
            .junctions
            .iter()
            .map(|(_, j)| j)
            .find(|j| j.arms.len() == 3)
            .expect("the T node");
        assert!(t.polygon.len() >= 4);
        for a in &t.arms {
            let curve = v.junctions.trimmed_curve(&v.net, a.segment).unwrap();
            let seg = v.net.segment(a.segment).unwrap();
            assert!(curve.p3.distance(seg.curve.p3) > 1.0 || curve.p0.distance(seg.curve.p0) > 1.0);
        }
        let before: Vec<_> = v
            .junctions
            .iter()
            .map(|(n, j)| (*n, j.polygon.clone()))
            .collect();

        // The same commands with no refresh (applied inside the tick) end in the same state
        // and the same geometry: the refresh touched nothing the sim hashes.
        let mut quiet = sim_with_t();
        for _ in 0..3 {
            paused.tick_once();
            quiet.tick_once();
        }
        assert_eq!(paused.checksum(), quiet.checksum());
        let after: Vec<_> = paused
            .view()
            .junctions
            .iter()
            .map(|(n, j)| (*n, j.polygon.clone()))
            .collect();
        assert_eq!(before, after);
        let q: Vec<_> = quiet
            .view()
            .junctions
            .iter()
            .map(|(n, j)| (*n, j.polygon.clone()))
            .collect();
        assert_eq!(after, q);
    }

    #[test]
    fn with_limits_sets_the_capacity_resource() {
        let sim = SimRunner::with_limits(1, SimLimits::wasm());
        assert_eq!(*sim.world().resource::<SimLimits>(), SimLimits::wasm());
        let sim = SimRunner::new(1);
        assert_eq!(*sim.world().resource::<SimLimits>(), SimLimits::desktop());
    }

    #[test]
    fn x3_runs_triple_the_ticks_of_x1_for_the_same_frames() {
        // 50 ms frames: ×1 owes exactly 1 tick/frame, ×3 owes exactly 3.
        let x1 = run_frames(SimSpeed::X1, 100, FRAME);
        let x3 = run_frames(SimSpeed::X3, 100, FRAME);
        assert_eq!((x1, x3), (100, 300));
        assert_eq!(run_frames(SimSpeed::X2, 100, FRAME), 200);
        assert_eq!(run_frames(SimSpeed::Pause, 100, FRAME), 0);
    }

    #[test]
    fn fractional_frames_accumulate_without_drift() {
        // 10 ms frames at ×1: one tick every 5 frames.
        assert_eq!(run_frames(SimSpeed::X1, 100, Duration::from_millis(10)), 20);
    }

    #[test]
    fn at_most_four_ticks_per_call_and_surplus_is_dropped() {
        let mut sim = SimRunner::new(1);
        assert_eq!(sim.step(Duration::from_secs(10)), 4);
        assert_eq!(sim.tick(), Tick(4));
        // Surplus was dropped: a zero-length step owes nothing.
        assert_eq!(sim.step(Duration::ZERO), 0);
        // Exactly 4 owed is allowed and leaves nothing behind.
        assert_eq!(sim.step(FRAME * 4), 4);
    }

    #[test]
    fn tick_increments_once_per_tick_after_stats() {
        let mut sim = SimRunner::new(1);
        assert_eq!(sim.tick(), Tick(0));
        sim.tick_once();
        sim.tick_once();
        assert_eq!(sim.tick(), Tick(2));
        assert_eq!(sim.tick().game_time().tick_of_day, 2);
    }

    #[test]
    fn paused_sim_can_be_resumed_by_a_command_at_the_current_tick() {
        let mut sim = SimRunner::new(1);
        sim.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::Pause,
            },
            sim.tick(),
        );
        sim.apply_due_commands();
        assert_eq!(sim.step(FRAME * 4), 0);
        sim.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::X2,
            },
            sim.tick(),
        );
        sim.apply_due_commands();
        assert_eq!(sim.speed(), SimSpeed::X2);
        assert_eq!(sim.step(FRAME), 2);
        let log = sim.world().resource::<CommandLog<SimCommand>>();
        assert_eq!(log.len(), 2);
        assert_eq!(log.entries()[0].tick, Tick(0));
        // Applying again does nothing: nothing is due.
        sim.apply_due_commands();
        assert_eq!(sim.world().resource::<CommandLog<SimCommand>>().len(), 2);
    }

    #[test]
    fn t056_pause_in_the_first_tick_of_a_multi_tick_step_stops_the_call() {
        let set = |speed| SimCommand::SetSpeed { speed };
        let mut sim = SimRunner::new(1);
        sim.world_mut().insert_resource(SimSpeed::X3);
        sim.push_command(set(SimSpeed::Pause), Tick(0));
        // 4 ticks owed (capped), but the pause applied by the first one stops the rest.
        assert_eq!(sim.step(FRAME * 2), 1);
        assert_eq!(sim.tick(), Tick(1));
        assert_eq!(sim.speed(), SimSpeed::Pause);
        // On pause nothing runs and the accumulator is empty...
        assert_eq!(sim.step(FRAME * 10), 0);
        assert_eq!(sim.tick(), Tick(1));
        // ...so after resuming at x1 a 30 ms frame owes nothing (a leftover would tick).
        sim.push_command(set(SimSpeed::X1), sim.tick());
        assert_eq!(sim.step(Duration::from_millis(30)), 0);
        assert_eq!(sim.speed(), SimSpeed::X1);
        assert_eq!(sim.step(Duration::from_millis(20)), 1);
    }

    #[test]
    fn t056_changing_between_running_speeds_inside_a_call_keeps_the_tick_count() {
        let set = |speed| SimCommand::SetSpeed { speed };
        for (from, to) in [(SimSpeed::X1, SimSpeed::X3), (SimSpeed::X3, SimSpeed::X1)] {
            let mut sim = SimRunner::new(1);
            sim.world_mut().insert_resource(from);
            sim.push_command(set(to), Tick(0));
            let expected = if from == SimSpeed::X3 { 3 } else { 1 };
            assert_eq!(sim.step(FRAME), expected, "{from:?} -> {to:?}");
        }
    }

    #[test]
    fn bug_005_paused_step_applies_a_queued_resume() {
        let resume = |speed| SimCommand::SetSpeed { speed };
        let mut sim = SimRunner::new(1);
        sim.push_command(resume(SimSpeed::Pause), Tick(0));
        assert_eq!(sim.step(FRAME), 1); // tick 0 runs and applies the pause
        assert_eq!(sim.speed(), SimSpeed::Pause);
        assert_eq!(sim.step(FRAME * 4), 0); // paused: nothing ticks
        sim.push_command(resume(SimSpeed::X1), sim.tick());
        assert_eq!(sim.step(FRAME), 1); // resumed and ticked in the same step
        assert_eq!(sim.speed(), SimSpeed::X1);
        assert_eq!(sim.tick(), Tick(2));
    }

    #[test]
    fn pause_resume_run_replays_from_its_log_to_the_same_checksum() {
        let mut live = SimRunner::new(7);
        let mut frame = 0u64;
        let push_at = |sim: &mut SimRunner, speed| {
            sim.push_command(SimCommand::SetSpeed { speed }, sim.tick());
        };
        for _ in 0..40 {
            frame += 1;
            match frame {
                5 => push_at(&mut live, SimSpeed::Pause),
                20 => push_at(&mut live, SimSpeed::X3), // sent while paused
                30 => push_at(&mut live, SimSpeed::X2),
                _ => {}
            }
            live.step(FRAME);
        }
        let ticks = live.tick().0;
        assert!(ticks > 10);
        let log = live
            .world()
            .resource::<CommandLog<SimCommand>>()
            .entries()
            .to_vec();
        assert_eq!(log.len(), 3);
        let live_sum = live.checksum();

        let mut replay = SimRunner::new(7);
        for e in &log {
            replay.push_command(e.command.clone(), e.tick);
        }
        for _ in 0..ticks {
            replay.tick_once();
        }
        assert_eq!(replay.tick(), live.tick());
        assert_eq!(replay.checksum(), live_sum);
    }

    #[test]
    fn set_speed_command_changes_stepping() {
        let mut sim = SimRunner::new(1);
        sim.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::Pause,
            },
            Tick(2),
        );
        // Ticks 0, 1, 2 run; the pause fires in tick 2 and the 4th owed tick is dropped
        // (T-056; this used to assert 4).
        assert_eq!(sim.step(FRAME * 10), 3);
        assert_eq!(sim.tick(), Tick(3));
        assert_eq!(sim.speed(), SimSpeed::Pause);
        assert_eq!(sim.step(FRAME * 10), 0);
        assert_eq!(sim.world().resource::<CommandLog<SimCommand>>().len(), 1);
    }

    #[test]
    fn outcomes_are_readable_for_two_ticks_then_gone() {
        let mut sim = SimRunner::new(1);
        sim.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::X2,
            },
            Tick(0),
        );
        let mut cursor = MessageCursor::<CommandOutcome>::default();
        let count = |sim: &SimRunner, cursor: &mut MessageCursor<CommandOutcome>| {
            cursor
                .read(sim.world().resource::<Messages<CommandOutcome>>())
                .count()
        };
        sim.tick_once(); // tick 0 applies it
        assert_eq!(count(&sim, &mut cursor), 1);

        let mut late = MessageCursor::<CommandOutcome>::default();
        sim.tick_once(); // tick 1: previous-buffer, still readable
        assert_eq!(count(&sim, &mut late), 1);
        sim.tick_once(); // tick 2: rotated out
        assert_eq!(
            count(&sim, &mut MessageCursor::<CommandOutcome>::default()),
            0
        );
    }

    #[test]
    fn net_state_is_part_of_the_checksum() {
        use bevy_math::Vec3;
        use cp_net::{Elevation, NetGraph, NodeKind};
        let mut a = SimRunner::new(1);
        let mut b = SimRunner::new(1);
        assert_eq!(a.checksum(), b.checksum());
        let add = |s: &mut SimRunner| {
            s.world_mut().resource_mut::<NetGraph>().add_node(
                Vec3::ONE,
                NodeKind::Outside,
                Elevation::Ground,
            );
        };
        add(&mut a);
        assert_ne!(a.checksum(), b.checksum());
        add(&mut b);
        assert_eq!(a.checksum(), b.checksum());
    }

    #[test]
    fn periodic_checksum_every_256_ticks() {
        let mut sim = SimRunner::new(1);
        for _ in 0..255 {
            sim.tick_once();
        }
        assert!(sim.world().resource::<SimChecksum>().last.is_none());
        sim.tick_once();
        let (tick, v) = sim.world().resource::<SimChecksum>().last.unwrap();
        assert_eq!(tick, Tick(255));
        let _ = v;
        for _ in 0..256 {
            sim.tick_once();
        }
        assert_eq!(
            sim.world().resource::<SimChecksum>().last.unwrap().0,
            Tick(511)
        );
    }

    #[test]
    fn phases_run_in_order() {
        use crate::plugin::{SimBuilder, SimPlugin};
        use bevy_ecs::resource::Resource;
        use bevy_ecs::system::ResMut;
        #[derive(Resource, Default)]
        struct Trace(Vec<SimPhase>);
        fn mark(phase: SimPhase) -> impl Fn(ResMut<Trace>) {
            move |mut t| t.0.push(phase)
        }
        struct P;
        impl SimPlugin for P {
            fn build(&self, sim: &mut SimBuilder) {
                sim.world_mut().init_resource::<Trace>();
                // Added in reverse order on purpose.
                for p in SimPhase::ORDER.into_iter().rev() {
                    sim.add_systems(p, mark(p));
                }
            }
        }
        let mut sim = SimRunner::new(1);
        sim.add_plugin(&P);
        sim.tick_once();
        assert_eq!(sim.world().resource::<Trace>().0, SimPhase::ORDER);
    }
}
