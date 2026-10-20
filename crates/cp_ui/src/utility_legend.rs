//! The source captions of the Water and Power overlays (T-183): over every source, what its
//! network gives and what its buildings ask for. Reads `cp_render::utility_report`. The
//! per-network list that stood bottom-left is now the tooltip of the top bar's gauges (T-279,
//! [`crate::gauges`]).

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use cp_render::{Overlay, OverlayState, RtsCamera, UtilityLayer, UtilityReport, utility_report};
use cp_sim::SimView;
use std::sync::Arc;

/// The cached report: recomputed only when the graph, the buildings or the zones changed.
#[derive(Default)]
pub struct Cached {
    key: [usize; 3],
    layer: Option<UtilityLayer>,
    report: UtilityReport,
}

pub fn source_captions(
    mut contexts: EguiContexts,
    view: Option<Res<SimView>>,
    overlay: Res<OverlayState>,
    mut cache: Local<Cached>,
    cameras: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
) {
    let layer = match overlay.active {
        Overlay::Power => UtilityLayer::Power,
        Overlay::Water => UtilityLayer::Water,
        _ => return,
    };
    let (Some(view), Ok(ctx), Ok((camera, transform))) =
        (view, contexts.ctx_mut(), cameras.single())
    else {
        return;
    };
    let (Some(db), Some(terrain)) = (view.content.as_deref(), view.terrain.as_deref()) else {
        return;
    };
    let key = [
        Arc::as_ptr(&view.utilities) as usize,
        Arc::as_ptr(&view.buildings) as usize,
        Arc::as_ptr(&view.zones) as usize,
    ];
    if cache.key != key || cache.layer != Some(layer) {
        cache.report = utility_report(layer, &view.utilities, &view.buildings, db);
        cache.key = key;
        cache.layer = Some(layer);
    }
    // A caption over each source: what its network gives / what it asks for.
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("utility_sources"),
    ));
    for n in &cache.report.networks {
        for (_, pos) in &n.sources {
            let Ok(p) = camera.world_to_viewport(
                transform,
                Vec3::new(pos.x, terrain.height(*pos) + 12.0, pos.y),
            ) else {
                continue;
            };
            let text = format!("{:.0} / {:.0}", n.capacity, n.demand);
            let font = egui::FontId::proportional(15.0);
            let at = egui::pos2(p.x, p.y);
            painter.text(
                at + egui::vec2(1.0, 1.0),
                egui::Align2::CENTER_BOTTOM,
                &text,
                font.clone(),
                egui::Color32::BLACK,
            );
            painter.text(
                at,
                egui::Align2::CENTER_BOTTOM,
                text,
                font,
                egui::Color32::WHITE,
            );
        }
    }
}
