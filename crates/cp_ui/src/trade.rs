//! The Trade table (§21.3, T-320c): per cargo the export and import price now against the
//! price list, the market event, the level of the last 30 days and the units that went out and
//! came in last week, under a line with the season and the week's money. The Metrics window
//! shows it as a section and the Industry panel as a tab. Every price comes from the sim's own
//! functions (`Market::export_price` / `import_price`), so the table shows what a load is paid.

use bevy_egui::egui;
use cp_content::{CargoKind, ContentDb, MarketEventKind, ResourceKind, ZoneType};
use cp_core::Tick;
use cp_core::time::{SEASON_DAYS, Season, TICKS_PER_DAY, season};
use cp_locale::{tr, trf};
use cp_sim::SimView;
use cp_tools::format_money;

/// The market state of a cargo as the table shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeState {
    Normal,
    /// A shortage or glut with the days left.
    Shortage(u32),
    Glut(u32),
}

/// One row of the table.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeRow {
    pub cargo: CargoKind,
    /// What a unit sold to the outside brings now, and one bought costs.
    pub export: f32,
    pub import: f32,
    /// The export price against the price list, whole percent (`+12`, `-8`).
    pub export_change: i32,
    pub state: TradeState,
    /// The price level of the last [`cp_content::market::MARKET_HISTORY`] days, oldest first.
    pub history: Vec<f32>,
    /// Units out and in last week.
    pub exported: u32,
    pub imported: u32,
    /// Whether the city has a building of its chain (the others are grey, at the bottom).
    pub in_city: bool,
}

/// The cargo the city's buildings make or use: the chains' inputs and outputs, and Goods
/// while there are factories or shops (M3).
fn cargo_in_city(view: &SimView, db: &ContentDb) -> [bool; 13] {
    let mut out = [false; 13];
    for (_, b) in view.buildings.iter() {
        let def = db.building(b.def);
        if let Some(spec) = &def.industry {
            for &(c, _) in spec.inputs.iter().chain(&spec.outputs) {
                out[c as usize] = true;
            }
        } else if matches!(
            def.zone,
            ZoneType::Industrial | ZoneType::ComLow | ZoneType::ComHigh
        ) {
            out[CargoKind::Goods as usize] = true;
        }
    }
    out
}

/// The 13 rows in [`CargoKind::ALL`] order, the cargo of the city's chains first.
pub fn trade_rows(view: &SimView) -> Vec<TradeRow> {
    let Some(db) = view.content.as_deref() else {
        return Vec::new();
    };
    let m = &view.market;
    let now = view.tick;
    let day = now.0 / TICKS_PER_DAY;
    let in_city = cargo_in_city(view, db);
    let last = &view.industry_ledger.last_by_cargo;
    let n = cp_content::market::MARKET_HISTORY;
    let mut rows: Vec<TradeRow> = CargoKind::ALL
        .into_iter()
        .map(|cargo| {
            let i = cargo as usize;
            let list = db.cargo(cargo).map_or(0.0, |c| c.export_price);
            let export = m.export_price(db, cargo, now);
            let state = match m.get(cargo).event {
                Some(e) => {
                    let left = u64::from(e.until_day).saturating_sub(day).max(1) as u32;
                    match e.kind {
                        MarketEventKind::Shortage => TradeState::Shortage(left),
                        MarketEventKind::Glut => TradeState::Glut(left),
                    }
                }
                None => TradeState::Normal,
            };
            TradeRow {
                cargo,
                export,
                import: m.import_price(db, cargo, now),
                export_change: if list > 0.0 {
                    ((export / list - 1.0) * 100.0).round() as i32
                } else {
                    0
                },
                state,
                history: (0..n)
                    .rev()
                    .map(|d| f32::from(m.history_at(cargo, d)))
                    .collect(),
                exported: last[i].export,
                imported: last[i].import,
                in_city: in_city[i],
            }
        })
        .collect();
    // Stable: `ALL` order within each group.
    rows.sort_by_key(|r| !r.in_city);
    rows
}

/// The English key of a season's name.
pub fn season_name(s: Season) -> &'static str {
    match s {
        Season::Spring => "Spring",
        Season::Summer => "Summer",
        Season::Autumn => "Autumn",
        Season::Winter => "Winter",
    }
}

/// The game days left of the season of `tick`, today counted.
pub fn season_days_left(tick: Tick) -> u32 {
    (SEASON_DAYS - (tick.0 / TICKS_PER_DAY) % SEASON_DAYS) as u32
}

/// "Autumn, 3 days left".
pub fn season_line(tick: Tick) -> String {
    let n = season_days_left(tick);
    trf!(plural "{season}, {n} days left", n = n, season = tr(season_name(season(tick))))
}

/// The tooltip of the season icon of the top bar: what the season does.
pub fn season_hint(s: Season) -> String {
    tr(match s {
        Season::Spring => "Spring: prices as listed",
        Season::Summer => "Summer: crops and oil a little cheaper to export",
        Season::Autumn => "Autumn: crops cheap to export",
        Season::Winter => "Winter: crops and oil dear",
    })
}

/// The square glyph of the season of `tick`, drawn at `size` (top bar, §21.3): a sprout, a sun, a leaf, a
/// snowflake. Hover tells what the season does.
pub fn season_icon(ui: &mut egui::Ui, tick: Tick, size: f32) -> egui::Response {
    let s = season(tick);
    use egui::{Color32, Shape, Stroke, pos2, vec2};
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let c = rect.center();
    let r = size * 0.36;
    let at = |x: f32, y: f32| pos2(c.x + x * r, c.y + y * r);
    match s {
        Season::Spring => {
            let green = Color32::from_rgb(120, 200, 110);
            p.line_segment(
                [at(0.0, 1.0), at(0.0, -0.1)],
                Stroke::new(size * 0.08, green),
            );
            for side in [-1.0, 1.0] {
                p.add(Shape::convex_polygon(
                    vec![
                        at(0.0, -0.1),
                        at(side * 0.55, -0.75),
                        at(side * 0.95, -0.35),
                    ],
                    green,
                    Stroke::NONE,
                ));
            }
        }
        Season::Summer => {
            let sun = Color32::from_rgb(245, 200, 70);
            p.circle_filled(c, r * 0.5, sun);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let (x, y) = (a.cos(), a.sin());
                p.line_segment(
                    [at(x * 0.7, y * 0.7), at(x, y)],
                    Stroke::new(size * 0.07, sun),
                );
            }
        }
        Season::Autumn => {
            let leaf = Color32::from_rgb(225, 130, 60);
            p.add(Shape::convex_polygon(
                vec![at(0.0, -1.0), at(0.75, -0.2), at(0.0, 0.6), at(-0.75, -0.2)],
                leaf,
                Stroke::NONE,
            ));
            p.line_segment([at(0.0, 0.4), at(0.0, 1.0)], Stroke::new(size * 0.08, leaf));
        }
        Season::Winter => {
            let snow = Color32::from_rgb(170, 215, 245);
            for i in 0..3 {
                let a = i as f32 * std::f32::consts::FRAC_PI_3 + std::f32::consts::FRAC_PI_2;
                let (x, y) = (a.cos(), a.sin());
                p.line_segment([at(-x, -y), at(x, y)], Stroke::new(size * 0.08, snow));
            }
        }
    }
    response.on_hover_text(format!("{}\n{}", season_line(tick), season_hint(s)))
}

/// "Shortage · 6 days left", "Glut · 3 days left", "—".
pub fn state_text(s: TradeState) -> String {
    match s {
        TradeState::Normal => "—".to_string(),
        TradeState::Shortage(n) => {
            trf!(plural "Shortage · {n} days left", n = n)
        }
        TradeState::Glut(n) => trf!(plural "Glut · {n} days left", n = n),
    }
}

/// "▲12%", "▼8%", "" at the list price.
pub fn change_text(pct: i32) -> String {
    match pct {
        0 => String::new(),
        p if p > 0 => format!("▲{p}%"),
        p => format!("▼{}%", -p),
    }
}

/// The price of one unit as money.
fn unit_price(p: f32) -> String {
    format_money(cp_core::Cost::scaled(p, 1.0))
}

/// The colour of a cargo's swatch: its chain's resource; Goods neutral.
fn cargo_swatch(db: &ContentDb, cargo: CargoKind) -> egui::Color32 {
    ResourceKind::ALL
        .into_iter()
        .find(|&k| cp_industry::chain_cargo(db, Some(k)).contains(&cargo))
        .map_or(egui::Color32::from_gray(170), crate::industry::swatch)
}

fn state_colour(s: TradeState) -> egui::Color32 {
    match s {
        TradeState::Normal => egui::Color32::from_gray(150),
        TradeState::Shortage(_) => egui::Color32::from_rgb(230, 120, 90),
        TradeState::Glut(_) => egui::Color32::from_rgb(110, 170, 230),
    }
}

/// Today's level against the mean of the last 7 days: "▲", "▼" or "" (within 2 points).
pub fn trend(m: &cp_content::Market, cargo: CargoKind) -> &'static str {
    let mean = (0..7)
        .map(|d| f32::from(m.history_at(cargo, d)))
        .sum::<f32>()
        / 7.0;
    let now = f32::from(m.get(cargo).index);
    if now > mean + 2.0 {
        "▲"
    } else if now < mean - 2.0 {
        "▼"
    } else {
        ""
    }
}

/// The cargo an inspector prices: a chain building's inputs and outputs; a warehouse's one
/// cargo, else what it holds.
fn building_cargo(view: &SimView, db: &ContentDb, id: cp_building::BuildingId) -> Vec<CargoKind> {
    let Some(spec) = view
        .buildings
        .get(id)
        .and_then(|b| db.building(b.def).industry.as_ref())
    else {
        return Vec::new();
    };
    let mut out: Vec<CargoKind> = if spec.role == cp_content::IndustryRole::Warehouse {
        match view.warehouses.get(id).cargo {
            Some(c) => vec![c],
            None => view
                .industry
                .stock
                .get(&id)
                .map(|s| s.iter().map(|&(c, _)| c).collect())
                .unwrap_or_default(),
        }
    } else {
        spec.inputs
            .iter()
            .chain(&spec.outputs)
            .map(|&(c, _)| c)
            .collect()
    };
    out.sort_unstable();
    out.dedup();
    out
}

/// Inspector rows (§21.3): per cargo of the building its export price now, the trend against
/// the week and the market event: ("Price: Logs", "$9 ▲ · Shortage").
pub fn price_rows(view: &SimView, id: cp_building::BuildingId) -> Vec<(String, String)> {
    let Some(db) = view.content.as_deref() else {
        return Vec::new();
    };
    let m = &view.market;
    building_cargo(view, db, id)
        .into_iter()
        .map(|cargo| {
            let mut v = format!(
                "{} {}",
                unit_price(m.export_price(db, cargo, view.tick)),
                trend(m, cargo)
            )
            .trim_end()
            .to_string();
            match m.get(cargo).event.map(|e| e.kind) {
                Some(MarketEventKind::Shortage) => v += &format!(" · {}", tr("Shortage")),
                Some(MarketEventKind::Glut) => v += &format!(" · {}", tr("Glut")),
                None => {}
            }
            (trf!("Price: {cargo}", cargo = tr(&format!("{cargo:?}"))), v)
        })
        .collect()
}

/// A warehouse's factor this far below 1 says "cheap now: Fill", this far above "dear: Empty".
pub const CHEAP: f32 = 0.85;
pub const DEAR: f32 = 1.15;

/// The hint by a warehouse's mode buttons (§21.3) for the first of its cargo whose market
/// (level × season × event) is cheap or dear now.
pub fn warehouse_hint(view: &SimView, id: cp_building::BuildingId) -> Option<String> {
    let db = view.content.as_deref()?;
    let k = &db.balance().market;
    building_cargo(view, db, id).into_iter().find_map(|cargo| {
        let f = view.market.export_factor(k, cargo, view.tick);
        let name = tr(&format!("{cargo:?}"));
        if f < CHEAP {
            Some(trf!("{cargo} is cheap now: Fill", cargo = name))
        } else if f > DEAR {
            Some(trf!("{cargo} is dear now: Empty", cargo = name))
        } else {
            None
        }
    })
}

/// Draws the season line and the week's money above the table (the Metrics window).
pub fn trade_contents(ui: &mut egui::Ui, view: &SimView) {
    trade_heading(ui, view);
    trade_table(ui, view);
}

/// The season line and the week's money.
pub fn trade_heading(ui: &mut egui::Ui, view: &SimView) {
    let l = &view.industry_ledger;
    ui.label(season_line(view.tick));
    ui.weak(trf!(
        "Last week: exports {exports} · imports {imports}",
        exports = format_money(l.last_exports + view.economy.last_week.exports),
        imports = format_money(l.last_imports)
    ));
}

/// The low Industry tray: the heading on the left, the table scrolling beside it.
pub fn trade_tray(ui: &mut egui::Ui, view: &SimView, height: f32) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(TRAY_HEADING_W, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(TRAY_HEADING_W);
                trade_heading(ui, view);
            },
        );
        egui::ScrollArea::vertical()
            .id_salt("trade")
            .max_height(height)
            .show(ui, |ui| trade_table(ui, view));
    });
}

/// The width of the heading beside the table in the tray.
const TRAY_HEADING_W: f32 = 200.0;

/// The table: a header row and a row per cargo.
pub fn trade_table(ui: &mut egui::Ui, view: &SimView) {
    let Some(db) = view.content.as_deref() else {
        return;
    };
    // Rows top-down whatever the parent's layout (the tray lays out left to right).
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 2.0);
        table_rows(ui, view, db);
    });
}

fn table_rows(ui: &mut egui::Ui, view: &SimView, db: &ContentDb) {
    // Fixed columns, not a `Grid`: a grid's sizing pass would draw the frame twice.
    let cell = |ui: &mut egui::Ui, w: f32, text: egui::WidgetText| {
        ui.allocate_ui_with_layout(
            egui::vec2(w, 16.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(w);
                ui.add(egui::Label::new(text).truncate());
            },
        );
    };
    ui.horizontal(|ui| {
        for (h, w) in ["Cargo", "Export", "Import", "Market", "30 days", "Out / in"]
            .into_iter()
            .zip(COLUMNS)
        {
            cell(ui, w, egui::RichText::new(tr(h)).weak().into());
        }
    });
    for r in trade_rows(view) {
        let text = |t: String| {
            let t = egui::RichText::new(t);
            if r.in_city { t } else { t.weak() }.into()
        };
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 2.0, cargo_swatch(db, r.cargo));
            cell(ui, COLUMNS[0] - SWATCH, text(tr(&format!("{:?}", r.cargo))));
            cell(
                ui,
                COLUMNS[1],
                text(
                    format!("{} {}", unit_price(r.export), change_text(r.export_change))
                        .trim_end()
                        .to_string(),
                ),
            );
            cell(ui, COLUMNS[2], text(unit_price(r.import)));
            cell(
                ui,
                COLUMNS[3],
                egui::RichText::new(state_text(r.state))
                    .color(state_colour(r.state))
                    .into(),
            );
            crate::metrics::sparkline(ui, &r.history, egui::vec2(COLUMNS[4], 14.0));
            cell(
                ui,
                COLUMNS[5],
                text(if r.exported + r.imported == 0 {
                    "—".to_string()
                } else {
                    format!("{} / {}", r.exported, r.imported)
                }),
            );
        });
    }
}

/// The swatch before a cargo's name with its gap (the item spacing).
const SWATCH: f32 = 16.0;
/// The widths of the table's columns.
const COLUMNS: [f32; 6] = [120.0, 100.0, 64.0, 180.0, 90.0, 70.0];

#[cfg(test)]
mod tests {
    use super::*;
    use cp_content::{Market, MarketEvent, MarketFrozen};

    /// §21.6 item 6: 13 rows priced by the sim's own functions, the event and its days left, the
    /// city's chain first, the history oldest first.
    #[test]
    fn the_table_has_13_rows_priced_by_the_market_with_the_citys_cargo_first() {
        use crate::industry::tests::{put, world_with_areas};
        let (mut sim, db, at) = world_with_areas(1);
        sim.world_mut().insert_resource(MarketFrozen);
        {
            let mut m = sim.world_mut().resource_mut::<Market>();
            m.get_mut(CargoKind::Planks).index = 150;
            m.get_mut(CargoKind::Logs).event = Some(MarketEvent {
                kind: MarketEventKind::Shortage,
                until_day: 6,
            });
            m.get_mut(CargoKind::Ore).index = 80;
            m.record_day();
        }
        put(&mut sim, &db, "forest_processor_a", at[0]);
        let v = sim.view();
        let rows = trade_rows(&v);
        assert_eq!(rows.len(), 13);
        let m = &v.market;
        for r in &rows {
            assert_eq!(r.export, m.export_price(&db, r.cargo, v.tick));
            assert_eq!(r.import, m.import_price(&db, r.cargo, v.tick));
            assert_eq!(r.history.len(), 30);
        }
        // The sawmill's cargo first, in `ALL` order; the rest grey below.
        let firsts: Vec<CargoKind> = rows
            .iter()
            .take_while(|r| r.in_city)
            .map(|r| r.cargo)
            .collect();
        assert_eq!(firsts, [CargoKind::Logs, CargoKind::Planks]);
        let planks = rows.iter().find(|r| r.cargo == CargoKind::Planks).unwrap();
        assert_eq!(planks.export_change, 50);
        assert_eq!(change_text(planks.export_change), "▲50%");
        let logs = rows.iter().find(|r| r.cargo == CargoKind::Logs).unwrap();
        assert_eq!(logs.state, TradeState::Shortage(6));
        let ore = rows.iter().find(|r| r.cargo == CargoKind::Ore).unwrap();
        assert_eq!(ore.export_change, -20);
        assert_eq!(*ore.history.last().unwrap(), 80.0);
        assert_eq!(ore.history[0], 100.0);
    }

    /// §21.3: the inspector prices each cargo of the building with the trend and the event, and
    /// a warehouse says "Fill" when its cargo is cheap and "Empty" when dear.
    #[test]
    fn the_inspector_prices_the_buildings_cargo_and_hints_the_warehouse_mode() {
        use crate::industry::tests::{put, world_with_areas};
        let (mut sim, db, at) = world_with_areas(1);
        sim.world_mut().insert_resource(MarketFrozen);
        let mill = put(&mut sim, &db, "forest_processor_a", at[0]);
        let store = put(&mut sim, &db, "forest_warehouse", at[0]);
        sim.world_mut()
            .resource_mut::<cp_industry::IndustryState>()
            .stock
            .insert(store, vec![(CargoKind::Logs, 100.0)]);
        {
            let mut m = sim.world_mut().resource_mut::<Market>();
            m.get_mut(CargoKind::Logs).index = 120;
            m.get_mut(CargoKind::Logs).event = Some(MarketEvent {
                kind: MarketEventKind::Glut,
                until_day: 4,
            });
        }
        let v = sim.view();
        let rows = price_rows(&v, mill);
        let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["Price: Logs", "Price: Planks"]);
        let logs = &rows[0].1;
        assert!(logs.contains('▲') && logs.ends_with("· Glut"), "{logs}");
        assert!(!rows[1].1.contains('·'), "{}", rows[1].1);
        // 120 % x glut 0.5 = 0.6: cheap, fill up.
        assert_eq!(
            warehouse_hint(&v, store).as_deref(),
            Some("Logs is cheap now: Fill")
        );
        sim.world_mut()
            .resource_mut::<Market>()
            .get_mut(CargoKind::Logs)
            .event = None;
        assert_eq!(
            warehouse_hint(&sim.view(), store).as_deref(),
            Some("Logs is dear now: Empty")
        );
        sim.world_mut()
            .resource_mut::<Market>()
            .get_mut(CargoKind::Logs)
            .index = 100;
        assert_eq!(warehouse_hint(&sim.view(), store), None);
    }

    #[test]
    fn the_season_line_counts_the_days_left() {
        assert_eq!(season_days_left(Tick(0)), 7);
        assert_eq!(season_days_left(Tick(TICKS_PER_DAY * 18)), 3);
        assert_eq!(season(Tick(TICKS_PER_DAY * 18)), Season::Autumn);
        assert_eq!(season_line(Tick(TICKS_PER_DAY * 18)), "Autumn, 3 days left");
        assert_eq!(state_text(TradeState::Glut(1)), "Glut · 1 day left");
        assert_eq!(change_text(0), "");
    }
}
