//! The question a road click asks before it takes down protected buildings (BUG-096): "Building
//! this road will demolish: Police station … Continue?". Yes sends the held commands (as
//! `NetDemolish`); No puts the road tool back as it was before the click and sends nothing.

use bevy_egui::egui;
use cp_locale::{tr, trf};
use cp_tools::{CommandOutbox, ToolState};

/// The question's text for `names`.
pub fn question(names: &[String]) -> String {
    let names: Vec<String> = names.iter().map(|n| tr(n)).collect();
    trf!(
        "Building this road will demolish: {names}. Continue?",
        names = names.join(", ")
    )
}

/// What the player answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
}

/// Applies `answer` to the held question of `tools`.
pub fn answer(tools: &mut ToolState, outbox: &mut CommandOutbox, answer: Answer) {
    let Some(ask) = tools.confirm.take() else {
        return;
    };
    match answer {
        Answer::Yes => {
            for c in ask.commands {
                outbox.push(c);
            }
        }
        Answer::No => {
            if let Some(road) = ask.restore {
                tools.road = road;
            }
        }
    }
}

/// The window, while a question is held.
pub fn confirm_window(ctx: &egui::Context, tools: &mut ToolState, outbox: &mut CommandOutbox) {
    let Some(ask) = tools.confirm.as_ref() else {
        return;
    };
    let text = question(&ask.names);
    let mut picked = None;
    egui::Window::new(tr("Demolish buildings?"))
        .id(egui::Id::new("demolish_confirm"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(text);
            ui.horizontal(|ui| {
                if ui.button(tr("Yes")).clicked() {
                    picked = Some(Answer::Yes);
                }
                if ui.button(tr("No")).clicked() {
                    picked = Some(Answer::No);
                }
            });
        });
    if let Some(a) = picked {
        answer(tools, outbox, a);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec2;
    use cp_sim::SimCommand;

    fn held(tools: &mut ToolState) {
        tools.confirm = Some(cp_tools::ConfirmDemolish {
            names: vec!["Police station".into()],
            commands: vec![SimCommand::SetSpeed {
                speed: cp_core::SimSpeed::X1,
            }],
            restore: Some(cp_tools::RoadTool {
                points: vec![Vec2::new(1.0, 2.0)],
                ..Default::default()
            }),
        });
    }

    #[test]
    fn yes_sends_the_held_commands_and_no_puts_the_tool_back() {
        let mut tools = ToolState::default();
        let mut outbox = CommandOutbox::default();
        held(&mut tools);
        assert!(question(&tools.confirm.as_ref().unwrap().names).contains("Police station"));
        answer(&mut tools, &mut outbox, Answer::No);
        assert!(tools.confirm.is_none());
        assert_eq!(outbox.iter().count(), 0);
        assert_eq!(tools.road.points, vec![Vec2::new(1.0, 2.0)]);
        held(&mut tools);
        answer(&mut tools, &mut outbox, Answer::Yes);
        assert!(tools.confirm.is_none());
        assert_eq!(outbox.iter().count(), 1);
    }
}
