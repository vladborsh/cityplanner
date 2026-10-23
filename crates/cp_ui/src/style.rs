//! Every size of the UI (T-097, `architecture/system-design.md` §13.7): icons, tiles, panel
//! widths and the text sizes. Panels never write a numeric icon width or height themselves.

use bevy_egui::egui;

/// The sizes of the UI, points.
pub struct Sizes;

impl Sizes {
    /// Icon of a category in the bottom bar (T-317).
    pub const PALETTE_ICON: f32 = 44.0;
    /// Height of the tool tray above the bottom bar: the tab row, one row of tiles (with the
    /// scroll bar) and the options strip.
    pub const TRAY_H: f32 = 196.0;
    /// Height of the tab row of the tray.
    pub const TRAY_TAB_H: f32 = 26.0;
    /// The one tile of every construction tool: its width...
    pub const TILE: f32 = 88.0;
    /// ...and its height: the icon frame and two rows of the name below it (T-274 follow-up:
    /// the bold names wrap instead of losing their variant number to "…").
    pub const TILE_H: f32 = 104.0;
    /// The square icon frame inside a tile.
    pub const TILE_ICON: f32 = 64.0;
    /// Gap between the tile's top and its icon frame (and between its sides and it).
    pub const TILE_PAD: f32 = 4.0;
    /// Corner radius of the icon frame.
    pub const TILE_ROUNDING: f32 = 6.0;
    /// Gap between tiles of a grid.
    pub const TILE_GAP: f32 = 4.0;
    /// The slim bottom bar (T-317): height, and of its demand block the zone icon, bar and
    /// column width.
    pub const DEMAND_H: f32 = 64.0;
    pub const DEMAND_ICON: f32 = 20.0;
    pub const DEMAND_BAR_H: f32 = 36.0;
    pub const DEMAND_COL_W: f32 = 44.0;
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
        egui::vec2(Self::TILE, Self::TILE_H)
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

/// The text face of the menus and panels (T-274, the user: "all the font bold"): Noto Sans Bold
/// (OFL, `assets/LICENSES.md`), Latin, Cyrillic and punctuation. It comes right after the symbol
/// faces, before egui's own Ubuntu Light (kept as the fallback for anything the subset lacks).
const TEXT_FONT: (&str, &[u8]) = (
    "noto_sans_bold",
    include_bytes!("../../../assets/fonts/NotoSans-Bold-subset.ttf"),
);

fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
}

/// egui's fonts with the symbol faces first, then the text face, then egui's own.
fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let (name, bytes) = TEXT_FONT;
    fonts.font_data.insert(
        name.to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(bytes)),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, name.to_owned());
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
    fonts
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

    /// T-274: the menus and panels are set in the bold text face; the symbol faces still come
    /// first and egui's own face stays as the fallback.
    #[test]
    fn the_text_face_is_bold_after_the_symbols() {
        let fonts = font_definitions();
        let family = &fonts.families[&egui::FontFamily::Proportional];
        let at = |name: &str| family.iter().position(|f| f == name).expect(name);
        assert!(at("noto_symbols2") < at("noto_sans_bold"));
        assert!(at("noto_sans_bold") < at("Ubuntu-Light"));
        assert!(
            !fonts.families[&egui::FontFamily::Monospace].contains(&"noto_sans_bold".to_owned()),
            "the numbers' monospace face stays"
        );
    }

    /// The code points of a TrueType font's `cmap` (formats 4 and 12).
    fn cmap(font: &[u8]) -> std::collections::HashSet<u32> {
        let u16_at = |o: usize| u16::from_be_bytes([font[o], font[o + 1]]) as usize;
        let u32_at =
            |o: usize| u32::from_be_bytes([font[o], font[o + 1], font[o + 2], font[o + 3]]);
        let tables = (0..u16_at(4))
            .map(|i| 12 + 16 * i)
            .find(|&r| &font[r..r + 4] == b"cmap")
            .expect("a cmap table");
        let cmap = u32_at(tables + 8) as usize;
        let mut out = std::collections::HashSet::new();
        for i in 0..u16_at(cmap + 2) {
            let sub = cmap + u32_at(cmap + 4 + 8 * i + 4) as usize;
            match u16_at(sub) {
                4 => {
                    let seg = u16_at(sub + 6) / 2;
                    for k in 0..seg {
                        let end = u16_at(sub + 14 + 2 * k) as u32;
                        let start = u16_at(sub + 16 + 2 * seg + 2 * k) as u32;
                        if start != 0xFFFF {
                            out.extend(start..=end);
                        }
                    }
                }
                12 => {
                    for k in 0..u32_at(sub + 12) as usize {
                        let g = sub + 16 + 12 * k;
                        out.extend(u32_at(g)..=u32_at(g + 4));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// T-284: our faces draw every character of the Russian texts (no fallback to egui's face,
    /// no tofu): the letters the bold text face, the symbols (`−`, `▸`) the symbol faces before it.
    #[test]
    fn our_faces_have_every_character_of_the_russian_texts() {
        let mut have = cmap(include_bytes!(
            "../../../assets/fonts/NotoSans-Bold-subset.ttf"
        ));
        let letters = have.clone();
        have.extend(cmap(include_bytes!(
            "../../../assets/fonts/NotoSansSymbols2-subset.ttf"
        )));
        have.extend(cmap(include_bytes!(
            "../../../assets/fonts/NotoSansMath-subset.ttf"
        )));
        for c in '\u{410}'..='\u{44F}' {
            assert!(letters.contains(&(c as u32)), "{c} not in the bold face");
        }
        // The Russian thousands separator comes from code (T-284b: "1 500").
        cp_locale::set_lang(cp_locale::Lang::Ru);
        let sep = cp_locale::thousands_separator();
        cp_locale::set_lang(cp_locale::Lang::En);
        assert!(
            letters.contains(&(sep as u32)),
            "{sep:?} not in the bold face"
        );
        let mut missing: Vec<char> = cp_locale::Lang::Ru
            .source()
            .chars()
            .filter(|c| !c.is_whitespace() && !have.contains(&(*c as u32)))
            .collect();
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "not in our faces: {missing:?}");
    }

    #[test]
    fn install_sets_body_15_and_small_13() {
        let ctx = egui::Context::default();
        install(&ctx);
        let style = ctx.global_style();
        assert_eq!(style.text_styles[&egui::TextStyle::Body].size, 15.0);
        assert_eq!(style.text_styles[&egui::TextStyle::Small].size, 13.0);
    }
}
