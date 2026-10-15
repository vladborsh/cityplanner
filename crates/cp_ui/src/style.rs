//! Every size of the UI (T-097, `architecture/system-design.md` §13.7): icons, tiles, panel
//! widths and the text sizes. Panels never write a numeric icon width or height themselves.

use bevy_egui::egui;

/// The sizes of the UI, points.
pub struct Sizes;

impl Sizes {
    /// Width of the palette column.
    pub const PALETTE_W: f32 = 72.0;
    /// Icon of a palette category.
    pub const PALETTE_ICON: f32 = 52.0;
    /// Width of the tool panel beside the palette.
    pub const TOOL_PANEL_W: f32 = 300.0;
    /// The one tile of every construction tool: side of the square.
    pub const TILE: f32 = 88.0;
    /// The square icon frame inside a tile.
    pub const TILE_ICON: f32 = 64.0;
    /// Gap between the tile's top and its icon frame (and between its sides and it).
    pub const TILE_PAD: f32 = 4.0;
    /// Corner radius of the icon frame.
    pub const TILE_ROUNDING: f32 = 6.0;
    /// Gap between tiles of a grid.
    pub const TILE_GAP: f32 = 4.0;
    /// The bottom demand panel: height, collapsed height, zone icon, bar and column width.
    pub const DEMAND_H: f32 = 120.0;
    pub const DEMAND_COLLAPSED_H: f32 = 28.0;
    pub const DEMAND_ICON: f32 = 40.0;
    pub const DEMAND_BAR_H: f32 = 36.0;
    pub const DEMAND_COL_W: f32 = 64.0;
    /// Width of the middle group of the top bar (clock and speeds).
    pub const TOP_CENTER_W: f32 = 360.0;
    /// Icons of the view menu.
    pub const VIEW_MENU_ICON: f32 = 40.0;
    /// Height of the top bar.
    pub const TOP_BAR_H: f32 = 44.0;
    /// Icons of the top bar.
    pub const TOP_BAR_ICON: f32 = 30.0;
    /// Icon in a window header ([`crate::frame::window`]).
    pub const WINDOW_ICON: f32 = 24.0;
    /// Icon in the header of the tool panel.
    pub const PANEL_HEADER_ICON: f32 = 28.0;
    /// Icon in front of a section title (the services panel).
    pub const SECTION_ICON: f32 = 20.0;
    /// Icon in front of a text row (cards of the services panel).
    pub const ROW_ICON: f32 = 24.0;
    /// Padding between an icon button's frame and its icon.
    pub const BUTTON_PAD: f32 = 2.0;
    /// Text sizes, points.
    pub const BODY_PT: f32 = 15.0;
    pub const SMALL_PT: f32 = 13.0;

    pub const fn tile() -> egui::Vec2 {
        egui::vec2(Self::TILE, Self::TILE)
    }

    pub const fn tile_icon() -> egui::Vec2 {
        egui::vec2(Self::TILE_ICON, Self::TILE_ICON)
    }
}

/// Sets the text sizes of the egui style: `Body` 15 pt, `Small` 13 pt (and the headings and
/// buttons that follow the body size). Called once at start.
pub fn install(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        for (text_style, size) in [
            (egui::TextStyle::Body, Sizes::BODY_PT),
            (egui::TextStyle::Button, Sizes::BODY_PT),
            (egui::TextStyle::Small, Sizes::SMALL_PT),
        ] {
            style
                .text_styles
                .insert(text_style, egui::FontId::proportional(size));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_sets_body_15_and_small_13() {
        let ctx = egui::Context::default();
        install(&ctx);
        let style = ctx.global_style();
        assert_eq!(style.text_styles[&egui::TextStyle::Body].size, 15.0);
        assert_eq!(style.text_styles[&egui::TextStyle::Small].size, 13.0);
    }
}
