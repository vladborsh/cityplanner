//! The legend of the active overlay (T-317): a few colour swatches with their meaning at the
//! bottom right of the map. Only overlays whose colours are one ramp or a fixed palette have
//! one; the colours are the ones the overlay layers paint with.

use bevy_egui::egui;
use cp_content::{ParkKind, ResourceKind, ZoneType};
use cp_locale::tr;
use cp_render::{Overlay, park_color, resource_color};

use crate::panels::UiState;
use crate::style::Sizes;

/// One swatch and what it means.
pub type Row = (egui::Color32, String);

fn rgba(c: [f32; 4]) -> egui::Color32 {
    egui::Color32::from_rgb(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
    )
}

fn rgb(c: [f32; 3]) -> egui::Color32 {
    rgba([c[0], c[1], c[2], 1.0])
}

/// A ramp as three swatches: low, middle, high.
fn ramp(f: fn(f32) -> [f32; 4], names: [&str; 3]) -> Vec<Row> {
    [0.0, 0.5, 1.0]
        .into_iter()
        .zip(names)
        .map(|(v, n)| (rgba(f(v)), tr(n)))
        .collect()
}

/// The rows of `overlay`'s legend; empty for an overlay without one.
pub fn rows(overlay: Overlay) -> Vec<Row> {
    match overlay {
        Overlay::Traffic => ramp(cp_render::traffic_color, ["Free", "Busy", "Jammed"]),
        Overlay::LandValue => ramp(
            cp_render::land_value_color,
            ["Cheap", "Average", "Expensive"],
        ),
        // Noise and pollution are banded: `band_legend` draws their legend.
        Overlay::Garbage
        | Overlay::Health
        | Overlay::Deathcare
        | Overlay::Fire
        | Overlay::Police
        | Overlay::Education
        | Overlay::Mail => ramp(cp_render::coverage_color, ["Poor", "Fair", "Full"]),
        Overlay::Zones => [
            ZoneType::ResLow,
            ZoneType::ComLow,
            ZoneType::Industrial,
            ZoneType::Office,
        ]
        .into_iter()
        .filter_map(|z| cp_render::zone_color(z).map(|c| (rgba(c), tr(cp_tools::zone_name(z)))))
        .collect(),
        Overlay::Resources | Overlay::IndustryAreas => ResourceKind::ALL
            .into_iter()
            .map(|k| {
                (
                    rgb(resource_color(k)),
                    tr(crate::industry::resource_name(k)),
                )
            })
            .collect(),
        Overlay::Parks => ParkKind::ALL
            .into_iter()
            .map(|k| (rgb(park_color(k)), tr(crate::parks::kind_name(k))))
            .collect(),
        _ => Vec::new(),
    }
}

/// Draws the legend at the bottom right of the map, above the bottom bar and the open tray.
pub fn legend(ctx: &egui::Context, state: &UiState, overlay: Overlay) -> Option<egui::Rect> {
    if crate::band_legend::bands_of(overlay).is_some() {
        return crate::band_legend::band_legend(ctx, overlay, crate::overlays_panel::width(state));
    }
    let rows = rows(overlay);
    if rows.is_empty() {
        return None;
    }
    let lift = Sizes::DEMAND_H
        + if state.category.is_some() {
            Sizes::TRAY_H
        } else {
            0.0
        };
    let out = egui::Area::new(egui::Id::new("overlay_legend"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-8.0, -(lift + 8.0)))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.strong(tr(overlay.label()));
                for (color, name) in &rows {
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(16.0, 12.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, *color);
                        ui.label(egui::RichText::new(name).small());
                    });
                }
            });
        });
    Some(out.response.rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlays_with_a_palette_have_a_legend_and_the_others_none() {
        for o in Overlay::ALL {
            let n = rows(o).len();
            match o {
                Overlay::Traffic
                | Overlay::LandValue
                | Overlay::Garbage
                | Overlay::Health
                | Overlay::Deathcare
                | Overlay::Fire
                | Overlay::Police
                | Overlay::Education
                | Overlay::Mail => assert_eq!(n, 3, "{o:?}"),
                Overlay::Pollution | Overlay::Noise => assert_eq!(n, 0, "{o:?}: band legend"),
                Overlay::Zones => assert_eq!(n, 4),
                Overlay::Resources | Overlay::IndustryAreas => {
                    assert_eq!(n, ResourceKind::ALL.len(), "{o:?}")
                }
                Overlay::Parks => assert_eq!(n, ParkKind::ALL.len()),
                _ => assert_eq!(n, 0, "{o:?}"),
            }
        }
    }

    #[test]
    fn the_legend_is_drawn_bottom_right_above_the_bar() {
        let ctx = egui::Context::default();
        let state = UiState::default();
        let mut at = None;
        for t in [0.0, 0.1, 0.2] {
            crate::testutil::frame(&ctx, crate::testutil::input(t, vec![]), |ui| {
                at = legend(ui.ctx(), &state, Overlay::Traffic);
            });
        }
        let r = at.expect("a legend for traffic");
        assert!(r.right() <= 1280.0 && r.bottom() <= 720.0 - Sizes::DEMAND_H);
        assert!(r.left() > 640.0, "{r:?}");
        let mut none = Some(r);
        crate::testutil::frame(&ctx, crate::testutil::input(0.3, vec![]), |ui| {
            none = legend(ui.ctx(), &state, Overlay::Water);
        });
        assert!(none.is_none());
    }
}
