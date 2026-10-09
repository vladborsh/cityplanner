//! `cp_tools` — client tools. See `architecture/system-design.md` §1, §13.
//!
//! Tools and UI never touch sim state: they push [`SimCommand`]s into the
//! [`CommandOutbox`], and the app drains it into the sim once per frame.

use bevy_ecs::resource::Resource;
use cp_sim::{SimCommand, SimRunner};

/// Commands produced by tools and UI this frame, waiting for the app to submit them.
#[derive(Resource, Default, Debug)]
pub struct CommandOutbox(Vec<SimCommand>);

impl CommandOutbox {
    pub fn push(&mut self, command: SimCommand) {
        self.0.push(command);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &SimCommand> {
        self.0.iter()
    }

    /// Submits everything to the sim's `CommandQueue` for the next tick to run (the design's
    /// "current tick + 1", where the current tick is the last one completed), in the order
    /// it was pushed, then applies what is due so commands also take effect while the sim is
    /// paused. Returns how many were submitted.
    pub fn flush_into(&mut self, sim: &mut SimRunner) -> usize {
        let n = self.0.len();
        if n == 0 {
            return 0;
        }
        let at = sim.tick();
        for command in self.0.drain(..) {
            sim.push_command(command, at);
        }
        sim.apply_due_commands();
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_core::{CommandLog, SimSpeed, Tick};
    use std::time::Duration;

    fn set(speed: SimSpeed) -> SimCommand {
        SimCommand::SetSpeed { speed }
    }

    #[test]
    fn flush_submits_in_order_through_the_queue_into_the_log() {
        let mut sim = SimRunner::new(1);
        let mut out = CommandOutbox::default();
        out.push(set(SimSpeed::X2));
        out.push(set(SimSpeed::X3));
        assert_eq!(out.flush_into(&mut sim), 2);
        assert!(out.is_empty());
        let log = sim.world().resource::<CommandLog<SimCommand>>();
        assert_eq!(log.len(), 2);
        assert_eq!(log.entries()[0].command, set(SimSpeed::X2));
        assert_eq!(log.entries()[1].command, set(SimSpeed::X3));
        assert_eq!(sim.speed(), SimSpeed::X3);
        assert_eq!(out.flush_into(&mut sim), 0);
    }

    #[test]
    fn a_paused_sim_can_be_resumed_through_the_outbox() {
        let mut sim = SimRunner::new(1);
        let mut out = CommandOutbox::default();
        out.push(set(SimSpeed::Pause));
        out.flush_into(&mut sim);
        assert_eq!(sim.step(Duration::from_millis(500)), 0);
        out.push(set(SimSpeed::X1));
        out.flush_into(&mut sim);
        assert_eq!(sim.step(Duration::from_millis(100)), 2);
        assert_eq!(sim.tick(), Tick(2));
    }
}
