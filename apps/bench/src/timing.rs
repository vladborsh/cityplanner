//! Per-phase wall-clock timing for the bench, built as a `SimPlugin`.
//!
//! Sim crates may not read the clock, so the bench adds **marker systems** between the
//! phase sets: marker `i` runs after phase `i-1` and before phase `i` (marker 0 before
//! `Commands`, marker 10 after `Stats`). Phase `i` time = marker `i+1` − marker `i`.
//! The checksum set runs after `Stats` and is deliberately outside the phase numbers; it
//! is part of the wall-clock `total` measured around `tick_once` by the caller.
//!
//! **Overhead:** one `Instant::now()` (tens of ns) per marker, 11 per tick, plus the
//! scheduler cost of 11 extra systems and the ordering edges they add. That is a small
//! constant (well under 0.05 ms/tick) shared by baseline and current runs, so it cancels
//! out in the relative comparison.

use std::time::{Duration, Instant};

use bevy_ecs::prelude::*;
use cp_sim::{SimBuilder, SimPhase, SimPlugin};

pub const PHASES: usize = SimPhase::ORDER.len();

/// Samples per phase in nanoseconds, one entry per recorded tick.
#[derive(Resource)]
pub struct PhaseClock {
    marks: [Instant; PHASES + 1],
    pub samples: Vec<[u64; PHASES]>,
}

impl PhaseClock {
    fn new() -> Self {
        Self {
            marks: [Instant::now(); PHASES + 1],
            samples: Vec::new(),
        }
    }

    fn mark(&mut self, i: usize) {
        self.marks[i] = Instant::now();
        if i == PHASES {
            let mut row = [0u64; PHASES];
            for (p, slot) in row.iter_mut().enumerate() {
                *slot = self.marks[p + 1].duration_since(self.marks[p]).as_nanos() as u64;
            }
            self.samples.push(row);
        }
    }
}

/// Test hook for the CI gate: sleeps for `duration` inside `phase` every tick.
pub struct InjectedSleep {
    pub phase: SimPhase,
    pub duration: Duration,
}

pub struct PhaseTiming {
    pub sleep: Option<InjectedSleep>,
}

impl SimPlugin for PhaseTiming {
    fn build(&self, sim: &mut SimBuilder) {
        sim.world_mut().insert_resource(PhaseClock::new());
        for i in 0..=PHASES {
            let marker = move |mut clock: ResMut<PhaseClock>| clock.mark(i);
            match (i.checked_sub(1), SimPhase::ORDER.get(i)) {
                (Some(prev), Some(&next)) => {
                    sim.add_systems_unphased(marker.after(SimPhase::ORDER[prev]).before(next))
                }
                (None, Some(&next)) => sim.add_systems_unphased(marker.before(next)),
                (Some(prev), None) => sim.add_systems_unphased(marker.after(SimPhase::ORDER[prev])),
                (None, None) => unreachable!(),
            };
        }
        if let Some(s) = &self.sleep {
            let d = s.duration;
            sim.add_systems(s.phase, move || std::thread::sleep(d));
        }
    }
}
