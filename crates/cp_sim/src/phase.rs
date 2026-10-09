//! Schedule phases (`architecture/system-design.md` §2). They run strictly in this
//! order inside one tick; systems inside a phase may run in parallel unless the
//! domain orders them explicitly.

use bevy_ecs::schedule::SystemSet;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SimPhase {
    /// Apply commands due this tick (`dispatch_commands`).
    Commands,
    /// Network graph / intersection updates for dirty nodes.
    Network,
    /// Vehicle and pedestrian movement.
    Agents,
    /// Hand out finished paths.
    Paths,
    Citizens,
    Buildings,
    Utilities,
    Economy,
    Services,
    /// Counters and per-tick statistics. The periodic checksum runs right after.
    Stats,
}

impl SimPhase {
    /// All phases in execution order.
    pub const ORDER: [SimPhase; 10] = [
        SimPhase::Commands,
        SimPhase::Network,
        SimPhase::Agents,
        SimPhase::Paths,
        SimPhase::Citizens,
        SimPhase::Buildings,
        SimPhase::Utilities,
        SimPhase::Economy,
        SimPhase::Services,
        SimPhase::Stats,
    ];
}
