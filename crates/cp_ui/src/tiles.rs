//! The land section of the economy window (T-150): the 5×5 tiles of the map, owned ones marked,
//! the ones that can be bought with their price.

use bevy_egui::egui;
use cp_sim::SimView;
use cp_sim::tiles::{TileState, tile_state};
use cp_terrain::TILES_PER_SIDE;
use cp_tools::format_money;

/// The grid; a click on a buyable tile sets `pending`, which `land::confirm_window` confirms.
pub fn tile_section(ui: &mut egui::Ui, view: &SimView, pending: &mut Option<(u8, u8)>) {
    ui.separator();
    ui.strong("Land");
    let size = egui::vec2(46.0, 34.0);
    for tz in 0..TILES_PER_SIDE as u8 {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
            for tx in 0..TILES_PER_SIDE as u8 {
                let tile = (tx, tz);
                let state = tile_state(view, tile);
                let (label, hover, enabled) = match state {
                    TileState::Owned => ("own".to_owned(), "Owned".to_owned(), false),
                    TileState::Buyable(p) => (
                        format_money(p),
                        format!("Buy for {}", format_money(p)),
                        true,
                    ),
                    TileState::NeedsMilestone => (
                        "—".to_owned(),
                        "Opens at the next milestone".to_owned(),
                        false,
                    ),
                    TileState::Far => (String::new(), "Not next to your land".to_owned(), false),
                };
                let selected = *pending == Some(tile);
                let button =
                    egui::Button::new(label).selected(selected || state == TileState::Owned);
                if ui
                    .add_enabled(enabled, button.min_size(size))
                    .on_hover_text(hover)
                    .on_disabled_hover_text(match state {
                        TileState::Owned => "Owned",
                        TileState::NeedsMilestone => "Opens at the next milestone",
                        _ => "Not next to your land",
                    })
                    .clicked()
                {
                    *pending = Some(tile);
                }
            }
        });
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
