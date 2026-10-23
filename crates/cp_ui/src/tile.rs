//! The one tile of every construction tool (T-097, `architecture/system-design.md` §13.7):
//! road types, zone tools, buildings, pipe, power line, remove, bulldozer, modes. A square
//! [`Sizes::TILE`] wide, [`Sizes::TILE_H`] high, with a square [`Sizes::TILE_ICON`] frame (one
//! background, a 1 px border, a 6 px corner) on top and the name in up to two centred rows below;
//! numbers and explanations go to the hover text the caller adds to the returned response.

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
            egui::Stroke::new(SELECTED_STROKE, fade(visuals.selection.stroke.color)),
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
    let galley = name_galley(ui, name, fade(visuals.text_color()));
    // A centred galley is laid out around x = 0.
    let pos = egui::pos2(rect.center().x, rect.top() + NAME_TOP);
    painter.galley(pos, galley, visuals.text_color());
}

/// The smallest size a tile name steps down to so that its longest word fits (T-284).
const MIN_NAME_PT: f32 = 10.0;
/// Row spacing of a tile name, in font sizes (T-284b: two rows fit under the icon).
const NAME_LINE: f32 = 1.15;
/// Width of the frame of a selected tile.
const SELECTED_STROKE: f32 = 2.0;
/// Where a tile's name starts below its top: the icon frame and half a pad.
const NAME_TOP: f32 = Sizes::TILE_PAD + Sizes::TILE_ICON + Sizes::TILE_PAD * 0.5;

/// The name of a tile: wrapped at words into at most two centred rows ("Highway, 3 lanes" keeps
/// its number); only a name longer than that is cut with "…". A word wider than the tile (long
/// Russian words: «Мусоросжигатель», T-284) would be broken inside it: the face steps down until
/// every word fits, to [`MIN_NAME_PT`] at most.
fn name_galley(ui: &egui::Ui, name: &str, color: egui::Color32) -> std::sync::Arc<egui::Galley> {
    let width = Sizes::TILE - 2.0 * Sizes::TILE_PAD;
    let mut font = egui::TextStyle::Small.resolve(ui.style());
    let widest = |font: &egui::FontId| {
        name.split_whitespace()
            .map(|w| {
                ui.fonts_mut(|f| f.layout_no_wrap(w.to_string(), font.clone(), color))
                    .size()
                    .x
            })
            .fold(0.0f32, f32::max)
    };
    while font.size > MIN_NAME_PT && widest(&font) > width {
        font.size = (font.size - 0.5).max(MIN_NAME_PT);
    }
    // Two rows end inside the tile, clear of the selection frame (T-284b): the rows sit closer
    // than the face's default spacing.
    let line = (font.size * NAME_LINE).round();
    let mut job = egui::text::LayoutJob::simple(name.to_string(), font, color, width);
    job.sections[0].format.line_height = Some(line);
    job.wrap.max_rows = 2;
    job.wrap.overflow_character = Some('…');
    job.halign = egui::Align::Center;
    ui.fonts_mut(|f| f.layout_job(job))
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
    fn a_tile_is_88_by_78_with_a_40_square_icon_frame_centred_below_a_4_pad() {
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

    /// T-274 follow-up: in the bold face the road names wrap into two rows and keep their
    /// variant number (it was cut to "Highway, …"); the rows fit the tile.
    #[test]
    fn a_long_name_wraps_into_two_rows_and_keeps_its_number() {
        let ctx = egui::Context::default();
        crate::style::install(&ctx);
        let names = [
            "Highway, 1 lane (one-way)",
            "Highway, 3 lanes (one-way)",
            "Collector (3 lanes, one-way)",
            "Collector (4 lanes)",
            "Arterial (6 lanes)",
            "Local street (one-way)",
        ];
        frame(&ctx, input(0.0, vec![]), |ui| {
            for name in names {
                let g = name_galley(ui, name, egui::Color32::WHITE);
                assert!(g.rows.len() <= 2, "{name}: {} rows", g.rows.len());
                assert!(
                    g.size().x <= Sizes::TILE - 2.0 * Sizes::TILE_PAD + 0.5,
                    "{name}"
                );
                let shown: String = g
                    .rows
                    .iter()
                    .flat_map(|r| r.glyphs.iter().map(|gl| gl.chr))
                    .collect();
                let digit = name.chars().find(char::is_ascii_digit);
                assert!(
                    digit.is_none_or(|d| shown.contains(d)),
                    "{name}: the tile shows {shown:?}"
                );
            }
        });
    }

    /// T-284: in English and Russian every name a tile shows (buildings, roads, the zone and tool cards)
    /// fits its two rows whole: nothing is cut with "…".
    #[test]
    fn every_tile_name_fits_its_tile_in_both_languages() {
        let db = cp_content::ContentDb::load_dir(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content"),
        )
        .unwrap();
        let mut names: Vec<String> = db.buildings().map(|(_, d)| d.name.clone()).collect();
        names.extend(db.roads().map(|(_, d)| d.name.clone()));
        names.extend(db.utilities().map(|(_, d)| d.name.clone()));
        {
            use cp_content::ZoneType as Z;
            use cp_tools::ZoneToolKind as K;
            for z in [
                Z::ResLow,
                Z::ResHigh,
                Z::ComLow,
                Z::ComHigh,
                Z::Industrial,
                Z::Office,
            ] {
                names.push(cp_tools::zone_name(z).to_string());
            }
            for k in [
                K::FillBlock,
                K::BrushSmall,
                K::BrushLarge,
                K::Rectangle,
                K::Eraser,
            ] {
                names.push(k.label().to_string());
            }
        }
        names.sort();
        names.dedup();
        let ctx = egui::Context::default();
        crate::style::install(&ctx);
        let mut cut = Vec::new();
        frame(&ctx, input(0.0, vec![]), |ui| {
            for (lang, name) in cp_locale::Lang::ALL
                .into_iter()
                .flat_map(|l| names.iter().map(move |n| (l, n)))
            {
                let text = cp_locale::locale(lang).get(name).to_string();
                let text = text.as_str();
                // T-284b: the rows end inside the tile, clear of a selected tile's frame.
                let room = Sizes::TILE_H - NAME_TOP - SELECTED_STROKE;
                let g = name_galley(ui, text, egui::Color32::WHITE);
                if g.size().y > room + 0.01 {
                    cut.push(format!(
                        "{lang:?} {name:?}: {} rows {:.1} px, room {room:.1}",
                        g.rows.len(),
                        g.size().y
                    ));
                }
                // Whole, in Russian (a long English name was cut before T-284 and keeps its
                // number, see above).
                if lang != cp_locale::Lang::Ru {
                    continue;
                }
                let shown: String = g
                    .rows
                    .iter()
                    .flat_map(|r| r.glyphs.iter().map(|gl| gl.chr))
                    .filter(|c| !c.is_whitespace())
                    .collect();
                let whole: String = text.chars().filter(|c| !c.is_whitespace()).collect();
                if shown != whole {
                    cut.push(format!("{name:?} -> {text:?} shows {shown:?}"));
                }
                // And no word is broken across the rows.
                let words: Vec<&str> = text.split_whitespace().collect();
                for row in &g.rows {
                    let row_text: String = row.glyphs.iter().map(|gl| gl.chr).collect();
                    if let Some(w) = row_text.split_whitespace().find(|w| !words.contains(w)) {
                        cut.push(format!("{name:?} -> {text:?}: {w:?} is a broken word"));
                    }
                }
            }
        });
        assert!(cut.is_empty(), "{} cut:\n{}", cut.len(), cut.join("\n"));
    }

    #[test]
    fn a_name_too_long_for_two_rows_is_cut_to_the_tile() {
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
