//! The legend and the source captions of the Water and Power overlays (T-183): one line per
//! network with what its sources give and what its buildings ask for, the count of buildings
//! without supply, and a caption over every source. Reads `cp_render::utility_report`.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use cp_render::{
    Overlay, OverlayState, RtsCamera, UtilityLayer, UtilityReport, network_color, utility_report,
};
use cp_sim::SimView;
use std::sync::Arc;

/// `(colour, text)` lines of the legend of `layer`.
pub fn legend_lines(layer: UtilityLayer, report: &UtilityReport) -> Vec<([f32; 4], String)> {
    let (what, unit) = match layer {
        UtilityLayer::Power => ("Power", "power"),
        UtilityLayer::Water => ("Water", "water"),
    };
    let mut lines = vec![([1.0; 4], format!("{what}: supply / demand ({unit})"))];
    if report.networks.is_empty() {
        lines.push(([0.7; 4], "No network yet".to_string()));
    }
    for (k, n) in report.networks.iter().enumerate() {
        lines.push((
            network_color(layer, k, 1.0),
            format!(
                "Network {}: {:.0} / {:.0}, {} buildings{}",
                k + 1,
                n.capacity,
                n.demand,
                n.buildings,
                if n.deficit > 0 {
                    format!(", {} short", n.deficit)
                } else {
                    String::new()
                }
            ),
        ));
    }
    lines.push((
        [0.95, 0.2, 0.15, 1.0],
        format!("Not connected: {}", report.unconnected),
    ));
    lines.push(([1.0, 0.55, 0.05, 1.0], {
        let short: u32 = report.networks.iter().map(|n| n.deficit).sum();
        format!("In reach but short: {short}")
    }));
    lines
}

fn rgba(c: [f32; 4]) -> egui::Color32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    egui::Color32::from_rgba_unmultiplied(b(c[0]), b(c[1]), b(c[2]), b(c[3]))
}

/// The cached report: recomputed only when the graph, the buildings or the zones changed.
#[derive(Default)]
pub struct Cached {
    key: [usize; 3],
    layer: Option<UtilityLayer>,
    report: UtilityReport,
}

pub fn utility_legend(
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
    egui::Area::new(egui::Id::new("utility_legend"))
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -12.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                for (c, text) in legend_lines(layer, &cache.report) {
                    ui.colored_label(rgba(c), text);
                }
            });
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use cp_render::NetworkStats;

    #[test]
    fn the_legend_lists_each_network_with_its_sums_and_the_unsupplied_count() {
        let report = UtilityReport {
            networks: vec![
                NetworkStats {
                    capacity: 120.0,
                    demand: 95.0,
                    buildings: 40,
                    ..Default::default()
                },
                NetworkStats {
                    capacity: 30.0,
                    demand: 50.0,
                    buildings: 9,
                    deficit: 4,
                    ..Default::default()
                },
            ],
            unconnected: 7,
            ..Default::default()
        };
        let text: Vec<String> = legend_lines(UtilityLayer::Power, &report)
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        assert!(text.iter().any(|t| t.contains("Network 1: 120 / 95")));
        assert!(
            text.iter()
                .any(|t| t.contains("Network 2: 30 / 50") && t.contains("4 short"))
        );
        assert!(text.iter().any(|t| t == "Not connected: 7"));
        let c = |k| network_color(UtilityLayer::Power, k, 1.0);
        assert_ne!(c(0), c(1));
    }
}
