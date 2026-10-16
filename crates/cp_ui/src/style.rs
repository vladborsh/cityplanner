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

/// Symbol fonts (OFL, `assets/LICENSES.md`) for the glyphs egui's own fonts lack or draw as a
/// square (⚠ ✔ ✖ ▸ ▾ ∥ ⊥ 💰 ☻, T-184). They hold no Latin letters, so they go first in the
/// family: egui treats every glyph of the face that holds its replacement character (◻) as
/// missing, and the default emoji face is that one (so ◻ is left out of the subsets).
const SYMBOL_FONTS: [(&str, &[u8]); 2] = [
    (
        "noto_symbols2",
        include_bytes!("../../../assets/fonts/NotoSansSymbols2-subset.ttf"),
    ),
    (
        "noto_math",
        include_bytes!("../../../assets/fonts/NotoSansMath-subset.ttf"),
    ),
];

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in SYMBOL_FONTS {
        fonts.font_data.insert(
            name.to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(bytes)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, name.to_owned());
        }
    }
    ctx.set_fonts(fonts);
}

/// Sets the text sizes of the egui style: `Body` 15 pt, `Small` 13 pt (and the headings and
/// buttons that follow the body size). Called once at start.
pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
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
