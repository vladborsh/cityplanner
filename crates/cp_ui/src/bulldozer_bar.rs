//! The black-and-orange striped bar across the bottom of the screen while the bulldozer is the
//! active tool (T-175).

use bevy_egui::egui;
use cp_locale::tr;
use cp_tools::{ActiveTool, ToolState};

pub const BAR_H: f32 = 24.0;
const STRIPE_W: f32 = 18.0;

/// Draws the bar over everything else at the bottom edge; the rect it covers, or `None` when
/// the bulldozer is not active. Not interactive: clicks reach whatever lies beneath.
pub fn bulldozer_bar(ctx: &egui::Context, tools: &ToolState) -> Option<egui::Rect> {
    if tools.active != ActiveTool::Bulldozer {
        return None;
    }
    let screen = ctx.viewport_rect();
    let rect = egui::Rect::from_min_max(egui::pos2(screen.min.x, screen.max.y - BAR_H), screen.max);
    egui::Area::new(egui::Id::new("bulldozer_bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .interactable(false)
        .show(ctx, |ui| {
            let painter = ui.painter();
            paint_stripes(painter, rect);
            let font = egui::FontId::proportional(14.0);
            let galley =
                painter.layout_no_wrap(tr("Bulldozer — Esc to exit"), font, egui::Color32::BLACK);
            let size = galley.size() + egui::vec2(24.0, 4.0);
            let plate = egui::Rect::from_center_size(rect.center(), size);
            painter.rect_filled(plate, 3.0, egui::Color32::from_rgb(255, 160, 0));
            painter.galley(
                plate.center() - galley.size() / 2.0,
                galley,
                egui::Color32::BLACK,
            );
        });
    Some(rect)
}

/// Diagonal black / orange stripes clipped to `rect`.
fn paint_stripes(painter: &egui::Painter, rect: egui::Rect) {
    let orange = egui::Color32::from_rgb(255, 160, 0);
    painter.rect_filled(rect, 0.0, egui::Color32::BLACK);
    let painter = painter.with_clip_rect(rect);
    let slant = rect.height();
    let mut x = rect.min.x - slant;
    while x < rect.max.x {
        let quad = vec![
            egui::pos2(x, rect.max.y),
            egui::pos2(x + STRIPE_W, rect.max.y),
            egui::pos2(x + STRIPE_W + slant, rect.min.y),
            egui::pos2(x + slant, rect.min.y),
        ];
        painter.add(egui::Shape::convex_polygon(
            quad,
            orange,
            egui::Stroke::NONE,
        ));
        x += STRIPE_W * 2.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(tools: &ToolState) -> Option<egui::Rect> {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };
        let mut out = None;
        crate::testutil::frame(&ctx, input, |_| out = bulldozer_bar(&ctx, tools));
        out
    }

    #[test]
    fn the_bar_shows_only_while_the_bulldozer_is_active() {
        let mut tools = ToolState::default();
        assert_eq!(frame(&tools), None);
        tools.select(ActiveTool::Bulldozer);
        let r = frame(&tools).expect("bar");
        assert_eq!((r.min.x, r.max.x, r.max.y), (0.0, 1280.0, 720.0));
        assert_eq!(r.height(), BAR_H);
        tools.select(ActiveTool::Road);
        assert_eq!(frame(&tools), None);
    }
}
