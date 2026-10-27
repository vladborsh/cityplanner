//! The read-only demand window the top bar and the Zones panel switch (T-088): one row per
//! zone type over `CityStats.zones` and the group demand of the sim view.

use bevy_egui::egui;
use cp_content::ZoneType;
use cp_locale::{tr, trf};
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
            trf!(plural "{n} homes free", n = s.homes_free)
        } else {
            trf!(plural "{n} jobs free", n = s.jobs_free)
        },
        levels: s.by_level,
        status: trf!(
            "{building} building / {abandoned} abandoned",
            building = s.constructing,
            abandoned = s.abandoned
        ),
    }
}

/// How the demand of a zone's group comes about (§11), for the bar's hover text.
pub fn demand_formula(zone: ZoneType) -> String {
    match zone {
        ZoneType::ResLow | ZoneType::ResHigh => tr(
            "Residential demand = free jobs (weighted) − unemployed above the commuter allowance + base attraction, less vacant homes. Shared by both residential types.",
        ),
        ZoneType::ComLow | ZoneType::ComHigh => tr(
            "Commercial demand = goods the residents need − commercial capacity. Shared by both commercial types.",
        ),
        ZoneType::Industrial => tr(
            "Industrial demand = goods the shops need − industrial output + available workforce.",
        ),
        ZoneType::Office | ZoneType::None => {
            tr("Office demand = educated workforce available − free office jobs.")
        }
    }
}

/// "Unlocks at 5,000 (Busy Town)" while painting `zone` waits for a milestone (T-231).
pub fn zone_lock(view: &SimView, zone: ZoneType) -> Option<String> {
    let db = view.content.as_deref()?;
    let m = view.progress.zone_locked_by(db, zone)?;
    let ms = db.milestones().get(m)?;
    Some(crate::buildings::unlocks_at(ms.population, &ms.name))
}

/// Whether no milestone holds `zone` back (T-283); before the content is in, every zone is.
/// The zoning palette and the tax rows (BUG-123) show only the open ones.
pub fn zone_open(view: &SimView, zone: ZoneType) -> bool {
    view.content
        .as_deref()
        .is_none_or(|db| view.progress.zone_locked_by(db, zone).is_none())
}

/// The zone group of `zone` has demand (20+) but no free painted cell a building could cover
/// (BUG-076b): the land is used up, or what is left is too narrow for the smallest lot.
/// "Industrial land left is too narrow for any building — zone more".
pub fn land_shortage(view: &SimView, zone: ZoneType) -> Option<String> {
    if zone_lock(view, zone).is_some() {
        return None;
    }
    let group = |z: ZoneType| zone_group_demand(z, &view.demand).0;
    let (letter, v) = zone_group_demand(zone, &view.demand);
    if v < 20.0 {
        return None;
    }
    let (mut valid, mut unfit, mut painted) = (0, 0, 0);
    for (z, s) in ZONES.iter().zip(view.city.zones.iter()) {
        if group(*z) == letter {
            valid += s.valid;
            unfit += s.unfit;
            painted += s.painted;
        }
    }
    if valid > unfit {
        return None;
    }
    let used_up = painted == 0 || valid == 0;
    Some(match (letter, used_up) {
        ("R", true) => tr("No free Residential land — zone more"),
        ("C", true) => tr("No free Commercial land — zone more"),
        ("I", true) => tr("No free Industrial land — zone more"),
        (_, true) => tr("No free Office land — zone more"),
        ("R", false) => tr("Residential land left is too narrow for any building — zone more"),
        ("C", false) => tr("Commercial land left is too narrow for any building — zone more"),
        ("I", false) => tr("Industrial land left is too narrow for any building — zone more"),
        (_, false) => tr("Office land left is too narrow for any building — zone more"),
    })
}

/// "R demand 37/100" and, when it is low, why (one line each, ending in a newline).
pub fn demand_line(view: &SimView, zone: ZoneType) -> String {
    let (letter, v) = zone_group_demand(zone, &view.demand);
    let shown = v.clamp(0.0, 100.0).round();
    let value = format!("{shown:.0}");
    let mut text = match letter {
        "R" => trf!("R demand {value}/100", value = value),
        "C" => trf!("C demand {value}/100", value = value),
        "I" => trf!("I demand {value}/100", value = value),
        _ => trf!("O demand {value}/100", value = value),
    };
    text.push('\n');
    if let Some(short) = land_shortage(view, zone) {
        text.push_str(&short);
        text.push('\n');
    }
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
                trf!(
                    "No free jobs: {n} unemployed — zone industry/commerce",
                    n = city.unemployed
                )
            } else {
                tr("Enough homes stand empty")
            }
        }
        ZoneType::ComLow | ZoneType::ComHigh => {
            tr("Enough shops for the residents — more residents need more shops")
        }
        ZoneType::Industrial if city.unemployed > 0 => trf!(
            "Enough industry for the shops; {n} unemployed mostly fit commerce and offices",
            n = city.unemployed
        ),
        ZoneType::Industrial => {
            tr("Enough industry for the shops and no manual workers without a job")
        }
        ZoneType::Office | ZoneType::None => {
            tr("Enough office jobs for the educated residents without a job")
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
        .map(|l| trf!("Locked: {what}", what = l) + "\n")
        .unwrap_or_default();
    format!(
        "{lock}{}\n{}{free}\n{}",
        tr(zone_name(zone)),
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
    resp.on_hover_text(trf!(
        "Buildings by level: L1 {l1} · L2 {l2} · L3 {l3} · L4 {l4} · L5 {l5}",
        l1 = levels[0],
        l2 = levels[1],
        l3 = levels[2],
        l4 = levels[3],
        l5 = levels[4]
    ));
}

/// Demand by zone: the group bar, cells, free homes or jobs, level histogram and status.
pub fn demand_window(ctx: &egui::Context, open: &mut bool, view: &SimView) {
    let title = tr("Demand by zone");
    frame::window("demand", Icon::Demand, &title, open).show(ctx, |ui| {
        egui::Grid::new("demand_rows")
            .num_columns(6)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                for h in [
                    tr("Zone"),
                    tr("Demand"),
                    tr("Cells (painted / built)"),
                    tr("Free"),
                    tr("Levels"),
                    tr("Status"),
                ] {
                    ui.weak(h);
                }
                ui.end_row();
                for r in rows(view) {
                    let tint = color(r.zone);
                    ui.colored_label(tint, tr(zone_name(r.zone)));
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
        // T-307: the number the advisor used to nag about; the Jobs window explains it.
        let city = &view.city;
        ui.add_space(4.0);
        ui.label(trf!(
            "Unemployed: {n} ({percent} %)",
            n = city.unemployed,
            percent = format!("{:.1}", crate::metrics::unemployment_percent(city))
        ));
    });
}

#[cfg(test)]
mod tests {
    /// BUG-076b: demand with no free cell a building could cover says so.
    #[test]
    fn land_shortage_names_a_too_narrow_or_used_up_zone() {
        let mut view = cp_sim::SimRunner::new(1).view();
        view.demand.i = 80.0;
        let mut stats = (*view.city).clone();
        let ind = ZONES
            .iter()
            .position(|&z| z == ZoneType::Industrial)
            .unwrap();
        stats.zones[ind].painted = 800;
        stats.zones[ind].valid = 114;
        stats.zones[ind].unfit = 114;
        view.city = std::sync::Arc::new(stats.clone());
        let t = zone_tooltip(&view, ZoneType::Industrial);
        assert!(
            t.contains("Industrial land left is too narrow for any building — zone more"),
            "{t}"
        );
        stats.zones[ind].valid = 0;
        stats.zones[ind].unfit = 0;
        view.city = std::sync::Arc::new(stats.clone());
        assert!(
            land_shortage(&view, ZoneType::Industrial)
                .unwrap()
                .contains("No free Industrial")
        );
        // Some cell fits: no complaint. Low demand: none either.
        stats.zones[ind].valid = 10;
        view.city = std::sync::Arc::new(stats);
        assert!(land_shortage(&view, ZoneType::Industrial).is_none());
        view.demand.i = 5.0;
        assert!(land_shortage(&view, ZoneType::Industrial).is_none());
    }

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
