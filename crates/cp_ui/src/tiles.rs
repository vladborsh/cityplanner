//! The land section of the economy window (T-150): the 5×5 tiles of the map, owned ones marked,
//! the ones that can be bought with their price, and a confirmation before `BuyTile`.

use bevy_egui::egui;
use cp_core::Cost;
use cp_sim::SimView;
use cp_sim::tiles::{tile_limit, tile_price};
use cp_terrain::{TILES_PER_SIDE, TerrainCommand};
use cp_tools::{CommandOutbox, format_money};

/// What a tile can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileState {
    Owned,
    /// Touches an owned tile and the milestone allows one more.
    Buyable(Cost),
    /// Touches an owned tile, but the milestone limit is reached.
    NeedsMilestone,
    /// Does not touch an owned tile.
    Far,
}

pub fn tile_state(view: &SimView, tile: (u8, u8)) -> TileState {
    let (tx, tz) = (usize::from(tile.0), usize::from(tile.1));
    if view.tiles.is_open(tx, tz) {
        return TileState::Owned;
    }
    if !view.tiles.touches_owned(tx, tz) {
        return TileState::Far;
    }
    let content = view.content.as_deref();
    if view.tiles.open_count() >= tile_limit(content, view.progress) {
        return TileState::NeedsMilestone;
    }
    let start = view
        .terrain
        .as_ref()
        .map_or((2, 2), |t| t.meta().start_tile);
    TileState::Buyable(tile_price(content, start, tile))
}

/// The grid, the confirmation and the buy button. `pending` is the tile awaiting confirmation.
pub fn tile_section(
    ui: &mut egui::Ui,
    view: &SimView,
    pending: &mut Option<(u8, u8)>,
    outbox: &mut CommandOutbox,
) {
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
    if let Some(tile) = *pending {
        match tile_state(view, tile) {
            TileState::Buyable(price) => {
                ui.horizontal(|ui| {
                    ui.label(format!("Buy this tile for {}?", format_money(price)));
                    if ui.button("Buy").clicked() {
                        outbox.push(cp_sim::SimCommand::Terrain(TerrainCommand::BuyTile {
                            tile,
                        }));
                        *pending = None;
                    }
                    if ui.button("Cancel").clicked() {
                        *pending = None;
                    }
                });
            }
            _ => *pending = None,
        }
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
            TileState::Buyable(Cost::from_units(5000.0))
        );
        assert_eq!(tile_state(&v, (0, 0)), TileState::Far);
    }
}
