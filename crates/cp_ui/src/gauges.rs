//! The utility gauges of the top bar (T-279): Power, Water and Sewage as a bar each, filled to
//! what the city asks for against what its sources give, green with a margin, yellow when the
//! margin is under [`TIGHT_MARGIN`], red when short. The per-network breakdown (what the old
//! bottom-left overlay legend listed) is in the tooltip; a click opens the matching overlay,
//! and the gauge of the active overlay is outlined.

use bevy_egui::egui;
use cp_building::BuildingFlags;
use cp_render::{OverlayState, UtilityLayer, UtilityReport, utility_report};
use cp_sim::SimView;
use cp_sim::city_services::{Sector, SectorBalance};
use std::sync::Arc;

use crate::icons::Icon;

/// Spare supply, as a share of the demand, under which a gauge turns yellow.
pub const TIGHT_MARGIN: f32 = 0.10;
/// The gauges in the order they stand.
pub const SECTORS: [Sector; 3] = [Sector::Power, Sector::Water, Sector::Sewage];
const ICON: f32 = 20.0;
const BAR: egui::Vec2 = egui::vec2(76.0, 18.0);

/// The colour band of a gauge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GaugeState {
    /// Supply covers the demand with at least [`TIGHT_MARGIN`] to spare.
    Ok,
    /// Covered, but with less than [`TIGHT_MARGIN`] to spare.
    Tight,
    /// The demand exceeds the supply.
    Short,
}

/// The band for `supply` against `demand`.
pub fn gauge_state(supply: f32, demand: f32) -> GaugeState {
    if demand <= 0.0 {
        GaugeState::Ok
    } else if supply < demand {
        GaugeState::Short
    } else if supply < demand * (1.0 + TIGHT_MARGIN) {
        GaugeState::Tight
    } else {
        GaugeState::Ok
    }
}

/// How far the bar is filled, `0..=1`: the demand over the supply (full when short).
pub fn fill(supply: f32, demand: f32) -> f32 {
    if demand <= 0.0 {
        0.0
    } else if supply <= 0.0 {
        1.0
    } else {
        (demand / supply).clamp(0.0, 1.0)
    }
}

/// A number short enough for the bar: `840`, `4000`, `12k`, `1.2M`.
pub fn short_number(v: f32) -> String {
    let v = v.max(0.0);
    if v >= 1_000_000.0 {
        format!("{:.1}M", v / 1_000_000.0)
    } else if v >= 10_000.0 {
        format!("{:.0}k", v / 1000.0)
    } else {
        format!("{v:.0}")
    }
}

/// The text on the bar, `"supply / demand"`.
pub fn bar_text(row: &SectorBalance) -> String {
    format!(
        "{} / {}",
        short_number(row.supply),
        short_number(row.demand)
    )
}

fn icon_of(sector: Sector) -> Icon {
    match sector {
        Sector::Power => Icon::Power,
        Sector::Water => Icon::Water,
        _ => Icon::Sewage,
    }
}

const RED: egui::Color32 = egui::Color32::from_rgb(200, 70, 65);
const YELLOW: egui::Color32 = egui::Color32::from_rgb(235, 195, 60);
const GREEN: egui::Color32 = egui::Color32::from_rgb(85, 180, 90);

fn colour(state: GaugeState) -> egui::Color32 {
    match state {
        GaugeState::Ok => GREEN,
        GaugeState::Tight => YELLOW,
        GaugeState::Short => RED,
    }
}

/// What the tooltip of a gauge says: the totals, then for power and water every network and
/// the buildings without supply split into not connected and in reach but short; for sewage
/// the buildings without it (`without_sewage`).
pub fn tooltip_lines(
    row: &SectorBalance,
    report: Option<&UtilityReport>,
    without_sewage: u32,
) -> Vec<String> {
    let what = match row.sector {
        Sector::Power => "power",
        Sector::Water => "water",
        _ => "sewage",
    };
    let mut lines = vec![format!(
        "{}: {:.0} / {:.0} {} (supply / demand)",
        row.sector.label(),
        row.supply,
        row.demand,
        row.unit
    )];
    let Some(report) = report else {
        lines.push(format!("Buildings without {what}: {without_sewage}"));
        return lines;
    };
    if report.networks.is_empty() {
        lines.push("No network yet".to_string());
    }
    for (k, n) in report.networks.iter().enumerate() {
        lines.push(format!(
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
        ));
    }
    lines.push(format!("Not connected: {}", report.unconnected));
    let short: u32 = report.networks.iter().map(|n| n.deficit).sum();
    lines.push(format!("Not enough {what}: {short}"));
    lines
}

/// The report of `layer`, cached in egui's memory until the graph, the buildings or the zones
/// change: it is worked out only while a tooltip shows.
fn cached_report(ctx: &egui::Context, view: &SimView, layer: UtilityLayer) -> Arc<UtilityReport> {
    let key = [
        Arc::as_ptr(&view.utilities) as usize,
        Arc::as_ptr(&view.buildings) as usize,
        Arc::as_ptr(&view.zones) as usize,
    ];
    let id = egui::Id::new(("utility_gauge_report", layer as u8));
    if let Some((k, r)) = ctx.data(|d| d.get_temp::<([usize; 3], Arc<UtilityReport>)>(id))
        && k == key
    {
        return r;
    }
    let report = Arc::new(match view.content.as_deref() {
        Some(db) => utility_report(layer, &view.utilities, &view.buildings, db),
        None => UtilityReport::default(),
    });
    ctx.data_mut(|d| d.insert_temp(id, (key, report.clone())));
    report
}

fn without_sewage(view: &SimView) -> u32 {
    view.buildings
        .iter()
        .filter(|(_, b)| {
            b.state == cp_building::BuildingState::Active && b.flags.has(BuildingFlags::NO_SEWAGE)
        })
        .count() as u32
}

/// One gauge: the icon and the bar with its numbers. Outlined when `active`.
fn gauge(ui: &mut egui::Ui, row: &SectorBalance, active: bool) -> egui::Response {
    let inner = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        icon_of(row.sector).show(ui, ICON);
        let (rect, _) = ui.allocate_exact_size(BAR, egui::Sense::hover());
        let p = ui.painter();
        let c = colour(gauge_state(row.supply, row.demand));
        p.rect_filled(rect, 3.0, egui::Color32::from_gray(35));
        let f = fill(row.supply, row.demand);
        if f > 0.0 {
            let mut full = rect;
            full.set_right(rect.left() + rect.width() * f);
            p.rect_filled(full, 3.0, c.gamma_multiply(0.85));
        }
        p.rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0, c),
            egui::StrokeKind::Inside,
        );
        let text = bar_text(row);
        let font = egui::FontId::proportional(12.0);
        p.text(
            rect.center() + egui::vec2(1.0, 1.0),
            egui::Align2::CENTER_CENTER,
            &text,
            font.clone(),
            egui::Color32::BLACK,
        );
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            font,
            egui::Color32::WHITE,
        );
    });
    let rect = inner.response.rect.expand(3.0);
    if active {
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    }
    ui.interact(
        rect,
        egui::Id::new(("utility_gauge", row.sector as u8)),
        egui::Sense::click(),
    )
}

/// The three gauges, left to right; a click opens the gauge's overlay. Returns the rect of
/// each gauge drawn (a locked utility has none).
pub fn utility_gauges(
    ui: &mut egui::Ui,
    view: &SimView,
    overlay: &mut OverlayState,
) -> Vec<(Sector, egui::Rect)> {
    let mut rects = Vec::new();
    for sector in SECTORS {
        let Some(row) = view
            .city_services()
            .iter()
            .find(|r| r.sector == sector && !r.locked)
        else {
            continue;
        };
        let r = gauge(
            ui,
            row,
            overlay.active == crate::services::overlay_of(sector),
        );
        rects.push((sector, r.rect));
        let r = r
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_ui(|ui| {
                let report = match sector {
                    Sector::Power => Some(cached_report(ui.ctx(), view, UtilityLayer::Power)),
                    Sector::Water => Some(cached_report(ui.ctx(), view, UtilityLayer::Water)),
                    _ => None,
                };
                let missing = if report.is_none() {
                    without_sewage(view)
                } else {
                    0
                };
                for line in tooltip_lines(row, report.as_deref(), missing) {
                    ui.label(line);
                }
            });
        if r.clicked() {
            overlay.open(crate::services::overlay_of(sector));
        }
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_render::NetworkStats;

    fn row(sector: Sector, supply: f32, demand: f32) -> SectorBalance {
        SectorBalance {
            sector,
            supply,
            demand,
            unit: "units/day",
            locked: false,
        }
    }

    /// The three gauges have three different icons; sewage has one of its own, not the treatment
    /// plant's building icon.
    #[test]
    fn each_gauge_has_its_own_icon_and_sewage_is_not_a_building_icon() {
        let icons: Vec<Icon> = SECTORS.into_iter().map(icon_of).collect();
        assert_eq!(icons, [Icon::Power, Icon::Water, Icon::Sewage]);
        assert_ne!(icon_of(Sector::Sewage), Icon::SewageTreatment);
        assert_eq!(Icon::Sewage.stem(), "sewage");
    }

    #[test]
    fn green_with_a_margin_yellow_under_ten_percent_red_when_short() {
        assert_eq!(gauge_state(340.0, 60.0), GaugeState::Ok);
        assert_eq!(gauge_state(110.0, 100.0), GaugeState::Ok);
        assert_eq!(gauge_state(109.0, 100.0), GaugeState::Tight);
        assert_eq!(gauge_state(100.0, 100.0), GaugeState::Tight);
        assert_eq!(gauge_state(99.0, 100.0), GaugeState::Short);
        assert_eq!(gauge_state(0.0, 5.0), GaugeState::Short);
        assert_eq!(gauge_state(0.0, 0.0), GaugeState::Ok);
        assert_eq!(gauge_state(400.0, 0.0), GaugeState::Ok);
    }

    #[test]
    fn the_bar_fills_to_the_demand_over_the_supply() {
        assert_eq!(fill(400.0, 100.0), 0.25);
        assert_eq!(fill(100.0, 400.0), 1.0);
        assert_eq!(fill(0.0, 10.0), 1.0);
        assert_eq!(fill(10.0, 0.0), 0.0);
        assert_eq!(bar_text(&row(Sector::Power, 340.0, 60.0)), "340 / 60");
        assert_eq!(
            bar_text(&row(Sector::Power, 12_000.0, 8069.0)),
            "12k / 8069"
        );
        assert_eq!(short_number(1_240_000.0), "1.2M");
    }

    #[test]
    fn the_tooltip_lists_each_network_and_the_buildings_without_supply() {
        let report = UtilityReport {
            networks: vec![
                NetworkStats {
                    capacity: 4000.0,
                    demand: 40.0,
                    buildings: 2,
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
        let lines = tooltip_lines(&row(Sector::Power, 4030.0, 90.0), Some(&report), 0);
        assert!(lines[0].contains("4030 / 90"), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|t| t == "Network 1: 4000 / 40, 2 buildings")
        );
        assert!(
            lines
                .iter()
                .any(|t| t.starts_with("Network 2: 30 / 50") && t.contains("4 short"))
        );
        assert!(lines.iter().any(|t| t == "Not connected: 7"));
        assert!(lines.iter().any(|t| t == "Not enough power: 4"));
        let sewage = tooltip_lines(&row(Sector::Sewage, 10.0, 40.0), None, 3);
        assert_eq!(sewage[1], "Buildings without sewage: 3");
    }
}
