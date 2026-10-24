//! The city metrics window (`architecture/system-design.md` §13.6): value rows and a sparkline
//! per section, all read from [`cp_sim::CityStats`] (`SimView::city`) and the view's treasury.
//! [`sections`] builds what is shown; [`metrics_window`] draws it.

use bevy_egui::egui;
use cp_content::UtilityKind;
use cp_locale::{tr, trf, trn};
use cp_sim::SimView;
use cp_sim::stats::{CityStats, ZONES};
use cp_tools::{format_money, zone_name};

use crate::frame;
use crate::icons::Icon;

/// One section of the window. The title and the row labels are the English keys; they are
/// translated where they are drawn ([`metrics_contents`]).
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

/// The eight contentment factors in `CityContentment::factors` order (§19.2).
pub const FACTOR_NAMES: [&str; cp_economy::FACTORS] = [
    "Services",
    "Environment",
    "Home problems",
    "Work",
    "Commute",
    "Leisure",
    "Taxes",
    "Trust",
];

/// The Happiness section (T-318c): the city's index, the mean score of each factor, one row per
/// district (and the homes outside them) and the index day by day.
fn happiness_section(view: &SimView) -> Section {
    let c = &view.city_contentment;
    let mut rows = vec![row(
        "Index",
        format!("{} ({})", c.city, cp_building::mood_word(c.city)),
    )];
    for (name, v) in FACTOR_NAMES.iter().zip(c.factors) {
        rows.push(row(name, v));
    }
    // Without districts every home is outside them: the city's row says it all.
    let rows_of_districts = if view.districts.iter().next().is_some() {
        &c.districts[..]
    } else {
        &[]
    };
    for d in rows_of_districts {
        let name = match d.district {
            Some(id) => view
                .districts
                .get(id)
                .map_or_else(|| format!("#{}", id.0), |d| d.name.clone()),
            None => tr("Outside districts"),
        };
        rows.push((name, d.index.to_string()));
    }
    let daily: Vec<f32> = view
        .city
        .daily
        .iter()
        .map(|d| f32::from(d.contentment))
        .collect();
    Section {
        title: "Happiness",
        rows,
        spark: (!daily.is_empty()).then_some(daily),
    }
}

/// What the window shows, from the view.
pub fn sections(view: &SimView) -> Vec<Section> {
    let s = &*view.city;
    let week = &view.economy.last_week;
    let mut out = Vec::new();
    // First, so the click on the bar's smiley opens the window on it.
    if view.city_contentment.homes > 0 {
        out.push(happiness_section(view));
    }

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
        trf!(
            "{jobs} / {free} free",
            jobs = s.zones.iter().map(|z| z.jobs).sum::<u32>(),
            free = s.zones.iter().map(|z| z.jobs_free).sum::<u32>()
        ),
    )];
    for (k, z) in ZONES.iter().enumerate() {
        jobs.push(row(
            zone_name(*z),
            trf!(
                "{jobs} / {free} free",
                jobs = s.zones[k].jobs,
                free = s.zones[k].jobs_free
            ),
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
            row(
                "By mode (foot / car / transit)",
                format!(
                    "{} / {} / {}",
                    t.trips_by_mode[0], t.trips_by_mode[1], t.trips_by_mode[2]
                ),
            ),
            row(
                "Average trip (foot / car)",
                trf!(
                    "{foot} / {car} min",
                    foot = format!("{:.1}", t.trip_minutes[0]),
                    car = format!("{:.1}", t.trip_minutes[1])
                ),
            ),
            row("Average load", percent(t.avg_load)),
            row("Maximum load", percent(t.max_load)),
            row("Overloaded segments", t.overloaded),
            row("Vanished in a jam", t.despawned_stuck),
            row("No path", t.no_path),
        ],
        spark: spark(s, |h| h.avg_load),
    });

    out.push(transport_section(view));
    out.push(industry_section(view));
    out.push(parks_section(view));

    let mut utilities = Vec::new();
    for (kind, name) in [
        (UtilityKind::Water, "Water"),
        (UtilityKind::Sewage, "Sewage"),
        (UtilityKind::Power, "Power"),
    ] {
        let n = &s.utilities[CityStats::utility_index(kind)];
        utilities.push(row(
            name,
            trf!(
                "capacity {capacity} · use {usage} · unsupplied {unsupplied}",
                capacity = format!("{:.0}", n.capacity),
                usage = format!("{:.0}", n.consumption),
                unsupplied = n.unsupplied
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

/// Passengers by mode, tourists, goods trucks, exports and imports of the week in progress,
/// and the shops without goods (T-171).
fn transport_section(view: &SimView) -> Section {
    use cp_building::BuildingFlags;
    use cp_transit::TransitMode;
    let riders = |mode: TransitMode| -> u32 {
        view.lines
            .iter()
            .filter(|l| l.mode == mode)
            .map(|l| l.riders_week)
            .sum()
    };
    let f = &view.freight;
    let l = &view.transit_ledger;
    let no_goods = view
        .buildings
        .iter()
        .filter(|(_, b)| b.flags.has(BuildingFlags::NO_GOODS))
        .count();
    Section {
        title: "Transport and goods",
        rows: vec![
            row("Bus riders (week)", riders(TransitMode::Bus)),
            row("Tram riders (week)", riders(TransitMode::Tram)),
            row("Trolleybus riders (week)", riders(TransitMode::Trolley)),
            row("Metro riders (week)", riders(TransitMode::Metro)),
            row("Train riders (week)", riders(TransitMode::Train)),
            row("Tourists now", f.tourists_now),
            row("Tourist visits (week)", f.tourists_week),
            row("Goods trucks out", f.trucks),
            row(
                "Exports (week)",
                trf!(plural "{n} loads · {money}", n = l.export_loads, money = format_money(l.exports)),
            ),
            row(
                "Imports (week)",
                trn("{n} loads", i64::from(l.import_loads)),
            ),
            row("Buildings without goods", no_goods),
        ],
        spark: None,
    }
}

/// The industrial areas (T-219): areas and their levels, chain buildings and jobs, what they
/// hold, the buildings with a problem, and the cargo trade of the week (`IndustryLedger`).
/// The Parks section (T-229): the areas, the visitors and the entrance fees.
fn parks_section(view: &SimView) -> Section {
    use cp_district::AreaKind;
    let ids: Vec<_> = view
        .areas
        .iter()
        .filter(|(_, a)| matches!(a.kind, AreaKind::Park(_)))
        .map(|(id, _)| id)
        .collect();
    let (mut inside, mut capacity, mut day, mut week) = (0u32, 0u32, 0u32, 0u32);
    let mut levels = Vec::new();
    let mut no_gate = 0u32;
    for &id in &ids {
        if let Some(s) = crate::parks::park_summary(view, id) {
            inside += s.inside;
            capacity += s.capacity;
            day += s.visitors_day;
            week += s.visitors_week;
            levels.push(s.level.to_string());
            no_gate += u32::from(s.gates == 0);
        }
    }
    let l = &view.park_ledger;
    Section {
        title: "Parks",
        rows: vec![
            row("Areas", ids.len()),
            row(
                "Area levels",
                if levels.is_empty() {
                    "—".to_string()
                } else {
                    levels.join(" · ")
                },
            ),
            row("Areas without a gate", no_gate),
            row("Visitors inside", format!("{inside} / {capacity}")),
            row(
                "Visitors",
                trf!("{day} today · {week} this week", day = day, week = week),
            ),
            row("Entrance fees (week)", format_money(l.fees)),
            row("Last week: entrance fees", format_money(l.last_fees)),
        ],
        spark: None,
    }
}

fn industry_section(view: &SimView) -> Section {
    use cp_district::AreaKind;
    let areas: Vec<_> = view
        .areas
        .iter()
        .filter(|(_, a)| matches!(a.kind, AreaKind::Industry(_)))
        .collect();
    let levels: Vec<String> = areas.iter().map(|(_, a)| a.level.to_string()).collect();
    let (mut buildings, mut jobs) = (0u32, 0u32);
    if let Some(db) = view.content.as_deref() {
        for (_, b) in view.buildings.iter() {
            let def = db.building(b.def);
            if def.industry.is_some() {
                buildings += 1;
                jobs += def.jobs.iter().map(|&j| u32::from(j)).sum::<u32>();
            }
        }
    }
    let stored: f32 = view
        .industry
        .stock
        .keys()
        .map(|&b| view.industry.total(b))
        .sum();
    let problems = view.industry.problems.len();
    let l = &view.industry_ledger;
    Section {
        title: "Industry",
        rows: vec![
            row("Areas", areas.len()),
            row(
                "Area levels",
                if levels.is_empty() {
                    "—".to_string()
                } else {
                    levels.join(" · ")
                },
            ),
            row(
                "Chain buildings",
                trf!(plural "{buildings} · {n} jobs", n = jobs, buildings = buildings),
            ),
            row("Cargo stored", format!("{stored:.0}")),
            row("Buildings with a problem", problems),
            row(
                "Cargo exports (week)",
                trf!(plural "{n} loads · {money}", n = l.export_loads, money = format_money(l.exports)),
            ),
            row(
                "Cargo imports (week)",
                trf!(plural "{n} loads · {money}", n = l.import_loads, money = format_money(l.imports)),
            ),
            row(
                "Last week: exports / imports",
                format!(
                    "{} / {}",
                    format_money(l.last_exports),
                    format_money(l.last_imports)
                ),
            ),
        ],
        spark: None,
    }
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
        (ServiceKind::Post, "Mail coverage"),
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
    rows.push(row("Mail piled up", count(counts[5])));
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
        ui.strong(tr(section.title));
        egui::Grid::new(("metrics", section.title))
            .num_columns(2)
            .show(ui, |ui| {
                for (label, value) in &section.rows {
                    ui.weak(tr(label));
                    // A factor of the Happiness section is a bar with its score.
                    match value.parse::<u8>() {
                        Ok(v)
                            if section.title == "Happiness"
                                && FACTOR_NAMES.contains(&label.as_str()) =>
                        {
                            ui.add(
                                egui::ProgressBar::new(f32::from(v) / 100.0)
                                    .desired_width(160.0)
                                    .fill(crate::happiness::Mood::of(v).colour())
                                    .text(value.as_str()),
                            );
                        }
                        _ if section.title == "Happiness" => {
                            ui.label(crate::info::tr_value(value));
                        }
                        _ => {
                            ui.label(value);
                        }
                    }
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
    frame::window("metrics", Icon::Metrics, &tr("City metrics"), open).show(ctx, |ui| {
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
    fn the_transport_section_sums_the_riders_by_mode_and_shows_goods_and_tourists() {
        use cp_sim::LineView;
        use cp_transit::{LineState, TransitMode};
        let line = |mode, riders_week| LineView {
            id: cp_core::Id::new(0, 0),
            name: "L".into(),
            mode,
            color: 0,
            stops: 2,
            vehicles: 1,
            vehicles_out: 1,
            budget: 100,
            running: 1,
            state: LineState::Ok,
            riders_today: 0,
            riders_week,
            waiting: 0,
        };
        let mut view = SimRunner::new(1).view();
        let secs = sections(&view);
        assert_eq!(
            value(&secs, "Transport and goods", "Bus riders (week)"),
            "0"
        );
        assert_eq!(value(&secs, "Transport and goods", "Goods trucks out"), "0");

        view.lines = vec![
            line(TransitMode::Bus, 30),
            line(TransitMode::Bus, 12),
            line(TransitMode::Metro, 7),
        ];
        view.freight = cp_sim::FreightView {
            trucks: 3,
            trucks_exporting: 1,
            trucks_importing: 1,
            tourists_now: 4,
            tourists_week: 9,
        };
        view.transit_ledger.export_loads = 2;
        view.transit_ledger.import_loads = 5;
        let secs = sections(&view);
        let v = |l: &str| value(&secs, "Transport and goods", l).to_string();
        assert_eq!(v("Bus riders (week)"), "42");
        assert_eq!(v("Metro riders (week)"), "7");
        assert_eq!(v("Train riders (week)"), "0");
        assert_eq!(v("Tourists now"), "4");
        assert_eq!(v("Tourist visits (week)"), "9");
        assert_eq!(v("Goods trucks out"), "3");
        assert!(v("Exports (week)").starts_with("2 loads"));
        assert_eq!(v("Imports (week)"), "5 loads");
        let text = painted(&view);
        for (_, value) in &secs
            .iter()
            .find(|s| s.title == "Transport and goods")
            .unwrap()
            .rows
        {
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

    /// T-219: the Industry section counts the areas and sums the week's cargo trade.
    #[test]
    fn the_industry_section_shows_areas_and_the_ledger_of_the_week() {
        use crate::industry::tests::{put, world_with_areas};
        let (mut sim, db, at) = world_with_areas(2);
        put(&mut sim, &db, "forest_main", at[0]);
        put(&mut sim, &db, "farm_main", at[1]);
        let mut st = cp_industry::IndustryState::default();
        st.stock.insert(
            cp_core::Id::new(0, 0),
            vec![(cp_content::CargoKind::Logs, 40.0)],
        );
        sim.world_mut().insert_resource(st);
        let mut view = sim.view();
        view.industry_ledger.export_loads = 3;
        view.industry_ledger.exports = cp_core::Cost(120_000);
        view.industry_ledger.import_loads = 1;
        let secs = sections(&view);
        let v = |l: &str| value(&secs, "Industry", l).to_string();
        assert_eq!(v("Areas"), "2");
        assert_eq!(v("Area levels"), "2 · 1");
        assert!(
            v("Chain buildings").starts_with("2 "),
            "{}",
            v("Chain buildings")
        );
        assert!(v("Cargo exports (week)").starts_with("3 loads"));
        assert!(v("Cargo imports (week)").starts_with("1 loads"));
        let text = painted(&view);
        for (_, value) in &secs.iter().find(|s| s.title == "Industry").unwrap().rows {
            assert!(text.iter().any(|t| t == value), "{value:?} not painted");
        }
    }
}
