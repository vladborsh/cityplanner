//! The city metrics window (`architecture/system-design.md` §13.6): value rows and a sparkline
//! per section, all read from [`cp_sim::CityStats`] (`SimView::city`) and the view's treasury.
//! [`sections`] builds what is shown; [`metrics_window`] draws it.

use bevy_egui::egui;
use cp_content::UtilityKind;
use cp_sim::SimView;
use cp_sim::stats::{CityStats, ZONES};
use cp_tools::{format_money, zone_name};

use crate::frame;
use crate::icons::Icon;

/// One section of the window.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub title: &'static str,
    /// `(label, value text)`.
    pub rows: Vec<(String, String)>,
    /// The history behind the sparkline, oldest first (none: no sparkline).
    pub spark: Option<Vec<f32>>,
}

fn row(label: &str, value: impl ToString) -> (String, String) {
    (label.to_string(), value.to_string())
}

fn percent(share: f32) -> String {
    format!("{:.0} %", share * 100.0)
}

/// Unemployed adults as a share of the adults, in percent (`0` without adults).
pub fn unemployment_percent(s: &CityStats) -> f32 {
    if s.adults == 0 {
        0.0
    } else {
        s.unemployed as f32 * 100.0 / s.adults as f32
    }
}

fn spark(s: &CityStats, f: impl Fn(&cp_sim::stats::HistorySample) -> f32) -> Option<Vec<f32>> {
    Some(s.history.iter().map(f).collect())
}

/// What the window shows, from the view.
pub fn sections(view: &SimView) -> Vec<Section> {
    let s = &*view.city;
    let week = &view.economy.last_week;
    let mut out = Vec::new();

    out.push(Section {
        title: "Population",
        rows: vec![
            row("Residents", s.population),
            row("Households", s.households),
            row("Employed", s.employed),
            row("Unemployed", s.unemployed),
            row("Unemployment", format!("{:.1} %", unemployment_percent(s))),
        ],
        spark: spark(s, |h| h.population as f32),
    });

    let mut jobs = vec![row(
        "All zones",
        format!(
            "{} / {} free",
            s.zones.iter().map(|z| z.jobs).sum::<u32>(),
            s.zones.iter().map(|z| z.jobs_free).sum::<u32>()
        ),
    )];
    for (k, z) in ZONES.iter().enumerate() {
        jobs.push(row(
            zone_name(*z),
            format!("{} / {} free", s.zones[k].jobs, s.zones[k].jobs_free),
        ));
    }
    out.push(Section {
        title: "Jobs (total / free)",
        rows: jobs,
        spark: spark(s, |h| h.employed as f32),
    });

    let b = &s.buildings;
    out.push(Section {
        title: "Buildings",
        rows: vec![
            row("Total", b.total),
            row("Active", b.active),
            row("Under construction", b.constructing),
            row("Abandoned", b.abandoned),
            row("Burned", b.burned),
            row("Without water", b.no_water),
            row("Without power", b.no_power),
            row("Without sewage", b.no_sewage),
            row("Without road", b.no_road),
        ],
        spark: None,
    });

    let t = &s.traffic;
    out.push(Section {
        title: "Traffic",
        rows: vec![
            row("Vehicles", t.vehicles),
            row("Pedestrians", t.pedestrians),
            row("Trips in transit", t.trips_in_transit),
            row("Average load", percent(t.avg_load)),
            row("Maximum load", percent(t.max_load)),
            row("Overloaded segments", t.overloaded),
            row("Vanished in a jam", t.despawned_stuck),
            row("No path", t.no_path),
        ],
        spark: spark(s, |h| h.avg_load),
    });

    let mut utilities = Vec::new();
    for (kind, name) in [
        (UtilityKind::Water, "Water"),
        (UtilityKind::Sewage, "Sewage"),
        (UtilityKind::Power, "Power"),
    ] {
        let n = &s.utilities[CityStats::utility_index(kind)];
        utilities.push(row(
            name,
            format!(
                "capacity {:.0} · use {:.0} · unsupplied {}",
                n.capacity, n.consumption, n.unsupplied
            ),
        ));
    }
    out.push(Section {
        title: "Utilities",
        rows: utilities,
        spark: None,
    });

    out.push(Section {
        title: "Environment",
        rows: vec![
            row("Land value (avg)", format!("{:.1}", s.land.avg_value)),
            row("Pollution (avg)", format!("{:.1}", s.land.avg_pollution)),
            row("Pollution (max)", format!("{:.1}", s.land.max_pollution)),
            row("Noise (avg)", format!("{:.1}", s.land.avg_noise)),
        ],
        spark: spark(s, |h| h.pollution),
    });

    out.push(services_section(view));

    out.push(Section {
        title: "Budget",
        rows: vec![
            row("Balance", format_money(view.treasury.balance)),
            row("Income last week", format_money(week.income())),
            row("Upkeep last week", format_money(week.upkeep())),
        ],
        spark: spark(s, |h| h.balance.0 as f32),
    });
    out
}

/// The M2 services (T-137): per kind the share of the buildings with people in them that a
/// service reaches, then the garbage piled up, the sick, the dead, fires and crime. A kind
/// with no building yet, and a count of nothing, read "—".
fn services_section(view: &SimView) -> Section {
    use cp_building::BuildingState;
    use cp_content::ServiceKind;
    const NONE: &str = "—";
    let kinds = [
        (ServiceKind::Garbage, "Garbage coverage"),
        (ServiceKind::Health, "Health coverage"),
        (ServiceKind::Deathcare, "Deathcare coverage"),
        (ServiceKind::Fire, "Fire coverage"),
        (ServiceKind::Police, "Police coverage"),
        (ServiceKind::Education, "Education coverage"),
    ];
    let mut rows = Vec::new();
    let db = view.content.as_deref();
    for (kind, label) in kinds {
        let mut have = false;
        let (mut served, mut total) = (0u32, 0u32);
        if let Some(db) = db {
            for (_, b) in view
                .buildings
                .iter()
                .filter(|(_, b)| b.state == BuildingState::Active)
            {
                let def = db.building(b.def);
                let svc = def
                    .service
                    .as_deref()
                    .and_then(|s| db.service_id(s))
                    .map(|s| db.service(s).kind);
                if svc == Some(kind) {
                    have = true;
                }
                if svc.is_none() && (def.households > 0 || def.jobs.iter().any(|&j| j > 0)) {
                    total += 1;
                    served += u32::from(view.service.coverage_of(kind, b) > 0.0);
                }
            }
        }
        rows.push(row(
            label,
            if have && total > 0 {
                percent(served as f32 / total as f32)
            } else {
                NONE.to_string()
            },
        ));
    }
    let garbage: u32 = view
        .service_state
        .records()
        .map(|(_, r)| u32::from(r.garbage))
        .sum();
    let counts = view.service_state.summary().counts;
    let count = |n: u32| {
        if n == 0 {
            NONE.to_string()
        } else {
            n.to_string()
        }
    };
    rows.push(row("Garbage piled up", count(garbage)));
    rows.push(row("Sick", count(counts[1])));
    rows.push(row("Dead", count(counts[2])));
    rows.push(row("Fires", count(counts[3])));
    rows.push(row("Crime", count(counts[4])));
    // Students (kept in the schools' `stored`) and places (their capacity).
    let (mut students, mut places) = (0u32, 0u32);
    if let Some(db) = db {
        for (id, b) in view
            .buildings
            .iter()
            .filter(|(_, b)| b.state == BuildingState::Active)
        {
            let svc = db
                .building(b.def)
                .service
                .as_deref()
                .and_then(|s| db.service_id(s))
                .map(|s| db.service(s))
                .filter(|s| s.kind == ServiceKind::Education);
            if let Some(svc) = svc {
                places += svc.capacity;
                students += view.service_state.facility(id).map_or(0, |f| f.stored);
            }
        }
    }
    rows.push(row(
        "Students / places",
        if places == 0 {
            NONE.to_string()
        } else {
            format!("{students} / {places}")
        },
    ));
    Section {
        title: "Services",
        rows,
        spark: None,
    }
}

/// A line through `values` filling `size`, scaled to their own min and max (flat when they
/// are all equal); nothing to draw before two samples exist.
pub fn sparkline(ui: &mut egui::Ui, values: &[f32], size: egui::Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(28));
    if values.len() >= 2 {
        let (lo, hi) = values
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        let span = (hi - lo).max(f32::EPSILON);
        let n = (values.len() - 1) as f32;
        let points: Vec<egui::Pos2> = values
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let x = rect.left() + rect.width() * i as f32 / n;
                let y = if hi > lo {
                    rect.bottom() - 2.0 - (rect.height() - 4.0) * (v - lo) / span
                } else {
                    rect.center().y
                };
                egui::pos2(x, y)
            })
            .collect();
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(110, 200, 140)),
        ));
    }
    response
}

/// The sections as the window lays them out (no scroll area: tests draw it into a tall `Ui`).
pub fn metrics_contents(ui: &mut egui::Ui, view: &SimView) {
    for section in sections(view) {
        ui.add_space(4.0);
        ui.strong(section.title);
        egui::Grid::new(("metrics", section.title))
            .num_columns(2)
            .show(ui, |ui| {
                for (label, value) in &section.rows {
                    ui.weak(label);
                    ui.label(value);
                    ui.end_row();
                }
            });
        if let Some(values) = &section.spark {
            sparkline(ui, values, egui::vec2(240.0, 28.0));
        }
    }
}

/// The window itself, shown while `*open`.
pub fn metrics_window(ctx: &egui::Context, open: &mut bool, view: &SimView) {
    frame::window("metrics", Icon::Metrics, "City metrics", open).show(ctx, |ui| {
        egui::ScrollArea::vertical()
            .max_height(520.0)
            .show(ui, |ui| metrics_contents(ui, view));
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

    fn painted(view: &SimView) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut all = Vec::new();
        for t in [0.0, 0.1] {
            // A screen taller than the contents: rows below the screen are clipped, not painted.
            let mut tall = input(t, vec![]);
            tall.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 4000.0),
            ));
            all = frame_texts(&ctx, tall, |ui| metrics_contents(ui, view))
                .into_iter()
                .map(|(text, _)| text)
                .collect();
        }
        all
    }

    fn value<'a>(secs: &'a [Section], title: &str, label: &str) -> &'a str {
        secs.iter()
            .find(|s| s.title == title)
            .and_then(|s| s.rows.iter().find(|(l, _)| l == label))
            .map(|(_, v)| v.as_str())
            .unwrap_or_else(|| panic!("{title}/{label}"))
    }

    #[test]
    fn an_empty_city_shows_zeros_and_draws() {
        let sim = SimRunner::new(1);
        let view = sim.view();
        let secs = sections(&view);
        assert_eq!(value(&secs, "Population", "Residents"), "0");
        assert_eq!(value(&secs, "Population", "Unemployment"), "0.0 %");
        assert_eq!(value(&secs, "Traffic", "Vehicles"), "0");
        assert_eq!(value(&secs, "Buildings", "Total"), "0");
        assert_eq!(
            value(&secs, "Utilities", "Water"),
            "capacity 0 · use 0 · unsupplied 0"
        );
        let text = painted(&view);
        assert!(text.iter().any(|t| t == "Residents"));

        // The window itself draws (and a closed one does not).
        let ctx = egui::Context::default();
        let mut open = true;
        let mut shown = Vec::new();
        for t in [0.0, 0.1] {
            shown = frame_texts(&ctx, input(t, vec![]), |ui| {
                metrics_window(ui.ctx(), &mut open, &view);
            });
        }
        assert!(shown.iter().any(|(t, _)| t == "City metrics"), "{shown:?}");
        open = false;
        let shown = frame_texts(&ctx, input(0.2, vec![]), |ui| {
            metrics_window(ui.ctx(), &mut open, &view);
        });
        assert!(shown.is_empty());
    }

    #[test]
    fn the_services_section_shows_the_coverage_from_the_view_and_dashes_for_nothing() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
        let content = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
        // Nothing built: dashes.
        let mut empty = cp_sim::SimRunner::new(3);
        empty.set_content(Arc::clone(&content));
        empty.tick_once();
        let secs = sections(&empty.view());
        assert_eq!(value(&secs, "Services", "Garbage coverage"), "—");
        assert_eq!(value(&secs, "Services", "Fire coverage"), "—");
        // A scripted city brings its own services (T-148): their coverage shows as a share.
        let (mut sim, _) = grid_city(200, 3, terrain, content, &GridCityConfig::default());
        for _ in 0..128 {
            sim.tick_once();
        }
        let secs = sections(&sim.view());
        for row in ["Garbage coverage", "Fire coverage"] {
            let got = value(&secs, "Services", row);
            assert!(got.ends_with(" %") && got != "0 %", "{row}: {got}");
        }
    }

    #[test]
    fn a_grid_city_shows_exactly_the_city_stats_fields() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
        let content = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
        let (mut sim, report) = grid_city(400, 3, terrain, content, &GridCityConfig::default());
        assert!(report.reached, "{report:?}");
        // Past the next publish so every stage has run.
        for _ in 0..cp_sim::stats::STATS_PERIOD * 2 {
            sim.tick_once();
        }
        let view = sim.view();
        let s = &*view.city;
        assert!(s.population >= 400 && s.buildings.total > 0 && !s.history.is_empty());

        let secs = sections(&view);
        let v = |t: &str, l: &str| value(&secs, t, l).to_string();
        assert_eq!(v("Population", "Residents"), s.population.to_string());
        assert_eq!(v("Population", "Households"), s.households.to_string());
        assert_eq!(v("Population", "Employed"), s.employed.to_string());
        assert_eq!(v("Population", "Unemployed"), s.unemployed.to_string());
        assert_eq!(
            v("Population", "Unemployment"),
            format!("{:.1} %", s.unemployed as f32 * 100.0 / s.adults as f32)
        );
        let jobs: u32 = s.zones.iter().map(|z| z.jobs).sum();
        let free: u32 = s.zones.iter().map(|z| z.jobs_free).sum();
        assert_eq!(
            v("Jobs (total / free)", "All zones"),
            format!("{jobs} / {free} free")
        );
        for (k, z) in ZONES.iter().enumerate() {
            assert_eq!(
                v("Jobs (total / free)", zone_name(*z)),
                format!("{} / {} free", s.zones[k].jobs, s.zones[k].jobs_free)
            );
        }
        let b = &s.buildings;
        for (label, n) in [
            ("Total", b.total),
            ("Active", b.active),
            ("Under construction", b.constructing),
            ("Abandoned", b.abandoned),
            ("Burned", b.burned),
            ("Without water", b.no_water),
            ("Without power", b.no_power),
            ("Without sewage", b.no_sewage),
            ("Without road", b.no_road),
        ] {
            assert_eq!(v("Buildings", label), n.to_string(), "{label}");
        }
        let t = &s.traffic;
        for (label, n) in [
            ("Vehicles", u64::from(t.vehicles)),
            ("Pedestrians", u64::from(t.pedestrians)),
            ("Trips in transit", u64::from(t.trips_in_transit)),
            ("Overloaded segments", u64::from(t.overloaded)),
            ("Vanished in a jam", t.despawned_stuck),
            ("No path", t.no_path),
        ] {
            assert_eq!(v("Traffic", label), n.to_string(), "{label}");
        }
        assert_eq!(
            v("Traffic", "Average load"),
            format!("{:.0} %", t.avg_load * 100.0)
        );
        assert_eq!(
            v("Traffic", "Maximum load"),
            format!("{:.0} %", t.max_load * 100.0)
        );
        for (name, i) in [("Water", 0), ("Sewage", 1), ("Power", 2)] {
            let n = &s.utilities[i];
            assert_eq!(
                v("Utilities", name),
                format!(
                    "capacity {:.0} · use {:.0} · unsupplied {}",
                    n.capacity, n.consumption, n.unsupplied
                )
            );
        }
        assert_eq!(
            v("Environment", "Land value (avg)"),
            format!("{:.1}", s.land.avg_value)
        );
        assert_eq!(
            v("Environment", "Pollution (avg)"),
            format!("{:.1}", s.land.avg_pollution)
        );
        assert_eq!(
            v("Environment", "Pollution (max)"),
            format!("{:.1}", s.land.max_pollution)
        );
        assert_eq!(
            v("Environment", "Noise (avg)"),
            format!("{:.1}", s.land.avg_noise)
        );
        assert_eq!(v("Budget", "Balance"), format_money(view.treasury.balance));
        assert_eq!(
            v("Budget", "Income last week"),
            format_money(view.economy.last_week.income())
        );

        // Every sparkline is the history of its field, in order.
        let spark_of = |title: &str| {
            secs.iter()
                .find(|x| x.title == title)
                .unwrap()
                .spark
                .clone()
                .unwrap()
        };
        let pop: Vec<f32> = s.history.iter().map(|h| h.population as f32).collect();
        assert_eq!(spark_of("Population"), pop);
        let load: Vec<f32> = s.history.iter().map(|h| h.avg_load).collect();
        assert_eq!(spark_of("Traffic"), load);

        // And the window paints every value.
        let text = painted(&view);
        // `cargo test -p cp_ui a_grid_city -- --nocapture` prints what the window paints.
        println!("{}", text.join(" | "));
        for (_, value) in secs.iter().flat_map(|sec| sec.rows.iter()) {
            assert!(text.iter().any(|t| t == value), "{value:?} not painted");
        }
    }

    #[test]
    fn the_sparkline_scales_to_its_own_range_and_survives_short_or_flat_history() {
        let ctx = egui::Context::default();
        for values in [vec![], vec![1.0], vec![5.0, 5.0, 5.0], vec![0.0, 10.0, 4.0]] {
            frame_texts(&ctx, input(0.0, vec![]), |ui| {
                let r = sparkline(ui, &values, egui::vec2(100.0, 20.0));
                assert_eq!(r.rect.size(), egui::vec2(100.0, 20.0));
            });
        }
    }
}
