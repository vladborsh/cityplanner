//! The small read-only demand window the top bar switches; it only shows what the sim view
//! already carries.

use bevy_egui::egui;
use cp_sim::SimView;

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
