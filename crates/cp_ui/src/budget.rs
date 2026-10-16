//! The money side of the UI (`architecture/system-design.md` §13.6, §11): the balance with its
//! debt warning, the RCIO demand bars, and the budget window with tax and service-budget
//! sliders.
//!
//! The UI only reads [`SimView`] and sends `SetTaxRate` / `SetBudget` through the outbox. A
//! slider keeps a *draft* while it is being dragged and sends **one** command when the
//! pointer is released (or on a click or key press), so a drag does not flood the command log.

use std::ops::RangeInclusive;

use bevy_egui::egui;
use cp_content::{ServiceKind, ZoneType};
use cp_core::Cost;
use cp_core::treasury::DEBT_FLOOR;
use cp_economy::state::{BUDGET_MAX, BUDGET_MIN, BUDGETED, TAX_MAX, TAX_MIN, TAXED_ZONES};
use cp_economy::{EconomyCommand, Loan};
use cp_sim::{SimCommand, SimView};
use cp_tools::{CommandOutbox, format_money, zone_name};

/// Demand values run from `-DEMAND_RANGE` (nobody wants it) to `+DEMAND_RANGE`.
pub const DEMAND_RANGE: f32 = 100.0;
/// Headroom above the debt floor under which the warning turns strong: 5 000.00.
pub const NEAR_FLOOR_MARGIN: Cost = Cost(500_000);

/// How worried the balance should look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebtLevel {
    Fine,
    /// Below zero.
    Debt,
    /// Within [`NEAR_FLOOR_MARGIN`] of the −20 000.00 floor (or below it): construction is
    /// about to be refused.
    NearFloor,
}

pub fn debt_level(balance: Cost) -> DebtLevel {
    if balance.0 <= DEBT_FLOOR.0 + NEAR_FLOOR_MARGIN.0 {
        DebtLevel::NearFloor
    } else if balance.0 < 0 {
        DebtLevel::Debt
    } else {
        DebtLevel::Fine
    }
}

/// The warning line for `balance`, if any.
pub fn debt_warning(balance: Cost) -> Option<String> {
    match debt_level(balance) {
        DebtLevel::Fine => None,
        DebtLevel::Debt => Some(format!(
            "In debt ({}). Building stops at {}.",
            format_money(balance),
            format_money(DEBT_FLOOR)
        )),
        DebtLevel::NearFloor => Some(format!(
            "Almost at the debt limit ({} of {})! Building will be refused.",
            format_money(balance),
            format_money(DEBT_FLOOR)
        )),
    }
}

fn debt_colour(level: DebtLevel) -> egui::Color32 {
    match level {
        DebtLevel::Fine => egui::Color32::LIGHT_GREEN,
        DebtLevel::Debt => egui::Color32::from_rgb(240, 180, 60),
        DebtLevel::NearFloor => egui::Color32::from_rgb(240, 70, 60),
    }
}

/// The balance label of the top bar, coloured by [`DebtLevel`].
pub fn money_label(ui: &mut egui::Ui, view: &SimView) -> egui::Response {
    let b = view.treasury.balance;
    let text =
        egui::RichText::new(format!("💰 {}", format_money(b))).color(debt_colour(debt_level(b)));
    let r = ui.label(text);
    match debt_warning(b) {
        Some(w) => r.on_hover_text(w),
        None => r,
    }
}

/// The treasury's change per week as the last day shows it (T-179): that day's tax income minus
/// its upkeep, times seven, minus the weekly loan payments. `None` before the first day was
/// settled (or after a load, until the next one).
pub fn weekly_trend(view: &SimView) -> Option<Cost> {
    let p = &view.payday;
    (p.tick > 0).then(|| Cost(p.net().0 * 7 - p.loans_week.0))
}

/// `▲ +$1,234.00/wk` or `▼ -$56.00/wk`.
pub fn trend_text(weekly: Cost) -> String {
    if weekly.0 >= 0 {
        format!("▲ +{}/wk", format_money(weekly))
    } else {
        format!("▼ -{}/wk", format_money(Cost(-weekly.0)))
    }
}

fn trend_colour(up: bool) -> egui::Color32 {
    if up {
        egui::Color32::LIGHT_GREEN
    } else {
        egui::Color32::from_rgb(240, 90, 80)
    }
}

/// The breakdown shown on hovering the money trend: last day's income and upkeep by line,
/// scaled to a week, and the loan payments.
pub fn trend_breakdown(view: &SimView) -> String {
    let p = &view.payday;
    let wk = |c: Cost| format_money(Cost(c.0 * 7));
    format!(
        "Per week, by the last day:\nTaxes +{}\nRoads -{}\nBuildings -{}\nUtilities -{}\nLoans -{}",
        wk(p.income),
        wk(p.road_upkeep),
        wk(p.building_upkeep),
        wk(p.utility_upkeep),
        format_money(p.loans_week),
    )
}

/// The money trend next to the balance, green or red, with the breakdown on hover.
pub fn trend_label(ui: &mut egui::Ui, view: &SimView) {
    let Some(weekly) = weekly_trend(view) else {
        return;
    };
    ui.label(egui::RichText::new(trend_text(weekly)).color(trend_colour(weekly.0 >= 0)))
        .on_hover_text(trend_breakdown(view));
}

/// How long the "+$N" pop-up of a payday stays, in ticks.
pub const PAYDAY_POP_TICKS: u64 = 120;

/// The text and age fraction `0..1` of the payday pop-up, `None` when it is over (or nothing was
/// booked).
pub fn payday_pop(view: &SimView) -> Option<(String, f32)> {
    let p = &view.payday;
    let age = view.tick.0.checked_sub(p.tick)?;
    (p.tick > 0 && age < PAYDAY_POP_TICKS).then(|| {
        let n = p.net();
        let sign = if n.0 >= 0 { "+" } else { "-" };
        (
            format!("{sign}{}", format_money(Cost(n.0.abs()))),
            age as f32 / PAYDAY_POP_TICKS as f32,
        )
    })
}

/// The pop-up drawn under `anchor`, drifting down and fading out.
pub fn payday_pop_ui(ui: &egui::Ui, view: &SimView, anchor: egui::Rect) {
    let Some((text, t)) = payday_pop(view) else {
        return;
    };
    let alpha = ((1.0 - t) * 255.0) as u8;
    let up = view.payday.net().0 >= 0;
    let c = if up {
        egui::Color32::from_rgba_unmultiplied(120, 230, 130, alpha)
    } else {
        egui::Color32::from_rgba_unmultiplied(240, 90, 80, alpha)
    };
    ui.ctx().debug_painter().text(
        anchor.left_bottom() + egui::vec2(0.0, 2.0 + 10.0 * t),
        egui::Align2::LEFT_TOP,
        text,
        egui::FontId::proportional(13.0),
        c,
    );
}

/// The population change over the last game day: `(net, text, hover)`; `▲ +N`, `▼ -N` or `0`.
pub fn population_trend(view: &SimView) -> (i64, String, String) {
    let d = view.city.pop_day;
    let net = (d.arrived + d.born) as i64 - (d.left + d.died) as i64;
    let text = match net {
        0 => "0".to_string(),
        n if n > 0 => format!("▲ +{n}"),
        n => format!("▼ -{}", -n),
    };
    let hover = format!(
        "Last day:\nMoved in +{}\nBorn +{}\nMoved out -{}\nDied -{}",
        d.arrived, d.born, d.left, d.died
    );
    (net, text, hover)
}

/// The population trend next to the population: green, red, grey at zero.
pub fn population_trend_label(ui: &mut egui::Ui, view: &SimView) {
    let (net, text, hover) = population_trend(view);
    let c = match net {
        0 => egui::Color32::GRAY,
        n => trend_colour(n > 0),
    };
    ui.label(egui::RichText::new(text).color(c))
        .on_hover_text(hover);
}

/// Fraction `0..=1` of a bar for a demand value (`0` is the middle).
pub fn demand_fraction(v: f32) -> f32 {
    let v = if v.is_nan() { 0.0 } else { v };
    (v.clamp(-DEMAND_RANGE, DEMAND_RANGE) + DEMAND_RANGE) / (2.0 * DEMAND_RANGE)
}

/// What a tax slider sends.
pub fn tax_command(zone: ZoneType, pct: u8) -> SimCommand {
    SimCommand::Economy(EconomyCommand::SetTaxRate { zone, pct })
}

/// What a service-budget slider sends.
pub fn budget_command(service: ServiceKind, pct: u8) -> SimCommand {
    SimCommand::Economy(EconomyCommand::SetBudget { service, pct })
}

/// A slider value that is being edited but not settled yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draft {
    pub value: u8,
    /// The command for `value` was sent; the draft goes once the sim shows it.
    pub sent: bool,
}

/// The tabs of the economy window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BudgetTab {
    #[default]
    Overview,
    Taxes,
    Budgets,
    Loans,
}

impl BudgetTab {
    pub const ALL: [BudgetTab; 4] = [
        BudgetTab::Overview,
        BudgetTab::Taxes,
        BudgetTab::Budgets,
        BudgetTab::Loans,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BudgetTab::Overview => "Overview",
            BudgetTab::Taxes => "Taxes",
            BudgetTab::Budgets => "Service budgets",
            BudgetTab::Loans => "Loans",
        }
    }
}

/// State of the economy window: the open tab and the slider drafts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BudgetDraft {
    pub tab: BudgetTab,
    taxes: [Option<Draft>; 6],
    budgets: [Option<Draft>; 4],
    /// The tile the land section asks to confirm buying (T-150).
    pub buy_tile: Option<(u8, u8)>,
}

impl BudgetDraft {
    /// A fresh state with `tab` open.
    pub fn on(tab: BudgetTab) -> Self {
        BudgetDraft {
            tab,
            ..Default::default()
        }
    }
}

/// One slider. Returns the value to send when the edit is finished (pointer released, or a
/// click / key press that is not a drag), once per edit.
fn pct_slider(
    ui: &mut egui::Ui,
    label: &str,
    current: u8,
    draft: &mut Option<Draft>,
    range: RangeInclusive<u8>,
) -> Option<u8> {
    if let Some(d) = *draft
        && d.sent
        && d.value == current
    {
        *draft = None;
    }
    let mut v = draft.map_or(current, |d| d.value);
    let r = ui.add(egui::Slider::new(&mut v, range).suffix("%").text(label));
    if r.changed() {
        *draft = Some(Draft {
            value: v,
            sent: false,
        });
    }
    let finished = r.drag_stopped() || (r.changed() && !r.dragged());
    match *draft {
        Some(d) if finished && !d.sent => {
            *draft = Some(Draft { sent: true, ..d });
            Some(d.value)
        }
        _ => None,
    }
}

/// The economy window: the balance, then the tabs Overview (history and breakdown), Taxes
/// and Service budgets. Slider releases push commands into `outbox`.
pub fn budget_window(
    ctx: &egui::Context,
    open: &mut bool,
    view: &SimView,
    draft: &mut BudgetDraft,
    outbox: &mut CommandOutbox,
) {
    crate::frame::window(
        "budget",
        crate::icons::Icon::Budget,
        "Economy & budget",
        open,
    )
    .show(ctx, |ui| {
        budget_contents(ui, view, draft, outbox);
        crate::tiles::tile_section(ui, view, &mut draft.buy_tile);
    });
}

/// One road class in the overview's expenses.
#[derive(Clone, Debug, PartialEq)]
pub struct RoadRow {
    pub name: String,
    pub segments: u32,
    pub meters: f32,
    pub upkeep_week: Cost,
}

/// The numbers of the Overview tab, read from the view.
#[derive(Clone, Debug, PartialEq)]
pub struct Overview {
    /// Tax income of last week per zone type, in [`TAXED_ZONES`] order.
    pub income_by_zone: Vec<(ZoneType, Cost)>,
    pub income: Cost,
    pub upkeep: Cost,
    pub net: Cost,
    /// The roads by class (`CityStats::roads`), weekly upkeep at today's budget.
    pub roads: Vec<RoadRow>,
    /// Sum of [`Overview::roads`].
    pub roads_sum: Cost,
    /// Last week's settled road upkeep (`WeekReport::road_upkeep`).
    pub road_upkeep: Cost,
    pub building_upkeep: Cost,
    pub utility_upkeep: Cost,
    /// Last week's loan payments (principal and interest); part of `upkeep`.
    pub loan_payments: Cost,
    /// Last week's fares (part of `income`) and bus upkeep (part of `upkeep`), T-160.
    pub transit_fares: Cost,
    pub transit_upkeep: Cost,
    /// Balance of every hourly history sample (7 game days at most), oldest first.
    pub balance_history: Vec<f32>,
}

pub fn overview(view: &SimView) -> Overview {
    let week = &view.economy.last_week;
    let roads: Vec<RoadRow> = view
        .city
        .roads
        .iter()
        .map(|g| RoadRow {
            name: view.content.as_ref().map_or_else(
                || format!("road {}", g.road.index()),
                |c| c.road(g.road).name.clone(),
            ),
            segments: g.segments,
            meters: g.meters,
            upkeep_week: g.upkeep_week,
        })
        .collect();
    Overview {
        income_by_zone: TAXED_ZONES
            .iter()
            .enumerate()
            .map(|(i, z)| (*z, week.income_by_zone[i]))
            .collect(),
        income: week.income() + week.transit_fares,
        upkeep: week.upkeep() + week.loan_payments + week.transit_upkeep,
        net: week.balance(),
        roads_sum: roads.iter().map(|r| r.upkeep_week).sum(),
        roads,
        road_upkeep: week.road_upkeep,
        building_upkeep: week.building_upkeep,
        utility_upkeep: week.utility_upkeep,
        loan_payments: week.loan_payments,
        transit_fares: week.transit_fares,
        transit_upkeep: week.transit_upkeep,
        balance_history: view
            .city
            .history
            .iter()
            .map(|h| h.balance.0 as f32)
            .collect(),
    }
}

/// The Overview tab: last week, the balance history, income by zone and the expenses.
pub fn overview_tab(ui: &mut egui::Ui, view: &SimView) {
    let o = overview(view);
    ui.strong("Last week");
    egui::Grid::new("overview_week").show(ui, |ui| {
        for (name, cost) in [("Income", o.income), ("Expenses", o.upkeep), ("Net", o.net)] {
            ui.label(name);
            ui.label(format_money(cost));
            ui.end_row();
        }
    });
    ui.weak("Balance, 7 days");
    crate::metrics::sparkline(ui, &o.balance_history, egui::vec2(260.0, 36.0));
    ui.separator();
    ui.strong("Income by zone");
    egui::Grid::new("overview_income").show(ui, |ui| {
        for (zone, cost) in &o.income_by_zone {
            ui.label(zone_name(*zone));
            ui.label(format_money(*cost));
            ui.end_row();
        }
        ui.label("Fares");
        ui.label(format_money(o.transit_fares));
        ui.end_row();
    });
    ui.separator();
    ui.strong("Expenses");
    egui::Grid::new("overview_expenses").show(ui, |ui| {
        for r in &o.roads {
            ui.label(format!(
                "Roads: {} ({} seg, {:.0} m)",
                r.name, r.segments, r.meters
            ));
            ui.label(format_money(r.upkeep_week));
            ui.end_row();
        }
        ui.label("Roads, all classes");
        ui.label(format_money(o.roads_sum));
        ui.end_row();
        ui.label("Roads, settled last week");
        ui.label(format_money(o.road_upkeep));
        ui.end_row();
        ui.label("Service buildings");
        ui.label(format_money(o.building_upkeep));
        ui.end_row();
        ui.label("Pipes and power lines");
        ui.label(format_money(o.utility_upkeep));
        ui.end_row();
        ui.label("Public transport");
        ui.label(format_money(o.transit_upkeep));
        ui.end_row();
        ui.label("Loans");
        ui.label(format_money(o.loan_payments));
        ui.end_row();
    });
}

/// The inside of the economy window (separate so tests can draw it into a plain `Ui`): the
/// balance header, the tab row and the open tab.
pub fn budget_contents(
    ui: &mut egui::Ui,
    view: &SimView,
    draft: &mut BudgetDraft,
    outbox: &mut CommandOutbox,
) {
    let week = &view.economy.last_week;
    let balance = view.treasury.balance;
    ui.horizontal(|ui| {
        money_label(ui, view);
        if week.week == 0 {
            ui.weak("no week settled yet");
        } else {
            ui.label(format!("week {}", week.week));
        }
    });
    if let Some(w) = debt_warning(balance) {
        ui.colored_label(debt_colour(debt_level(balance)), format!("⚠ {w}"));
    }
    ui.horizontal(|ui| {
        for tab in BudgetTab::ALL {
            if ui.selectable_label(draft.tab == tab, tab.label()).clicked() {
                draft.tab = tab;
            }
        }
    });
    ui.separator();
    match draft.tab {
        BudgetTab::Overview => overview_tab(ui, view),
        BudgetTab::Taxes => taxes_tab(ui, view, draft, outbox),
        BudgetTab::Budgets => budgets_tab(ui, view, draft, outbox),
        BudgetTab::Loans => loans_tab(ui, view, outbox),
    }
}

/// The Taxes tab: the tax slider of every zone type with last week's income (T-045).
fn taxes_tab(
    ui: &mut egui::Ui,
    view: &SimView,
    draft: &mut BudgetDraft,
    outbox: &mut CommandOutbox,
) {
    let (state, week) = (&view.economy, &view.economy.last_week);
    ui.strong("Taxes (income last week)");
    egui::Grid::new("budget_taxes").show(ui, |ui| {
        for (i, zone) in TAXED_ZONES.into_iter().enumerate() {
            ui.label(zone_name(zone));
            ui.label(format_money(week.income_by_zone[i]));
            let current = state.taxes.get(zone).unwrap_or(0);
            if let Some(pct) = pct_slider(ui, "", current, &mut draft.taxes[i], TAX_MIN..=TAX_MAX) {
                outbox.push(tax_command(zone, pct));
            }
            ui.end_row();
        }
    });
}

/// The Service budgets tab: one slider per budgeted service (T-045).
fn budgets_tab(
    ui: &mut egui::Ui,
    view: &SimView,
    draft: &mut BudgetDraft,
    outbox: &mut CommandOutbox,
) {
    let state = &view.economy;
    ui.strong("Service budgets");
    for (i, kind) in BUDGETED.into_iter().enumerate() {
        let current = state.budgets.get(kind);
        let name = match kind {
            ServiceKind::Water => "Water",
            ServiceKind::Power => "Power",
            ServiceKind::Roads => "Roads",
            ServiceKind::Sewage => "Sewage",
            _ => continue,
        };
        if let Some(pct) = pct_slider(
            ui,
            name,
            current,
            &mut draft.budgets[i],
            BUDGET_MIN..=BUDGET_MAX,
        ) {
            outbox.push(budget_command(kind, pct));
        }
    }
}

/// The Loans tab (T-130): the bank's offers with a Take button, then the active loans with a
/// pay-off button. Only sends `TakeLoan` / `RepayLoan`; the sim checks the rest.
fn loans_tab(ui: &mut egui::Ui, view: &SimView, outbox: &mut CommandOutbox) {
    let offers = view.loan_offers();
    ui.strong("Bank offers");
    if offers.is_empty() {
        ui.weak("The bank has no offers.");
    }
    for o in &offers {
        let weekly = Loan::new(o.index, &o.offer, view.tick).weekly();
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{}  ·  {}% interest  ·  {} weeks",
                    format_money(Cost::from_units(o.offer.amount)),
                    o.offer.interest_pct,
                    o.offer.weeks
                ));
                let label = if o.active.is_some() { "Taken" } else { "Take" };
                let r = ui.add_enabled(o.available, egui::Button::new(label));
                let r = if o.active.is_some() {
                    r.on_disabled_hover_text("Already taken; pay it off to take it again.")
                } else if o.available {
                    r
                } else {
                    r.on_disabled_hover_text(format!(
                        "The bank wants {} residents.",
                        o.offer.min_population
                    ))
                };
                if r.clicked() {
                    outbox.push(take_loan_command(o.index));
                }
            });
            ui.weak(format!(
                "Weekly payment {}{}",
                format_money(weekly),
                if o.offer.min_population > 0 {
                    format!("  ·  from {} residents", o.offer.min_population)
                } else {
                    String::new()
                }
            ));
        });
    }
    ui.separator();
    ui.strong("Active loans");
    if view.economy.loans.is_empty() {
        ui.weak("No active loans.");
    }
    for l in &view.economy.loans {
        ui.group(|ui| {
            ui.label(format!(
                "Offer {}: debt left {}, interest left {}",
                u16::from(l.offer) + 1,
                format_money(l.principal_left),
                format_money(l.interest_left)
            ));
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} weeks left  ·  {} a week",
                    l.weeks_left,
                    format_money(l.weekly())
                ));
                let can = view.treasury.balance >= l.principal_left;
                let r = ui.add_enabled(
                    can,
                    egui::Button::new(format!(
                        "Pay off early — {}",
                        format_money(l.principal_left)
                    )),
                );
                let r = if can {
                    r
                } else {
                    r.on_disabled_hover_text("Not enough money.")
                };
                if r.clicked() {
                    outbox.push(repay_loan_command(l.offer));
                }
            });
        });
    }
}

pub fn take_loan_command(offer: u8) -> SimCommand {
    SimCommand::Economy(EconomyCommand::TakeLoan { offer })
}

pub fn repay_loan_command(offer: u8) -> SimCommand {
    SimCommand::Economy(EconomyCommand::RepayLoan { offer })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_core::time::TICKS_PER_WEEK;
    use cp_economy::EconomyState;
    use cp_sim::SimRunner;

    /// T-179: the top bar's money trend takes the sign and the amount from the last day.
    #[test]
    fn the_money_trend_shows_sign_and_week_amount_and_the_population_trend_its_balance() {
        let mut view = SimRunner::new(1).view();
        assert_eq!(weekly_trend(&view), None, "before the first payday");
        view.payday = cp_economy::Payday {
            tick: 4096,
            day: 1,
            income: Cost(1_000),
            road_upkeep: Cost(100),
            building_upkeep: Cost(200),
            utility_upkeep: Cost(50),
            loans_week: Cost(700),
            ..Default::default()
        };
        // (1000 - 350) * 7 - 700 = 3850 cents.
        assert_eq!(weekly_trend(&view), Some(Cost(3_850)));
        assert_eq!(trend_text(Cost(3_850)), "▲ +$38.50/wk");
        assert_eq!(trend_text(Cost(-12_345)), "▼ -$123.45/wk");
        view.tick = cp_core::Tick(4096 + 10);
        let (text, age) = payday_pop(&view).expect("just paid");
        assert_eq!(text, "+$6.50");
        assert!(age < 0.1);
        view.tick = cp_core::Tick(4096 + PAYDAY_POP_TICKS);
        assert!(payday_pop(&view).is_none(), "the pop-up is over");

        let city = std::sync::Arc::make_mut(&mut view.city);
        city.pop_day = cp_citizen::PopFlow {
            arrived: 12,
            born: 3,
            left: 4,
            died: 1,
        };
        assert_eq!(population_trend(&view).1, "▲ +10");
        let city = std::sync::Arc::make_mut(&mut view.city);
        city.pop_day = cp_citizen::PopFlow {
            left: 5,
            ..Default::default()
        };
        assert_eq!(population_trend(&view).1, "▼ -5");
        let city = std::sync::Arc::make_mut(&mut view.city);
        city.pop_day = Default::default();
        assert_eq!(
            population_trend(&view),
            (0, "0".into(), population_trend(&view).2)
        );
    }

    fn input(t: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            time: Some(t),
            events,
            ..Default::default()
        }
    }

    #[test]
    fn debt_levels_follow_the_balance_and_the_floor() {
        assert_eq!(debt_level(Cost(7_000_000)), DebtLevel::Fine);
        assert_eq!(debt_level(Cost(0)), DebtLevel::Fine);
        assert_eq!(debt_level(Cost(-1)), DebtLevel::Debt);
        assert_eq!(debt_level(Cost(-1_499_999)), DebtLevel::Debt);
        // Within 5 000.00 of the −20 000.00 floor the warning is the strong one.
        assert_eq!(debt_level(Cost(-1_500_000)), DebtLevel::NearFloor);
        assert_eq!(debt_level(DEBT_FLOOR), DebtLevel::NearFloor);
        assert_eq!(debt_level(Cost(-2_500_000)), DebtLevel::NearFloor);
        assert!(debt_warning(Cost(100)).is_none());
        assert!(debt_warning(Cost(-100)).unwrap().contains("In debt"));
        assert!(
            debt_warning(Cost(-1_900_000))
                .unwrap()
                .contains("debt limit")
        );
        assert!(debt_warning(Cost(-100)).unwrap().contains("-$20,000.00"));
    }

    #[test]
    fn demand_bars_map_the_range_onto_the_bar() {
        assert_eq!(demand_fraction(-100.0), 0.0);
        assert_eq!(demand_fraction(0.0), 0.5);
        assert_eq!(demand_fraction(100.0), 1.0);
        assert_eq!(demand_fraction(1e9), 1.0);
        assert_eq!(demand_fraction(f32::NAN), 0.5);
    }

    #[test]
    fn the_commands_are_the_economy_ones() {
        assert_eq!(
            tax_command(ZoneType::ComLow, 12),
            SimCommand::Economy(EconomyCommand::SetTaxRate {
                zone: ZoneType::ComLow,
                pct: 12
            })
        );
        assert_eq!(
            budget_command(ServiceKind::Roads, 120),
            SimCommand::Economy(EconomyCommand::SetBudget {
                service: ServiceKind::Roads,
                pct: 120
            })
        );
    }

    /// Draws the window contents for `view`, with a pointer event script, and returns what
    /// the outbox got.
    fn frames(
        view: &SimView,
        draft: &mut BudgetDraft,
        script: Vec<(f64, Vec<egui::Event>)>,
    ) -> Vec<SimCommand> {
        let ctx = egui::Context::default();
        let mut outbox = CommandOutbox::default();
        for (t, events) in script {
            crate::testutil::frame(&ctx, input(t, events), |ui| {
                budget_contents(ui, view, draft, &mut outbox);
            });
        }
        outbox.iter().cloned().collect()
    }

    #[test]
    fn the_window_draws_with_and_without_debt_and_sends_nothing_by_itself() {
        let mut sim = SimRunner::new(1);
        let mut draft = BudgetDraft::on(BudgetTab::Taxes);
        assert!(frames(&sim.view(), &mut draft, vec![(0.0, vec![]), (0.1, vec![])]).is_empty());
        sim.world_mut().resource_mut::<cp_core::Treasury>().balance = Cost(-1_900_000);
        assert!(frames(&sim.view(), &mut draft, vec![(0.0, vec![])]).is_empty());
    }

    /// Drags the first tax slider to the right and releases: exactly one command.
    #[test]
    fn a_drag_sends_one_command_on_release() {
        let sim = SimRunner::new(1);
        let view = sim.view();
        let ctx = egui::Context::default();
        let mut draft = BudgetDraft::on(BudgetTab::Taxes);
        let mut outbox = CommandOutbox::default();
        let draw = |t: f64,
                    events: Vec<egui::Event>,
                    outbox: &mut CommandOutbox,
                    draft: &mut BudgetDraft| {
            crate::testutil::frame(&ctx, input(t, events), |ui| {
                budget_contents(ui, &view, draft, outbox);
            });
        };
        draw(0.0, vec![], &mut outbox, &mut draft);
        // Find the first slider: the first widget that reacts to a press is hard to locate
        // without its rect, so scan along the row of the first tax slider.
        let mut start = None;
        let ys = [60.0f32, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0];
        'search: for y in ys {
            for x in (200..520).step_by(10) {
                let pos = egui::pos2(x as f32, y);
                let ctx2 = egui::Context::default();
                let mut d = BudgetDraft::on(BudgetTab::Taxes);
                let mut o = CommandOutbox::default();
                let ev = |pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                for (t, events) in [
                    (0.0, vec![]),
                    (0.1, vec![egui::Event::PointerMoved(pos), ev(true)]),
                    (0.2, vec![ev(false)]),
                ] {
                    crate::testutil::frame(&ctx2, input(t, events), |ui| {
                        budget_contents(ui, &view, &mut d, &mut o);
                    });
                }
                if o.len() == 1 {
                    start = Some(pos);
                    break 'search;
                }
            }
        }
        let pos = start.expect("a click on a tax slider changes it");
        // Now a real drag from there to the right: many moves, one command.
        let ev = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        draw(
            0.3,
            vec![egui::Event::PointerMoved(pos), ev(true)],
            &mut outbox,
            &mut draft,
        );
        for k in 1..=8 {
            let to = egui::pos2(pos.x + 4.0 * k as f32, pos.y);
            draw(
                0.3 + 0.01 * k as f64,
                vec![egui::Event::PointerMoved(to)],
                &mut outbox,
                &mut draft,
            );
        }
        assert!(outbox.is_empty(), "nothing is sent while dragging");
        let to = egui::pos2(pos.x + 32.0, pos.y);
        draw(
            0.5,
            vec![egui::Event::PointerMoved(to), {
                egui::Event::PointerButton {
                    pos: to,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }
            }],
            &mut outbox,
            &mut draft,
        );
        draw(0.6, vec![], &mut outbox, &mut draft);
        assert_eq!(outbox.len(), 1, "one command for the whole drag");
        assert!(matches!(
            outbox.iter().next(),
            Some(SimCommand::Economy(EconomyCommand::SetTaxRate { .. }))
        ));
    }

    /// The Done-when of T-045: the command from the UI path lands in the command log and the
    /// next weekly report's income for that zone changes accordingly.
    #[test]
    fn a_tax_change_reaches_the_command_log_and_next_weeks_income() {
        use cp_building::{AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingState, Lot};
        use cp_citizen::CitizenGrid;
        use cp_content::ContentDb;
        use cp_core::{CommandLog, Id, Tick};
        use std::sync::Arc;

        let db = Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        );
        // One week of a one-house city, with the tax of that house's zone left alone or set
        // through the UI path (outbox -> sim) at the start.
        let run_week = |tax: Option<u8>| {
            let mut sim = SimRunner::new(1);
            sim.set_content(Arc::clone(&db));
            let house = sim
                .world_mut()
                .resource_mut::<BuildingGrid>()
                .insert(Building {
                    def: db.building_id("house_low_1").unwrap(),
                    footprint: cp_building::Footprint::Lot(Lot {
                        block: Id::new(0, 0),
                        col: 0,
                        w: 1,
                        d: 1,
                    }),
                    level: 1,
                    state: BuildingState::Active,
                    ready_at: Tick(0),
                    pos: bevy::math::Vec2::new(5000.0, 5000.0),
                    rot: 0.0,
                    access: Some(AccessPoint {
                        segment: Id::new(0, 0),
                        t: 0.0,
                    }),
                    flags: BuildingFlags::default(),
                    level_progress: 0.0,
                });
            {
                let mut c = sim.world_mut().resource_mut::<CitizenGrid>();
                c.ensure_derived();
                c.add_household(house, 0, Tick(0), &[(30 * 365, 0, false)]);
            }
            if let Some(pct) = tax {
                // The slider's release: command into the outbox, outbox into the sim.
                let mut outbox = CommandOutbox::default();
                outbox.push(tax_command(ZoneType::ResLow, pct));
                outbox.flush_into(&mut sim);
                let log = sim.world().resource::<CommandLog<SimCommand>>();
                assert!(
                    log.entries()
                        .iter()
                        .any(|e| e.command == tax_command(ZoneType::ResLow, pct)),
                    "the change is in the command log"
                );
                assert_eq!(sim.view().economy.taxes.get(ZoneType::ResLow), Some(pct));
            }
            while sim.tick().0 <= TICKS_PER_WEEK {
                sim.tick_once();
            }
            let week = sim.world().resource::<EconomyState>().last_week;
            assert_eq!(week.week, 1);
            (sim, week.income_by_zone[0])
        };
        let (_, at_9) = run_week(None);
        let (sim, at_18) = run_week(Some(18));
        assert!(at_9.0 > 0, "the house pays tax at the default 9%");
        assert_eq!(at_18.0, at_9.0 * 2, "18% pays twice what 9% did");
        // The window shows it.
        let mut draft = BudgetDraft::on(BudgetTab::Taxes);
        assert!(frames(&sim.view(), &mut draft, vec![(0.0, vec![])]).is_empty());
    }

    /// Draws the whole window on a tall screen for `ticks` frames and returns the painted texts.
    fn painted(view: &SimView, draft: &mut BudgetDraft) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut outbox = CommandOutbox::default();
        let mut all = Vec::new();
        for t in [0.0, 0.1] {
            let mut tall = input(t, vec![]);
            tall.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 4000.0),
            ));
            all = crate::testutil::frame_texts(&ctx, tall, |ui| {
                budget_contents(ui, view, draft, &mut outbox)
            })
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        }
        all
    }

    #[test]
    fn the_overview_draws_on_an_empty_city() {
        let sim = SimRunner::new(1);
        let view = sim.view();
        let text = painted(&view, &mut BudgetDraft::default());
        for want in [
            "Last week",
            "Income by zone",
            "Expenses",
            "Roads, all classes",
        ] {
            assert!(text.iter().any(|t| t == want), "{want}: {text:?}");
        }
        let o = overview(&view);
        assert_eq!(o.roads_sum, Cost(0));
        assert!(o.balance_history.is_empty());
        assert_eq!(o.income, Cost(0));
    }

    /// A built-up city after more than a week: the road classes of the overview add up to the
    /// week's settled road upkeep, and every number of the tab is painted.
    #[test]
    fn the_road_classes_add_up_to_the_weeks_road_upkeep() {
        use cp_content::ContentDb;
        use cp_sim::scenario::{GridCityConfig, grid_city};
        use cp_terrain::Terrain;
        use std::sync::Arc;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
        let content = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
        let (mut sim, report) = grid_city(400, 3, terrain, content, &GridCityConfig::default());
        assert!(report.reached, "{report:?}");
        // Past the next weekly settlement and the stats publish after it.
        let start = sim.tick().0;
        let week_end = (start / TICKS_PER_WEEK + 1) * TICKS_PER_WEEK;
        while sim.tick().0 < week_end + 200 {
            sim.tick_once();
        }
        let view = sim.view();
        assert!(view.economy.last_week.week >= 1);
        let o = overview(&view);
        assert!(o.roads.len() >= 2, "avenues and streets: {:?}", o.roads);
        assert!(o.road_upkeep.0 > 0);
        assert_eq!(o.roads_sum, o.road_upkeep, "{:?}", o.roads);
        assert_eq!(
            o.roads.iter().map(|r| r.segments).sum::<u32>() as usize,
            sim.world()
                .resource::<cp_net::NetGraph>()
                .segments()
                .count()
        );
        assert!(!o.balance_history.is_empty());

        let text = painted(&view, &mut BudgetDraft::default());
        // `cargo test -p cp_ui road_classes -- --nocapture` prints what the Overview paints.
        println!("{}", text.join(" | "));
        let week = &view.economy.last_week;
        let mut values = vec![
            format_money(o.roads_sum),
            format_money(o.road_upkeep),
            format_money(week.building_upkeep),
            format_money(week.utility_upkeep),
            format_money(week.income()),
            format_money(week.upkeep()),
        ];
        values.extend(o.income_by_zone.iter().map(|(_, c)| format_money(*c)));
        values.extend(o.roads.iter().map(|r| format_money(r.upkeep_week)));
        for v in values {
            assert!(text.contains(&v), "{v} not painted: {text:?}");
        }
    }

    #[test]
    fn clicking_the_tabs_switches_them() {
        let sim = SimRunner::new(1);
        let view = sim.view();
        let ctx = egui::Context::default();
        let mut draft = BudgetDraft::default();
        let mut outbox = CommandOutbox::default();
        let mut at = Vec::new();
        let mut t = 0.0;
        let mut draw = |draft: &mut BudgetDraft, events: Vec<egui::Event>| {
            t += 0.1;
            crate::testutil::frame_texts(&ctx, input(t, events), |ui| {
                budget_contents(ui, &view, draft, &mut outbox)
            })
        };
        draw(&mut draft, vec![]);
        for tab in [BudgetTab::Taxes, BudgetTab::Budgets, BudgetTab::Overview] {
            let texts = draw(&mut draft, vec![]);
            let (_, pos) = texts
                .iter()
                .find(|(s, _)| s == tab.label())
                .unwrap_or_else(|| panic!("{tab:?} in {texts:?}"));
            at.push(*pos);
            let [down, up] = crate::testutil::click(*pos + egui::vec2(3.0, 3.0));
            draw(&mut draft, down);
            draw(&mut draft, up);
            assert_eq!(draft.tab, tab);
        }
        // Each tab shows its own content.
        let text = |tab| painted(&view, &mut BudgetDraft::on(tab));
        assert!(
            text(BudgetTab::Overview)
                .iter()
                .any(|t| t == "Income by zone")
        );
        assert!(
            text(BudgetTab::Taxes)
                .iter()
                .any(|t| t.starts_with("Taxes"))
        );
        assert!(
            text(BudgetTab::Budgets)
                .iter()
                .any(|t| t == "Service budgets")
        );
        assert!(
            !text(BudgetTab::Budgets)
                .iter()
                .any(|t| t == "Income by zone")
        );
    }

    /// Draws the Loans tab, then clicks along a grid of points until a command appears.
    fn click_for_command(view: &SimView) -> Option<SimCommand> {
        for y in (30..600).step_by(6) {
            for x in (20..600).step_by(12) {
                let pos = egui::pos2(x as f32, y as f32);
                let ctx = egui::Context::default();
                let mut draft = BudgetDraft::on(BudgetTab::Loans);
                let mut outbox = CommandOutbox::default();
                let ev = |pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                for (t, events) in [
                    (0.0, vec![]),
                    (0.1, vec![egui::Event::PointerMoved(pos), ev(true)]),
                    (0.2, vec![ev(false)]),
                ] {
                    crate::testutil::frame(&ctx, input(t, events), |ui| {
                        budget_contents(ui, view, &mut draft, &mut outbox);
                    });
                }
                if let Some(c) = outbox.iter().next() {
                    return Some(c.clone());
                }
            }
        }
        None
    }

    #[test]
    fn the_loans_tab_takes_a_loan_and_pays_one_off_through_the_outbox() {
        use std::sync::Arc;
        let db = Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        );
        let mut sim = SimRunner::new(1);
        sim.set_content(db);
        // No loan yet: the only reachable button is "Take" of the open offer (the others need
        // residents), and it sends TakeLoan for offer 0.
        assert_eq!(click_for_command(&sim.view()), Some(take_loan_command(0)));
        // One active loan: the pay-off button shows its debt and sends RepayLoan.
        sim.push_command(take_loan_command(0), sim.tick());
        sim.tick_once();
        let view = sim.view();
        let loan = *view.economy.loan(0).expect("taken");
        assert_eq!(loan.weeks_left, 26);
        let cmd = click_for_command(&view);
        assert_eq!(cmd, Some(repay_loan_command(0)));
        // Without the money the button is dead.
        sim.world_mut().resource_mut::<cp_core::Treasury>().balance = Cost(100);
        assert_eq!(click_for_command(&sim.view()), None);
    }
}
