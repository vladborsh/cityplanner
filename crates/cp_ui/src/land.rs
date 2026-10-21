//! Buying land on the map (T-152): while the Land overlay is on, the price is drawn over each
//! buyable tile and a click on one asks to confirm (`UiState::budget.buy_tile`); the confirm
//! window sends `BuyTile`.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use cp_render::{Overlay, OverlayState, RtsCamera};
use cp_sim::SimView;
use cp_sim::tiles::{TileState, tile_state};
use cp_terrain::{TILE_SIZE, TILES_PER_SIDE, TerrainCommand};
use cp_tools::{CommandOutbox, UiCapture, format_money};

use cp_locale::{tr, trf};

use crate::panels::UiState;

/// The tile under a world point, if on the map.
pub fn tile_at(p: Vec2) -> Option<(u8, u8)> {
    let (tx, tz) = ((p.x / TILE_SIZE).floor(), (p.y / TILE_SIZE).floor());
    let side = TILES_PER_SIDE as f32;
    (tx >= 0.0 && tz >= 0.0 && tx < side && tz < side).then_some((tx as u8, tz as u8))
}

/// Draws the prices of the buyable tiles and turns a click on one into a pending purchase.
pub fn land_overlay(
    mut contexts: EguiContexts,
    view: Option<Res<SimView>>,
    overlay: Res<OverlayState>,
    capture: Res<UiCapture>,
    mut state: ResMut<UiState>,
    cameras: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
) {
    let Some(view) = view else { return };
    if overlay.active != Overlay::Land {
        return;
    }
    let (Ok(ctx), Ok((camera, transform))) = (contexts.ctx_mut(), cameras.single()) else {
        return;
    };
    let Some(terrain) = view.terrain.as_deref() else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("land_prices"),
    ));
    for tz in 0..TILES_PER_SIDE as u8 {
        for tx in 0..TILES_PER_SIDE as u8 {
            let TileState::Buyable(price) = tile_state(&view, (tx, tz)) else {
                continue;
            };
            let at = Vec2::new(
                (f32::from(tx) + 0.5) * TILE_SIZE,
                (f32::from(tz) + 0.5) * TILE_SIZE,
            );
            let Ok(p) =
                camera.world_to_viewport(transform, Vec3::new(at.x, terrain.height(at), at.y))
            else {
                continue;
            };
            let pos = egui::pos2(p.x, p.y);
            let text = format_money(price);
            let font = egui::FontId::proportional(18.0);
            painter.text(
                pos + egui::vec2(1.0, 1.0),
                egui::Align2::CENTER_CENTER,
                &text,
                font.clone(),
                egui::Color32::BLACK,
            );
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                text,
                font,
                egui::Color32::WHITE,
            );
        }
    }
    // A click on the map (not on a panel) picks the tile under the cursor.
    if capture.pointer_blocked() || ctx.is_pointer_over_egui() {
        return;
    }
    let clicked = ctx.input(|i| i.pointer.primary_clicked());
    let Some(pointer) = ctx.pointer_latest_pos().filter(|_| clicked) else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(transform, Vec2::new(pointer.x, pointer.y)) else {
        return;
    };
    let Some(hit) =
        cp_render::camera::math::ray_terrain_hit(terrain, ray.origin, *ray.direction, 60_000.0)
    else {
        return;
    };
    if let Some(tile) = tile_at(Vec2::new(hit.x, hit.z))
        && matches!(tile_state(&view, tile), TileState::Buyable(_))
    {
        state.budget.buy_tile = Some(tile);
    }
}

/// The confirmation of a pending purchase, wherever it was started.
pub fn confirm_window(
    ctx: &egui::Context,
    view: &SimView,
    pending: &mut Option<(u8, u8)>,
    outbox: &mut CommandOutbox,
) {
    let Some(tile) = *pending else { return };
    let TileState::Buyable(price) = tile_state(view, tile) else {
        *pending = None;
        return;
    };
    let mut keep = true;
    egui::Window::new(tr("Buy land"))
        .id(egui::Id::new("buy_land"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(trf!(
                "Buy this tile for {price}?",
                price = format_money(price)
            ));
            ui.horizontal(|ui| {
                if ui.button(tr("Buy")).clicked() {
                    outbox.push(cp_sim::SimCommand::Terrain(TerrainCommand::BuyTile {
                        tile,
                    }));
                    keep = false;
                }
                if ui.button(tr("Cancel")).clicked() {
                    keep = false;
                }
            });
        });
    if !keep {
        *pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_world_point_maps_to_its_tile() {
        assert_eq!(tile_at(Vec2::new(10.0, 10.0)), Some((0, 0)));
        assert_eq!(
            tile_at(Vec2::new(TILE_SIZE * 3.5, TILE_SIZE * 2.5)),
            Some((3, 2))
        );
        assert_eq!(tile_at(Vec2::new(-1.0, 5.0)), None);
        assert_eq!(tile_at(Vec2::new(TILE_SIZE * 5.0 + 1.0, 5.0)), None);
    }
}
