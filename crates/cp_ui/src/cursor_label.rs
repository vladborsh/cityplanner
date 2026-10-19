//! The length of the road being drawn in cells and its angle, next to the cursor (§6.8.3 item 9,
//! T-264): "5 cells · 90°". The text is the ghost's own (`cp_tools::Ghost::label`).

use bevy_egui::egui;
use cp_tools::{ActiveTool, ToolState};

/// How far from the pointer the label stands, points.
const OFFSET: egui::Vec2 = egui::vec2(18.0, 20.0);

/// Draws the label of the road ghost beside the pointer; nothing when another tool is active,
/// there is no ghost or the pointer is over a panel. Not interactive.
pub fn cursor_label(ctx: &egui::Context, tools: &ToolState) {
    if tools.active != ActiveTool::Road || tools.hover.is_none() {
        return;
    }
    // Without a pointer (a held ghost of a screenshot scene) the label stands at the top left.
    let pos = ctx.pointer_latest_pos().unwrap_or(egui::pos2(24.0, 24.0));
    let Some(text) = tools.ghost.as_ref().and_then(|g| g.label.as_deref()) else {
        return;
    };
    let valid = tools.ghost.as_ref().is_some_and(|g| g.valid);
    egui::Area::new(egui::Id::new("road_cursor_label"))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos + OFFSET)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                let color = if valid {
                    egui::Color32::LIGHT_GREEN
                } else {
                    egui::Color32::LIGHT_RED
                };
                ui.colored_label(color, text);
            });
        });
}
