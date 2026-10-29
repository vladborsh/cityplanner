//! The Chains and Market tabs of the Industry window (T-337, `architecture/industry-ui.md`
//! §2.3 and §2.4): the map of the 13 cargo of the city with the four factories, and the market
//! of the cargo the city makes. UI only: it reads the [`SimView`].

use bevy_egui::egui;
use cp_content::{
    BuildingDefId, CargoKind, ContentDb, IndustryRole, MarketEventKind, ResourceKind,
};
use cp_core::time::{SEASON_DAYS, TICKS_PER_DAY, season};
use cp_district::{AreaId, AreaKind, DistrictCommand};
use cp_locale::{tr, trf};
use cp_render::OverlayState;
use cp_sim::{SimCommand, SimView};
use cp_tools::{ActiveTool, CommandOutbox, ToolState, format_money};

use crate::icons::Icon;
use crate::industry::{new_name, next_id, resource_icon, resource_name, show_overlays, swatch};
use crate::industry_data::{
    CargoStatus, best_industrial_area, cargo_status, factory_lock, has_main,
};
use crate::industry_win::{IndustryTab, cargo_color, def_image};
use crate::power::tile;
use crate::trade::{TradeRow, TradeState, trade_rows, unit_price};

const GOOD: egui::Color32 = egui::Color32::from_rgb(120, 200, 120);
const COST: egui::Color32 = egui::Color32::from_rgb(235, 150, 70);
const BLUE: egui::Color32 = egui::Color32::from_rgb(90, 160, 230);

fn cargo_name(c: CargoKind) -> String {
    tr(&format!("{c:?}"))
}

/// A pill that can be clicked: an optional colour square, a text, a border.
fn pill(
    ui: &mut egui::Ui,
    dot: Option<egui::Color32>,
    text: &str,
    ink: egui::Color32,
    border: Option<(egui::Color32, bool)>,
) -> egui::Response {
    let font = egui::FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, ink);
    let pad = egui::vec2(7.0, 3.0);
    let dot_w = if dot.is_some() { 14.0 } else { 0.0 };
    let size = egui::vec2(
        galley.size().x + dot_w + 2.0 * pad.x,
        galley.size().y + 2.0 * pad.y,
    );
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(
        rect,
        9.0,
        if resp.hovered() {
            egui::Color32::from_gray(58)
        } else {
            egui::Color32::from_gray(44)
        },
    );
    if let Some((colour, dashed)) = border {
        let stroke = egui::Stroke::new(1.5, colour);
        if dashed {
            let r = rect.shrink(0.75);
            let pts = [
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
                r.left_top(),
            ];
            p.extend(egui::Shape::dashed_line(&pts, stroke, 4.0, 3.0));
        } else {
            p.rect_stroke(rect, 9.0, stroke, egui::StrokeKind::Inside);
        }
    }
    let mut x = rect.min.x + pad.x;
    if let Some(c) = dot {
        p.rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(x + 5.0, rect.center().y),
                egui::vec2(10.0, 10.0),
            ),
            2.0,
            c,
        );
        x += dot_w;
    }
    p.galley(egui::pos2(x, rect.min.y + pad.y), galley, ink);
    resp
}

/// A cargo chip with what the city did with it last week: green and solid when made, orange
/// and dashed when it was bought as well, grey when not made; `locked` is the level that
/// opens its product.
pub(crate) fn status_chip(
    ui: &mut egui::Ui,
    db: &ContentDb,
    cargo: CargoKind,
    status: CargoStatus,
    locked: Option<u8>,
    short: bool,
) -> egui::Response {
    let name = cargo_name(cargo);
    let (text, ink, border, tip) = match status {
        CargoStatus::Made { units, imported } if imported > 0 => (
            format!("{name} {units} +{imported}"),
            egui::Color32::from_gray(225),
            Some((COST, true)),
            trf!(
                "Made {units} last week and bought {imported} more",
                units = units,
                imported = imported
            ),
        ),
        CargoStatus::Made { units, .. } => (
            format!("{name} {units}"),
            egui::Color32::from_gray(225),
            Some((GOOD, false)),
            trf!("Made {units} last week", units = units),
        ),
        CargoStatus::None => match locked {
            Some(level) => (
                format!("{name} · {}", trf!("Lvl {level}", level = level)),
                egui::Color32::from_gray(130),
                None,
                trf!("Opens at area level {level}", level = level),
            ),
            None => (
                name,
                egui::Color32::from_gray(140),
                None,
                tr("Not made in the city last week"),
            ),
        },
    };
    let text = if short { cargo_name(cargo) } else { text };
    pill(ui, Some(cargo_color(db, cargo)), &text, ink, border).on_hover_text(tip)
}

fn arrow(ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("▸").color(egui::Color32::from_gray(120)));
}

// ----- Chains -----------------------------------------------------------------------------------

/// The extractor outputs and the processor products of a resource, by building order.
fn chain_cargo_of(db: &ContentDb, res: ResourceKind) -> (Vec<CargoKind>, Vec<(CargoKind, u8)>) {
    let (mut raw, mut made): (Vec<CargoKind>, Vec<(CargoKind, u8)>) = (Vec::new(), Vec::new());
    let mut defs: Vec<_> = db
        .buildings()
        .filter_map(|(_, d)| {
            Some((
                d.industry.as_ref().filter(|s| s.resource == Some(res))?,
                &d.id,
            ))
        })
        .collect();
    defs.sort_by_key(|(s, id)| (s.role as u8, (*id).clone()));
    for (s, _) in defs {
        match s.role {
            IndustryRole::Extractor => {
                for &(c, _) in &s.outputs {
                    if !raw.contains(&c) {
                        raw.push(c);
                    }
                }
            }
            IndustryRole::Processor => {
                for &(c, _) in &s.outputs {
                    if !made.iter().any(|&(m, _)| m == c) {
                        made.push((c, s.min_level));
                    }
                }
            }
            _ => {}
        }
    }
    (raw, made)
}

/// Opens the zone of `res` on the Areas tab, or makes one.
fn go_to_resource(
    view: &SimView,
    res: ResourceKind,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    outbox: &mut CommandOutbox,
    tab: &mut IndustryTab,
) {
    let of_kind = |id: AreaId| {
        view.areas
            .get(id)
            .is_some_and(|a| a.kind == AreaKind::Industry(res))
    };
    let existing = tools
        .area
        .selected
        .filter(|&a| of_kind(a))
        .or_else(|| view.areas.iter().map(|(id, _)| id).find(|&id| of_kind(id)));
    let chosen = existing.or_else(|| {
        next_id(view).inspect(|&id| {
            outbox.push(SimCommand::District(DistrictCommand::CreateArea {
                kind: AreaKind::Industry(res),
                name: new_name(res, id),
            }));
        })
    });
    if chosen.is_some() {
        tools.area.selected = chosen;
        if tools.active != ActiveTool::Area {
            tools.select(ActiveTool::Area);
        }
        show_overlays(overlay);
        *tab = IndustryTab::Areas;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn chains_tab(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    outbox: &mut CommandOutbox,
    tab: &mut IndustryTab,
    focus: &mut Option<CargoKind>,
) {
    let status = cargo_status(view);
    let mut go_market = None;
    ui.weak(tr(
        "What the city makes itself, what it also buys and what it lacks (last week).",
    ));
    ui.add_space(4.0);
    for res in ResourceKind::ALL {
        let areas: Vec<(AreaId, u8, bool)> = view
            .areas
            .iter()
            .filter(|(_, a)| a.kind == AreaKind::Industry(res))
            .map(|(id, a)| (id, a.level, has_main(view, db, id)))
            .collect();
        let best = areas.iter().filter(|a| a.2).map(|a| a.1).max();
        let (raw, made) = chain_cargo_of(db, res);
        ui.horizontal(|ui| {
            let (rect, resp) =
                ui.allocate_exact_size(egui::vec2(200.0, 40.0), egui::Sense::click());
            let p = ui.painter();
            p.rect_filled(
                rect,
                5.0,
                if resp.hovered() {
                    egui::Color32::from_gray(52)
                } else {
                    egui::Color32::from_gray(38)
                },
            );
            let sq =
                egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 4.0), egui::vec2(32.0, 32.0));
            p.rect_filled(sq, 4.0, swatch(res));
            egui::Image::new(resource_icon(res).image()).paint_at(ui, sq.shrink(3.0));
            let p = ui.painter();
            p.text(
                rect.min + egui::vec2(42.0, 5.0),
                egui::Align2::LEFT_TOP,
                tr(resource_name(res)),
                egui::FontId::proportional(13.0),
                egui::Color32::from_gray(225),
            );
            let sub = if areas.is_empty() {
                tr("no area")
            } else if best.is_none() {
                tr("no main building")
            } else {
                trf!(
                    plural "{n} areas · level {level}",
                    n = areas.len(),
                    level = best.unwrap_or(1)
                )
            };
            p.text(
                rect.min + egui::vec2(42.0, 22.0),
                egui::Align2::LEFT_TOP,
                sub,
                egui::FontId::proportional(11.0),
                if best.is_some() {
                    egui::Color32::from_gray(160)
                } else {
                    COST
                },
            );
            if resp
                .on_hover_text(tr("Open the area on the Areas tab"))
                .clicked()
            {
                go_to_resource(view, res, tools, overlay, outbox, tab);
            }
            arrow(ui);
            for c in &raw {
                if status_chip(ui, db, *c, status[*c as usize], None, false).clicked() {
                    go_market = Some(*c);
                }
            }
            arrow(ui);
            for &(c, min) in &made {
                let closed = (best.unwrap_or(0) < min).then_some(min);
                if status_chip(ui, db, c, status[c as usize], closed, false).clicked() {
                    go_market = Some(c);
                }
            }
        });
        ui.add_space(2.0);
    }
    ui.separator();
    ui.strong(tr("Factories"));
    let factories: Vec<BuildingDefId> = db
        .buildings()
        .filter(|(_, d)| {
            d.industry
                .as_ref()
                .is_some_and(|s| s.role == IndustryRole::Factory)
        })
        .map(|(id, _)| id)
        .collect();
    ui.columns(2, |cols| {
        for (i, def) in factories.iter().enumerate() {
            factory_card(
                &mut cols[i % 2],
                view,
                db,
                *def,
                &status,
                tools,
                overlay,
                &mut go_market,
                false,
            );
        }
    });
    if let Some(c) = go_market {
        *focus = Some(c);
        *tab = IndustryTab::Market;
    }
}

/// A factory of the city: what it takes, what it makes and whether it can be placed now.
#[allow(clippy::too_many_arguments)]
pub(crate) fn factory_card(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    def: BuildingDefId,
    status: &[CargoStatus; 13],
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    go_market: &mut Option<CargoKind>,
    compact: bool,
) {
    let d = db.building(def);
    let Some(spec) = d.industry.as_ref() else {
        return;
    };
    let lock = factory_lock(view, db, def);
    let on = tools.active == ActiveTool::Place && tools.building.def == Some(def);
    let frame = egui::Frame::new()
        .fill(if on {
            egui::Color32::from_rgb(34, 52, 78)
        } else {
            egui::Color32::from_gray(36)
        })
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                def_image(ui, db, def, 36.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(tr(&d.name));
                        ui.weak(format_money(cp_core::Cost::from_units(d.cost)));
                    });
                    match &lock {
                        None => {
                            let at = best_industrial_area(view, db).and_then(|(id, l)| {
                                view.areas
                                    .get(id)
                                    .map(|a| (a.name.chars().take(12).collect::<String>(), l))
                            });
                            let text = match at {
                                Some((name, level)) => trf!(
                                    "✓ can build: {area} · level {level}",
                                    area = name,
                                    level = level
                                ),
                                None => tr("✓ can build"),
                            };
                            ui.label(egui::RichText::new(text).size(12.0).color(GOOD));
                        }
                        Some(l) => {
                            ui.label(egui::RichText::new(format!("▲ {l}")).size(12.0).color(COST));
                        }
                    }
                });
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 3.0);
                for &(c, _) in &spec.inputs {
                    if status_chip(ui, db, c, status[c as usize], None, compact).clicked() {
                        *go_market = Some(c);
                    }
                }
                ui.label(egui::RichText::new("▸").color(egui::Color32::from_gray(120)));
                for &(c, r) in &spec.outputs {
                    pill(
                        ui,
                        Some(cargo_color(db, c)),
                        &if compact {
                            format!("{} {:.0}", cargo_name(c), r)
                        } else {
                            format!("{} {:.0}/{}", cargo_name(c), r, tr("day"))
                        },
                        egui::Color32::from_gray(210),
                        None,
                    );
                }
            });
            // Where to put it and what must be bought.
            let buy: Vec<String> = spec
                .inputs
                .iter()
                .filter(|&&(c, _)| status[c as usize] == CargoStatus::None)
                .map(|&(c, _)| cargo_name(c))
                .collect();
            let line = if buy.is_empty() {
                String::new()
            } else {
                trf!("Would have to buy: {cargo}", cargo = buy.join(", "))
            };
            if !compact && !line.is_empty() {
                ui.weak(line);
            }
        });
    let resp = ui.interact(
        frame.response.rect,
        egui::Id::new(("factory_card", def.0)),
        egui::Sense::click(),
    );
    if resp.clicked() && lock.is_none() {
        crate::buildings::select(tools, def);
        show_overlays(overlay);
    }
}

// ----- Market -----------------------------------------------------------------------------------

fn state_colour(s: TradeState) -> egui::Color32 {
    match s {
        TradeState::Normal => egui::Color32::from_gray(150),
        TradeState::Shortage(_) => egui::Color32::from_rgb(230, 120, 90),
        TradeState::Glut(_) => egui::Color32::from_rgb(110, 170, 230),
    }
}

/// The price level of the last days with the list price dashed (index 100), over `rect`.
fn price_line(p: &egui::Painter, rect: egui::Rect, history: &[f32], colour: egui::Color32) {
    p.rect_filled(rect, 2.0, egui::Color32::from_gray(28));
    if history.len() < 2 {
        return;
    }
    let (lo, hi) = (40.0f32, 180.0f32);
    let y = |v: f32| {
        rect.bottom() - 2.0 - (rect.height() - 4.0) * ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
    };
    let norm = [
        egui::pos2(rect.left(), y(100.0)),
        egui::pos2(rect.right(), y(100.0)),
    ];
    p.extend(egui::Shape::dashed_line(
        &norm,
        egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
        3.0,
        3.0,
    ));
    let n = (history.len() - 1) as f32;
    let points = history
        .iter()
        .enumerate()
        .map(|(i, &v)| egui::pos2(rect.left() + rect.width() * i as f32 / n, y(v)))
        .collect();
    p.add(egui::Shape::line(points, egui::Stroke::new(1.5, colour)));
}

fn season_tile(ui: &mut egui::Ui, view: &SimView, db: &ContentDb) {
    let tick = view.tick;
    let s = season(tick);
    let left = crate::trade::season_days_left(tick);
    let done = SEASON_DAYS as f32 - left as f32 + 1.0;
    let tourists = db.balance().tourism.season_mul(tick);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(52.0, 52.0), egui::Sense::hover());
            let c = rect.center();
            let p = ui.painter();
            p.circle_stroke(
                c,
                24.0,
                egui::Stroke::new(4.0, egui::Color32::from_gray(48)),
            );
            let steps = 40;
            let upto = (steps as f32 * done / SEASON_DAYS as f32) as usize;
            let arc: Vec<egui::Pos2> = (0..=upto)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2
                        + std::f32::consts::TAU * i as f32 / steps as f32;
                    c + egui::vec2(a.cos(), a.sin()) * 24.0
                })
                .collect();
            if arc.len() >= 2 {
                p.add(egui::Shape::line(arc, egui::Stroke::new(4.0, BLUE)));
            }
            let inner = egui::Rect::from_center_size(c, egui::vec2(30.0, 30.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                crate::trade::season_icon(ui, tick, 30.0, tourists);
            });
            ui.vertical(|ui| {
                ui.strong(trf!(
                    plural "{season} · {n} days left",
                    n = left,
                    season = tr(crate::trade::season_name(s))
                ));
                ui.weak(crate::trade::season_hint(s));
            });
        });
    });
}

#[allow(clippy::too_many_arguments)]
pub fn market_tab(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    focus: &mut Option<CargoKind>,
) {
    let l = &view.industry_ledger;
    ui.columns(3, |cols| {
        season_tile(&mut cols[0], view, db);
        tile(
            &mut cols[1],
            Icon::Budget,
            &tr("Exported last week"),
            |ui| {
                ui.label(
                    egui::RichText::new(format_money(l.last_exports))
                        .size(18.0)
                        .strong()
                        .color(GOOD),
                );
            },
        );
        tile(
            &mut cols[2],
            Icon::Budget,
            &tr("Imported last week"),
            |ui| {
                ui.label(
                    egui::RichText::new(format_money(l.last_imports))
                        .size(18.0)
                        .strong()
                        .color(COST),
                );
            },
        );
    });
    ui.add_space(4.0);
    let rows = trade_rows(view);
    let (mine, others): (Vec<&TradeRow>, Vec<&TradeRow>) = rows.iter().partition(|r| r.in_city);
    ui.strong(tr("The city's cargo"));
    egui::ScrollArea::vertical()
        .id_salt("industry_market")
        .min_scrolled_height(470.0)
        .max_height(470.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.columns(2, |cols| {
                for (i, r) in mine.iter().enumerate() {
                    market_card(&mut cols[i % 2], view, db, r, *focus == Some(r.cargo));
                }
            });
            ui.add_space(4.0);
            ui.weak(tr("The city neither makes nor buys"));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 3.0);
                for r in &others {
                    let text = format!(
                        "{} {} {}",
                        cargo_name(r.cargo),
                        unit_price(r.export),
                        crate::trade::trend(&view.market, r.cargo)
                    );
                    pill(
                        ui,
                        Some(cargo_color(db, r.cargo)),
                        text.trim_end(),
                        egui::Color32::from_gray(160),
                        None,
                    );
                }
            });
        });
}

fn market_card(ui: &mut egui::Ui, view: &SimView, db: &ContentDb, r: &TradeRow, focused: bool) {
    let k = &db.balance().market;
    let size = egui::vec2(ui.available_width(), 112.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(rect, 6.0, egui::Color32::from_gray(36));
    if focused {
        p.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.5, BLUE),
            egui::StrokeKind::Inside,
        );
    }
    let colour = cargo_color(db, r.cargo);
    egui::Image::new(Icon::cargo(r.cargo).image()).paint_at(
        ui,
        egui::Rect::from_min_size(rect.min + egui::vec2(6.0, 5.0), egui::vec2(20.0, 20.0)),
    );
    p.text(
        rect.min + egui::vec2(32.0, 6.0),
        egui::Align2::LEFT_TOP,
        cargo_name(r.cargo),
        egui::FontId::proportional(14.0),
        egui::Color32::from_gray(230),
    );
    if r.state != TradeState::Normal {
        let word = match r.state {
            TradeState::Shortage(n) => trf!(plural "Shortage · {n} days left", n = n),
            TradeState::Glut(n) => trf!(plural "Glut · {n} days left", n = n),
            TradeState::Normal => String::new(),
        };
        p.text(
            egui::pos2(rect.right() - 8.0, rect.min.y + 7.0),
            egui::Align2::RIGHT_TOP,
            word,
            egui::FontId::proportional(11.0),
            state_colour(r.state),
        );
    }
    p.text(
        rect.min + egui::vec2(8.0, 26.0),
        egui::Align2::LEFT_TOP,
        unit_price(r.export),
        egui::FontId::proportional(20.0),
        egui::Color32::from_gray(235),
    );
    if r.export_change != 0 {
        let up = r.export_change > 0;
        p.text(
            rect.min + egui::vec2(8.0, 50.0),
            egui::Align2::LEFT_TOP,
            trf!(
                "{arrow} {pct}% against the usual",
                arrow = if up { "▲" } else { "▼" },
                pct = format!("{:+}", r.export_change)
            ),
            egui::FontId::proportional(11.0),
            if up { GOOD } else { COST },
        );
    }
    price_line(
        &p,
        egui::Rect::from_min_size(
            rect.min + egui::vec2(rect.width() - 150.0, 28.0),
            egui::vec2(142.0, 34.0),
        ),
        &r.history,
        colour,
    );
    // Out and in last week: units and money.
    let units_max = r.exported.max(r.imported).max(1) as f32;
    for (row, (label, units, price, tint)) in [
        (tr("out"), r.exported, r.export, GOOD),
        (tr("in"), r.imported, r.import, COST),
    ]
    .into_iter()
    .enumerate()
    {
        let y = rect.min.y + 70.0 + row as f32 * 18.0;
        p.text(
            egui::pos2(rect.min.x + 8.0, y),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::proportional(11.0),
            egui::Color32::from_gray(150),
        );
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 40.0, y + 3.0),
            egui::vec2(110.0, 7.0),
        );
        p.rect_filled(bar, 3.0, egui::Color32::from_gray(52));
        p.rect_filled(
            egui::Rect::from_min_size(
                bar.min,
                egui::vec2(bar.width() * units as f32 / units_max, 7.0),
            ),
            3.0,
            tint,
        );
        p.text(
            egui::pos2(rect.min.x + 158.0, y),
            egui::Align2::LEFT_TOP,
            format!(
                "{units} · ≈ {}",
                format_money(cp_core::Cost::scaled(price, f64::from(units)))
            ),
            egui::FontId::proportional(11.0),
            egui::Color32::from_gray(190),
        );
    }
    let ticks = view.tick.0 / TICKS_PER_DAY;
    let _ = ticks;
    let tip = match r.state {
        TradeState::Normal => tr("The price follows the market level of the last days."),
        TradeState::Shortage(_) => trf!(
            "Shortage: export price ×{xe}, import price ×{xi}; {fail}% of the orders from the outside fail.",
            xe = k.shortage_export_mul,
            xi = k.shortage_import_mul,
            fail = (k.shortage_fail * 100.0).round()
        ),
        TradeState::Glut(_) => trf!(
            "Glut: export price ×{xe}, import price ×{xi}.",
            xe = k.glut_export_mul,
            xi = k.glut_import_mul
        ),
    };
    let _ = MarketEventKind::Glut;
    resp.on_hover_text(tip);
}
