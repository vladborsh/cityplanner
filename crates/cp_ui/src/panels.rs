//! Top bar, bottom category bar.

use bevy::prelude::Resource;
use bevy_egui::egui;
use cp_core::SimSpeed;
use cp_sim::SimView;
use cp_tools::CommandOutbox;

use crate::set_speed;

/// Build-menu categories along the bottom. Tabs are empty until their tools land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Roads,
    Zones,
    Water,
    Power,
    Services,
    Transit,
    Districts,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Roads,
        Category::Zones,
        Category::Water,
        Category::Power,
        Category::Services,
        Category::Transit,
        Category::Districts,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Roads => "Roads",
            Category::Zones => "Zones",
            Category::Water => "Water",
            Category::Power => "Power",
            Category::Services => "Services (M2)",
            Category::Transit => "Transit (M3)",
            Category::Districts => "Districts (M4)",
        }
    }
}

#[derive(Resource, Debug, Default)]
pub struct UiState {
    pub category: Option<Category>,
    pub debug_open: bool,
}

pub const SPEEDS: [(SimSpeed, &str); 4] = [
    (SimSpeed::Pause, "⏸"),
    (SimSpeed::X1, "×1"),
    (SimSpeed::X2, "×2"),
    (SimSpeed::X3, "×3"),
];

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// "Mon 12:30, day 4" from the game time.
pub fn format_time(view: &SimView) -> String {
    let t = view.game_time;
    let minutes = u64::from(t.tick_of_day) * 24 * 60 / cp_core::time::TICKS_PER_DAY;
    format!(
        "{} {:02}:{:02}  ·  day {}",
        WEEKDAYS[t.day_of_week() as usize],
        minutes / 60,
        minutes % 60,
        t.day + 1
    )
}

/// Full-screen background `Ui` that panels are docked into. Create one per frame.
pub fn root_ui(ctx: &egui::Context) -> egui::Ui {
    egui::Ui::new(
        ctx.clone(),
        "cp_ui_root".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    )
}

/// Top bar: money and population (stubs), date/time, speed buttons. Returns the on-screen
/// rectangle of each speed button (tests use it to click them). A click pushes
/// `SetSpeed` into `outbox`.
pub fn top_bar(
    root: &mut egui::Ui,
    view: &SimView,
    outbox: &mut CommandOutbox,
) -> Vec<(SimSpeed, egui::Rect)> {
    let mut rects = Vec::new();
    egui::Panel::top("top_bar").show(root, |ui| {
        ui.horizontal(|ui| {
            ui.label("💰 $0"); // stub until the economy lands
            ui.separator();
            ui.label("👥 —"); // stub until citizens land
            ui.separator();
            ui.label(format_time(view));
            ui.separator();
            for (speed, label) in SPEEDS {
                let r = ui.selectable_label(view.speed == speed, label);
                rects.push((speed, r.rect));
                if r.clicked() && view.speed != speed {
                    outbox.push(set_speed(speed));
                }
            }
        });
    });
    rects
}

/// Bottom category bar. Clicking a tab selects it (click again to close); the panel
/// content is empty for now.
pub fn bottom_bar(root: &mut egui::Ui, state: &mut UiState) {
    egui::Panel::bottom("categories").show(root, |ui| {
        ui.horizontal(|ui| {
            for c in Category::ALL {
                let selected = state.category == Some(c);
                if ui.selectable_label(selected, c.label()).clicked() {
                    state.category = if selected { None } else { Some(c) };
                }
            }
        });
        if let Some(c) = state.category {
            ui.separator();
            ui.weak(format!("{} — nothing to build here yet", c.label()));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_sim::{SimCommand, SimRunner};

    fn input(t: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            time: Some(t),
            events,
            ..Default::default()
        }
    }

    /// Runs three frames: layout, press, release on `speed`'s button.
    fn click_speed(view: &SimView, speed: SimSpeed, outbox: &mut CommandOutbox) {
        let ctx = egui::Context::default();
        let mut rects = Vec::new();
        crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| {
            rects = top_bar(ui, view, &mut CommandOutbox::default());
        });
        let pos = rects.iter().find(|(s, _)| *s == speed).unwrap().1.center();
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for (t, events) in [
            (0.1, vec![egui::Event::PointerMoved(pos), button(true)]),
            (0.2, vec![button(false)]),
        ] {
            crate::testutil::frame(&ctx, input(t, events), |ui| {
                top_bar(ui, view, outbox);
            });
        }
    }

    #[test]
    fn clicking_a_speed_button_sends_set_speed_into_the_command_log() {
        let mut sim = SimRunner::new(1);
        let mut outbox = CommandOutbox::default();
        click_speed(&sim.view(), SimSpeed::X3, &mut outbox);
        assert_eq!(outbox.len(), 1);
        assert_eq!(
            outbox.iter().next(),
            Some(&SimCommand::SetSpeed {
                speed: SimSpeed::X3
            })
        );

        // The app's submit path: queue -> dispatcher -> log.
        outbox.flush_into(&mut sim);
        sim.tick_once();
        let log = sim.world().resource::<cp_core::CommandLog<SimCommand>>();
        assert_eq!(log.len(), 1);
        assert_eq!(
            log.entries()[0].command,
            SimCommand::SetSpeed {
                speed: SimSpeed::X3
            }
        );
        assert_eq!(sim.view().speed, SimSpeed::X3);

        // Pause, then resume from the paused state, both via buttons.
        click_speed(&sim.view(), SimSpeed::Pause, &mut outbox);
        outbox.flush_into(&mut sim);
        assert_eq!(sim.view().speed, SimSpeed::Pause);
        click_speed(&sim.view(), SimSpeed::X1, &mut outbox);
        outbox.flush_into(&mut sim);
        assert_eq!(sim.view().speed, SimSpeed::X1);
        assert_eq!(
            sim.world()
                .resource::<cp_core::CommandLog<SimCommand>>()
                .len(),
            3
        );
    }

    #[test]
    fn clicking_the_active_speed_sends_nothing() {
        let sim = SimRunner::new(1);
        let mut outbox = CommandOutbox::default();
        click_speed(&sim.view(), SimSpeed::X1, &mut outbox);
        assert!(outbox.is_empty());
    }

    #[test]
    fn time_is_formatted_from_game_time() {
        let mut sim = SimRunner::new(1);
        sim.world_mut().insert_resource(cp_core::Tick(
            cp_core::time::TICKS_PER_DAY * 8 + cp_core::time::TICKS_PER_DAY / 2,
        ));
        assert_eq!(format_time(&sim.view()), "Tue 12:00  ·  day 9");
    }

    #[test]
    fn bottom_bar_draws_with_no_category_selected() {
        let ctx = egui::Context::default();
        let mut state = UiState::default();
        crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| bottom_bar(ui, &mut state));
        assert_eq!(state.category, None);
    }
}
