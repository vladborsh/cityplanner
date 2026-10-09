//! Extension point for domain crates.

use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule};
use bevy_ecs::system::ScheduleSystem;
use bevy_ecs::world::World;

use crate::checksum::{ChecksumFn, SimChecksum};
use crate::phase::SimPhase;
use crate::view::ArenaGauges;

/// A domain adds its resources, systems and checksum parts to the sim.
pub trait SimPlugin {
    fn build(&self, sim: &mut SimBuilder);
}

/// What a [`SimPlugin`] can touch while building.
pub struct SimBuilder<'a> {
    pub(crate) world: &'a mut World,
    pub(crate) schedule: &'a mut Schedule,
}

impl SimBuilder<'_> {
    pub fn world_mut(&mut self) -> &mut World {
        self.world
    }

    /// Adds systems to a phase. Ordering inside the phase is up to the caller.
    pub fn add_systems<M>(
        &mut self,
        phase: SimPhase,
        systems: impl IntoScheduleConfigs<ScheduleSystem, M>,
    ) -> &mut Self {
        self.schedule.add_systems(systems.in_set(phase));
        self
    }

    /// Adds systems to the schedule **without** placing them in a phase, so the caller can
    /// order them against `SimPhase` sets (e.g. `.after(SimPhase::A).before(SimPhase::B)`).
    /// For tooling such as `apps/bench` phase timing; domain systems use [`Self::add_systems`].
    pub fn add_systems_unphased<M>(
        &mut self,
        systems: impl IntoScheduleConfigs<ScheduleSystem, M>,
    ) -> &mut Self {
        self.schedule.add_systems(systems);
        self
    }

    /// Reports an arena's fill in `SimView::stats` (debug window). `len` returns the number
    /// of live elements; `capacity` is normally the matching `SimLimits` field.
    pub fn register_arena_gauge(
        &mut self,
        name: &'static str,
        capacity: u32,
        len: fn(&World) -> usize,
    ) -> &mut Self {
        self.world
            .resource_mut::<ArenaGauges>()
            .register(name, capacity, len);
        self
    }

    /// Registers state to be hashed into `SimChecksum` (see `checksum` module docs).
    pub fn register_checksum(&mut self, name: &'static str, part: ChecksumFn) -> &mut Self {
        self.world
            .resource_mut::<SimChecksum>()
            .register(name, part);
        self
    }
}
