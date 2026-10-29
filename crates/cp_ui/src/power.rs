//! The Power window (T-335, `architecture/corruption-ui.md`): the grey deals, the corruption axis,
//! the institutions and the scandals in one window. A header that stays on every tab (trust,
//! corruption, institutions, the axis across the window, the scandal risk in five segments),
//! then the tabs Offers, Institutions and Scandals. Chips and icons say what a deal brings and
//! costs; the detail is in the tooltips. Mechanics: `architecture/corruption.md`.
//!
//! UI only: it reads the [`SimView`]; Accept, Decline and the tenders switch are commands.

use bevy_egui::egui;
use cp_content::{ContentDb, CorruptionKnobs, DealDef, InstitutionKind, PolicyKind};
use cp_district::DistrictCommand;
use cp_economy::corruption::{Edge, day_of};
use cp_economy::{CorruptionCommand, CorruptionState, Evidence};
use cp_locale::{tr, trf, trn};
use cp_sim::{SimCommand, SimView};
use cp_tools::{CommandOutbox, ToolState};

use crate::axis::{self, CLAN, GUESS, LAW, MIDDLE, SCALE};
use crate::icons::Icon;
use crate::offers::{self, Chip};
use crate::panels::UiState;

/// The tabs of the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PowerTab {
    #[default]
    Offers,
    Institutions,
    Scandals,
}

/// Scandal chances a day at which the risk scale lights its 1st to 5th segment. // tune later
pub const RISK_STEPS: [f32; 5] = [0.002, 0.005, 0.01, 0.02, 0.04];
/// The days the scandal timeline covers.
pub const TIMELINE_DAYS: u32 = 90;

const WIDTH: f32 = 600.0;
const GOOD: egui::Color32 = egui::Color32::from_rgb(120, 200, 120);
const COST: egui::Color32 = egui::Color32::from_rgb(235, 150, 70);
const PREVIEW: egui::Color32 = egui::Color32::from_rgb(240, 150, 60);
const SPARK_ICON: f32 = 20.0;

/// The colour of trust `t`: green, yellow below 50, red below 30.
pub fn trust_colour(t: f32) -> egui::Color32 {
    if t < 30.0 {
        egui::Color32::from_rgb(210, 70, 60)
    } else if t < 50.0 {
        egui::Color32::from_rgb(220, 190, 70)
    } else {
        egui::Color32::from_rgb(92, 184, 92)
    }
}

/// How many segments of the risk scale a daily scandal chance lights.
pub fn risk_segments(chance: f32) -> usize {
    RISK_STEPS.iter().filter(|&&t| chance >= t).count()
}

/// The state with the evidence of `def` added, as if the deal were accepted (the preview).
pub fn after_accepting(
    c: &CorruptionState,
    today: u32,
    deal: &str,
    def: &DealDef,
) -> CorruptionState {
    let mut next = c.clone();
    if def.evidence > 0.0 {
        next.evidence.push(Evidence {
            deal: deal.to_string(),
            accepted_day: today,
            weight: def.evidence,
        });
    }
    next
}

/// Where the axis stands after accepting `def` (the preview marker).
pub fn preview_axis(c: &CorruptionState, def: &DealDef) -> f32 {
    c.axis() + def.corruption
}

/// What declining does, in a few words (§3).
pub fn decline_note(c: &CorruptionState, k: &CorruptionKnobs) -> String {
    let mut d = 0.0;
    if c.edge(k) == Edge::Clan {
        d -= k.decline_trust;
    }
    if c.has(InstitutionKind::Press) {
        d += k.press_decline_trust;
    }
    if d > 0.0 {
        trf!("declining: trust +{n}", n = format!("{d:.0}"))
    } else if d < 0.0 {
        trf!("declining: trust {n}", n = format!("{d:.0}"))
    } else {
        tr("declining changes nothing")
    }
}

/// The days an offer still waits, and the colour of its ring.
fn ring_colour(left: u32) -> egui::Color32 {
    if left <= 3 {
        egui::Color32::from_rgb(210, 70, 60)
    } else if left <= 5 {
        egui::Color32::from_rgb(235, 150, 70)
    } else {
        egui::Color32::from_rgb(92, 184, 92)
    }
}

/// The Power window.
pub fn power_window(
    ctx: &egui::Context,
    st: &mut UiState,
    view: &SimView,
    tools: &mut ToolState,
    outbox: &mut CommandOutbox,
) {
    let UiState {
        power_open,
        power_tab,
        ..
    } = st;
    crate::frame::window("power", Icon::WindowPower, &tr("Mayor's power"), power_open).show(
        ctx,
        |ui| {
            let Some(db) = view.content.as_deref() else {
                return;
            };
            ui.set_width(WIDTH);
            let c = &view.corruption;
            let today = day_of(view.tick);
            let preview_id = egui::Id::new("power_preview");
            let preview: Option<String> = ui.data(|d| d.get_temp(preview_id));
            let preview = preview.and_then(|id| db.deal(&id));
            header(ui, view, db, preview, today);
            ui.add_space(4.0);
            tabs(ui, power_tab, c.inbox.len());
            ui.separator();
            let mut hovered = None;
            match *power_tab {
                PowerTab::Offers => hovered = offers_tab(ui, view, db, outbox),
                PowerTab::Institutions => institutions_tab(ui, view, db, tools, outbox),
                PowerTab::Scandals => scandals_tab(ui, view, db, today),
            }
            ui.data_mut(|d| d.insert_temp(preview_id, hovered));
        },
    );
}

fn tabs(ui: &mut egui::Ui, tab: &mut PowerTab, offers: usize) {
    ui.horizontal(|ui| {
        for (t, label) in [
            (PowerTab::Offers, tr("Offers")),
            (PowerTab::Institutions, tr("Institutions")),
            (PowerTab::Scandals, tr("Scandals")),
        ] {
            let label = if t == PowerTab::Offers && offers > 0 {
                format!("{label} ({offers})")
            } else {
                label
            };
            if ui.selectable_label(*tab == t, label).clicked() {
                *tab = t;
            }
        }
    });
}

// ----- chips -------------------------------------------------------------------------------

/// A chip: icon and text on a tinted pill.
pub fn chip(ui: &mut egui::Ui, icon: Icon, text: &str, tint: Option<bool>) -> egui::Response {
    let (fill, ink) = match tint {
        Some(true) => (egui::Color32::from_rgb(34, 62, 40), GOOD),
        Some(false) => (egui::Color32::from_rgb(72, 48, 26), COST),
        None => (egui::Color32::from_gray(44), egui::Color32::from_gray(210)),
    };
    egui::Frame::new()
        .fill(fill)
        .corner_radius(9.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                icon.show(ui, 14.0);
                ui.label(egui::RichText::new(text).size(12.0).color(ink));
            });
        })
        .response
}

fn chip_row(ui: &mut egui::Ui, chips: &[Chip]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 3.0);
        for c in chips {
            chip(ui, c.icon, &c.text, Some(c.good));
        }
    });
}

// ----- header ------------------------------------------------------------------------------

/// A bar `0..=1` with a tick at `tick`.
pub(crate) fn bar(ui: &mut egui::Ui, fill: f32, tick: Option<f32>, colour: egui::Color32) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, egui::Color32::from_gray(40));
    let w = rect.width() * fill.clamp(0.0, 1.0);
    p.rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(w, rect.height())),
        3.0,
        colour,
    );
    if let Some(t) = tick {
        let x = rect.left() + rect.width() * t.clamp(0.0, 1.0);
        p.line_segment(
            [
                egui::pos2(x, rect.top() - 2.0),
                egui::pos2(x, rect.bottom() + 2.0),
            ],
            egui::Stroke::new(1.5, egui::Color32::WHITE),
        );
    }
}

pub(crate) fn tile(
    ui: &mut egui::Ui,
    icon: Icon,
    title: &str,
    f: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    egui::Frame::group(ui.style())
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                icon.show(ui, SPARK_ICON);
                ui.weak(title);
            });
            f(ui);
        })
        .response
}

/// Lines `0..=100` over the same box, one colour each.
pub fn lines(ui: &mut egui::Ui, series: &[(Vec<f32>, egui::Color32)], size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, egui::Color32::from_gray(28));
    for (values, colour) in series {
        if values.len() < 2 {
            continue;
        }
        let n = (values.len() - 1) as f32;
        let points = values
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                egui::pos2(
                    rect.left() + rect.width() * i as f32 / n,
                    rect.bottom() - 2.0 - (rect.height() - 4.0) * (v / 100.0).clamp(0.0, 1.0),
                )
            })
            .collect();
        p.add(egui::Shape::line(points, egui::Stroke::new(1.5, *colour)));
    }
}

const TRUST_LINE: egui::Color32 = egui::Color32::from_rgb(92, 184, 92);
const CORRUPTION_LINE: egui::Color32 = egui::Color32::from_rgb(235, 150, 70);
const INSTITUTIONS_LINE: egui::Color32 = egui::Color32::from_rgb(90, 160, 230);

fn header(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    preview: Option<&DealDef>,
    today: u32,
) {
    let k = &db.balance().corruption;
    let c = &view.corruption;
    let exact = c.sees_exactly();
    ui.columns(3, |cols| {
        let t = c.trust_now(k);
        let ceiling = c.trust_ceiling(k);
        tile(&mut cols[0], Icon::Trust, &tr("Trust"), |ui| {
            ui.strong(format!("{} / {}", axis::num(t), axis::num(ceiling)));
            bar(ui, t / 100.0, Some(ceiling / 100.0), trust_colour(t));
        })
        .on_hover_text(trf!(
            "Trust in the mayor: {n} (at most {m})",
            n = axis::num(t),
            m = axis::num(ceiling)
        ));
        tile(&mut cols[1], Icon::Corruption, &tr("Corruption"), |ui| {
            if exact {
                ui.strong(axis::num(c.corruption));
                bar(ui, c.corruption / 100.0, None, CORRUPTION_LINE);
            } else {
                ui.label(
                    egui::RichText::new(format!("“{}”", offers::rumour(c.corruption))).italics(),
                );
            }
        })
        .on_hover_ui(|ui| {
            for l in offers::standing(c) {
                ui.label(l);
            }
        });
        tile(&mut cols[2], Icon::WindowPower, &tr("Institutions"), |ui| {
            ui.strong(axis::num(c.institutions));
            let mut series = vec![
                (
                    view.city.history.iter().map(|h| h.institutions).collect(),
                    INSTITUTIONS_LINE,
                ),
                (
                    view.city.history.iter().map(|h| h.trust).collect(),
                    TRUST_LINE,
                ),
            ];
            if exact {
                series.push((
                    view.city.history.iter().map(|h| h.corruption).collect(),
                    CORRUPTION_LINE,
                ));
            }
            lines(ui, &series, egui::vec2(ui.available_width(), 18.0));
        })
        .on_hover_text(tr("Institutions, trust and corruption by the hour"));
    });
    ui.add_space(4.0);
    axis_bar(ui, c, k, preview);
    ui.add_space(2.0);
    risk_row(ui, view, db, preview, today);
}

/// The axis across the window, the city's place on it and, when `preview` is a deal under the
/// pointer, where accepting it would put the city.
fn axis_bar(
    ui: &mut egui::Ui,
    c: &CorruptionState,
    k: &CorruptionKnobs,
    preview: Option<&DealDef>,
) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(w, 16.0 + 22.0 + 4.0 + 14.0 + 6.0),
        egui::Sense::hover(),
    );
    let top = egui::Rect::from_min_size(rect.min, egui::vec2(w, 14.0));
    let bar = egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 16.0), egui::vec2(w, 22.0));
    // x of an axis value: +SCALE (clan rule) on the left.
    let x = |a: f32| bar.center().x - a.clamp(-SCALE, SCALE) / SCALE * w * 0.5;
    let p = ui.painter().clone();
    let part = |from: f32, to: f32, col: egui::Color32| {
        p.rect_filled(
            egui::Rect::from_x_y_ranges(x(from)..=x(to), bar.y_range()),
            0.0,
            col,
        );
    };
    part(SCALE, k.edge, CLAN);
    part(k.edge, -k.edge, MIDDLE);
    part(-k.edge, -SCALE, LAW);
    // The edges, with their ticks.
    for e in [k.edge, -k.edge] {
        p.line_segment(
            [
                egui::pos2(x(e), bar.top() - 3.0),
                egui::pos2(x(e), bar.bottom() + 3.0),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_gray(200)),
        );
    }
    let font = egui::FontId::proportional(12.0);
    let ink = egui::Color32::from_gray(200);
    p.text(
        egui::pos2(x(k.edge), bar.bottom() + 4.0),
        egui::Align2::CENTER_TOP,
        trf!("edge {n}", n = format!("+{}", axis::num(k.edge))),
        font.clone(),
        ink,
    );
    p.text(
        egui::pos2(x(-k.edge), bar.bottom() + 4.0),
        egui::Align2::CENTER_TOP,
        trf!("edge {n}", n = format!("−{}", axis::num(k.edge))),
        font.clone(),
        ink,
    );
    p.text(
        egui::pos2(bar.center().x, bar.bottom() + 4.0),
        egui::Align2::CENTER_TOP,
        axis::edge_text(c.edge(k)),
        font.clone(),
        egui::Color32::WHITE,
    );
    // The edge captions, each with the plus and the minus of the edge in a tooltip.
    let icon_at = |left: bool, icon: Icon| {
        let r = if left {
            egui::Rect::from_min_size(top.min, egui::vec2(14.0, 14.0))
        } else {
            egui::Rect::from_min_size(
                egui::pos2(top.max.x - 14.0, top.min.y),
                egui::vec2(14.0, 14.0),
            )
        };
        egui::Image::new(icon.image()).paint_at(ui, r);
    };
    icon_at(true, Icon::EdgeClan);
    icon_at(false, Icon::EdgeLaw);
    p.text(
        top.left_center() + egui::vec2(18.0, 0.0),
        egui::Align2::LEFT_CENTER,
        tr("Clan rule"),
        font.clone(),
        CLAN,
    );
    p.text(
        top.right_center() - egui::vec2(18.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        tr("Rule of law"),
        font,
        LAW,
    );
    let clan_tip = egui::Rect::from_min_size(top.min, egui::vec2(140.0, 14.0));
    let law_tip = egui::Rect::from_min_size(
        egui::pos2(top.max.x - 140.0, top.min.y),
        egui::vec2(140.0, 14.0),
    );
    let tip = |ui: &egui::Ui, r: egui::Rect, id: &str, lines: [String; 2], col| {
        ui.interact(r, ui.id().with(id), egui::Sense::hover())
            .on_hover_ui(|ui| {
                for l in lines {
                    ui.colored_label(col, l);
                }
            });
    };
    tip(ui, clan_tip, "clan_tip", axis::clan_lines(k), CLAN);
    tip(ui, law_tip, "law_tip", axis::law_lines(k), LAW);
    // The city.
    let a = c.axis();
    let target = preview.map(|d| preview_axis(c, d));
    if c.sees_exactly() {
        p.line_segment(
            [
                egui::pos2(x(a), top.bottom()),
                egui::pos2(x(a), bar.bottom() + 2.0),
            ],
            egui::Stroke::new(3.0, egui::Color32::WHITE),
        );
        let hit =
            egui::Rect::from_center_size(egui::pos2(x(a), bar.center().y), egui::vec2(10.0, 30.0));
        ui.interact(hit, ui.id().with("marker"), egui::Sense::hover())
            .on_hover_text(trf!(
                "C {c} − I {i} = {a} (edges at ±{e})",
                c = axis::num(c.corruption),
                i = axis::num(c.institutions),
                a = axis::num(a),
                e = axis::num(k.edge)
            ));
        if let Some(t) = target {
            p.line_segment(
                [
                    egui::pos2(x(t), top.bottom()),
                    egui::pos2(x(t), bar.bottom() + 2.0),
                ],
                egui::Stroke::new(3.0, PREVIEW),
            );
        }
    } else {
        let band = |centre: f32, col: egui::Color32| {
            let r =
                egui::Rect::from_x_y_ranges(x(centre + GUESS)..=x(centre - GUESS), bar.y_range());
            p.rect_filled(r, 2.0, col);
        };
        band(a, egui::Color32::WHITE.gamma_multiply(0.45));
        if let Some(t) = target {
            band(t, PREVIEW.gamma_multiply(0.5));
        }
        let hit = egui::Rect::from_x_y_ranges(x(a + GUESS)..=x(a - GUESS), bar.y_range());
        ui.interact(hit, ui.id().with("guess"), egui::Sense::hover())
            .on_hover_text(tr(
                "Without an audit chamber the city's place is a guess (the band).",
            ));
    }
}

const SEGMENT_COLOURS: [egui::Color32; 5] = [
    egui::Color32::from_rgb(110, 190, 90),
    egui::Color32::from_rgb(170, 190, 70),
    egui::Color32::from_rgb(225, 180, 60),
    egui::Color32::from_rgb(230, 125, 50),
    egui::Color32::from_rgb(210, 65, 55),
];

/// "Scandal risk": five segments, and the chips of the evidence kept and the stations bought.
fn risk_row(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    preview: Option<&DealDef>,
    today: u32,
) {
    let k = &db.balance().corruption;
    let c = &view.corruption;
    let chance = c.scandal_chance(k);
    let now = risk_segments(chance);
    let after = preview.map_or(now, |d| {
        risk_segments(after_accepting(c, today, &d.id, d).scandal_chance(k))
    });
    let paused = today < c.scandal_cooldown_until;
    ui.horizontal(|ui| {
        Icon::Scandal.show(ui, 18.0);
        let label = ui.label(tr("Scandal risk"));
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(5.0 * 34.0, 14.0), egui::Sense::hover());
        let p = ui.painter();
        for (i, col) in SEGMENT_COLOURS.iter().copied().enumerate() {
            let seg = egui::Rect::from_min_size(
                rect.min + egui::vec2(i as f32 * 34.0, 0.0),
                egui::vec2(30.0, 14.0),
            );
            if i < now {
                p.rect_filled(seg, 2.0, if paused { col.gamma_multiply(0.4) } else { col });
            } else {
                p.rect_filled(seg, 2.0, egui::Color32::from_gray(40));
                if i < after {
                    // What accepting would add: lighter, with a dashed top edge.
                    p.rect_filled(seg, 2.0, col.gamma_multiply(0.35));
                    let mut x = seg.left();
                    while x < seg.right() {
                        p.line_segment(
                            [
                                egui::pos2(x, seg.top()),
                                egui::pos2((x + 4.0).min(seg.right()), seg.top()),
                            ],
                            egui::Stroke::new(1.5, col),
                        );
                        x += 7.0;
                    }
                }
            }
        }
        let tip = |ui: &mut egui::Ui| risk_tip(ui, c, k, chance, paused);
        resp.on_hover_ui(tip);
        label.on_hover_ui(|ui| risk_tip(ui, c, k, chance, paused));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let exact = c.sees_exactly();
            let captured = if exact {
                c.visible_captured().len().to_string()
            } else {
                "?".to_string()
            };
            chip(ui, Icon::Captured, &captured, None).on_hover_text(axis::police_terms(
                k,
                view.budgets.get(cp_content::ServiceKind::Police),
            ));
            chip(
                ui,
                Icon::Evidence,
                &if exact {
                    c.evidence.len().to_string()
                } else {
                    "?".to_string()
                },
                None,
            )
            .on_hover_text(tr("Evidence of your deals that may come out"));
        });
    });
}

/// The risk, broken into its three bands: the weight of the evidence, how findable it is and the
/// roof the bought stations give.
fn risk_tip(
    ui: &mut egui::Ui,
    c: &CorruptionState,
    k: &CorruptionKnobs,
    chance: f32,
    paused: bool,
) {
    let band = |ui: &mut egui::Ui, label: String, v: f32, max: f32, col: egui::Color32| {
        ui.horizontal(|ui| {
            ui.label(label);
            let (r, _) = ui.allocate_exact_size(egui::vec2(90.0, 8.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(r, 2.0, egui::Color32::from_gray(40));
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    r.min,
                    egui::vec2(r.width() * (v / max).clamp(0.0, 1.0), 8.0),
                ),
                2.0,
                col,
            );
        });
    };
    band(ui, tr("Evidence"), c.evidence_weight(), 8.0, COST);
    band(
        ui,
        tr("Detection"),
        c.scandal_exposure(k).max(0.0),
        1.5,
        CLAN,
    );
    band(ui, tr("Roof"), c.roof(k), 1.0, LAW);
    if c.sees_exactly() {
        ui.label(trf!(
            "About {pct}% a day",
            pct = format!("{:.1}", chance * 100.0)
        ));
    } else {
        ui.weak(tr("Roughly: an audit chamber shows the numbers."));
    }
    if paused {
        ui.weak(trf!(
            "No scandal before day {n}",
            n = c.scandal_cooldown_until.to_string()
        ));
    }
}

// ----- tabs --------------------------------------------------------------------------------

/// The Offers tab; returns the deal whose Accept button is under the pointer (the preview).
fn offers_tab(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    outbox: &mut CommandOutbox,
) -> Option<String> {
    let k = &db.balance().corruption;
    let c = &view.corruption;
    let today = day_of(view.tick);
    let mut hovered = None;
    if c.inbox.is_empty() {
        ui.weak(trf!(
            "No offers. Grey deals start to come at {n} residents; the game does not pause for them.",
            n = format!("{:.0}", k.deals_population)
        ));
    }
    for offer in &c.inbox {
        let Some(def) = db.deal(&offer.deal) else {
            continue;
        };
        let left = offer.expires_day.saturating_sub(today);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                offers::deal_icon(def).show(ui, 32.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(tr(&def.name));
                        Icon::Info.show(ui, 14.0).on_hover_text(tr(&def.text));
                    });
                    if let Some(t) = offers::target_text(view, db, offer) {
                        ui.weak(egui::RichText::new(t).size(11.0));
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ring(ui, left, u32::from(def.answer_days));
                });
            });
            chip_row(ui, &offers::chips(def));
            ui.horizontal(|ui| {
                let accept = ui.button(tr("Accept"));
                if accept.hovered() {
                    hovered = Some(offer.deal.clone());
                }
                if accept.clicked() {
                    outbox.push(SimCommand::Corruption(CorruptionCommand::Accept {
                        deal: offer.deal.clone(),
                    }));
                }
                if ui.button(tr("Decline")).clicked() {
                    outbox.push(SimCommand::Corruption(CorruptionCommand::Decline {
                        deal: offer.deal.clone(),
                    }));
                }
                ui.weak(decline_note(c, k));
            })
            .response
            .on_hover_text(offers::terms(def).join("\n"));
        });
    }
    hovered
}

/// The countdown ring: the days left in the middle, the arc the share of the time left.
fn ring(ui: &mut egui::Ui, left: u32, total: u32) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
    let p = ui.painter();
    let colour = ring_colour(left);
    p.circle_stroke(
        rect.center(),
        15.0,
        egui::Stroke::new(3.0, egui::Color32::from_gray(50)),
    );
    let share = if total == 0 {
        1.0
    } else {
        (left as f32 / total as f32).clamp(0.0, 1.0)
    };
    let steps = 32;
    let points: Vec<egui::Pos2> = (0..=(steps as f32 * share) as usize)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * i as f32 / steps as f32;
            rect.center() + 15.0 * egui::vec2(a.cos(), a.sin())
        })
        .collect();
    if points.len() >= 2 {
        p.add(egui::Shape::line(points, egui::Stroke::new(3.0, colour)));
    }
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        left.to_string(),
        egui::FontId::proportional(13.0),
        colour,
    );
    resp.on_hover_text(trn("Answer within {n} days", i64::from(left)));
}

fn institutions_tab(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    tools: &mut ToolState,
    outbox: &mut CommandOutbox,
) {
    let k = &db.balance().corruption;
    let c = &view.corruption;
    ui.columns(3, |cols| {
        for (col, kind) in cols.iter_mut().zip(InstitutionKind::ALL) {
            let standing = c.has(kind);
            let status = view
                .uniques
                .iter()
                .find(|u| db.building(u.def).institution == Some(kind));
            let can_build = !standing && status.is_some_and(|u| u.unlocked);
            let building = db.buildings().find(|(_, d)| d.institution == Some(kind));
            let resp = egui::Frame::group(col.style())
                .show(col, |ui| {
                    ui.set_width(ui.available_width());
                    if let Some((_, d)) = building {
                        let stem = if d.icon.is_empty() { &d.id } else { &d.icon };
                        ui.add(
                            egui::Image::new(crate::icons::def_icon(stem))
                                .fit_to_exact_size(egui::vec2(44.0, 44.0)),
                        );
                    }
                    ui.strong(axis::institution_name(db, kind));
                    let text = if standing {
                        tr("stands")
                    } else if can_build {
                        tr("can be built")
                    } else {
                        tr("not open yet")
                    };
                    let colour = if standing {
                        GOOD
                    } else if can_build {
                        egui::Color32::from_gray(220)
                    } else {
                        egui::Color32::from_gray(130)
                    };
                    ui.colored_label(colour, text);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(4.0, 3.0);
                        chip(
                            ui,
                            Icon::WindowPower,
                            &format!("I +{}", axis::num(axis::institution_gain(k, kind))),
                            Some(true),
                        );
                        chip(
                            ui,
                            Icon::TermRisk,
                            &format!("+{}%", axis::pct(axis::institution_exposure(k, kind))),
                            Some(false),
                        );
                    });
                })
                .response;
            let resp = if can_build {
                resp.interact(egui::Sense::click())
            } else {
                resp
            };
            let resp = resp.on_hover_ui(|ui| {
                let [now, later, risk] = axis::institution_terms(k, kind);
                for (icon, line) in [
                    (Icon::TermNow, now),
                    (Icon::TermLater, later),
                    (Icon::TermRisk, risk),
                ] {
                    ui.horizontal(|ui| {
                        icon.show(ui, 14.0);
                        ui.label(line);
                    });
                }
            });
            if can_build
                && resp.clicked()
                && let Some(u) = status
            {
                crate::buildings::select(tools, u.def);
            }
        }
    });
    ui.add_space(4.0);
    tenders_row(ui, view, db, outbox);
}

/// The Transparent tenders row, with the switch (the same command as the City policies window).
fn tenders_row(ui: &mut egui::Ui, view: &SimView, db: &ContentDb, outbox: &mut CommandOutbox) {
    let k = &db.balance().corruption;
    let Some(def) = db.policy(PolicyKind::TransparentTenders) else {
        return;
    };
    let on = crate::policies::is_on(view, None, PolicyKind::TransparentTenders);
    let lock = crate::policies::lock_text(view, db, PolicyKind::TransparentTenders);
    ui.horizontal(|ui| {
        ui.add(
            egui::Image::new(crate::icons::def_icon(&def.icon))
                .fit_to_exact_size(egui::vec2(28.0, 28.0)),
        );
        ui.strong(tr(&def.name));
        chip(
            ui,
            Icon::WindowPower,
            &format!("I +{}", axis::num(k.i_tenders)),
            Some(true),
        );
        chip(
            ui,
            Icon::DealBuild,
            &format!("+{}%", axis::pct(k.tenders_build)),
            Some(false),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut value = on;
            let r = ui
                .add_enabled(lock.is_none(), egui::Checkbox::without_text(&mut value))
                .on_hover_text(lock.unwrap_or_else(|| {
                    crate::policies::effect_text(PolicyKind::TransparentTenders, def.value)
                }));
            if r.changed() {
                outbox.push(SimCommand::District(DistrictCommand::SetPolicy {
                    district: None,
                    policy: PolicyKind::TransparentTenders,
                    on: value,
                }));
            }
        });
    });
}

fn scandals_tab(ui: &mut egui::Ui, view: &SimView, db: &ContentDb, today: u32) {
    let c = &view.corruption;
    // The last 90 days, one stroke a scandal, as tall as the trust it cost.
    let first = today.saturating_sub(TIMELINE_DAYS);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, egui::Color32::from_gray(28));
    let base = rect.bottom() - 4.0;
    p.line_segment(
        [
            egui::pos2(rect.left() + 4.0, base),
            egui::pos2(rect.right() - 4.0, base),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
    );
    for (i, s) in c.scandals.iter().enumerate() {
        if s.day < first {
            continue;
        }
        let x = rect.left()
            + 8.0
            + (rect.width() - 16.0) * (s.day - first) as f32 / TIMELINE_DAYS as f32;
        let h = 8.0 + 28.0 * (s.trust / 100.0).clamp(0.0, 1.0);
        p.line_segment(
            [egui::pos2(x, base), egui::pos2(x, base - h)],
            egui::Stroke::new(3.0, CLAN),
        );
        p.circle_filled(egui::pos2(x, base - h), 4.0, CLAN);
        let hit =
            egui::Rect::from_center_size(egui::pos2(x, base - h * 0.5), egui::vec2(12.0, h + 8.0));
        ui.interact(hit, ui.id().with(("scandal", i)), egui::Sense::hover())
            .on_hover_text(offers::scandal_text(db, s));
    }
    ui.weak(trf!("The last {n} days", n = TIMELINE_DAYS.to_string()));
    if c.scandals.is_empty() {
        ui.label(tr("No scandal yet."));
    }
    ui.separator();
    ui.strong(tr("Evidence"));
    if c.evidence.is_empty() {
        ui.weak(tr("No evidence: nothing can come out."));
    }
    for e in &c.evidence {
        ui.horizontal(|ui| {
            Icon::Evidence.show(ui, 16.0);
            ui.label(
                db.deal(&e.deal)
                    .map_or_else(|| e.deal.clone(), |d| tr(&d.name)),
            );
            ui.weak(trf!("day {n}", n = (e.accepted_day).to_string()));
            ui.label(offers::evidence_dots(e.weight));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{click, frame_texts, input};
    use cp_economy::Offer;
    use std::sync::Arc;

    fn db() -> Arc<ContentDb> {
        Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        )
    }

    /// A view with the state `f` made.
    fn view_with(f: impl FnOnce(&mut CorruptionState)) -> SimView {
        let mut sim = cp_sim::SimRunner::new(1);
        sim.set_content(db());
        f(&mut sim.world_mut().resource_mut::<CorruptionState>());
        sim.view()
    }

    /// Draws the window twice (it lays itself out on its first frame) and returns the texts.
    struct Rig {
        ctx: egui::Context,
        st: UiState,
        tools: ToolState,
        outbox: CommandOutbox,
        view: SimView,
    }

    impl Rig {
        fn new(view: SimView) -> Self {
            let st = UiState {
                power_open: true,
                ..UiState::default()
            };
            let mut rig = Self {
                ctx: egui::Context::default(),
                st,
                tools: ToolState::default(),
                outbox: CommandOutbox::default(),
                view,
            };
            rig.frame(vec![]);
            rig
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<(String, egui::Pos2)> {
            let Self {
                ctx,
                st,
                tools,
                outbox,
                view,
            } = self;
            frame_texts(ctx, input(0.0, events), |ui| {
                power_window(ui.ctx(), st, view, tools, outbox)
            })
        }

        fn click_text(&mut self, label: &str) {
            let texts = self.frame(vec![]);
            let at = texts
                .iter()
                .find(|(t, _)| t == label)
                .map(|(_, p)| *p + egui::vec2(4.0, 4.0))
                .unwrap_or_else(|| panic!("no {label:?} in {texts:?}"));
            let [down, up] = click(at);
            self.frame(down);
            self.frame(up);
        }
    }

    fn with_offer(c: &mut CorruptionState) {
        c.inbox.push(Offer {
            deal: "land".into(),
            target: None,
            expires_day: 5,
        });
    }

    #[test]
    fn accept_sends_the_corruption_command() {
        let mut rig = Rig::new(view_with(with_offer));
        rig.frame(vec![]);
        let texts = rig.frame(vec![]);
        assert!(
            texts.iter().any(|(t, _)| t == "Land for friends"),
            "{texts:?}"
        );
        rig.click_text("Accept");
        assert_eq!(
            rig.outbox.iter().cloned().collect::<Vec<_>>(),
            [SimCommand::Corruption(CorruptionCommand::Accept {
                deal: "land".into()
            })]
        );
    }

    #[test]
    fn the_institutions_tab_names_the_three_institutions() {
        let mut rig = Rig::new(view_with(|_| {}));
        let before = rig.frame(vec![]);
        assert!(!before.iter().any(|(t, _)| t == "Audit chamber"));
        // The tab and the header tile are both "Institutions": the tab is the last one.
        let at = before
            .iter()
            .rfind(|(t, _)| t == "Institutions")
            .map(|(_, p)| *p + egui::vec2(4.0, 4.0))
            .expect("an Institutions tab");
        let [down, up] = click(at);
        rig.frame(down);
        rig.frame(up);
        assert_eq!(rig.st.power_tab, PowerTab::Institutions);
        let texts = rig.frame(vec![]);
        for name in [
            "Audit chamber",
            "Free press",
            "Prosecutor's office and court",
        ] {
            assert!(texts.iter().any(|(t, _)| t == name), "{name}: {texts:?}");
        }
    }

    #[test]
    fn the_tenders_switch_sends_the_policy_command() {
        let mut rig = Rig::new(view_with(|_| {}));
        rig.st.power_tab = PowerTab::Institutions;
        rig.frame(vec![]);
        let texts = rig.frame(vec![]);
        let (_, name) = texts
            .iter()
            .find(|(t, _)| t == "Transparent tenders")
            .expect("the tenders row");
        // The row is WIDTH wide from the icon (28 + a gap) before the name; the switch is at its end.
        let at = egui::pos2(name.x - 36.0 + WIDTH - 10.0, name.y + 7.0);
        let [down, up] = click(at);
        rig.frame(down);
        rig.frame(up);
        assert_eq!(
            rig.outbox.iter().cloned().collect::<Vec<_>>(),
            [SimCommand::District(DistrictCommand::SetPolicy {
                district: None,
                policy: PolicyKind::TransparentTenders,
                on: true
            })]
        );
    }

    #[test]
    fn the_old_names_open_the_power_window() {
        use crate::panels::UiWindow;
        for name in ["offers", "axis", "power"] {
            let mut st = UiState::default();
            st.open_window(UiWindow::parse(name).unwrap());
            assert!(st.power_open, "--open {name}");
        }
        let mut st = UiState::default();
        st.open_window(UiWindow::parse("offers").unwrap());
        assert_eq!(st.power_tab, PowerTab::Offers);
        st.open_window(UiWindow::parse("scandals").unwrap());
        assert_eq!(st.power_tab, PowerTab::Scandals);
    }

    #[test]
    fn the_risk_scale_lights_segments_by_the_steps() {
        assert_eq!(risk_segments(0.0), 0);
        assert_eq!(risk_segments(0.002), 1);
        assert_eq!(risk_segments(0.0099), 2);
        assert_eq!(risk_segments(0.01), 3);
        assert_eq!(risk_segments(0.5), 5);
    }

    #[test]
    fn accepting_moves_the_axis_marker_and_adds_to_the_risk() {
        let db = db();
        let k = &db.balance().corruption;
        let c = CorruptionState {
            corruption: 10.0,
            institutions: 4.0,
            ..CorruptionState::default()
        };
        let casino = db.deal("casino").unwrap();
        assert_eq!(preview_axis(&c, casino), c.axis() + casino.corruption);
        let after = after_accepting(&c, 3, "casino", casino);
        assert!(after.scandal_chance(k) > c.scandal_chance(k));
        assert_eq!(c.evidence.len(), 0, "the real state is untouched");
    }

    #[test]
    fn declining_says_what_it_gives() {
        let k = CorruptionKnobs::default();
        let mut c = CorruptionState::default();
        assert_eq!(decline_note(&c, &k), "declining changes nothing");
        c.present = InstitutionKind::Press.bit();
        assert!(decline_note(&c, &k).starts_with("declining: trust +"));
        c.present = 0;
        c.corruption = 80.0;
        assert!(decline_note(&c, &k).starts_with("declining: trust -"));
    }
}
