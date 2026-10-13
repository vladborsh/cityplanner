//! The small read-only windows the top bar switches: demand and metrics. Both only show what
//! the sim view already carries; richer content comes with the UI tasks that own them.

use bevy_egui::egui;
use cp_sim::SimView;
use cp_tools::format_money;

use crate::budget::demand_bars;
use crate::frame;
use crate::icons::Icon;

/// RCIO demand: the bars and the numbers behind them.
pub fn demand_window(ctx: &egui::Context, open: &mut bool, view: &SimView) {
    frame::window("demand", Icon::Demand, "Demand", open).show(ctx, |ui| {
        ui.horizontal(|ui| demand_bars(ui, &view.demand));
        egui::Grid::new("demand_values").show(ui, |ui| {
            for (name, v) in [
                ("Residential", view.demand.r),
                ("Commercial", view.demand.c),
                ("Industrial", view.demand.i),
                ("Office", view.demand.o),
            ] {
                ui.label(name);
                ui.label(format!("{v:+.0}"));
                ui.end_row();
            }
        });
    });
}

/// City figures of the view: balance, last week's income and upkeep, tick.
pub fn metrics_window(ctx: &egui::Context, open: &mut bool, view: &SimView) {
    frame::window("metrics", Icon::Metrics, "Metrics", open).show(ctx, |ui| {
        let week = &view.economy.last_week;
        egui::Grid::new("metrics_values").show(ui, |ui| {
            for (name, value) in [
                ("Balance", format_money(view.treasury.balance)),
                ("Income last week", format_money(week.income())),
                ("Upkeep last week", format_money(week.upkeep())),
                ("Tick", view.tick.0.to_string()),
            ] {
                ui.label(name);
                ui.label(value);
                ui.end_row();
            }
        });
    });
}
