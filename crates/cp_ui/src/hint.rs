//! The tool hint (T-317): one short line next to the cursor — the verdict of the tool in hand,
//! what it would cost, or why it cannot go here — instead of a paragraph at the bottom of a
//! panel.

use bevy_egui::egui;
use cp_tools::{ActiveTool, ToolState};

use crate::palette::Category;
use crate::panels::{UiState, verdict};

/// How far from the pointer the hint stands, points (below the road's own length label).
const OFFSET: egui::Vec2 = egui::vec2(18.0, 52.0);

/// The category whose verdict texts describe `tool`, for a tool used with its tray closed.
fn category_of(tool: ActiveTool) -> Category {
    match tool {
        ActiveTool::Zone => Category::Zones,
        ActiveTool::Place | ActiveTool::Utility => Category::Buildings,
        ActiveTool::Line => Category::Transit,
        ActiveTool::District => Category::Districts,
        ActiveTool::Area => Category::Industry,
        _ => Category::Roads,
    }
}

/// Draws the hint beside the pointer while a tool is in hand and the pointer is over the map.
pub fn cursor_hint(ctx: &egui::Context, state: &UiState, tools: &ToolState) {
    if tools.active == ActiveTool::None {
        return;
    }
    let Some(pos) = ctx
        .pointer_latest_pos()
        .or(state.demo_pointer.map(|(x, y)| egui::pos2(x, y)))
    else {
        return;
    };
    if ctx.is_pointer_over_egui() || crate::pointer_over_panels(ctx) {
        return;
    }
    let category = state.category.unwrap_or_else(|| category_of(tools.active));
    egui::Area::new(egui::Id::new("tool_hint"))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos + OFFSET)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(360.0);
                ui.vertical(|ui| verdict(ui, category, tools));
            });
        });
}
