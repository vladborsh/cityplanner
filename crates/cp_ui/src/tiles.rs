//! The land section of the economy window (T-150): the 5×5 tiles of the map, owned ones marked,
//! the ones that can be bought with their price.

use bevy_egui::egui;
use cp_locale::{tr, trf};
use cp_sim::SimView;
use cp_sim::tiles::{TileState, tile_state};
use cp_terrain::TILES_PER_SIDE;
use cp_tools::{format_money, format_money_short};

/// The grid of the Land tab: owned tiles in blue, buyable ones with their short price, the rest
/// dashed and without a price. A click on a buyable tile sets `pending`.
pub fn tile_section(ui: &mut egui::Ui, view: &SimView, pending: &mut Option<(u8, u8)>) {
    let size = egui::vec2(66.0, 52.0);
    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
    for tz in 0..TILES_PER_SIDE as u8 {
        ui.horizontal(|ui| {
            for tx in 0..TILES_PER_SIDE as u8 {
                let tile = (tx, tz);
                let state = tile_state(view, tile);
                let (rect, r) = ui.allocate_exact_size(
                    size,
                    if matches!(state, TileState::Buyable(_)) {
                        egui::Sense::click()
                    } else {
                        egui::Sense::hover()
                    },
                );
                let p = ui.painter();
                let selected = *pending == Some(tile);
                let (label, hover) = match state {
                    TileState::Owned => {
                        p.rect_filled(rect, 5.0, egui::Color32::from_rgb(40, 78, 130));
                        (tr("own"), tr("Owned"))
                    }
                    TileState::Buyable(price) => {
                        let fill = if selected || r.hovered() { 60 } else { 44 };
                        p.rect_filled(rect, 5.0, egui::Color32::from_gray(fill));
                        p.rect_stroke(
                            rect,
                            5.0,
                            egui::Stroke::new(
                                if selected { 2.0 } else { 1.0 },
                                egui::Color32::from_rgb(120, 200, 120),
                            ),
                            egui::StrokeKind::Inside,
                        );
                        (
                            format_money_short(price),
                            trf!("Buy for {price}", price = format_money(price)),
                        )
                    }
                    TileState::NeedsMilestone => {
                        dashed(p, rect);
                        (String::new(), tr("Opens at the next milestone"))
                    }
                    TileState::Far => {
                        dashed(p, rect);
                        (String::new(), tr("Not next to your land"))
                    }
                };
                p.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(14.0),
                    egui::Color32::from_gray(225),
                );
                if r.on_hover_text(hover).clicked() {
                    *pending = Some(tile);
                }
            }
        });
    }
}

/// A dashed outline: a tile that cannot be had.
fn dashed(p: &egui::Painter, rect: egui::Rect) {
    let stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(80));
    let r = rect.shrink(1.0);
    for side in [
        [r.left_top(), r.right_top()],
        [r.right_top(), r.right_bottom()],
        [r.right_bottom(), r.left_bottom()],
        [r.left_bottom(), r.left_top()],
    ] {
        p.extend(egui::Shape::dashed_line(&side, stroke, 4.0, 3.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> SimView {
        let content = std::sync::Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        );
        let mut sim = cp_sim::SimRunner::new(1);
        sim.set_content(content);
        sim.world_mut()
            .insert_resource(cp_terrain::TileOwnership::only((2, 2)));
        sim.world_mut().insert_resource(cp_sim::Progress::START);
        sim.view()
    }

    #[test]
    fn tiles_are_owned_buyable_by_ring_or_far() {
        let v = view();
        assert_eq!(tile_state(&v, (2, 2)), TileState::Owned);
        assert_eq!(
            tile_state(&v, (3, 2)),
            TileState::Buyable(cp_core::Cost::from_units(5000.0))
        );
        assert_eq!(tile_state(&v, (0, 0)), TileState::Far);
    }
}
