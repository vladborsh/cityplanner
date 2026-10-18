//! The read-only demand window the top bar and the Zones panel switch (T-088): one row per
//! zone type over `CityStats.zones` and the group demand of the sim view.

use bevy_egui::egui;
use cp_content::ZoneType;
use cp_sim::SimView;
use cp_sim::stats::{ZONES, ZoneStats};
use cp_tools::{zone_color, zone_name};

use crate::budget::demand_fraction;
use crate::frame;
use crate::icons::Icon;
use crate::panels::zone_group_demand;

/// What a row of the window shows, as text: the numbers come from `CityStats.zones` as they
/// are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneRow {
    pub zone: ZoneType,
    /// `painted / built` cells (built: covered by a building's lot).
    pub cells: String,
    /// "N homes free" for residential zones, "N jobs free" for the others.
    pub free: String,
    /// Buildings per level 1..=5.
    pub levels: [u32; 5],
    /// "N building / M abandoned".
    pub status: String,
}

/// The six rows, in palette order.
pub fn rows(view: &SimView) -> Vec<ZoneRow> {
    ZONES
        .iter()
        .zip(view.city.zones.iter())
        .map(|(&zone, s)| row(zone, s))
        .collect()
}

fn row(zone: ZoneType, s: &ZoneStats) -> ZoneRow {
    let residential = matches!(zone, ZoneType::ResLow | ZoneType::ResHigh);
    ZoneRow {
        zone,
        cells: format!("{} / {}", s.painted, s.occupied),
        free: if residential {
            format!("{} homes free", s.homes_free)
        } else {
            format!("{} jobs free", s.jobs_free)
        },
        levels: s.by_level,
        status: format!("{} building / {} abandoned", s.constructing, s.abandoned),
    }
}

/// How the demand of a zone's group comes about (§11), for the bar's hover text.
pub fn demand_formula(zone: ZoneType) -> &'static str {
    match zone {
        ZoneType::ResLow | ZoneType::ResHigh => {
            "Residential demand = free jobs (weighted) − unemployed above the commuter allowance + base attraction, \
             less vacant homes. Shared by both residential types."
        }
        ZoneType::ComLow | ZoneType::ComHigh => {
            "Commercial demand = goods the residents need − commercial capacity. \
             Shared by both commercial types."
        }
        ZoneType::Industrial => {
            "Industrial demand = goods the shops need − industrial output + available workforce."
        }
        ZoneType::Office | ZoneType::None => {
            "Office demand = educated workforce available − free office jobs."
        }
    }
}

/// "Opens at 5 000 residents (Large town)" while painting `zone` waits for a milestone (T-231).
pub fn zone_lock(view: &SimView, zone: ZoneType) -> Option<String> {
    let db = view.content.as_deref()?;
    let m = view.progress.zone_locked_by(db, zone)?;
    let ms = db.milestones().get(m)?;
    Some(format!(
        "Opens at {} residents ({})",
        crate::buildings::spaced(ms.population),
        ms.name
    ))
}

/// "R demand 37/100" and, when it is low, why (one line each, ending in a newline).
pub fn demand_line(view: &SimView, zone: ZoneType) -> String {
    let (letter, v) = zone_group_demand(zone, &view.demand);
    let shown = v.clamp(0.0, 100.0).round();
    let mut text = format!("{letter} demand {shown:.0}/100\n");
    if v < 20.0
        && let Some(why) = low_demand_reason(view, zone)
    {
        text.push_str(&why);
        text.push('\n');
    }
    text
}

/// The main reason a zone group's demand is low.
fn low_demand_reason(view: &SimView, zone: ZoneType) -> Option<String> {
    let city = &view.city;
    Some(match zone {
        ZoneType::ResLow | ZoneType::ResHigh => {
            if city.unemployed > 0 {
                format!(
                    "No free jobs: {} unemployed — zone industry/commerce",
                    city.unemployed
                )
            } else {
                "Enough homes stand empty".to_string()
            }
        }
        ZoneType::ComLow | ZoneType::ComHigh => {
            "Enough shops for the residents — more residents need more shops".to_string()
        }
        ZoneType::Industrial if city.unemployed > 0 => format!(
            "Enough industry for the shops; {} unemployed mostly fit commerce and offices",
            city.unemployed
        ),
        ZoneType::Industrial => {
            "Enough industry for the shops and no manual workers without a job".to_string()
        }
        ZoneType::Office | ZoneType::None => {
            "Enough office jobs for the educated residents without a job".to_string()
        }
    })
}

/// The hover text of a zone's demand bar or tile: the group's value, the free homes or jobs of
/// the type and the formula (one text for the window, the panel and the tiles).
pub fn zone_tooltip(view: &SimView, zone: ZoneType) -> String {
    let free = ZONES
        .iter()
        .position(|&z| z == zone)
        .and_then(|i| view.city.zones.get(i))
        .map(|s| row(zone, s).free)
        .unwrap_or_default();
    let lock = zone_lock(view, zone)
        .map(|l| format!("Locked: {l}\n"))
        .unwrap_or_default();
    format!(
        "{lock}{}\n{}{free}\n{}",
        zone_name(zone),
        demand_line(view, zone),
        demand_formula(zone)
    )
}

pub(crate) fn color(zone: ZoneType) -> egui::Color32 {
    let [r, g, b] = zone_color(zone);
    egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// A five-bar histogram of buildings by level.
fn level_histogram(ui: &mut egui::Ui, levels: [u32; 5], tint: egui::Color32) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(40.0, 14.0), egui::Sense::hover());
    let max = levels.iter().copied().max().unwrap_or(0).max(1) as f32;
    let w = rect.width() / 5.0;
    for (i, n) in levels.iter().enumerate() {
        let h = (rect.height() * *n as f32 / max).max(if *n > 0 { 1.0 } else { 0.0 });
        let x = rect.min.x + w * i as f32;
        let bar = egui::Rect::from_min_max(
            egui::pos2(x + 1.0, rect.max.y - h),
            egui::pos2(x + w - 1.0, rect.max.y),
        );
        ui.painter().rect_filled(bar, 0.0, tint);
    }
    resp.on_hover_text(format!(
        "Buildings by level: L1 {} · L2 {} · L3 {} · L4 {} · L5 {}",
        levels[0], levels[1], levels[2], levels[3], levels[4]
    ));
}

/// Demand by zone: the group bar, cells, free homes or jobs, level histogram and status.
pub fn demand_window(ctx: &egui::Context, open: &mut bool, view: &SimView) {
    frame::window("demand", Icon::Demand, "Demand by zone", open).show(ctx, |ui| {
        egui::Grid::new("demand_rows")
            .num_columns(6)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                for h in [
                    "Zone",
                    "Demand",
                    "Cells (painted / built)",
                    "Free",
                    "Levels",
                    "Status",
                ] {
                    ui.weak(h);
                }
                ui.end_row();
                for r in rows(view) {
                    let tint = color(r.zone);
                    ui.colored_label(tint, zone_name(r.zone));
                    let (_, v) = zone_group_demand(r.zone, &view.demand);
                    ui.add(
                        egui::ProgressBar::new(demand_fraction(v))
                            .desired_width(90.0)
                            .desired_height(10.0)
                            .fill(tint),
                    )
                    .on_hover_text(format!(
                        "{}{}",
                        demand_line(view, r.zone),
                        demand_formula(r.zone)
                    ));
                    ui.label(&r.cells);
                    ui.label(&r.free);
                    level_histogram(ui, r.levels, tint);
                    ui.label(&r.status);
                    ui.end_row();
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{frame_texts, input};
    use cp_content::ContentDb;
    use cp_sim::SimRunner;
    use cp_sim::scenario::{GridCityConfig, grid_city};
    use cp_terrain::Terrain;
    use std::sync::Arc;

    fn texts(view: &SimView, open: &mut bool) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut all = Vec::new();
        for t in [0.0, 0.1, 0.2] {
            all = frame_texts(&ctx, input(t, vec![]), |ui| {
                let _ = ui;
                demand_window(ui.ctx(), open, view);
            })
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        }
        all
    }

    #[test]
    fn an_empty_city_draws_six_zero_rows() {
        let view = SimRunner::new(1).view();
        let rows = rows(&view);
        assert_eq!(rows.len(), 6);
        assert!(
            rows.iter()
                .all(|r| r.cells == "0 / 0" && r.levels == [0; 5])
        );
        let mut open = true;
        let text = texts(&view, &mut open);
        assert!(text.iter().any(|t| t == "Residential (low)"), "{text:?}");
        assert!(text.iter().any(|t| t == "0 homes free"), "{text:?}");
        assert!(text.iter().any(|t| t == "0 jobs free"), "{text:?}");
        let mut closed = false;
        assert!(texts(&view, &mut closed).is_empty());
    }

    #[test]
    fn a_grid_city_shows_the_city_stats_zone_numbers() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
        let content = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
        let (mut sim, report) = grid_city(400, 3, terrain, content, &GridCityConfig::default());
        assert!(report.reached, "{report:?}");
        for _ in 0..cp_sim::stats::STATS_PERIOD * 2 {
            sim.tick_once();
        }
        let view = sim.view();
        assert!(view.city.zones.iter().any(|z| z.painted > 0));
        let mut open = true;
        let text = texts(&view, &mut open);
        for (r, s) in rows(&view).iter().zip(view.city.zones.iter()) {
            assert_eq!(r.cells, format!("{} / {}", s.painted, s.occupied));
            assert_eq!(r.levels, s.by_level);
            let free = if matches!(r.zone, ZoneType::ResLow | ZoneType::ResHigh) {
                s.homes_free
            } else {
                s.jobs_free
            };
            assert!(r.free.starts_with(&free.to_string()), "{r:?}");
            for painted in [&r.cells, &r.free, &r.status] {
                assert!(
                    text.iter().any(|t| t == painted),
                    "{painted:?} not in {text:?}"
                );
            }
        }
    }

    #[test]
    fn the_demand_icon_opens_and_closes_the_window() {
        let view = SimRunner::new(1).view();
        let mut open = false;
        assert!(texts(&view, &mut open).is_empty());
        open = true;
        assert!(!texts(&view, &mut open).is_empty());
    }
}
