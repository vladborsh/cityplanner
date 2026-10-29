//! The money side of the UI (`architecture/system-design.md` §13.6, §11): the balance with its
//! debt warning, the RCIO demand bars, and the budget window with tax and service-budget
//! sliders.
//!
//! The UI only reads [`SimView`] and sends `SetTaxRate` / `SetBudget` through the outbox. A
//! slider keeps a *draft* while it is being dragged and sends **one** command when the
//! pointer is released (or on a click or key press), so a drag does not flood the command log.

use std::ops::RangeInclusive;

use crate::icons::Icon;
use crate::style::Sizes;
use crate::windows::zone_open;
use bevy_egui::egui;
use cp_content::budget::{power_factor, reach_factor};
use cp_content::{ServiceKind, ZoneType};
use cp_core::Cost;
use cp_core::time::TICKS_PER_DAY;
use cp_core::treasury::DEBT_FLOOR;
use cp_economy::contentment::tax_score;
use cp_economy::state::{BUDGET_MAX, BUDGET_MIN, BUDGETED, TAX_MAX, TAX_MIN, TAXED_ZONES};
use cp_economy::{EconomyCommand, Flow, Loan, SERVICE_ORDER, WeekReport};
use cp_locale::{tr, trf};
use cp_sim::city_services::Sector;
use cp_sim::tiles::{TileState, tile_state};
use cp_sim::{SimCommand, SimView};
use cp_terrain::{TILES_PER_SIDE, TerrainCommand};
use cp_tools::{CommandOutbox, format_money, format_money_short, zone_name};

/// Demand bars show `0` (nobody wants it) to `DEMAND_RANGE`.
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
        DebtLevel::Debt => Some(trf!(
            "In debt ({balance}). Building stops at {floor}.",
            balance = format_money_short(balance),
            floor = format_money_short(DEBT_FLOOR)
        )),
        DebtLevel::NearFloor => Some(trf!(
            "Almost at the debt limit ({balance} of {floor})! Building will be refused.",
            balance = format_money_short(balance),
            floor = format_money_short(DEBT_FLOOR)
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
    let text = egui::RichText::new(format!("💰 {}", format_money_short(b)))
        .color(debt_colour(debt_level(b)));
    let r = ui.label(text);
    let tip = match debt_warning(b) {
        Some(w) => format!("{}\n{w}\n\n{}", format_money(b), trend_breakdown(view)),
        None => format!("{}\n\n{}", format_money(b), trend_breakdown(view)),
    };
    r.on_hover_text(tip)
}

/// What the treasury gained or lost over the last 24 game hours (BUG-040); `None` before the
/// first hour was booked (or after a load, until the next one).
pub fn daily_trend(view: &SimView) -> Option<Cost> {
    view.flow.last_day(&view.economy).map(|d| d.net())
}

/// `+$1,234.00/day` or `-$56.00/day` (the arrow is drawn by [`trend_arrow`]).
pub fn trend_text(daily: Cost) -> String {
    if daily.0 >= 0 {
        trf!("+{money}/day", money = format_money_short(daily))
    } else {
        trf!("-{money}/day", money = format_money_short(Cost(-daily.0)))
    }
}

/// The trend arrow: a filled triangle, up or down, drawn as a shape (the fonts have no ▲ / ▼,
/// BUG-039). Returns the rectangle it took.
pub fn trend_arrow(ui: &mut egui::Ui, up: bool, colour: egui::Color32) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 8.0), egui::Sense::hover());
    let (l, r, t, b) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    let points = if up {
        vec![
            egui::pos2(l, b),
            egui::pos2(r, b),
            egui::pos2((l + r) / 2.0, t),
        ]
    } else {
        vec![
            egui::pos2(l, t),
            egui::pos2(r, t),
            egui::pos2((l + r) / 2.0, b),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        colour,
        egui::Stroke::NONE,
    ));
    rect
}

fn trend_colour(up: bool) -> egui::Color32 {
    if up {
        egui::Color32::LIGHT_GREEN
    } else {
        egui::Color32::from_rgb(240, 90, 80)
    }
}

/// The name of a service line of the money flow.
pub fn service_line_name(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Water => "Water buildings",
        ServiceKind::Power => "Power buildings",
        ServiceKind::Roads => "Road buildings",
        ServiceKind::Sewage => "Sewage buildings",
        ServiceKind::Garbage => "Garbage",
        ServiceKind::Health => "Health",
        ServiceKind::Deathcare => "Deathcare",
        ServiceKind::Fire => "Fire",
        ServiceKind::Police => "Police",
        ServiceKind::Education => "Education",
        ServiceKind::Transit => "Bus depots",
        ServiceKind::Landmark => "Landmarks",
        ServiceKind::Industry => "Industry",
        ServiceKind::Park => "Parks",
        ServiceKind::Cargo => "Cargo",
        ServiceKind::Post => "Post",
        ServiceKind::Parking => "Parking",
        ServiceKind::Tourism => "Hotels",
    }
}

/// One line of the money flow: its icon, name and amount in each column (last day, this week
/// so far, last week; `None` where the figure is not kept).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowRow {
    pub icon: Icon,
    pub name: String,
    pub money: [Option<Cost>; 3],
}

fn row(icon: Icon, name: String, money: [Option<Cost>; 3]) -> FlowRow {
    FlowRow { icon, name, money }
}

/// The income and expense rows of the cash-flow: columns last day, this week so far, last
/// week (`None` where the figure is not kept). Expenses are positive numbers. Rows that are
/// zero in every column are left out, except the taxes of the open zones and the buses once
/// Transit is open (BUG-123: what no milestone has opened shows only if it moved money).
pub fn flow_rows(
    view: &SimView,
    day: &Flow,
    week: &Flow,
    last: &WeekReport,
) -> (Vec<FlowRow>, Vec<FlowRow>) {
    let mut income: Vec<FlowRow> = Vec::new();
    for (i, zone) in TAXED_ZONES.into_iter().enumerate() {
        let money = [
            day.income_by_zone[i],
            week.income_by_zone[i],
            last.income_by_zone[i],
        ];
        if !zone_open(view, zone) && money.iter().all(|c| *c == Cost::ZERO) {
            continue;
        }
        income.push(row(
            crate::icons::zone_icon(zone),
            trf!("Tax: {zone}", zone = tr(zone_name(zone))),
            money.map(Some),
        ));
    }
    let mut expenses: Vec<FlowRow> = vec![
        row(
            Icon::Roads,
            tr("Roads"),
            [
                Some(day.road_upkeep),
                Some(week.road_upkeep),
                Some(last.road_upkeep),
            ],
        ),
        row(
            Icon::PowerLine,
            tr("Pipes and power lines"),
            [
                Some(day.utility_upkeep),
                Some(week.utility_upkeep),
                Some(last.utility_upkeep),
            ],
        ),
    ];
    for (i, kind) in SERVICE_ORDER.into_iter().enumerate() {
        if day.services[i] != Cost::ZERO || week.services[i] != Cost::ZERO {
            expenses.push(row(
                crate::buildings::kind_icon(kind),
                tr(service_line_name(kind)),
                [Some(day.services[i]), Some(week.services[i]), None],
            ));
        }
    }
    // Last week's settled report keeps the service buildings as one figure.
    if last.building_upkeep != Cost::ZERO {
        expenses.push(row(
            Icon::Services,
            tr("Service buildings"),
            [None, None, Some(last.building_upkeep)],
        ));
    }
    let buses = [day.transit_upkeep, week.transit_upkeep, last.transit_upkeep];
    if service_open(view, ServiceKind::Transit) || buses.iter().any(|c| *c != Cost::ZERO) {
        expenses.push(row(Icon::Transit, tr("Buses"), buses.map(Some)));
    }
    expenses.push(row(
        Icon::Budget,
        tr("Loans"),
        [day.loans, week.loans, last.loan_payments].map(Some),
    ));
    if day.policies != Cost::ZERO || week.policies != Cost::ZERO || last.policies != Cost::ZERO {
        expenses.push(row(
            Icon::DistrictTabPolicies,
            tr("Policies"),
            [Some(day.policies), Some(week.policies), Some(last.policies)],
        ));
    }
    if day.fares != Cost::ZERO || week.fares != Cost::ZERO || last.transit_fares != Cost::ZERO {
        income.push(row(
            Icon::Transit,
            tr("Fares"),
            [Some(day.fares), Some(week.fares), Some(last.transit_fares)],
        ));
    }
    if day.tourism != Cost::ZERO || week.tourism != Cost::ZERO || last.tourism != Cost::ZERO {
        income.push(row(
            Icon::Tourism,
            tr("Tourism"),
            [Some(day.tourism), Some(week.tourism), Some(last.tourism)],
        ));
    }
    // Hotel nights (T-321, §22.5).
    if day.hotels != Cost::ZERO || week.hotels != Cost::ZERO || last.hotels != Cost::ZERO {
        income.push(row(
            Icon::Tourism,
            tr("Hotels"),
            [Some(day.hotels), Some(week.hotels), Some(last.hotels)],
        ));
    }
    if day.exports != Cost::ZERO || week.exports != Cost::ZERO || last.exports != Cost::ZERO {
        income.push(row(
            Icon::Industry,
            tr("Exports"),
            [Some(day.exports), Some(week.exports), Some(last.exports)],
        ));
    }
    // Cargo trade (T-210): kept for the week so far only.
    if week.industry_export != Cost::ZERO {
        income.push(row(
            Icon::Industry,
            tr("Industry export"),
            [None, Some(week.industry_export), None],
        ));
    }
    // Entrance fees of the park areas (T-225): kept for the week so far only.
    if week.park_fees != Cost::ZERO {
        income.push(row(
            Icon::Parks,
            tr("Park entry fees"),
            [None, Some(week.park_fees), None],
        ));
    }
    // What bulldozing gave back (BUG-097): the last day and the week so far.
    if day.refunds != Cost::ZERO || week.refunds != Cost::ZERO {
        income.push(row(
            Icon::Bulldozer,
            tr("Demolition refund"),
            [Some(day.refunds), Some(week.refunds), None],
        ));
    }
    if week.industry_import != Cost::ZERO {
        expenses.push(row(
            Icon::Industry,
            tr("Industry import"),
            [None, Some(week.industry_import), None],
        ));
    }
    (income, expenses)
}

/// The text of the money tooltip: the last day's income and expenses by line.
pub fn trend_breakdown(view: &SimView) -> String {
    let Some(day) = view.flow.last_day(&view.economy) else {
        return tr("No hour booked yet.");
    };
    let (income, expenses) = flow_rows(view, &day, &day, &view.economy.last_week);
    let mut out = tr("Last 24 hours:");
    for r in income {
        if let Some(c) = r.money[0].filter(|c| c.0 != 0) {
            out += &format!("\n{} +{}", r.name, format_money(c));
        }
    }
    for r in expenses {
        if let Some(c) = r.money[0].filter(|c| c.0 != 0) {
            out += &format!("\n{} -{}", r.name, format_money(c));
        }
    }
    out
}

/// The money trend next to the balance, green or red, with the breakdown on hover.
pub fn trend_label(ui: &mut egui::Ui, view: &SimView) {
    let Some(daily) = daily_trend(view) else {
        return;
    };
    let colour = trend_colour(daily.0 >= 0);
    trend_arrow(ui, daily.0 >= 0, colour);
    ui.label(egui::RichText::new(trend_text(daily)).color(colour))
        .on_hover_text(trend_breakdown(view));
}

/// The population change over the last game day: `(net, text, hover)`; `+N`, `-N` or `0`.
pub fn population_trend(view: &SimView) -> (i64, String, String) {
    let d = view.city.pop_day;
    let net = (d.arrived + d.born) as i64 - (d.left + d.died) as i64;
    let text = match net {
        0 => "0".to_string(),
        n if n > 0 => format!("+{n}"),
        n => format!("-{}", -n),
    };
    let hover = trf!(
        "Last day:\nMoved in +{arrived}\nBorn +{born}\nMoved out -{left}\nDied -{died}",
        arrived = d.arrived,
        born = d.born,
        left = d.left,
        died = d.died
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
    if net != 0 {
        trend_arrow(ui, net > 0, c);
    }
    ui.label(egui::RichText::new(text).color(c))
        .on_hover_text(hover);
}

/// Fraction `0..=1` of a bar for a demand value: it fills up from the bottom, and a demand
/// of zero or less is an empty bar (demand either exists or not).
pub fn demand_fraction(v: f32) -> f32 {
    let v = if v.is_nan() { 0.0 } else { v };
    v.clamp(0.0, DEMAND_RANGE) / DEMAND_RANGE
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
    Land,
}

impl BudgetTab {
    pub const ALL: [BudgetTab; 5] = [
        BudgetTab::Overview,
        BudgetTab::Taxes,
        BudgetTab::Budgets,
        BudgetTab::Loans,
        BudgetTab::Land,
    ];

    /// The tab named `name` (`overview`, `taxes`, `budgets` or `services`, `loans`, `land`), for
    /// `--budget-tab`.
    pub fn parse(name: &str) -> Option<BudgetTab> {
        Some(match name {
            "overview" => BudgetTab::Overview,
            "taxes" => BudgetTab::Taxes,
            "budgets" | "services" => BudgetTab::Budgets,
            "loans" => BudgetTab::Loans,
            "land" => BudgetTab::Land,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            BudgetTab::Overview => "Overview",
            BudgetTab::Taxes => "Taxes",
            BudgetTab::Budgets => "Services",
            BudgetTab::Loans => "Loans",
            BudgetTab::Land => "Land",
        }
    }

    fn icon(self) -> Icon {
        match self {
            BudgetTab::Overview => Icon::Metrics,
            BudgetTab::Taxes => Icon::DistrictTabTaxes,
            BudgetTab::Budgets => Icon::Services,
            BudgetTab::Loans => Icon::Budget,
            BudgetTab::Land => Icon::OverlayLand,
        }
    }
}

/// The span the Overview's figures are for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Period {
    Day,
    Week,
    #[default]
    LastWeek,
}

impl Period {
    pub const ALL: [Period; 3] = [Period::Day, Period::Week, Period::LastWeek];

    pub fn label(self) -> &'static str {
        match self {
            Period::Day => "Last day",
            Period::Week => "This week",
            Period::LastWeek => "Last week",
        }
    }

    /// The column of [`FlowRow::money`].
    fn column(self) -> usize {
        match self {
            Period::Day => 0,
            Period::Week => 1,
            Period::LastWeek => 2,
        }
    }
}

/// State of the economy window: the open tab and the slider drafts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BudgetDraft {
    pub tab: BudgetTab,
    /// The Overview's period (kept while the window is open).
    pub period: Period,
    taxes: [Option<Draft>; 6],
    budgets: [Option<Draft>; BUDGETED.len()],
    /// The budget row an advice pointed at (§20.3): drawn highlighted.
    pub highlight: Option<ServiceKind>,
    /// The tile the land overlay asks to confirm buying (T-150).
    pub buy_tile: Option<(u8, u8)>,
    /// The tile picked in the Land tab: its card is on the right.
    pub land_pick: Option<(u8, u8)>,
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

// ----- colours ----------------------------------------------------------------------------

const GOOD: egui::Color32 = egui::Color32::from_rgb(120, 200, 120);
const WARN: egui::Color32 = egui::Color32::from_rgb(240, 190, 70);
const BAD: egui::Color32 = egui::Color32::from_rgb(235, 90, 80);
const COST: egui::Color32 = egui::Color32::from_rgb(235, 150, 70);
const BLUE: egui::Color32 = egui::Color32::from_rgb(110, 170, 235);

/// The colour of the `i`-th item of a stacked bar or a list.
fn item_colour(i: usize) -> egui::Color32 {
    const PALETTE: [egui::Color32; 10] = [
        egui::Color32::from_rgb(110, 170, 235),
        egui::Color32::from_rgb(235, 150, 70),
        egui::Color32::from_rgb(120, 200, 120),
        egui::Color32::from_rgb(200, 120, 200),
        egui::Color32::from_rgb(230, 200, 90),
        egui::Color32::from_rgb(90, 200, 200),
        egui::Color32::from_rgb(230, 110, 110),
        egui::Color32::from_rgb(160, 160, 230),
        egui::Color32::from_rgb(190, 160, 110),
        egui::Color32::from_rgb(140, 200, 160),
    ];
    PALETTE[i % PALETTE.len()]
}

/// A short sum with its exact amount (cents) on hover.
fn money_label_short(ui: &mut egui::Ui, c: Cost) -> egui::Response {
    ui.label(format_money_short(c))
        .on_hover_text(format_money(c))
}

// ----- sliders ----------------------------------------------------------------------------

/// What one frame of [`pct_slider`] reports.
struct SliderOut {
    /// The value to send: the edit is finished (pointer released, or a click / key press that is
    /// not a drag, or a double click), once per edit.
    send: Option<u8>,
    /// The value shown now: the draft while one is open.
    shown: u8,
    response: egui::Response,
}

/// What a slider looks like beyond the plain widget.
#[derive(Clone, Copy, Default)]
struct SliderLook {
    width: f32,
    /// A tick on the track at this value.
    tick: Option<u8>,
    /// The track above `tick` is orange.
    warn_above: bool,
    /// A double click sets this value.
    reset: Option<u8>,
}

/// One slider. Keeps a draft while it is dragged.
fn pct_slider(
    ui: &mut egui::Ui,
    current: u8,
    draft: &mut Option<Draft>,
    range: RangeInclusive<u8>,
    look: SliderLook,
) -> SliderOut {
    if let Some(d) = *draft
        && d.sent
        && d.value == current
    {
        *draft = None;
    }
    let mut v = draft.map_or(current, |d| d.value);
    let (lo, hi) = (*range.start(), *range.end());
    let r = ui
        .scope(|ui| {
            ui.spacing_mut().slider_width = look.width;
            ui.add(egui::Slider::new(&mut v, range).show_value(false))
        })
        .inner;
    if r.changed() {
        *draft = Some(Draft {
            value: v,
            sent: false,
        });
    }
    if let Some(reset) = look.reset
        && r.double_clicked()
    {
        *draft = Some(Draft {
            value: reset,
            sent: false,
        });
        v = reset;
    }
    paint_track(ui, &r, (lo, hi), v, look);
    let finished = r.drag_stopped()
        || (r.changed() && !r.dragged())
        || (look.reset.is_some() && r.double_clicked());
    let send = match *draft {
        Some(d) if finished && !d.sent => {
            *draft = Some(Draft { sent: true, ..d });
            Some(d.value)
        }
        _ => None,
    };
    SliderOut {
        send,
        shown: draft.map_or(current, |d| d.value),
        response: r,
    }
}

/// The tick and the orange part over a slider's track.
fn paint_track(ui: &egui::Ui, r: &egui::Response, (lo, hi): (u8, u8), v: u8, look: SliderLook) {
    let Some(tick) = look.tick else { return };
    // egui's slider: the handle's centre runs `rect.height() / 2.5` inside the rect.
    let radius = r.rect.height() / 2.5;
    let (x0, x1) = (r.rect.left() + radius, r.rect.right() - radius);
    let at = |value: u8| {
        let t = f32::from(value.clamp(lo, hi) - lo) / f32::from((hi - lo).max(1));
        x0 + (x1 - x0) * t
    };
    let y = r.rect.center().y;
    let p = ui.painter();
    if look.warn_above {
        let handle = at(v);
        for (a, b) in [(at(tick), handle - radius), (handle + radius, x1)] {
            if b > a {
                p.line_segment(
                    [egui::pos2(a.max(at(tick)), y), egui::pos2(b, y)],
                    egui::Stroke::new(3.0, COST),
                );
            }
        }
    }
    let x = at(tick);
    p.line_segment(
        [egui::pos2(x, y - 6.0), egui::pos2(x, y + 6.0)],
        egui::Stroke::new(1.5, egui::Color32::from_gray(230)),
    );
}

/// The economy window: the header, the tabs Overview, Taxes, Services, Loans and Land. Slider
/// releases push commands into `outbox`.
pub fn budget_window(
    ctx: &egui::Context,
    open: &mut bool,
    view: &SimView,
    draft: &mut BudgetDraft,
    outbox: &mut CommandOutbox,
) {
    crate::frame::window("budget", Icon::Budget, &tr("Economy & budget"), open).show(ctx, |ui| {
        budget_contents(ui, view, draft, outbox);
    });
}

/// The width the window keeps on every tab, so it does not jump when the tab changes.
const WINDOW_WIDTH: f32 = 700.0;

// ----- header and tabs --------------------------------------------------------------------

/// The inside of the economy window (separate so tests can draw it into a plain `Ui`): the
/// header tiles, the debt banner, the tab row and the open tab.
pub fn budget_contents(
    ui: &mut egui::Ui,
    view: &SimView,
    draft: &mut BudgetDraft,
    outbox: &mut CommandOutbox,
) {
    ui.set_min_width(WINDOW_WIDTH);
    header(ui, view);
    banner(ui, view, draft);
    ui.horizontal(|ui| {
        for tab in BudgetTab::ALL {
            let icon = egui::Image::new(tab.icon().image())
                .fit_to_exact_size(egui::vec2(Sizes::TAB_ICON, Sizes::TAB_ICON));
            let button =
                egui::Button::image_and_text(icon, tr(tab.label())).selected(draft.tab == tab);
            if ui.add(button).clicked() {
                draft.tab = tab;
            }
        }
    });
    ui.separator();
    match draft.tab {
        BudgetTab::Overview => overview_tab(ui, view, draft),
        BudgetTab::Taxes => taxes_tab(ui, view, draft, outbox),
        BudgetTab::Budgets => budgets_tab(ui, view, draft, outbox),
        BudgetTab::Loans => loans_tab(ui, view, outbox),
        BudgetTab::Land => land_tab(ui, view, draft, outbox),
    }
}

/// The three tiles: the treasury with its 7-day sparkline, the day and the week.
fn header(ui: &mut egui::Ui, view: &SimView) {
    let balance = view.treasury.balance;
    let week = &view.economy.last_week;
    ui.columns(3, |cols| {
        crate::power::tile(&mut cols[0], Icon::Budget, &tr("Treasury"), |ui| {
            ui.label(
                egui::RichText::new(format_money_short(balance))
                    .size(18.0)
                    .strong()
                    .color(debt_colour(debt_level(balance))),
            );
            let hist: Vec<f32> = view
                .city
                .history
                .iter()
                .map(|h| h.balance.0 as f32)
                .collect();
            crate::metrics::sparkline(ui, &hist, egui::vec2(ui.available_width(), 22.0));
        })
        .on_hover_text(format_money(balance));
        crate::power::tile(&mut cols[1], Icon::Metrics, &tr("Per day"), |ui| {
            let r = match daily_trend(view) {
                Some(d) => {
                    let colour = trend_colour(d.0 >= 0);
                    ui.horizontal(|ui| {
                        trend_arrow(ui, d.0 >= 0, colour);
                        ui.label(
                            egui::RichText::new(signed_short(d))
                                .size(18.0)
                                .strong()
                                .color(colour),
                        );
                    })
                    .response
                    .on_hover_text(format_money(d))
                }
                None => ui.weak(tr("no hour booked yet")),
            };
            r.on_hover_text(trend_breakdown(view));
        });
        crate::power::tile(
            &mut cols[2],
            Icon::DistrictTabTaxes,
            &tr("Week result"),
            |ui| {
                let net = week.balance();
                ui.label(
                    egui::RichText::new(signed_short(net))
                        .size(18.0)
                        .strong()
                        .color(trend_colour(net.0 >= 0)),
                )
                .on_hover_text(format_money(net));
            },
        )
        .on_hover_text(if week.week > 0 {
            trf!("Last week closed: week {week}", week = week.week)
        } else {
            tr("No week closed yet.")
        });
    });
}

/// `+$1.2k` or `−$56`.
fn signed_short(c: Cost) -> String {
    if c.0 >= 0 {
        format!("+{}", format_money_short(c))
    } else {
        format_money_short(c)
    }
}

/// The debt banner under the header: yellow below zero, red near the floor; the button opens
/// the Loans tab.
fn banner(ui: &mut egui::Ui, view: &SimView, draft: &mut BudgetDraft) {
    let balance = view.treasury.balance;
    let Some(text) = debt_warning(balance) else {
        return;
    };
    let colour = debt_colour(debt_level(balance));
    egui::Frame::new()
        .fill(colour.gamma_multiply(0.22))
        .stroke(egui::Stroke::new(1.0, colour))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(colour, text);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Loans")).clicked() {
                        draft.tab = BudgetTab::Loans;
                    }
                });
            });
        });
}

// ----- overview ---------------------------------------------------------------------------

/// One line of the Overview: an item and its amount in the chosen period.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub icon: Icon,
    pub name: String,
    pub amount: Cost,
}

/// The income and expense lines of `period`, biggest first; what is zero or not kept for that
/// span is left out.
pub fn period_lines(view: &SimView, period: Period) -> (Vec<Line>, Vec<Line>) {
    let day = view.flow.last_day(&view.economy).unwrap_or_default();
    let (income, expenses) =
        flow_rows(view, &day, &view.flow.week_to_date, &view.economy.last_week);
    let pick = |rows: Vec<FlowRow>| {
        let mut lines: Vec<Line> = rows
            .into_iter()
            .filter_map(|r| {
                let amount = r.money[period.column()].filter(|c| c.0 != 0)?;
                Some(Line {
                    icon: r.icon,
                    name: r.name,
                    amount,
                })
            })
            .collect();
        lines.sort_by_key(|l| std::cmp::Reverse(l.amount.0));
        lines
    };
    (pick(income), pick(expenses))
}

fn lines_total(lines: &[Line]) -> Cost {
    lines.iter().map(|l| l.amount).sum()
}

/// A horizontal stacked bar of `segments` against `scale` (the shared maximum), the total at
/// its right. A segment says its name and exact sum on hover.
fn stack_bar(
    ui: &mut egui::Ui,
    title: &str,
    segments: &[(egui::Color32, String, Cost)],
    scale: i64,
    total: Cost,
) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [70.0, 18.0],
            egui::Label::new(egui::RichText::new(title).strong()),
        );
        let width = (ui.available_width() - 80.0).max(60.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 16.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 3.0, egui::Color32::from_gray(34));
        let mut x = rect.left();
        for (i, (colour, name, amount)) in segments.iter().enumerate() {
            let w = width * amount.0.max(0) as f32 / scale.max(1) as f32;
            let seg =
                egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, rect.height()));
            ui.painter()
                .rect_filled(seg.shrink2(egui::vec2(0.0, 0.0)), 0.0, *colour);
            if i + 1 < segments.len() {
                ui.painter().line_segment(
                    [seg.right_top(), seg.right_bottom()],
                    egui::Stroke::new(1.0, egui::Color32::from_gray(30)),
                );
            }
            ui.interact(seg, ui.id().with((title, i)), egui::Sense::hover())
                .on_hover_text(format!("{name}: {}", format_money(*amount)));
            x += w;
        }
        money_label_short(ui, total);
    });
}

/// One list row: icon, name and short sum over a bar `frac` long in the item's colour.
fn money_row(ui: &mut egui::Ui, line: &Line, colour: egui::Color32, frac: f32, tip: &str) {
    let w = ui.available_width();
    let bg = ui.painter().add(egui::Shape::Noop);
    let r = ui.horizontal(|ui| {
        ui.set_min_size(egui::vec2(w, 22.0));
        line.icon.show(ui, Sizes::SECTION_ICON);
        ui.label(&line.name);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format_money_short(line.amount));
        });
    });
    let rect = r.response.rect;
    ui.painter().set(
        bg,
        egui::Shape::rect_filled(
            egui::Rect::from_min_size(
                rect.min,
                egui::vec2(w * frac.clamp(0.0, 1.0), rect.height()),
            ),
            3.0,
            colour.gamma_multiply(0.22),
        ),
    );
    r.response
        .on_hover_text(format!("{}\n{tip}", format_money(line.amount)).trim_end());
}

/// The Overview tab: the period switch, the income and expense bars on one scale, the two lists
/// and the treasury over 7 days.
pub fn overview_tab(ui: &mut egui::Ui, view: &SimView, draft: &mut BudgetDraft) {
    ui.horizontal(|ui| {
        for p in Period::ALL {
            if ui
                .selectable_label(draft.period == p, tr(p.label()))
                .clicked()
            {
                draft.period = p;
            }
        }
    });
    let (income, expenses) = period_lines(view, draft.period);
    let (total_in, total_out) = (lines_total(&income), lines_total(&expenses));
    let scale = total_in.0.max(total_out.0);
    let segs = |lines: &[Line], offset: usize| -> Vec<(egui::Color32, String, Cost)> {
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| (item_colour(i + offset), l.name.clone(), l.amount))
            .collect()
    };
    stack_bar(ui, &tr("Income"), &segs(&income, 0), scale, total_in);
    stack_bar(ui, &tr("Expenses"), &segs(&expenses, 3), scale, total_out);
    ui.add_space(4.0);
    let roads = overview(view).roads;
    ui.columns(2, |cols| {
        for (col, lines, offset) in [(0usize, &income, 0usize), (1, &expenses, 3)] {
            let ui = &mut cols[col];
            if lines.is_empty() {
                ui.weak(tr("Nothing booked yet."));
            }
            for (i, line) in lines.iter().enumerate() {
                let tip = if line.icon == Icon::Roads && col == 1 {
                    roads_tip(&roads)
                } else {
                    String::new()
                };
                let frac = line.amount.0 as f32 / scale.max(1) as f32;
                money_row(ui, line, item_colour(i + offset), frac, &tip);
            }
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        Icon::Metrics.show(ui, Sizes::SECTION_ICON);
        ui.strong(tr("Treasury, 7 days"));
    });
    let samples: Vec<(u64, f32)> = view
        .city
        .history
        .iter()
        .map(|h| (h.tick, h.balance.0 as f32))
        .collect();
    treasury_chart(ui, &samples, egui::vec2(ui.available_width(), 110.0));
}

/// The roads' weekly upkeep by class, for the tooltip of the Roads line.
fn roads_tip(roads: &[RoadRow]) -> String {
    roads
        .iter()
        .map(|r| {
            trf!(
                "{name}: {money} a week",
                name = tr(&r.name),
                money = format_money_short(r.upkeep_week)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The treasury over the hourly history: the area under the line, the zero line, the debt floor
/// (dashed) when it is in range, a vertical at each game day, and the value under the pointer.
pub fn treasury_chart(ui: &mut egui::Ui, samples: &[(u64, f32)], size: egui::Vec2) {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, egui::Color32::from_gray(26));
    if samples.len() < 2 {
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            tr("No history yet."),
            egui::FontId::proportional(13.0),
            egui::Color32::GRAY,
        );
        return;
    }
    let floor = DEBT_FLOOR.0 as f32;
    let (mut lo, mut hi) = samples
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &(_, v)| {
            (lo.min(v), hi.max(v))
        });
    // The debt floor is in the scale only once the treasury has gone far below zero.
    if lo < floor * 0.5 {
        lo = lo.min(floor);
    }
    let pad = ((hi - lo) * 0.08)
        .max(hi.abs().max(lo.abs()) * 0.002)
        .max(100.0);
    (lo, hi) = (lo - pad, hi + pad);
    let plot = rect.shrink2(egui::vec2(6.0, 6.0));
    let y_of = |v: f32| plot.bottom() - plot.height() * (v - lo) / (hi - lo);
    let (t0, t1) = (
        samples[0].0,
        samples[samples.len() - 1].0.max(samples[0].0 + 1),
    );
    let x_of = |t: u64| plot.left() + plot.width() * (t - t0) as f32 / (t1 - t0) as f32;
    // Verticals at each game-day boundary.
    let mut day = t0 / TICKS_PER_DAY + 1;
    while day * TICKS_PER_DAY < t1 {
        let x = x_of(day * TICKS_PER_DAY);
        p.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            egui::Stroke::new(1.0, egui::Color32::from_gray(44)),
        );
        day += 1;
    }
    // The area hangs from the zero line, or from the plot's edge when zero is out of range.
    let zero = y_of(0.0).clamp(plot.top(), plot.bottom());
    for w in samples.windows(2) {
        let ((ta, va), (tb, vb)) = (w[0], w[1]);
        let colour = if va + vb >= 0.0 { GOOD } else { BAD }.gamma_multiply(0.25);
        p.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(x_of(ta), y_of(va)),
                egui::pos2(x_of(tb), y_of(vb)),
                egui::pos2(x_of(tb), zero),
                egui::pos2(x_of(ta), zero),
            ],
            colour,
            egui::Stroke::NONE,
        ));
    }
    if lo < 0.0 && hi > 0.0 {
        p.line_segment(
            [
                egui::pos2(plot.left(), zero),
                egui::pos2(plot.right(), zero),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_gray(120)),
        );
    }
    if floor > lo {
        let y = y_of(floor);
        p.extend(egui::Shape::dashed_line(
            &[egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0, BAD),
            5.0,
            4.0,
        ));
    }
    let line: Vec<egui::Pos2> = samples
        .iter()
        .map(|&(t, v)| egui::pos2(x_of(t), y_of(v)))
        .collect();
    p.add(egui::Shape::line(line, egui::Stroke::new(1.8, BLUE)));
    if let Some(pos) = response.hover_pos() {
        let t = t0 + ((pos.x - plot.left()) / plot.width() * (t1 - t0) as f32).max(0.0) as u64;
        let i = samples
            .partition_point(|&(st, _)| st < t)
            .min(samples.len() - 1);
        let (st, v) = samples[i];
        let x = x_of(st);
        p.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            egui::Stroke::new(1.0, egui::Color32::from_gray(200)),
        );
        p.circle_filled(egui::pos2(x, y_of(v)), 3.0, egui::Color32::WHITE);
        let hour = st % TICKS_PER_DAY * 24 / TICKS_PER_DAY;
        response.on_hover_text_at_pointer(trf!(
            "{money} · day {day}, {hour}:00",
            money = format_money(Cost(v as i64)),
            day = st / TICKS_PER_DAY + 1,
            hour = hour
        ));
    }
}

// ----- kept helpers ------------------------------------------------------------------------

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
    /// Tax income of last week per zone type, in [`TAXED_ZONES`] order; a zone no milestone has
    /// opened only if it brought money (BUG-123).
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
    /// Last week's tourism income (part of `income`), T-168.
    pub tourism: Cost,
    /// Last week's hotel nights (part of `income`), T-321.
    pub hotels: Cost,
    /// Last week's export income (part of `income`), T-170.
    pub exports: Cost,
    /// Last week's policy costs (part of `upkeep`), T-190.
    pub policies: Cost,
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
            .filter(|&(z, c)| zone_open(view, z) || c != Cost::ZERO)
            .collect(),
        income: week.income() + week.transit_fares + week.tourism + week.hotels + week.exports,
        upkeep: week.upkeep() + week.loan_payments + week.transit_upkeep + week.policies,
        net: week.balance(),
        roads_sum: roads.iter().map(|r| r.upkeep_week).sum(),
        roads,
        road_upkeep: week.road_upkeep,
        building_upkeep: week.building_upkeep,
        utility_upkeep: week.utility_upkeep,
        loan_payments: week.loan_payments,
        transit_fares: week.transit_fares,
        transit_upkeep: week.transit_upkeep,
        tourism: week.tourism,
        hotels: week.hotels,
        exports: week.exports,
        policies: week.policies,
        balance_history: view
            .city
            .history
            .iter()
            .map(|h| h.balance.0 as f32)
            .collect(),
    }
}

/// The budget rows the tab shows (§20.3): the [`BUDGETED`] kinds a milestone has opened
/// ([`cp_sim::kind_open`]); before the content is in, the four of the start.
pub fn budget_rows(view: &SimView) -> Vec<ServiceKind> {
    BUDGETED
        .into_iter()
        .filter(|&k| service_open(view, k))
        .collect()
}

/// Whether a milestone has opened service `kind` ([`cp_sim::kind_open`]); before the content
/// is in, the four of the start. The budget window shows nothing of a closed one (BUG-123).
pub fn service_open(view: &SimView, kind: ServiceKind) -> bool {
    match view.content.as_deref() {
        Some(db) => cp_sim::kind_open(&view.progress, db, kind),
        None => matches!(
            kind,
            ServiceKind::Water | ServiceKind::Sewage | ServiceKind::Power | ServiceKind::Roads
        ),
    }
}

/// The sectors behind a budget row's balance bar: its own; Education the three school levels,
/// Post collecting and sorting; none for Roads, Transit and Park.
pub fn budget_sectors(kind: ServiceKind) -> &'static [Sector] {
    match kind {
        ServiceKind::Water => &[Sector::Water],
        ServiceKind::Sewage => &[Sector::Sewage],
        ServiceKind::Power => &[Sector::Power],
        ServiceKind::Garbage => &[Sector::Garbage],
        ServiceKind::Health => &[Sector::Health],
        ServiceKind::Deathcare => &[Sector::Deathcare],
        ServiceKind::Police => &[Sector::Police],
        ServiceKind::Fire => &[Sector::Fire],
        ServiceKind::Education => &[Sector::Elementary, Sector::HighSchool, Sector::University],
        ServiceKind::Post => &[Sector::Mail, Sector::Sorting],
        _ => &[],
    }
}

/// The balance (`-1..=1`) a budget row's bar shows: the worst of its open sectors, `None`
/// without one.
pub fn budget_balance(view: &SimView, kind: ServiceKind) -> Option<f32> {
    let sectors = budget_sectors(kind);
    view.city
        .services
        .iter()
        .filter(|r| !r.locked && sectors.contains(&r.sector))
        .map(|r| r.balance())
        .reduce(f32::min)
}

/// What `kind` cost over the last day, as a week (×7): its buildings' upkeep, and the roads
/// for Roads, the buses for Transit. `None` before the first hour was booked.
pub fn week_cost(view: &SimView, kind: ServiceKind) -> Option<Cost> {
    let day = view.flow.last_day(&view.economy)?;
    let i = SERVICE_ORDER.iter().position(|&k| k == kind)?;
    let mut c = day.services[i];
    match kind {
        ServiceKind::Roads => c += day.road_upkeep,
        ServiceKind::Transit => c += day.transit_upkeep,
        _ => {}
    }
    Some(Cost(c.0 * 7))
}

/// A week's cost at budget `old` restated at `new`: upkeep is linear in the budget (§20.1).
pub fn cost_at(cost: Cost, old: u8, new: u8) -> Cost {
    Cost((cost.0 as f64 * f64::from(new) / f64::from(old.max(1))).round() as i64)
}

/// The effect of `pct` on `kind` in words: "capacity ×0.75 · reach ×0.87" (the sim's factors);
/// Roads "Upkeep only".
pub fn effect_text(kind: ServiceKind, pct: u8, bal: &cp_content::Balance) -> String {
    let f = format!("{:.2}", power_factor(pct, bal));
    let r = format!("{:.2}", reach_factor(pct, bal));
    match kind {
        ServiceKind::Roads => tr("Upkeep only"),
        ServiceKind::Water | ServiceKind::Sewage | ServiceKind::Power => {
            trf!("capacity ×{f}", f = f)
        }
        ServiceKind::Transit => trf!("buses ×{f}", f = f),
        ServiceKind::Park => trf!("attractiveness ×{f}", f = f),
        _ => trf!("capacity ×{f} · reach ×{r}", f = f, r = r),
    }
}

/// The Transit row's hint at budget `pct`: the buses the lines run of those assigned.
pub fn transit_hint(view: &SimView, pct: u8, bal: &cp_content::Balance) -> String {
    let (mut running, mut assigned) = (0usize, 0usize);
    for l in &view.lines {
        assigned += usize::from(l.vehicles);
        running += cp_content::budget::budgeted(u32::from(l.vehicles), pct, bal) as usize;
    }
    trf!(
        "Buses on the road: {n} of {m} assigned",
        n = running,
        m = assigned
    )
}

// ----- taxes ------------------------------------------------------------------------------

/// A cell of fixed width in a row of controls.
fn cell<R>(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 22.0),
        egui::Layout::left_to_right(egui::Align::Center),
        add,
    )
    .inner
}

/// A week's tax at rate `new`, from the week's income at rate `old`: income is linear in the
/// rate. `old` of zero does not divide by zero.
pub fn tax_estimate(income: Cost, old: u8, new: u8) -> Cost {
    cost_at(income, old, new)
}

/// The colour of a demand bar: grey while nobody wants it, green, orange when it is hot.
fn demand_colour(v: f32) -> egui::Color32 {
    if v < 20.0 {
        egui::Color32::from_gray(130)
    } else if v < 60.0 {
        GOOD
    } else {
        COST
    }
}

/// The tooltip of a tax slider: the rate before and after, the estimate and, for homes, the
/// contentment factor "taxes" before and after (`tax_score`).
pub fn tax_tip(view: &SimView, zone: ZoneType, current: u8, shown: u8, income: Cost) -> String {
    let mut out = trf!(
        "{old}% -> {new}%\n≈ {money} a week (was {was})",
        old = current,
        new = shown,
        money = format_money(tax_estimate(income, current, shown)),
        was = format_money(income)
    );
    let homes = matches!(zone, ZoneType::ResLow | ZoneType::ResHigh);
    if homes {
        let knobs = view
            .content
            .as_deref()
            .map(|db| db.balance().contentment.clone())
            .unwrap_or_default();
        out += &trf!(
            "\nContentment, factor “taxes”: {old} -> {new}",
            old = tax_score(current, &knobs),
            new = tax_score(shown, &knobs)
        );
    } else {
        out += &tr("\nTaxes do not change the contentment of these zones.");
    }
    out
}

/// The Taxes tab: the week's taxes and their share, a row per open zone (slider with the
/// neutral-rate tick, the estimate at the slider's rate, the zone's current demand) and the
/// zones a milestone has not opened yet as chips.
fn taxes_tab(
    ui: &mut egui::Ui,
    view: &SimView,
    draft: &mut BudgetDraft,
    outbox: &mut CommandOutbox,
) {
    let (state, week) = (&view.economy, &view.economy.last_week);
    let taxes = week.income();
    let total = lines_total(&period_lines(view, Period::LastWeek).0);
    let share = if total.0 > 0 {
        (taxes.0 as f64 * 100.0 / total.0 as f64).round() as i64
    } else {
        0
    };
    ui.horizontal(|ui| {
        Icon::DistrictTabTaxes.show(ui, Sizes::SECTION_ICON);
        ui.strong(trf!(
            "Taxes for the week {money} · {share}% of income",
            money = format_money_short(taxes),
            share = share
        ))
        .on_hover_text(format_money(taxes));
    });
    let knobs = view
        .content
        .as_deref()
        .map(|db| db.balance().contentment.clone())
        .unwrap_or_default();
    ui.add_space(4.0);
    for (i, zone) in TAXED_ZONES.into_iter().enumerate() {
        if !zone_open(view, zone) {
            continue;
        }
        let current = state.taxes.get(zone).unwrap_or(0);
        let income = week.income_by_zone[i];
        ui.horizontal(|ui| {
            cell(ui, 150.0, |ui| zone_label(ui, zone));
            let look = SliderLook {
                width: 190.0,
                tick: Some(knobs.tax_neutral),
                warn_above: true,
                reset: None,
            };
            let out = cell(ui, 220.0, |ui| {
                pct_slider(ui, current, &mut draft.taxes[i], TAX_MIN..=TAX_MAX, look)
            });
            if let Some(pct) = out.send {
                outbox.push(tax_command(zone, pct));
            }
            let shown = out.shown;
            cell(ui, 44.0, |ui| {
                let colour = if shown > knobs.tax_neutral {
                    COST
                } else {
                    ui.visuals().text_color()
                };
                ui.label(egui::RichText::new(format!("{shown}%")).color(colour));
            });
            cell(ui, 110.0, |ui| {
                ui.label(trf!(
                    "≈ {money} a week",
                    money = format_money_short(tax_estimate(income, current, shown))
                ))
                .on_hover_text(trf!(
                    "was {money} at {pct}%",
                    money = format_money(income),
                    pct = current
                ));
            });
            let (_, demand) = crate::panels::zone_group_demand(zone, &view.demand);
            cell(ui, 100.0, |ui| {
                ui.add(
                    egui::ProgressBar::new(demand_fraction(demand))
                        .desired_width(90.0)
                        .desired_height(8.0)
                        .fill(demand_colour(demand)),
                )
                .on_hover_text(trf!("Demand now: {n}", n = format!("{demand:.0}")));
            });
            let tip = tax_tip(view, zone, current, shown, income);
            if out.response.dragged() {
                egui::Tooltip::always_open(
                    ui.ctx().clone(),
                    ui.layer_id(),
                    out.response.id,
                    egui::PopupAnchor::Pointer,
                )
                .show(|ui| {
                    ui.label(tip);
                });
            } else {
                out.response.on_hover_text(tip);
            }
        });
    }
    let later = closed_zones(view);
    if !later.is_empty() {
        ui.add_space(6.0);
        ui.weak(tr("Open later:"));
        ui.horizontal_wrapped(|ui| {
            for (zone, pop) in later {
                crate::power::chip(
                    ui,
                    crate::icons::zone_icon(zone),
                    &format!(
                        "{} · {}",
                        tr(zone_name(zone)),
                        crate::buildings::thousands(pop)
                    ),
                    None,
                );
            }
        });
    }
}

/// The taxed zones no milestone has opened yet, with the residents the milestone needs.
pub fn closed_zones(view: &SimView) -> Vec<(ZoneType, u32)> {
    let Some(db) = view.content.as_deref() else {
        return Vec::new();
    };
    TAXED_ZONES
        .into_iter()
        .filter(|&z| !zone_open(view, z))
        .filter_map(|z| {
            let m = db.zone_unlock_milestone(z)?;
            Some((z, db.milestones().get(m)?.population))
        })
        .collect()
}

/// A zone's name after its square icon of the zoning palette (BUG-123).
fn zone_label(ui: &mut egui::Ui, zone: ZoneType) {
    ui.horizontal(|ui| {
        crate::icons::zone_icon(zone).show(ui, Sizes::SECTION_ICON);
        ui.label(tr(zone_name(zone)));
    });
}

// ----- services ---------------------------------------------------------------------------

/// The groups of the Services tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceGroup {
    Utilities,
    City,
    Transport,
    Leisure,
}

impl ServiceGroup {
    const ALL: [ServiceGroup; 4] = [
        ServiceGroup::Utilities,
        ServiceGroup::City,
        ServiceGroup::Transport,
        ServiceGroup::Leisure,
    ];

    fn label(self) -> &'static str {
        match self {
            ServiceGroup::Utilities => "Utilities",
            ServiceGroup::City => "City services",
            ServiceGroup::Transport => "Transport",
            ServiceGroup::Leisure => "Leisure",
        }
    }
}

pub fn service_group(kind: ServiceKind) -> ServiceGroup {
    match kind {
        ServiceKind::Water | ServiceKind::Sewage | ServiceKind::Power => ServiceGroup::Utilities,
        ServiceKind::Roads | ServiceKind::Transit => ServiceGroup::Transport,
        ServiceKind::Park => ServiceGroup::Leisure,
        _ => ServiceGroup::City,
    }
}

/// The open budget rows by group, in [`SERVICE_ORDER`] inside a group; empty groups are left
/// out.
pub fn budget_groups(view: &SimView) -> Vec<(ServiceGroup, Vec<ServiceKind>)> {
    let rows = budget_rows(view);
    ServiceGroup::ALL
        .into_iter()
        .filter_map(|g| {
            let mut kinds: Vec<ServiceKind> = rows
                .iter()
                .copied()
                .filter(|&k| service_group(k) == g)
                .collect();
            kinds.sort_by_key(|k| {
                SERVICE_ORDER
                    .iter()
                    .position(|o| o == k)
                    .unwrap_or(usize::MAX)
            });
            (!kinds.is_empty()).then_some((g, kinds))
        })
        .collect()
}

/// The open sector of `kind` with the least supply against its demand.
fn tightest_sector(
    view: &SimView,
    kind: ServiceKind,
) -> Option<&cp_sim::city_services::SectorBalance> {
    let sectors = budget_sectors(kind);
    view.city
        .services
        .iter()
        .filter(|r| !r.locked && sectors.contains(&r.sector))
        .min_by(|a, b| coverage_of(a).total_cmp(&coverage_of(b)))
}

fn coverage_of(r: &cp_sim::city_services::SectorBalance) -> f32 {
    r.supply / r.demand.max(1.0)
}

/// How much of its demand a service's sector supplies: `supply / demand.max(1)` of the
/// tightest open sector, not clamped (`1.0` = exactly enough). `None` without a sector (roads,
/// transit, parks).
pub fn budget_coverage(view: &SimView, kind: ServiceKind) -> Option<f32> {
    tightest_sector(view, kind).map(coverage_of)
}

/// The colour of a coverage: under 90 % red, to 105 % yellow, above green.
pub fn coverage_colour(ratio: f32) -> egui::Color32 {
    if ratio < 0.9 {
        BAD
    } else if ratio <= 1.05 {
        WARN
    } else {
        GOOD
    }
}

/// The coverage bar: filled to `ratio` of 150 %, a tick at 100 %; the label says the percent.
fn coverage_bar(ui: &mut egui::Ui, view: &SimView, kind: ServiceKind) {
    let Some(sector) = tightest_sector(view, kind) else {
        ui.weak("—");
        return;
    };
    let ratio = coverage_of(sector);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(90.0, 8.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, egui::Color32::from_gray(40));
    let w = rect.width() * (ratio / 1.5).clamp(0.0, 1.0);
    p.rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(w, rect.height())),
        3.0,
        coverage_colour(ratio),
    );
    let x = rect.left() + rect.width() / 1.5;
    p.line_segment(
        [
            egui::pos2(x, rect.top() - 2.0),
            egui::pos2(x, rect.bottom() + 2.0),
        ],
        egui::Stroke::new(1.5, egui::Color32::WHITE),
    );
    ui.label(egui::RichText::new(format!("{:.0}%", ratio * 100.0)).color(coverage_colour(ratio)));
    response.on_hover_ui(|ui| {
        ui.label(trf!(
            "Supplies {supply} — needs {demand} — margin {margin}%",
            supply = format!("{:.0} {}", sector.supply, sector.unit),
            demand = format!("{:.0} {}", sector.demand, sector.unit),
            margin = format!("{:+.0}", (ratio - 1.0) * 100.0)
        ));
        for (frac, colour) in [
            (sector.supply, coverage_colour(ratio)),
            (sector.demand, egui::Color32::from_gray(160)),
        ] {
            let top = sector.supply.max(sector.demand).max(1.0);
            ui.add(
                egui::ProgressBar::new(frac / top)
                    .desired_width(160.0)
                    .desired_height(6.0)
                    .fill(colour),
            );
        }
    });
}

/// The tooltip of a service's name: what its budget does in words, the cost against 100 % and,
/// for the police, the bribery risk.
pub fn service_tip(
    view: &SimView,
    kind: ServiceKind,
    current: u8,
    shown: u8,
    bal: &cp_content::Balance,
) -> String {
    let mut out = effect_text(kind, shown, bal);
    if kind == ServiceKind::Transit {
        out += &format!("\n{}", transit_hint(view, shown, bal));
    }
    if let Some(c) = week_cost(view, kind).filter(|c| c.0 != 0) {
        let diff = cost_at(c, current, shown).0 - cost_at(c, current, 100).0;
        let sign = if diff >= 0 { "+" } else { "−" };
        out += &trf!(
            "\nAgainst 100%: {sign}{money} a week",
            sign = sign,
            money = format_money(Cost(diff.abs()))
        );
    }
    if kind == ServiceKind::Police
        && let Some(db) = view.content.as_deref()
    {
        out += &format!(
            "\n{}",
            crate::axis::police_terms(&db.balance().corruption, shown)
        );
    }
    out
}

/// The Services tab (§20.3): the weekly cost of the services as one bar, then the open rows by
/// group: icon and name, the slider (a tick at 100), the percent, the week's cost at it and the
/// coverage of the sector.
fn budgets_tab(
    ui: &mut egui::Ui,
    view: &SimView,
    draft: &mut BudgetDraft,
    outbox: &mut CommandOutbox,
) {
    let fallback = cp_content::Balance::default();
    let bal = view.content.as_deref().map_or(&fallback, |db| db.balance());
    let segs: Vec<(egui::Color32, String, Cost)> = budget_rows(view)
        .into_iter()
        .enumerate()
        .filter_map(|(i, k)| {
            let c = week_cost(view, k).filter(|c| c.0 > 0)?;
            Some((
                item_colour(i),
                tr(crate::buildings::kind_label(k).as_str()),
                c,
            ))
        })
        .collect();
    let total: Cost = segs.iter().map(|s| s.2).sum();
    stack_bar(ui, &tr("Per week"), &segs, total.0, total);
    ui.add_space(2.0);
    // Many rows on a small screen scroll, clear of the bottom bar.
    let groups = budget_groups(view);
    let rows: usize = groups.iter().map(|(_, k)| k.len() + 1).sum();
    let free = ui.ctx().content_rect().height() * 0.62;
    let body = |ui: &mut egui::Ui| {
        for (group, kinds) in groups {
            ui.weak(tr(group.label()));
            for kind in kinds {
                service_row(ui, view, draft, outbox, bal, kind);
            }
        }
    };
    if rows as f32 * 30.0 <= free {
        body(ui);
    } else {
        egui::ScrollArea::vertical()
            .id_salt("budget_rows")
            .max_height(free.max(240.0))
            .auto_shrink([false, false])
            .show(ui, body);
    }
}

fn service_row(
    ui: &mut egui::Ui,
    view: &SimView,
    draft: &mut BudgetDraft,
    outbox: &mut CommandOutbox,
    bal: &cp_content::Balance,
    kind: ServiceKind,
) {
    let Some(i) = BUDGETED.iter().position(|&k| k == kind) else {
        return;
    };
    let current = view.budgets.get(kind);
    let mut frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(4, 1));
    if draft.highlight == Some(kind) {
        frame = frame
            .fill(ui.visuals().selection.bg_fill.gamma_multiply(0.35))
            .corner_radius(4.0);
    }
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            let name = tr(crate::buildings::kind_label(kind).as_str());
            let shown_now = draft.budgets[i].map_or(current, |d| d.value);
            let tip = service_tip(view, kind, current, shown_now, bal);
            cell(ui, 150.0, |ui| {
                kind_row_label(ui, kind, &name, &tip);
            });
            let look = SliderLook {
                width: 170.0,
                tick: Some(100),
                warn_above: false,
                reset: Some(100),
            };
            let out = cell(ui, 200.0, |ui| {
                pct_slider(
                    ui,
                    current,
                    &mut draft.budgets[i],
                    BUDGET_MIN..=BUDGET_MAX,
                    look,
                )
            });
            if let Some(pct) = out.send {
                outbox.push(budget_command(kind, pct));
            }
            let shown = out.shown;
            cell(ui, 62.0, |ui| {
                let colour = match shown.cmp(&100) {
                    std::cmp::Ordering::Less => COST,
                    std::cmp::Ordering::Equal => ui.visuals().text_color(),
                    std::cmp::Ordering::Greater => BLUE,
                };
                ui.label(egui::RichText::new(format!("{shown}%")).color(colour));
                if shown != 100
                    && ui
                        .small_button("↺")
                        .on_hover_text(tr("Back to 100%"))
                        .clicked()
                {
                    draft.budgets[i] = Some(Draft {
                        value: 100,
                        sent: true,
                    });
                    outbox.push(budget_command(kind, 100));
                }
            });
            cell(ui, 84.0, |ui| {
                if let Some(c) = week_cost(view, kind).filter(|c| c.0 != 0) {
                    let c = cost_at(c, current, shown);
                    crate::power::chip(ui, Icon::Budget, &format_money_short(c), Some(false))
                        .on_hover_text(trf!("-{money} / week", money = format_money(c)));
                }
            });
            cell(ui, 140.0, |ui| coverage_bar(ui, view, kind));
        });
    });
}

fn kind_row_label(ui: &mut egui::Ui, kind: ServiceKind, name: &str, tip: &str) {
    ui.horizontal(|ui| {
        crate::buildings::kind_icon(kind).show(ui, Sizes::SECTION_ICON);
        ui.label(name);
    })
    .response
    .on_hover_text(tip);
}

// ----- loans ------------------------------------------------------------------------------

/// The payments' share of the week's income that the bar's tick marks.
pub const LOAN_SHARE_LIMIT: f32 = 0.30;

/// Weekly loan payments as a share of the week's income; with payments but no income, 1.
pub fn loan_share(payments: Cost, income: Cost) -> f32 {
    if payments.0 <= 0 {
        0.0
    } else if income.0 <= 0 {
        1.0
    } else {
        payments.0 as f32 / income.0 as f32
    }
}

/// Under 15 % green, to 30 % yellow, above red.
pub fn loan_share_colour(share: f32) -> egui::Color32 {
    if share < 0.15 {
        GOOD
    } else if share <= LOAN_SHARE_LIMIT {
        WARN
    } else {
        BAD
    }
}

/// What a loan costs over its term above the sum borrowed.
pub fn loan_overpay(weekly: Cost, weeks: u16, amount: Cost) -> Cost {
    Cost(weekly.0 * i64::from(weeks) - amount.0)
}

/// The Loans tab (T-130): the debt and payment tiles, the bank's offers as cards with a Take
/// button, then the active loans with a pay-off button. Only sends `TakeLoan` / `RepayLoan`;
/// the sim checks the rest.
fn loans_tab(ui: &mut egui::Ui, view: &SimView, outbox: &mut CommandOutbox) {
    let offers = view.loan_offers();
    let debt: Cost = view
        .economy
        .loans
        .iter()
        .map(|l| l.principal_left + l.interest_left)
        .sum();
    let weekly: Cost = view.economy.loans.iter().map(|l| l.weekly()).sum();
    let income = lines_total(&period_lines(view, Period::LastWeek).0);
    let share = loan_share(weekly, income);
    ui.columns(2, |cols| {
        crate::power::tile(&mut cols[0], Icon::Budget, &tr("Debt to the bank"), |ui| {
            ui.label(
                egui::RichText::new(format_money_short(debt))
                    .size(18.0)
                    .strong(),
            )
            .on_hover_text(format_money(debt));
        });
        crate::power::tile(
            &mut cols[1],
            Icon::DealMoney,
            &trf!(
                "Payments · {share}% of income",
                share = (share * 100.0).round() as i64
            ),
            |ui| {
                crate::power::bar(
                    ui,
                    share / (2.0 * LOAN_SHARE_LIMIT),
                    Some(0.5),
                    loan_share_colour(share),
                );
                ui.label(trf!("{money} a week", money = format_money_short(weekly)))
                    .on_hover_text(format_money(weekly));
            },
        );
    });
    ui.add_space(4.0);
    ui.strong(tr("Bank offers"));
    if offers.is_empty() {
        ui.weak(tr("The bank has no offers."));
    }
    for o in &offers {
        let amount = Cost::from_units(o.offer.amount);
        let weekly = Loan::new(o.index, &o.offer, view.tick).weekly();
        let taken = o.active.is_some();
        let closed = !o.available && !taken;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_enabled_ui(!closed, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format_money_short(amount))
                            .size(18.0)
                            .strong(),
                    )
                    .on_hover_text(format_money(amount));
                    crate::power::chip(
                        ui,
                        Icon::Info,
                        &trf!("rate {pct}%", pct = o.offer.interest_pct),
                        None,
                    );
                    crate::power::chip(
                        ui,
                        Icon::Deadline,
                        &trf!(plural "{n} weeks", n = o.offer.weeks),
                        None,
                    );
                    crate::power::chip(
                        ui,
                        Icon::DealMoney,
                        &trf!("−{money} a week", money = format_money_short(weekly)),
                        Some(false),
                    )
                    .on_hover_text(format_money(weekly));
                    let over = loan_overpay(weekly, o.offer.weeks, amount);
                    crate::power::chip(
                        ui,
                        Icon::Budget,
                        &trf!("overpay {money}", money = format_money_short(over)),
                        None,
                    )
                    .on_hover_text(format_money(over));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if taken {
                            ui.add_enabled(false, egui::Button::new(tr("Taken")))
                                .on_disabled_hover_text(tr(
                                    "Already taken; pay it off to take it again.",
                                ));
                        } else if o.available {
                            if ui.button(tr("Take")).clicked() {
                                outbox.push(take_loan_command(o.index));
                            }
                        }
                    });
                });
            });
            if closed {
                ui.weak(trf!(
                    plural "The bank wants {n} residents.",
                    n = o.offer.min_population
                ));
            }
        });
    }
    ui.separator();
    ui.strong(tr("Active loans"));
    if view.economy.loans.is_empty() {
        ui.weak(tr("No active loans."));
    }
    for l in &view.economy.loans {
        let offer = offers.iter().find(|o| o.index == l.offer).map(|o| &o.offer);
        let amount = offer.map_or(l.principal_left, |o| Cost::from_units(o.amount));
        let total_weeks = offer.map_or(l.weeks_left, |o| o.weeks).max(1);
        let paid = 1.0 - f32::from(l.weeks_left) / f32::from(total_weeks);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format_money_short(amount))
                        .size(16.0)
                        .strong(),
                )
                .on_hover_text(format_money(amount));
                ui.label(trf!(
                    plural "left {money} · {n} weeks",
                    n = l.weeks_left,
                    money = format_money_short(l.principal_left + l.interest_left)
                ))
                .on_hover_text(trf!(
                    "Debt left {debt}, interest left {interest}, {money} a week",
                    debt = format_money(l.principal_left),
                    interest = format_money(l.interest_left),
                    money = format_money(l.weekly())
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can = view.treasury.balance >= l.principal_left;
                    let r = ui.add_enabled(
                        can,
                        egui::Button::new(trf!(
                            "Pay off {money}",
                            money = format_money_short(l.principal_left)
                        )),
                    );
                    let r = if can {
                        r.on_hover_text(format_money(l.principal_left))
                    } else {
                        r.on_disabled_hover_text(trf!(
                            "Not enough: {money} short",
                            money = format_money(l.principal_left - view.treasury.balance)
                        ))
                    };
                    if r.clicked() {
                        outbox.push(repay_loan_command(l.offer));
                    }
                });
            });
            ui.add(
                egui::ProgressBar::new(paid.clamp(0.0, 1.0))
                    .desired_height(6.0)
                    .fill(GOOD),
            );
        });
    }
}

// ----- land -------------------------------------------------------------------------------

/// The Land tab: the tile grid and, beside it, the card of the picked tile with the purchase.
fn land_tab(
    ui: &mut egui::Ui,
    view: &SimView,
    draft: &mut BudgetDraft,
    outbox: &mut CommandOutbox,
) {
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| crate::tiles::tile_section(ui, view, &mut draft.land_pick));
        ui.separator();
        ui.vertical(|ui| {
            ui.set_min_width(220.0);
            let owned = (0..TILES_PER_SIDE as u8)
                .flat_map(|x| (0..TILES_PER_SIDE as u8).map(move |z| (x, z)))
                .filter(|&t| tile_state(view, t) == TileState::Owned)
                .count();
            ui.strong(trf!(
                "Owned: {n} of {m}",
                n = owned,
                m = TILES_PER_SIDE * TILES_PER_SIDE
            ));
            ui.add_space(6.0);
            match draft.land_pick.map(|t| (t, tile_state(view, t))) {
                Some((tile, TileState::Buyable(price))) => {
                    ui.label(trf!("Plot {x}·{z}", x = tile.0 + 1, z = tile.1 + 1));
                    ui.label(
                        egui::RichText::new(format_money_short(price))
                            .size(18.0)
                            .strong(),
                    )
                    .on_hover_text(format_money(price));
                    if ui
                        .button(trf!("Buy for {price}", price = format_money_short(price)))
                        .clicked()
                    {
                        outbox.push(SimCommand::Terrain(TerrainCommand::BuyTile { tile }));
                        draft.land_pick = None;
                    }
                }
                _ => {
                    ui.weak(tr("Pick a plot next to your land."));
                }
            }
        });
    });
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

    /// BUG-040: the top bar's money trend is the net of the last 24 hours, per day.
    #[test]
    fn the_money_trend_shows_sign_and_day_amount_and_the_population_trend_its_balance() {
        let mut view = SimRunner::new(1).view();
        assert_eq!(daily_trend(&view), None, "before the first hour");
        let mut f = Flow::default();
        f.income_by_zone[0] = Cost(100);
        f.road_upkeep = Cost(30);
        view.flow.hours.push_back((1, f));
        // One hour is scaled up to the day: (100 - 30) × 24.
        assert_eq!(daily_trend(&view), Some(Cost(1_680)));
        assert_eq!(trend_text(Cost(3_850)), "+$38/day");
        assert_eq!(trend_text(Cost(-12_345)), "-$123/day");
        assert!(trend_breakdown(&view).contains("Roads -"));

        let city = std::sync::Arc::make_mut(&mut view.city);
        city.pop_day = cp_citizen::PopFlow {
            arrived: 12,
            born: 3,
            left: 4,
            died: 1,
        };
        assert_eq!(population_trend(&view).1, "+10");
        let city = std::sync::Arc::make_mut(&mut view.city);
        city.pop_day = cp_citizen::PopFlow {
            left: 5,
            ..Default::default()
        };
        assert_eq!(population_trend(&view).1, "-5");
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
        assert!(debt_warning(Cost(-100)).unwrap().contains("−$20.0k"));
    }

    #[test]
    fn demand_bars_map_the_range_onto_the_bar() {
        assert_eq!(demand_fraction(-100.0), 0.0);
        assert_eq!(demand_fraction(-0.5), 0.0);
        assert_eq!(demand_fraction(0.0), 0.0);
        assert_eq!(demand_fraction(50.0), 0.5);
        assert_eq!(demand_fraction(100.0), 1.0);
        assert_eq!(demand_fraction(1e9), 1.0);
        assert_eq!(demand_fraction(f32::NAN), 0.0);
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
        let ys: Vec<f32> = (60..320).step_by(4).map(|y| y as f32).collect();
        'search: for y in ys {
            for x in (120..520).step_by(10) {
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
        for want in ["Last week", "Income", "Expenses", "Treasury, 7 days"] {
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
        // The Overview's default period is last week: the roads, buildings and pipes it lists,
        // in the short form.
        let mut values = vec![
            format_money_short(o.road_upkeep),
            format_money_short(week.building_upkeep),
            format_money_short(week.utility_upkeep),
        ];
        values.extend(
            o.income_by_zone
                .iter()
                .filter(|(_, c)| c.0 != 0)
                .map(|(_, c)| format_money_short(*c)),
        );
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
        for tab in [
            BudgetTab::Taxes,
            BudgetTab::Budgets,
            BudgetTab::Land,
            BudgetTab::Overview,
        ] {
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
                .any(|t| t == "Treasury, 7 days")
        );
        assert!(
            text(BudgetTab::Taxes)
                .iter()
                .any(|t| t.starts_with("Taxes for the week"))
        );
        assert!(text(BudgetTab::Budgets).iter().any(|t| t == "Per week"));
        assert!(
            !text(BudgetTab::Budgets)
                .iter()
                .any(|t| t == "Treasury, 7 days")
        );
    }

    /// Draws the Loans tab, then clicks each button ("Take", "Pay off …") until a command
    /// appears.
    fn click_for_command(view: &SimView) -> Option<SimCommand> {
        let ctx = egui::Context::default();
        let mut draft = BudgetDraft::on(BudgetTab::Loans);
        let mut outbox = CommandOutbox::default();
        let mut draw = |t: f64, events: Vec<egui::Event>, draft: &mut BudgetDraft| {
            crate::testutil::frame_texts(&ctx, input(t, events), |ui| {
                budget_contents(ui, view, draft, &mut outbox)
            })
        };
        draw(0.0, vec![], &mut draft);
        let texts = draw(0.1, vec![], &mut draft);
        let buttons: Vec<egui::Pos2> = texts
            .iter()
            .filter(|(t, _)| t == "Take" || t.starts_with("Pay off"))
            .map(|(_, p)| *p + egui::vec2(4.0, 4.0))
            .collect();
        let mut t = 0.2;
        for pos in buttons {
            let [down, up] = crate::testutil::click(pos);
            for events in [vec![egui::Event::PointerMoved(pos)], down, up] {
                t += 0.1;
                draw(t, events, &mut draft);
            }
            t += 0.1;
            draw(t, vec![], &mut draft);
        }
        outbox.iter().next().cloned()
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

    /// BUG-097: a demolition refund is an income line of the budget, as it came (not scaled up
    /// to a day like the hourly lines), and the bulldozer shows it as "+$N" before the click.
    #[test]
    fn a_demolition_refund_is_an_income_line_and_the_bulldozer_shows_it() {
        let mut view = SimRunner::new(1).view();
        let f = Flow {
            refunds: Cost(28_000),
            ..Flow::default()
        };
        view.flow.hours.push_back((1, f));
        view.flow.week_to_date = f;
        let day = view.flow.last_day(&view.economy).unwrap();
        assert_eq!(day.refunds, Cost(28_000));
        let (income, _) = flow_rows(
            &view,
            &day,
            &view.flow.week_to_date,
            &view.economy.last_week,
        );
        let row = income
            .iter()
            .find(|r| r.name == "Demolition refund")
            .expect("the refund line");
        assert_eq!(row.money, [Some(Cost(28_000)), Some(Cost(28_000)), None]);

        let mut tools = cp_tools::ToolState::default();
        tools.select(cp_tools::ActiveTool::Bulldozer);
        tools.bulldoze_refund = Cost(28_000);
        assert_eq!(crate::cursor_label::refund_text(&tools), None, "no cursor");
        tools.hover = Some(bevy::math::Vec2::ZERO);
        assert_eq!(
            crate::cursor_label::refund_text(&tools).as_deref(),
            Some("+$280.00")
        );
        tools.bulldoze_refund = Cost::ZERO;
        assert_eq!(
            crate::cursor_label::refund_text(&tools),
            None,
            "nothing back"
        );
    }

    /// §20.6 item 8: the budget tab shows the rows a milestone has opened: at Hamlet Water,
    /// Sewage, Power, Roads and Park (the small park opens there); Village adds Garbage,
    /// Health, Fire, Police; Roads says "Upkeep only".
    #[test]
    fn the_budget_tab_shows_the_rows_the_milestone_opened() {
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
        let mut view = sim.view();
        use ServiceKind::*;
        view.progress = cp_sim::Progress::at(0);
        assert_eq!(budget_rows(&view), [Water, Sewage, Power, Roads, Park]);
        let text = painted(&view, &mut BudgetDraft::on(BudgetTab::Budgets));
        assert!(text.iter().any(|t| t == "Water"), "{text:?}");
        assert!(!text.iter().any(|t| t == "Health"), "{text:?}");
        view.progress = cp_sim::Progress::at(1);
        assert_eq!(
            budget_rows(&view),
            [
                Water, Sewage, Power, Roads, Garbage, Health, Fire, Police, Park
            ]
        );
        let text = painted(&view, &mut BudgetDraft::on(BudgetTab::Budgets));
        assert!(text.iter().any(|t| t == "Health"), "{text:?}");
        // Everything open: all 13.
        view.progress = cp_sim::Progress::ALL;
        assert_eq!(budget_rows(&view).len(), BUDGETED.len());
    }

    /// BUG-123: before "Busy Town" the Taxes tab has no slider for ResHigh, ComHigh and Office,
    /// and the Overview no tax line for them (unless one brought money); after it, it has.
    #[test]
    fn the_taxes_tab_shows_the_zones_the_milestone_opened() {
        use std::sync::Arc;
        let db = Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        );
        let busy_town = db.zone_unlock_milestone(ZoneType::ResHigh).unwrap();
        assert_eq!(db.milestones()[busy_town].population, 5000);
        let mut sim = SimRunner::new(1);
        sim.set_content(db);
        let mut view = sim.view();
        view.flow.hours.push_back((1, Flow::default()));
        let later = [ZoneType::ResHigh, ZoneType::ComHigh, ZoneType::Office];
        let names = |zones: &[ZoneType]| -> Vec<String> {
            zones.iter().map(|&z| tr(zone_name(z))).collect()
        };
        let tax_lines = |zones: &[ZoneType]| -> Vec<String> {
            zones
                .iter()
                .map(|&z| trf!("Tax: {zone}", zone = tr(zone_name(z))))
                .collect()
        };

        // Hamlet: no line of a service that is not open yet (buses, fares, hotels) anywhere.
        view.progress = cp_sim::Progress::START;
        let mut text = painted(&view, &mut BudgetDraft::default());
        text.extend(painted(&view, &mut BudgetDraft::on(BudgetTab::Budgets)));
        for closed in [
            "Buses",
            "Fares",
            "Public transport",
            "Hotels",
            "Health",
            "Police",
        ] {
            assert!(
                !text.iter().any(|t| t == closed),
                "{closed} shown: {text:?}"
            );
        }
        assert!(text.iter().any(|t| t == "Loans"), "{text:?}");

        view.progress = cp_sim::Progress::at(u8::try_from(busy_town - 1).unwrap());
        let text = painted(&view, &mut BudgetDraft::on(BudgetTab::Taxes));
        for name in names(&[ZoneType::ResLow, ZoneType::ComLow, ZoneType::Industrial]) {
            assert!(text.contains(&name), "{name} missing: {text:?}");
        }
        for name in names(&later) {
            assert!(
                !text.contains(&name),
                "{name} shown before the milestone: {text:?}"
            );
        }
        // The flow lists the taxes of the open zones (the Overview hides the zero ones).
        let tax_rows = |view: &SimView| -> Vec<String> {
            let day = view.flow.last_day(&view.economy).unwrap_or_default();
            flow_rows(view, &day, &view.flow.week_to_date, &view.economy.last_week)
                .0
                .into_iter()
                .map(|r| r.name)
                .collect()
        };
        let rows = tax_rows(&view);
        for line in tax_lines(&later) {
            assert!(!rows.contains(&line), "{line} shown: {rows:?}");
        }
        assert!(
            rows.contains(&tax_lines(&[ZoneType::ResLow])[0]),
            "{rows:?}"
        );
        // Money of a closed zone (a district of an older city, say) is still shown.
        let mut f = Flow::default();
        f.income_by_zone[TAXED_ZONES
            .iter()
            .position(|&z| z == ZoneType::Office)
            .unwrap()] = Cost(100);
        view.flow.hours.push_back((2, f));
        let mut day_view = BudgetDraft::default();
        day_view.period = Period::Day;
        let text = painted(&view, &mut day_view);
        assert!(
            text.contains(&tax_lines(&[ZoneType::Office])[0]),
            "{text:?}"
        );

        view.progress = cp_sim::Progress::at(u8::try_from(busy_town).unwrap());
        let text = painted(&view, &mut BudgetDraft::on(BudgetTab::Taxes));
        for name in names(&later) {
            assert!(
                text.contains(&name),
                "{name} missing after the milestone: {text:?}"
            );
        }
        let rows = tax_rows(&view);
        for line in tax_lines(&later) {
            assert!(rows.contains(&line), "{line} missing: {rows:?}");
        }
    }

    /// The tab's figures come from the sim's factors: the effect words and the cost restated at
    /// the slider's value (linear).
    #[test]
    fn the_effect_and_the_cost_follow_the_budget() {
        let bal = cp_content::Balance::default();
        assert_eq!(
            effect_text(ServiceKind::Health, 75, &bal),
            "capacity ×0.75 · reach ×0.87"
        );
        assert_eq!(effect_text(ServiceKind::Water, 150, &bal), "capacity ×1.25");
        assert_eq!(effect_text(ServiceKind::Roads, 50, &bal), "Upkeep only");
        assert_eq!(cost_at(Cost(124_000), 100, 50), Cost(62_000));
        assert_eq!(cost_at(Cost(62_000), 50, 150), Cost(186_000));
        assert_eq!(budget_sectors(ServiceKind::Roads), &[] as &[Sector]);
        assert_eq!(
            budget_sectors(ServiceKind::Post),
            &[Sector::Mail, Sector::Sorting]
        );
    }

    fn content_db() -> std::sync::Arc<cp_content::ContentDb> {
        std::sync::Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        )
    }

    /// `budget_coverage` is supply over demand of the tightest open sector, not clamped; roads
    /// (no sector) have none.
    #[test]
    fn the_coverage_is_supply_over_demand_of_the_tightest_sector() {
        use cp_sim::city_services::SectorBalance;
        let mut view = SimRunner::new(1).view();
        let row = |sector, supply, demand, locked| SectorBalance {
            sector,
            supply,
            demand,
            unit: "u",
            locked,
        };
        std::sync::Arc::make_mut(&mut view.city).services = vec![
            row(Sector::Water, 150.0, 100.0, false),
            row(Sector::Elementary, 90.0, 100.0, false),
            row(Sector::HighSchool, 50.0, 100.0, false),
            row(Sector::University, 10.0, 100.0, true),
            row(Sector::Health, 5.0, 0.0, false),
        ];
        assert_eq!(budget_coverage(&view, ServiceKind::Water), Some(1.5));
        // Education: the worst of the open school levels; the locked one does not count.
        assert_eq!(budget_coverage(&view, ServiceKind::Education), Some(0.5));
        // No demand: over `demand.max(1)`.
        assert_eq!(budget_coverage(&view, ServiceKind::Health), Some(5.0));
        assert_eq!(budget_coverage(&view, ServiceKind::Roads), None);
        assert_eq!(budget_coverage(&view, ServiceKind::Fire), None);
        assert_eq!(coverage_colour(0.89), BAD);
        assert_eq!(coverage_colour(0.9), WARN);
        assert_eq!(coverage_colour(1.05), WARN);
        assert_eq!(coverage_colour(1.06), GOOD);
    }

    #[test]
    fn the_tax_estimate_scales_with_the_rate_and_survives_a_zero_rate() {
        assert_eq!(tax_estimate(Cost(90_000), 9, 12), Cost(120_000));
        assert_eq!(tax_estimate(Cost(90_000), 9, 9), Cost(90_000));
        assert_eq!(tax_estimate(Cost(90_000), 9, 3), Cost(30_000));
        assert_eq!(tax_estimate(Cost(500), 0, 5), Cost(2_500));
        assert_eq!(tax_estimate(Cost::ZERO, 0, 5), Cost::ZERO);
        // The tooltip: homes show the contentment factor, the others say they do not.
        let view = SimRunner::new(1).view();
        let homes = tax_tip(&view, ZoneType::ResLow, 9, 12, Cost(90_000));
        assert!(homes.contains("100 -> 70"), "{homes}");
        assert!(homes.contains("$1,200.00"), "{homes}");
        let shop = tax_tip(&view, ZoneType::ComLow, 9, 12, Cost(90_000));
        assert!(!shop.contains("factor"), "{shop}");
    }

    #[test]
    fn the_land_tab_is_parsed_and_only_it_draws_the_grid() {
        assert_eq!(BudgetTab::parse("land"), Some(BudgetTab::Land));
        assert_eq!(BudgetTab::parse("services"), Some(BudgetTab::Budgets));
        assert_eq!(BudgetTab::parse("budgets"), Some(BudgetTab::Budgets));
        assert_eq!(BudgetTab::ALL.len(), 5);
        let mut sim = SimRunner::new(1);
        sim.set_content(content_db());
        sim.world_mut()
            .insert_resource(cp_terrain::TileOwnership::only((2, 2)));
        sim.world_mut().insert_resource(cp_sim::Progress::START);
        let view = sim.view();
        // A buyable tile is $5.0k here.
        let price = format_money_short(Cost::from_units(5000.0));
        for tab in [
            BudgetTab::Overview,
            BudgetTab::Taxes,
            BudgetTab::Budgets,
            BudgetTab::Loans,
        ] {
            let text = painted(&view, &mut BudgetDraft::on(tab));
            assert!(!text.contains(&price), "{tab:?} shows a tile: {text:?}");
        }
        let text = painted(&view, &mut BudgetDraft::on(BudgetTab::Land));
        assert!(text.contains(&price), "{text:?}");
        assert!(text.iter().any(|t| t == "own"), "{text:?}");
    }

    /// A double click on a service slider sends its budget back to 100.
    #[test]
    fn a_double_click_on_a_service_slider_resets_it_to_100() {
        let mut sim = SimRunner::new(1);
        sim.set_content(content_db());
        let mut view = sim.view();
        view.budgets.set(ServiceKind::Water, 60);
        let mut found = None;
        'search: for y in (60..400).step_by(5) {
            for x in (200..700).step_by(15) {
                let pos = egui::pos2(x as f32, y as f32);
                let ctx = egui::Context::default();
                let mut draft = BudgetDraft::on(BudgetTab::Budgets);
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
                    (0.15, vec![ev(false)]),
                    (0.2, vec![ev(true)]),
                    (0.25, vec![ev(false)]),
                    (0.3, vec![]),
                ] {
                    crate::testutil::frame(&ctx, input(t, events), |ui| {
                        budget_contents(ui, &view, &mut draft, &mut outbox);
                    });
                }
                if outbox.iter().last() == Some(&budget_command(ServiceKind::Water, 100)) {
                    found = Some(pos);
                    break 'search;
                }
            }
        }
        assert!(found.is_some(), "a double click resets the Water slider");
    }

    #[test]
    fn the_loan_tiles_say_how_much_of_the_income_the_payments_take() {
        assert_eq!(loan_share(Cost::ZERO, Cost(100)), 0.0);
        assert_eq!(loan_share(Cost(10), Cost::ZERO), 1.0);
        assert_eq!(loan_share(Cost(10), Cost(100)), 0.1);
        assert_eq!(loan_share_colour(0.1), GOOD);
        assert_eq!(loan_share_colour(0.2), WARN);
        assert_eq!(loan_share_colour(0.31), BAD);
        assert_eq!(loan_overpay(Cost(110), 10, Cost(1_000)), Cost(100));
    }

    #[test]
    fn the_services_come_in_groups_in_the_sim_order() {
        let mut sim = SimRunner::new(1);
        sim.set_content(content_db());
        let mut view = sim.view();
        view.progress = cp_sim::Progress::ALL;
        let groups = budget_groups(&view);
        let names: Vec<_> = groups.iter().map(|(g, _)| *g).collect();
        assert_eq!(
            names,
            [
                ServiceGroup::Utilities,
                ServiceGroup::City,
                ServiceGroup::Transport,
                ServiceGroup::Leisure
            ]
        );
        assert_eq!(
            groups[0].1,
            [ServiceKind::Water, ServiceKind::Power, ServiceKind::Sewage]
        );
        let all: usize = groups.iter().map(|(_, k)| k.len()).sum();
        assert_eq!(all, BUDGETED.len());
    }

    /// The Overview period picks the column; zero lines are left out, the biggest comes first.
    #[test]
    fn the_overview_lines_follow_the_period_biggest_first() {
        let mut view = SimRunner::new(1).view();
        let mut week = Flow::default();
        week.road_upkeep = Cost(500);
        week.utility_upkeep = Cost(900);
        week.income_by_zone[0] = Cost(2_000);
        view.flow.week_to_date = week;
        view.flow.hours.push_back((1, Flow::default()));
        let (income, expenses) = period_lines(&view, Period::Week);
        assert_eq!(income.len(), 1);
        assert_eq!(income[0].amount, Cost(2_000));
        assert_eq!(expenses[0].amount, Cost(900));
        assert_eq!(expenses[1].amount, Cost(500));
        let (income, expenses) = period_lines(&view, Period::LastWeek);
        assert!(income.is_empty() && expenses.is_empty());
    }
}
