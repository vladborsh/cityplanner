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
use bevy_ecs::world::World;
use bevy_tasks::{ComputeTaskPool, TaskPool};
use cp_core::time::TICK_MILLIS;
use cp_core::{CommandId, CommandOutcome, CommandQueue, SimLimits, SimRng, SimSpeed, Tick};
use cp_terrain::TileOwnership;

use crate::checksum::{SimChecksum, update_checksum};
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
        // `par_iter` in sim systems needs the global compute pool; initialise it here so
        // headless runs (tests, bench) work. Idempotent; apps that set it up first win.
        ComputeTaskPool::get_or_init(TaskPool::default);
        let mut world = World::new();
        init_command_resources(&mut world);
        world.insert_resource(SimRng::seed_from_u64(seed));
        world.insert_resource(SimLimits::desktop());
        world.insert_resource(TileOwnership::m1_default());
        world.init_resource::<SimChecksum>();

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
        schedule.add_systems(update_checksum.in_set(ChecksumSet));
        Self {
            world,
            schedule,
            accumulator: 0,
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

    /// Runs exactly one tick, ignoring speed and the accumulator.
    pub fn tick_once(&mut self) {
        self.world
            .resource_mut::<Messages<CommandOutcome>>()
            .update();
        self.schedule.run(&mut self.world);
        self.world.resource_mut::<Tick>().advance();
    }

    /// Advances by `dt` of frame time at the current speed. Returns ticks executed
    /// (at most [`MAX_TICKS_PER_STEP`]; surplus time is dropped).
    pub fn step(&mut self, dt: Duration) -> u32 {
        let mult = multiplier(self.speed());
        if mult == 0 {
            return 0;
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
        for _ in 0..n {
            self.tick_once();
        }
        n
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

    const FRAME: Duration = Duration::from_millis(50);

    fn run_frames(speed: SimSpeed, frames: u32, dt: Duration) -> u64 {
        let mut sim = SimRunner::new(1);
        sim.world_mut().insert_resource(speed);
        (0..frames).map(|_| u64::from(sim.step(dt))).sum()
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
    fn set_speed_command_changes_stepping() {
        let mut sim = SimRunner::new(1);
        sim.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::Pause,
            },
            Tick(2),
        );
        assert_eq!(sim.step(FRAME * 10), 4); // ticks 0..4 run; command fires at tick 2
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
