//! The Lines window (`architecture/system-design.md` §16 M3 A "Клиент", T-160): a row per
//! transit line with its colour, name, mode, stops, buses, riders, queue, money and state.
//!
//! The UI only reads [`SimView::lines`] and sends `SetLineColor`, `RenameLine` and
//! `SetLineVehicles` (one command when an edit is finished, like the budget sliders) and
//! `DeleteLine` through the outbox.

use std::collections::HashMap;

use bevy_egui::egui;
use cp_core::Cost;
use cp_sim::{LineView, SimCommand, SimView};
use cp_tools::{CommandOutbox, format_money};
use cp_transit::{
    LineId, LineState, MAX_LINE_NAME, MAX_LINE_VEHICLES, PALETTE_LEN, TransitCommand, TransitMode,
};

use crate::frame;
use crate::icons::Icon;

/// The colours of the line palette (`Line::color` is an index into it).
pub const LINE_COLORS: [[u8; 3]; PALETTE_LEN as usize] = [
    [230, 80, 70],
    [235, 150, 50],
    [235, 205, 60],
    [140, 200, 70],
    [60, 175, 105],
    [60, 185, 195],
    [70, 140, 225],
    [100, 100, 220],
    [160, 100, 215],
    [215, 100, 190],
    [150, 150, 155],
    [170, 120, 80],
];

pub fn line_color(index: u8) -> egui::Color32 {
    let [r, g, b] = LINE_COLORS[usize::from(index) % LINE_COLORS.len()];
    egui::Color32::from_rgb(r, g, b)
}

pub fn mode_name(mode: TransitMode) -> &'static str {
    match mode {
        TransitMode::Bus => "Bus",
        TransitMode::Metro => "Metro",
        TransitMode::Train => "Train",
    }
}

/// The state text of a line, with what to do about it.
pub fn state_text(state: LineState) -> Option<&'static str> {
    match state {
        LineState::Ok => None,
        LineState::Broken => Some("Broken: a stretch has no road"),
        LineState::NoDepot => Some("No depot with room for a bus"),
    }
}

/// Drafts of the edits in progress.
#[derive(Clone, Debug, Default)]
pub struct LinesDraft {
    names: HashMap<LineId, String>,
    vehicles: HashMap<LineId, u8>,
    /// The line whose delete button was pressed once (a second press confirms).
    pub confirm_delete: Option<LineId>,
}

/// Ticket money of a line last week or so far: its riders times the fare, and the weekly
/// upkeep of its buses out.
pub fn line_money(view: &SimView, l: &LineView) -> (Cost, Cost) {
    let Some(db) = view.content.as_ref() else {
        return (Cost::ZERO, Cost::ZERO);
    };
    let b = db.balance();
    (
        Cost::scaled(b.transit_fare, f64::from(l.riders_week)),
        Cost::scaled(b.bus_upkeep_week, f64::from(l.vehicles_out)),
    )
}

pub fn rename_command(line: LineId, name: &str) -> SimCommand {
    SimCommand::Transit(TransitCommand::RenameLine {
        line,
        name: name.to_string(),
    })
}

pub fn vehicles_command(line: LineId, n: u8) -> SimCommand {
    SimCommand::Transit(TransitCommand::SetLineVehicles { line, n })
}

pub fn color_command(line: LineId, color: u8) -> SimCommand {
    SimCommand::Transit(TransitCommand::SetLineColor { line, color })
}

pub fn delete_command(line: LineId) -> SimCommand {
    SimCommand::Transit(TransitCommand::DeleteLine { line })
}

fn line_row(
    ui: &mut egui::Ui,
    view: &SimView,
    l: &LineView,
    draft: &mut LinesDraft,
    outbox: &mut CommandOutbox,
) {
    // Colour: a swatch that opens the palette.
    ui.menu_button(egui::RichText::new("⬛").color(line_color(l.color)), |ui| {
        egui::Grid::new(("line_palette", l.id)).show(ui, |ui| {
            for i in 0..PALETTE_LEN {
                let b = egui::Button::new(egui::RichText::new("⬛").color(line_color(i)))
                    .selected(i == l.color);
                if ui.add(b).clicked() {
                    outbox.push(color_command(l.id, i));
                    ui.close();
                }
                if i % 4 == 3 {
                    ui.end_row();
                }
            }
        });
    });
    // Name: edited in place, sent when the field loses focus with a changed text.
    let name = draft.names.entry(l.id).or_insert_with(|| l.name.clone());
    let r = ui.add(
        egui::TextEdit::singleline(name)
            .desired_width(110.0)
            .char_limit(MAX_LINE_NAME),
    );
    if r.lost_focus() {
        let text = name.trim().to_string();
        if !text.is_empty() && text != l.name {
            outbox.push(rename_command(l.id, &text));
        } else {
            draft.names.remove(&l.id);
        }
    } else if !r.has_focus() && *name != l.name {
        // The sim's name changed under the draft (or the command went through).
        draft.names.remove(&l.id);
    }
    ui.label(mode_name(l.mode));
    ui.label(l.stops.to_string());
    // Buses: a slider, one command when released.
    let mut n = draft.vehicles.get(&l.id).copied().unwrap_or(l.vehicles);
    let r = ui.add(egui::Slider::new(&mut n, 0..=MAX_LINE_VEHICLES));
    if r.changed() {
        draft.vehicles.insert(l.id, n);
    }
    if r.drag_stopped() || (r.changed() && !r.dragged()) {
        outbox.push(vehicles_command(l.id, n));
    }
    if n == l.vehicles && !r.dragged() {
        draft.vehicles.remove(&l.id);
    }
    ui.label(format!("{} out", l.vehicles_out));
    ui.label(l.riders_week.to_string());
    ui.label(l.waiting.to_string());
    let (fares, upkeep) = line_money(view, l);
    ui.label(format_money(fares));
    ui.label(format_money(upkeep));
    match state_text(l.state) {
        Some(t) => ui.colored_label(egui::Color32::from_rgb(240, 120, 60), t),
        None => ui.label("OK"),
    };
    // Delete asks twice.
    let pending = draft.confirm_delete == Some(l.id);
    let label = if pending { "Sure?" } else { "Delete" };
    if ui.small_button(label).clicked() {
        if pending {
            outbox.push(delete_command(l.id));
            draft.confirm_delete = None;
        } else {
            draft.confirm_delete = Some(l.id);
        }
    }
    ui.end_row();
}

/// The inside of the window (separate so tests can draw it into a plain `Ui`).
pub fn lines_contents(
    ui: &mut egui::Ui,
    view: &SimView,
    draft: &mut LinesDraft,
    outbox: &mut CommandOutbox,
) {
    if view.lines.is_empty() {
        ui.weak("No lines yet. Build bus stops with the Transport tools and close a ring.");
        return;
    }
    egui::Grid::new("lines_grid").striped(true).show(ui, |ui| {
        for h in [
            "",
            "Name",
            "Mode",
            "Stops",
            "Buses",
            "",
            "Riders/wk",
            "Waiting",
            "Fares/wk",
            "Upkeep/wk",
            "State",
            "",
        ] {
            ui.strong(h);
        }
        ui.end_row();
        for l in &view.lines {
            line_row(ui, view, l, draft, outbox);
        }
    });
    ui.separator();
    let w = &view.economy.last_week;
    ui.weak(format!(
        "Last week: fares {}, bus upkeep {}. This week so far: fares {}, upkeep {}.",
        format_money(w.transit_fares),
        format_money(w.transit_upkeep),
        format_money(view.transit_ledger.fares),
        format_money(view.transit_ledger.upkeep),
    ));
}

/// The window itself, shown while `*open`.
pub fn lines_window(
    ctx: &egui::Context,
    open: &mut bool,
    view: &SimView,
    draft: &mut LinesDraft,
    outbox: &mut CommandOutbox,
) {
    frame::window("lines", Icon::Lines, "Lines", open).show(ctx, |ui| {
        egui::ScrollArea::both()
            .max_height(520.0)
            .show(ui, |ui| lines_contents(ui, view, draft, outbox));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{frame_texts, input};
    use bevy::math::Vec3;
    use cp_content::ContentDb;
    use cp_core::curve::CubicBezier3;
    use cp_net::{Elevation, NetGraph};
    use cp_sim::SimRunner;
    use cp_traffic::Side;
    use cp_transit::StopRef;
    use std::sync::Arc;

    #[test]
    fn the_window_lists_a_line_and_its_buttons_send_commands() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let db = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
        let mut sim = SimRunner::new(1);
        sim.set_content(Arc::clone(&db));
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        let c = [
            (0.0, 0.0),
            (200.0, 0.0),
            (200.0, 200.0),
            (0.0, 200.0),
            (0.0, 0.0),
        ];
        let sides: Vec<_> = c
            .windows(2)
            .map(|w| {
                sim.world_mut()
                    .resource_mut::<NetGraph>()
                    .add_segment(
                        &def,
                        road,
                        CubicBezier3::line(
                            Vec3::new(w[0].0, 0.0, w[0].1),
                            Vec3::new(w[1].0, 0.0, w[1].1),
                        ),
                        Elevation::Ground,
                    )
                    .unwrap()
            })
            .collect();
        sim.tick_once();
        let stops = sides
            .iter()
            .map(|&seg| StopRef::NewRoad {
                seg,
                s: 100.0,
                side: Side::Right,
            })
            .collect();
        cp_transit::apply(
            sim.world_mut(),
            &TransitCommand::CreateLine {
                mode: TransitMode::Bus,
                stops,
            },
        )
        .unwrap();
        sim.tick_once();
        let view = sim.view();
        assert_eq!(view.lines.len(), 1);
        let l = view.lines[0].clone();
        assert_eq!(
            (l.stops, l.vehicles),
            (4, cp_transit::DEFAULT_LINE_VEHICLES)
        );

        let ctx = egui::Context::default();
        let mut draft = LinesDraft::default();
        let mut outbox = CommandOutbox::default();
        let mut texts = Vec::new();
        for t in [0.0, 0.1] {
            texts = frame_texts(&ctx, input(t, vec![]), |ui| {
                lines_contents(ui, &view, &mut draft, &mut outbox);
            });
        }
        let has = |s: &str| texts.iter().any(|(t, _)| t == s);
        assert!(has("Bus") && has("Riders/wk") && has("Delete"), "{texts:?}");
        // No line: the hint instead of the table.
        let empty = SimRunner::new(1).view();
        let texts = frame_texts(&ctx, input(0.2, vec![]), |ui| {
            lines_contents(
                ui,
                &empty,
                &mut LinesDraft::default(),
                &mut CommandOutbox::default(),
            );
        });
        assert!(texts.iter().any(|(t, _)| t.starts_with("No lines yet")));
    }

    #[test]
    fn commands_name_the_transit_variants() {
        let id = LineId::new(0, 0);
        assert_eq!(
            rename_command(id, "Ring"),
            SimCommand::Transit(TransitCommand::RenameLine {
                line: id,
                name: "Ring".into()
            })
        );
        assert_eq!(
            vehicles_command(id, 3),
            SimCommand::Transit(TransitCommand::SetLineVehicles { line: id, n: 3 })
        );
    }

    #[test]
    fn every_palette_index_has_a_colour() {
        for i in 0..PALETTE_LEN {
            assert_ne!(line_color(i), egui::Color32::TRANSPARENT);
        }
        assert_eq!(state_text(LineState::Ok), None);
        assert!(state_text(LineState::NoDepot).is_some());
    }
}
