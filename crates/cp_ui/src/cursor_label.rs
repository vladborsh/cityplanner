//! The length of the road being drawn in cells and its angle, next to the cursor (§6.8.3 item 9,
//! T-264): "5 cells · 90°". The text is the ghost's own (`cp_tools::Ghost::label`). With the
//! bulldozer, what a click would give back (BUG-097): "+$280.00".

use bevy_egui::egui;
use cp_tools::{ActiveTool, ToolState};

/// How far from the pointer the label stands, points.
const OFFSET: egui::Vec2 = egui::vec2(18.0, 20.0);

/// Draws the label of the road ghost beside the pointer; nothing when another tool is active,
/// there is no ghost or the pointer is over a panel. Not interactive.
pub fn cursor_label(ctx: &egui::Context, tools: &ToolState) {
    if tools.active == ActiveTool::Bulldozer {
        bulldozer_label(ctx, tools);
        return;
    }
    if tools.active != ActiveTool::Road || tools.hover.is_none() {
        return;
    }
    let (Some(text), Some(pos)) = (
        tools.ghost.as_ref().and_then(|g| g.label.as_deref()),
        ctx.pointer_latest_pos(),
    ) else {
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

/// The refund of the bulldozer's target beside the pointer, while there is one.
fn bulldozer_label(ctx: &egui::Context, tools: &ToolState) {
    let (Some(text), Some(pos)) = (refund_text(tools), ctx.pointer_latest_pos()) else {
        return;
    };
    egui::Area::new(egui::Id::new("bulldozer_cursor_label"))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos + OFFSET)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.colored_label(egui::Color32::LIGHT_GREEN, text);
            });
        });
}

/// `"+$280.00"`: what a bulldozer click would give back, if anything (BUG-097).
pub fn refund_text(tools: &ToolState) -> Option<String> {
    (tools.hover.is_some() && tools.bulldoze_refund.0 > 0)
        .then(|| format!("+{}", cp_tools::format_money(tools.bulldoze_refund)))
}
