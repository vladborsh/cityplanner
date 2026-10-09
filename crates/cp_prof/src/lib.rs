//! `cp_prof` — per-phase wall-clock timing, built as a `SimPlugin`. Shared by `apps/bench`
//! and the desktop debug window (`architecture/system-design.md` §1).
//!
//! Headless and **not a sim crate**: the only place outside `apps/*` allowed to read the
//! clock. Its systems neither read nor write sim state; [`PhaseClock`] is not saved and not
//! in the checksum, and a run with or without [`PhaseTiming`] has the same checksum (test).
//! No sim crate may depend on it.
//!
//! Sim crates may not read the clock, so the host adds **marker systems** between the
//! phase sets: marker `i` runs after phase `i-1` and before phase `i` (marker 0 before
//! `Commands`, marker 10 after `Stats`). Phase `i` time = marker `i+1` − marker `i`.
//! The checksum set runs after `Stats` and is deliberately outside the phase numbers; it
//! is part of the wall-clock `total` measured around `tick_once` by the caller.
//!
//! **Overhead:** one `Instant::now()` (tens of ns) per marker, 11 per tick, plus the
//! scheduler cost of 11 extra systems and the ordering edges they add. That is a small
//! constant (well under 0.05 ms/tick) shared by baseline and current runs, so it cancels
//! out in the relative comparison.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bevy_ecs::prelude::*;
use cp_sim::{SimBuilder, SimPhase, SimPlugin};

pub const PHASES: usize = SimPhase::ORDER.len();

/// How many per-tick samples [`PhaseClock`] keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum History {
    /// Every tick (benchmarks, which post-process all samples).
    Unbounded,
    /// Only the most recent `n` ticks (a running game).
    Window(usize),
}

/// Samples per phase in nanoseconds, one entry per recorded tick.
#[derive(Resource)]
pub struct PhaseClock {
    marks: [Instant; PHASES + 1],
    history: History,
    pub samples: VecDeque<[u64; PHASES]>,
}

impl PhaseClock {
    fn new(history: History) -> Self {
        Self {
            marks: [Instant::now(); PHASES + 1],
            history,
            samples: VecDeque::new(),
        }
    }

    /// Median milliseconds per phase over the kept samples (zeros when empty). Robust against
    /// the occasional slow tick, and the statistic `apps/bench` reports, so the debug window
    /// and the bench agree.
    pub fn median_ms(&self) -> [f64; PHASES] {
        let mut out = [0.0; PHASES];
        if self.samples.is_empty() {
            return out;
        }
        for (p, o) in out.iter_mut().enumerate() {
            let mut v: Vec<u64> = self.samples.iter().map(|row| row[p]).collect();
            v.sort_unstable();
            *o = v[v.len() / 2] as f64 / 1e6;
        }
        out
    }

    /// Mean milliseconds per phase over the kept samples (zeros when empty).
    pub fn mean_ms(&self) -> [f64; PHASES] {
        let mut out = [0.0; PHASES];
        if self.samples.is_empty() {
            return out;
        }
        for row in &self.samples {
            for (o, v) in out.iter_mut().zip(row) {
                *o += *v as f64;
            }
        }
        let n = self.samples.len() as f64 * 1e6;
        out.map(|v| v / n)
    }

    fn mark(&mut self, i: usize) {
        self.marks[i] = Instant::now();
        if i == PHASES {
            let mut row = [0u64; PHASES];
            for (p, slot) in row.iter_mut().enumerate() {
                *slot = self.marks[p + 1].duration_since(self.marks[p]).as_nanos() as u64;
            }
            if let History::Window(n) = self.history {
                while self.samples.len() >= n.max(1) {
                    self.samples.pop_front();
                }
            }
            self.samples.push_back(row);
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
    pub history: History,
}

impl PhaseTiming {
    /// Keeps every sample, no injected sleep.
    pub fn unbounded() -> Self {
        Self {
            sleep: None,
            history: History::Unbounded,
        }
    }

    /// Keeps the last `ticks` samples, no injected sleep.
    pub fn window(ticks: usize) -> Self {
        Self {
            sleep: None,
            history: History::Window(ticks),
        }
    }
}

impl SimPlugin for PhaseTiming {
    fn build(&self, sim: &mut SimBuilder) {
        sim.world_mut()
            .insert_resource(PhaseClock::new(self.history));
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

#[cfg(test)]
mod tests {
    use super::*;
    use cp_core::{SimSpeed, Tick};
    use cp_sim::{SimCommand, SimRunner};

    fn run(timing: Option<PhaseTiming>) -> (u64, SimRunner) {
        let mut sim = SimRunner::new(5);
        if let Some(t) = &timing {
            sim.add_plugin(t);
        }
        sim.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::X2,
            },
            Tick(3),
        );
        for _ in 0..300 {
            sim.tick_once();
        }
        (sim.checksum(), sim)
    }

    #[test]
    fn timing_does_not_change_the_checksum() {
        let (plain, _) = run(None);
        let (timed, sim) = run(Some(PhaseTiming::unbounded()));
        assert_eq!(plain, timed);
        assert_eq!(sim.world().resource::<PhaseClock>().samples.len(), 300);
    }

    #[test]
    fn window_keeps_only_recent_ticks_and_reports_means() {
        let (_, sim) = run(Some(PhaseTiming::window(64)));
        let clock = sim.world().resource::<PhaseClock>();
        assert_eq!(clock.samples.len(), 64);
        assert!(clock.mean_ms().iter().all(|m| m.is_finite() && *m >= 0.0));
        assert!(clock.median_ms().iter().all(|m| m.is_finite() && *m >= 0.0));
    }

    #[test]
    fn injected_sleep_shows_up_in_its_phase_mean() {
        let t = PhaseTiming {
            sleep: Some(InjectedSleep {
                phase: SimPhase::Economy,
                duration: Duration::from_millis(2),
            }),
            history: History::Unbounded,
        };
        let (_, sim) = run(Some(t));
        let means = sim.world().resource::<PhaseClock>().mean_ms();
        let econ = SimPhase::ORDER
            .iter()
            .position(|p| *p == SimPhase::Economy)
            .unwrap();
        assert!(means[econ] >= 2.0, "{means:?}");
    }
}
