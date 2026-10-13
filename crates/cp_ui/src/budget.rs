//! The money side of the UI (`architecture/system-design.md` §13.6, §11): the balance with its
//! debt warning, the RCIO demand bars, and the budget window with tax and service-budget
//! sliders.
//!
//! The UI only reads [`SimView`] and sends `SetTaxRate` / `SetBudget` through the outbox. A
//! slider keeps a *draft* while it is being dragged and sends **one** command when the
//! pointer is released (or on a click or key press), so a drag does not flood the command log.

use std::ops::RangeInclusive;

use bevy_egui::egui;
use cp_building::Demand;
use cp_content::{ServiceKind, ZoneType};
use cp_core::Cost;
use cp_core::treasury::DEBT_FLOOR;
use cp_economy::EconomyCommand;
use cp_economy::state::{BUDGET_MAX, BUDGET_MIN, BUDGETED, TAX_MAX, TAX_MIN, TAXED_ZONES};
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

/// Fraction `0..=1` of a bar for a demand value (`0` is the middle).
pub fn demand_fraction(v: f32) -> f32 {
    let v = if v.is_nan() { 0.0 } else { v };
    (v.clamp(-DEMAND_RANGE, DEMAND_RANGE) + DEMAND_RANGE) / (2.0 * DEMAND_RANGE)
}

/// The four RCIO bars (residential, commercial, industrial, office); the middle of a bar is
/// zero demand.
pub fn demand_bars(ui: &mut egui::Ui, d: &Demand) {
    for (letter, value, rgb) in [
        ("R", d.r, (80, 190, 90)),
        ("C", d.c, (90, 130, 240)),
        ("I", d.i, (230, 200, 60)),
        ("O", d.o, (180, 110, 220)),
    ] {
        ui.label(letter);
        ui.add(
            egui::ProgressBar::new(demand_fraction(value))
                .desired_width(34.0)
                .desired_height(10.0)
                .fill(egui::Color32::from_rgb(rgb.0, rgb.1, rgb.2)),
        )
        .on_hover_text(format!("{letter} demand {value:+.0}"));
    }
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

/// Slider drafts of the budget window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BudgetDraft {
    taxes: [Option<Draft>; 6],
    budgets: [Option<Draft>; 4],
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

/// The budget window: last week's income and upkeep, the tax slider of every zone type and
/// the budget slider of every service. Slider releases push commands into `outbox`.
pub fn budget_window(
    ctx: &egui::Context,
    open: &mut bool,
    view: &SimView,
    draft: &mut BudgetDraft,
    outbox: &mut CommandOutbox,
) {
    crate::frame::window("budget", crate::icons::Icon::Budget, "Budget", open).show(ctx, |ui| {
        budget_contents(ui, view, draft, outbox);
    });
}

/// The inside of the budget window (separate so tests can draw it into a plain `Ui`).
pub fn budget_contents(
    ui: &mut egui::Ui,
    view: &SimView,
    draft: &mut BudgetDraft,
    outbox: &mut CommandOutbox,
) {
    let (state, week) = (&view.economy, &view.economy.last_week);
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
    ui.separator();
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
    ui.separator();
    ui.strong("Upkeep last week");
    egui::Grid::new("budget_upkeep").show(ui, |ui| {
        for (name, cost) in [
            ("Roads", week.road_upkeep),
            ("Service buildings", week.building_upkeep),
            ("Pipes and power lines", week.utility_upkeep),
        ] {
            ui.label(name);
            ui.label(format_money(cost));
            ui.end_row();
        }
    });
    ui.separator();
    ui.strong("Service budgets");
    for (i, kind) in BUDGETED.into_iter().enumerate() {
        let current = state.budgets.get(kind);
        let name = match kind {
            ServiceKind::Water => "Water",
            ServiceKind::Power => "Power",
            ServiceKind::Roads => "Roads",
            ServiceKind::Sewage => "Sewage",
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
    ui.separator();
    ui.label(format!(
        "Last week: income {}, upkeep {}, balance {}",
        format_money(week.income()),
        format_money(week.upkeep()),
        format_money(week.balance())
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_core::time::TICKS_PER_WEEK;
    use cp_economy::EconomyState;
    use cp_sim::SimRunner;

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
        let mut draft = BudgetDraft::default();
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
        let mut draft = BudgetDraft::default();
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
                let mut d = BudgetDraft::default();
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
                    lot: Lot {
                        block: Id::new(0, 0),
                        col: 0,
                        w: 1,
                        d: 1,
                    },
                    level: 1,
                    state: BuildingState::Active,
                    ready_at: Tick(0),
                    pos: bevy::math::Vec2::new(5000.0, 5000.0),
                    rot: 0.0,
                    access: AccessPoint {
                        segment: Id::new(0, 0),
                        t: 0.0,
                    },
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
        let mut draft = BudgetDraft::default();
        assert!(frames(&sim.view(), &mut draft, vec![(0.0, vec![])]).is_empty());
    }
}
