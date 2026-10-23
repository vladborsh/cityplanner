//! The legend of the banded Noise and Pollution overlays (BUG-119): a small box at the top right,
//! left of the overlays panel, with a swatch per band and its range on the inspector's 0-100
//! scale, plus a line that what lies below the first band stays uncoloured.

use bevy_egui::egui;
use cp_locale::{tr, trf};
use cp_render::Overlay;
use cp_render::overlay::bands::{self, BANDS, Bands};

use crate::style::Sizes;

/// Side of a band swatch.
const SWATCH: f32 = 12.0;

/// The bands an overlay is drawn with, if it is banded.
pub fn bands_of(overlay: Overlay) -> Option<&'static Bands> {
    match overlay {
        Overlay::Noise => Some(&bands::NOISE),
        Overlay::Pollution => Some(&bands::POLLUTION),
        _ => None,
    }
}

/// The band ranges on the 0-100 scale of the inspector, quietest first ("15-30" ... "60+").
pub fn labels(b: &Bands) -> [String; BANDS] {
    let pct = |v: f32| (v * 100.0).round() as u32;
    std::array::from_fn(|i| match b.edges.get(i + 1) {
        Some(next) => format!("{}\u{2013}{}", pct(b.edges[i]), pct(*next)),
        None => format!("{}+", pct(b.edges[i])),
    })
}

fn color32(c: [f32; 4]) -> egui::Color32 {
    let u = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    // The swatch shows the band at full strength: on the panel its alpha would only wash it out.
    egui::Color32::from_rgb(u(c[0]), u(c[1]), u(c[2]))
}

/// Draws the legend of `overlay` (nothing for an overlay without bands); `inset` is the width
/// of the overlays panel it stands beside. Returns where it was drawn.
pub fn band_legend(ctx: &egui::Context, overlay: Overlay, inset: f32) -> Option<egui::Rect> {
    let b = bands_of(overlay)?;
    let area = egui::Area::new(egui::Id::new("band_legend"))
        .anchor(
            egui::Align2::RIGHT_TOP,
            egui::vec2(-(inset + 4.0), Sizes::TOP_BAR_H + 8.0),
        )
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(egui::RichText::new(tr(overlay.label())).strong());
                for (i, word) in labels(b).iter().enumerate().rev() {
                    ui.horizontal(|ui| {
                        let (r, _) = ui
                            .allocate_exact_size(egui::vec2(SWATCH, SWATCH), egui::Sense::hover());
                        ui.painter().rect_filled(r, 2.0, color32(b.colors[i]));
                        ui.painter().rect_stroke(
                            r,
                            2.0,
                            egui::Stroke::new(1.5, color32(b.line_color(i))),
                            egui::StrokeKind::Inside,
                        );
                        ui.label(word);
                    });
                }
                ui.small(trf!(
                    "Below {n}: no colour",
                    n = (b.edges[0] * 100.0).round() as u32
                ));
            });
        });
    Some(area.response.rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_and_pollution_have_a_legend_of_four_bands_and_others_none() {
        assert_eq!(bands_of(Overlay::Noise), Some(&bands::NOISE));
        assert_eq!(bands_of(Overlay::Pollution), Some(&bands::POLLUTION));
        assert_eq!(bands_of(Overlay::Off), None);
        assert_eq!(bands_of(Overlay::LandValue), None);
        let ctx = egui::Context::default();
        let mut shown = None;
        crate::testutil::frame(&ctx, crate::testutil::input(0.0, vec![]), |ui| {
            shown = band_legend(ui.ctx(), Overlay::Noise, 60.0);
            assert!(band_legend(ui.ctx(), Overlay::Traffic, 60.0).is_none());
        });
        assert!(
            shown.is_some_and(|r| r.height() > 4.0 * SWATCH),
            "{shown:?}"
        );
        assert_eq!(
            labels(&bands::NOISE),
            ["15\u{2013}30", "30\u{2013}45", "45\u{2013}60", "60+"]
        );
    }
}
