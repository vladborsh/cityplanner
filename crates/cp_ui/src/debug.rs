//! F3 debug window: ticks/s, ms per phase, checksum, arena fill.

use bevy::prelude::Resource;
use bevy_egui::egui;
use cp_sim::{SimPhase, SimView};

/// Ticks per real second, measured over ~0.5 s windows from `SimView::tick`.
#[derive(Resource, Debug, Default)]
pub struct TickRate {
    window_start: f64,
    window_tick: u64,
    rate: f32,
}

impl TickRate {
    /// Call every frame with real elapsed seconds and the view's tick.
    pub fn update(&mut self, now: f64, tick: u64) {
        if tick < self.window_tick {
            self.window_tick = tick; // loaded an earlier save
        }
        let dt = now - self.window_start;
        if dt >= 0.5 {
            self.rate = ((tick - self.window_tick) as f64 / dt) as f32;
            self.window_start = now;
            self.window_tick = tick;
        }
    }

    pub fn per_sec(&self) -> f32 {
        self.rate
    }
}

/// Per-phase timings for the debug window. The app fills it from `cp_prof`'s `PhaseClock`
/// (which lives in the sim world, so the UI can't read it directly).
#[derive(Resource, Debug, Clone, Default)]
pub struct PhaseTimes {
    /// `(phase, median ms per tick)` in execution order.
    pub phases: Vec<(SimPhase, f64)>,
}

pub struct DebugData<'a> {
    pub view: &'a SimView,
    pub ticks_per_sec: f32,
    pub phases: Option<&'a PhaseTimes>,
}

pub fn debug_window(ctx: &egui::Context, d: &DebugData, open: &mut bool) {
    crate::frame::window("debug", crate::icons::Icon::Debug, "Debug (F3)", open).show(ctx, |ui| {
        ui.label(format!(
            "tick {}   {:.1} ticks/s",
            d.view.tick.0, d.ticks_per_sec
        ));
        match d.view.stats.checksum {
            Some((tick, sum)) => ui.label(format!("checksum {sum:016x} @ tick {}", tick.0)),
            None => ui.label("checksum — (first one at tick 255)"),
        };
        ui.label(format!(
            "commands: {} applied, {} pending",
            d.view.stats.commands_applied, d.view.stats.commands_pending
        ));
        ui.separator();
        ui.strong("Phases (median ms/tick)");
        match d.phases {
            Some(p) => {
                for (phase, ms) in &p.phases {
                    ui.label(format!("{:<10} {ms:7.3}", format!("{phase:?}")));
                }
            }
            None => {
                ui.weak("phase timing not installed");
            }
        }
        ui.separator();
        ui.strong("Arenas");
        if d.view.stats.arenas.is_empty() {
            ui.weak("none registered yet");
        }
        for a in &d.view.stats.arenas {
            ui.label(format!("{:<10} {} / {}", a.name, a.used, a.capacity));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_rate_is_ticks_over_real_time() {
        let mut r = TickRate::default();
        r.update(0.0, 0);
        r.update(0.25, 5); // window not yet 0.5 s
        assert_eq!(r.per_sec(), 0.0);
        r.update(0.5, 10);
        assert!((r.per_sec() - 20.0).abs() < 1e-3);
        r.update(1.0, 70);
        assert!((r.per_sec() - 120.0).abs() < 1e-3);
    }

    #[test]
    fn debug_window_draws_without_panicking() {
        let view = cp_sim::SimRunner::new(1).view();
        let ctx = egui::Context::default();
        let mut open = true;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        let phases = PhaseTimes {
            phases: vec![(SimPhase::Commands, 0.01)],
        };
        crate::testutil::frame(&ctx, input, |ui| {
            debug_window(
                ui.ctx(),
                &DebugData {
                    view: &view,
                    ticks_per_sec: 20.0,
                    phases: Some(&phases),
                },
                &mut open,
            );
        });
        assert!(open);
    }
}
