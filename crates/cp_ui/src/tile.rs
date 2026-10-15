//! The one tile of every construction tool (T-097, `architecture/system-design.md` §13.7):
//! road types, zone tools, buildings, pipe, power line, remove, bulldozer, modes. A square
//! [`Sizes::TILE`] with a square [`Sizes::TILE_ICON`] frame (one background, a 1 px border, a
//! 6 px corner) on top and the name in one line below; numbers and explanations go to the
//! hover text the caller adds to the returned response.

use bevy_egui::egui;

use crate::icons::Icon;
use crate::style::Sizes;

/// What a tile shows in its icon frame.
pub enum TileIcon<'a> {
    /// A UI icon.
    Icon(Icon),
    /// The icon named by a content def (`BuildingDef.icon`, `UtilityDef.icon`).
    Def(&'a str),
    /// A procedural drawing (road types): it paints into the square, on the shared frame.
    Draw(&'a dyn Fn(&egui::Painter, egui::Rect)),
}

/// A tile that is always enabled.
pub fn tool_tile(ui: &mut egui::Ui, icon: TileIcon, name: &str, selected: bool) -> egui::Response {
    tool_tile_enabled(ui, icon, name, selected, true)
}

/// A tile; a disabled one (no money, wrong site) is drawn at half strength and does not click.
pub fn tool_tile_enabled(
    ui: &mut egui::Ui,
    icon: TileIcon,
    name: &str,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Sizes::tile(), sense);
    if ui.is_rect_visible(rect) {
        paint(ui, rect, &icon, name, selected, enabled, &response);
    }
    #[cfg(test)]
    record(ui.ctx(), rect, icon_rect(rect));
    response
}

/// The icon frame of a tile at `tile`: centred, `TILE_PAD` below the top.
fn icon_rect(tile: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(
            tile.center().x - Sizes::TILE_ICON * 0.5,
            tile.top() + Sizes::TILE_PAD,
        ),
        Sizes::tile_icon(),
    )
}

fn paint(
    ui: &egui::Ui,
    rect: egui::Rect,
    icon: &TileIcon,
    name: &str,
    selected: bool,
    enabled: bool,
    response: &egui::Response,
) {
    let visuals = ui.visuals();
    let alpha = if enabled { 1.0 } else { 0.5 };
    let fade = |c: egui::Color32| c.gamma_multiply(alpha);
    let painter = ui.painter();
    let fill = if selected {
        visuals.selection.bg_fill.gamma_multiply(0.5)
    } else if response.hovered() && enabled {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    painter.rect_filled(rect, Sizes::TILE_ROUNDING, fade(fill));
    if selected {
        painter.rect_stroke(
            rect,
            Sizes::TILE_ROUNDING,
            egui::Stroke::new(2.0, fade(visuals.selection.stroke.color)),
            egui::StrokeKind::Inside,
        );
    }
    // The frame every icon sits in.
    let frame = icon_rect(rect);
    painter.rect_filled(frame, Sizes::TILE_ROUNDING, fade(visuals.extreme_bg_color));
    match icon {
        TileIcon::Icon(i) => paint_image(ui, frame, i.image(), alpha),
        TileIcon::Def(def) => paint_image(ui, frame, crate::icons::def_icon(def), alpha),
        TileIcon::Draw(draw) => draw(&painter.with_clip_rect(frame), frame),
    }
    painter.rect_stroke(
        frame,
        Sizes::TILE_ROUNDING,
        egui::Stroke::new(1.0, fade(visuals.widgets.noninteractive.bg_stroke.color)),
        egui::StrokeKind::Inside,
    );
    // The name: one line, cut with "…".
    let mut job = egui::text::LayoutJob::simple_singleline(
        name.to_string(),
        egui::TextStyle::Small.resolve(ui.style()),
        fade(visuals.text_color()),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: rect.width() - 2.0 * Sizes::TILE_PAD,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let pos = egui::pos2(
        rect.center().x - galley.size().x * 0.5,
        frame.bottom() + Sizes::TILE_PAD * 0.5,
    );
    painter.galley(pos, galley, visuals.text_color());
}

fn paint_image(ui: &egui::Ui, frame: egui::Rect, source: egui::ImageSource<'_>, alpha: f32) {
    let inner = frame.shrink(Sizes::TILE_PAD);
    egui::Image::new(source)
        .tint(egui::Color32::WHITE.gamma_multiply(alpha))
        .paint_at(ui, inner);
}

/// The rectangles of the tiles drawn since the last [`take_rects`] (tests only): the tile and
/// its icon frame.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub struct TileRects(pub Vec<(egui::Rect, egui::Rect)>);

#[cfg(test)]
fn rects_id() -> egui::Id {
    egui::Id::new("cp_ui_tile_rects")
}

#[cfg(test)]
fn record(ctx: &egui::Context, tile: egui::Rect, icon: egui::Rect) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<TileRects>(rects_id())
            .0
            .push((tile, icon));
    });
}

/// Empties and returns the recorded tile rectangles.
#[cfg(test)]
pub fn take_rects(ctx: &egui::Context) -> TileRects {
    ctx.data_mut(|d| d.remove_temp::<TileRects>(rects_id()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{frame, input};

    #[test]
    fn a_tile_is_88_square_with_a_64_square_icon_frame_centred_below_a_4_pad() {
        let ctx = egui::Context::default();
        take_rects(&ctx);
        frame(&ctx, input(0.0, vec![]), |ui| {
            tool_tile(
                ui,
                TileIcon::Icon(Icon::Road),
                "A very long tool name here",
                false,
            );
            tool_tile(ui, TileIcon::Draw(&|_, _| {}), "Short", true);
        });
        let rects = take_rects(&ctx).0;
        assert_eq!(rects.len(), 2);
        for (tile, icon) in rects {
            assert_eq!(tile.size(), Sizes::tile());
            assert_eq!(icon.size(), Sizes::tile_icon());
            assert_eq!(icon.top() - tile.top(), Sizes::TILE_PAD);
            assert!((icon.center().x - tile.center().x).abs() < 0.01);
        }
    }

    #[test]
    fn the_name_is_one_line_cut_to_the_tile() {
        let ctx = egui::Context::default();
        let texts = crate::testutil::frame_texts(&ctx, input(0.0, vec![]), |ui| {
            tool_tile(
                ui,
                TileIcon::Icon(Icon::Road),
                "An extremely long name that cannot fit",
                false,
            );
        });
        // (The icon cannot load in a test and paints a placeholder text.)
        let names = texts
            .iter()
            .filter(|(t, _)| t.starts_with("An ext"))
            .count();
        assert_eq!(names, 1);
    }

    #[test]
    fn a_disabled_tile_does_not_click() {
        let ctx = egui::Context::default();
        let mut clicked = false;
        let pos = egui::pos2(40.0, 40.0);
        let [down, up] = crate::testutil::click(pos);
        for (t, events) in [(0.0, vec![]), (0.1, down), (0.2, up)] {
            frame(&ctx, input(t, events), |ui| {
                clicked |= tool_tile_enabled(ui, TileIcon::Icon(Icon::Road), "Off", false, false)
                    .clicked();
            });
        }
        assert!(!clicked);
    }
}
