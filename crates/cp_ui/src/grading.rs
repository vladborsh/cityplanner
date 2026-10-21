//! F8 colour grading panel (T-273, the user's "filters"): a slider and a number per value of
//! [`GradingSettings`], applied live, bottom right. "Copy values" logs the line and puts it on
//! the clipboard so the player can tell us the numbers; "Reset" goes back to the neutral
//! picture. A tuning panel: the defaults stay as they are until the player picks.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use cp_locale::tr;
use cp_render::GradingSettings;

use crate::UiState;

/// The panel's content: the sliders of `g`. Returns the line to copy when the button was
/// pressed.
pub fn grading_panel(
    ctx: &egui::Context,
    g: &mut GradingSettings,
    open: &mut bool,
) -> Option<String> {
    let mut copied = None;
    let title = tr("Colour grading (F8)");
    crate::frame::window("grading", crate::icons::Icon::Debug, &title, open)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -48.0))
        .show(ctx, |ui| {
            egui::Grid::new("grading_grid")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    for (name, value, range) in g.fields() {
                        ui.label(name);
                        ui.add(
                            egui::Slider::new(value, range)
                                .step_by(0.005)
                                // The step snapping of a range from -0.2 leaves -3e-9 for 0: shown
                                // as 0.000, not "-0.000".
                                .custom_formatter(|v, _| {
                                    format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v })
                                }),
                        );
                        ui.end_row();
                    }
                });
            ui.horizontal(|ui| {
                if ui.button(tr("Copy values")).clicked() {
                    copied = Some(g.to_line());
                }
                if ui.button(tr("Reset")).clicked() {
                    *g = GradingSettings::NEUTRAL;
                }
            });
        });
    copied
}

/// Draws the panel when open; a copied line goes to the log and the clipboard.
pub fn grading_ui(
    mut contexts: EguiContexts,
    mut state: ResMut<UiState>,
    mut grading: ResMut<GradingSettings>,
) -> Result {
    if !state.grading_open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    // Edit a copy: the resource changes (and the camera is re-graded) only when a value moved.
    let mut g = *grading;
    let line = grading_panel(ctx, &mut g, &mut state.grading_open);
    // A slider's step snapping leaves float dust (-3e-9 for 0): keep 4 decimals, so an
    // untouched value stays exactly what it was.
    for (_, v, _) in g.fields() {
        *v = (*v * 10_000.0).round() / 10_000.0 + 0.0;
    }
    if g != *grading {
        *grading = g;
    }
    if let Some(line) = line {
        info!("colour grading: {line}");
        ctx.copy_text(line);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{frame_texts, input};

    /// The panel shows every value with its number.
    #[test]
    fn the_panel_shows_each_value_and_its_number() {
        let ctx = egui::Context::default();
        let mut g = GradingSettings {
            contrast: 1.125,
            ..GradingSettings::NEUTRAL
        };
        let mut open = true;
        // A window lays itself out in its first frame and paints in the second.
        frame_texts(&ctx, input(0.0, Vec::new()), |ui| {
            grading_panel(ui.ctx(), &mut g, &mut open);
        });
        let texts: Vec<String> = frame_texts(&ctx, input(0.1, Vec::new()), |ui| {
            grading_panel(ui.ctx(), &mut g, &mut open);
        })
        .into_iter()
        .map(|(t, _)| t)
        .collect();
        for name in [
            "exposure",
            "contrast",
            "temperature",
            "vignette",
            "Copy values",
        ] {
            assert!(texts.iter().any(|t| t == name), "{name} in {texts:?}");
        }
        assert!(texts.iter().any(|t| t == "1.125"), "the number: {texts:?}");
    }
}
