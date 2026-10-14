//! Top bar and the content of the tool panels (the palette that opens them is in
//! [`crate::palette`]).

use bevy::prelude::Resource;
use bevy_egui::egui;
use cp_building::Demand;
use cp_content::UtilityKind;
use cp_core::SimSpeed;
use cp_net::ElevationMode;
use cp_render::{LightingSettings, Overlay, OverlayState};
use cp_sim::SimView;
use cp_tools::{
    ActiveTool, CommandOutbox, PALETTE, RoadMode, ToolState, UtilityMode, ZoneToolKind,
    format_money, snap_label, zone_color, zone_name,
};
use cp_utility::UtilityNet;

use crate::files::{FileMenu, toggle_saves};
use crate::icons::{self, Icon};
use crate::palette::Category;
use crate::set_speed;

#[derive(Resource, Debug, Default)]
pub struct UiState {
    /// The category whose tool panel is open.
    pub category: Option<Category>,
    pub debug_open: bool,
    /// The budget window is shown (button in the top bar).
    pub budget_open: bool,
    pub demand_open: bool,
    pub metrics_open: bool,
    /// Slider drafts of the budget window.
    pub budget: crate::budget::BudgetDraft,
}

/// The windows the top bar switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiWindow {
    Budget,
    Demand,
    Metrics,
    Saves,
    Debug,
}

impl UiWindow {
    pub const ALL: [UiWindow; 5] = [
        UiWindow::Budget,
        UiWindow::Demand,
        UiWindow::Metrics,
        UiWindow::Saves,
        UiWindow::Debug,
    ];

    pub fn label(self) -> &'static str {
        match self {
            UiWindow::Budget => "Budget",
            UiWindow::Demand => "Demand",
            UiWindow::Metrics => "Metrics",
            UiWindow::Saves => "Save / Load",
            UiWindow::Debug => "Debug (F3)",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            UiWindow::Budget => Icon::Budget,
            UiWindow::Demand => Icon::Demand,
            UiWindow::Metrics => Icon::Metrics,
            UiWindow::Saves => Icon::Saves,
            UiWindow::Debug => Icon::Debug,
        }
    }
}

pub const SPEEDS: [(SimSpeed, &str); 4] = [
    (SimSpeed::Pause, "⏸"),
    (SimSpeed::X1, "×1"),
    (SimSpeed::X2, "×2"),
    (SimSpeed::X3, "×3"),
];

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// "Mon 12:30, day 4" from the game time.
pub fn format_time(view: &SimView) -> String {
    let t = view.game_time;
    let minutes = u64::from(t.tick_of_day) * 24 * 60 / cp_core::time::TICKS_PER_DAY;
    format!(
        "{} {:02}:{:02}  ·  day {}",
        WEEKDAYS[t.day_of_week() as usize],
        minutes / 60,
        minutes % 60,
        t.day + 1
    )
}

/// Full-screen background `Ui` that panels are docked into. Create one per frame.
pub fn root_ui(ctx: &egui::Context) -> egui::Ui {
    egui::Ui::new(
        ctx.clone(),
        "cp_ui_root".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    )
}

/// The overlay switcher: one selectable label per overlay. Returns each label's rectangle
/// (tests use it to click them). Selecting writes `state`; the render side flips layer
/// visibility on the same frame.
pub fn overlay_buttons(ui: &mut egui::Ui, state: &mut OverlayState) -> Vec<(Overlay, egui::Rect)> {
    let mut rects = Vec::new();
    for o in Overlay::ALL {
        let r = ui.selectable_label(state.active == o, o.label());
        rects.push((o, r.rect));
        if r.clicked() {
            state.active = o;
        }
    }
    rects
}

/// Where the clickable parts of the top bar were drawn (tests click them).
#[derive(Clone, Debug, Default)]
pub struct TopRects {
    pub speeds: Vec<(SimSpeed, egui::Rect)>,
    pub windows: Vec<(UiWindow, egui::Rect)>,
}

/// Top bar: the window switches (budget, demand, metrics, saves, debug) with the money next to
/// the budget, demand bars, population (stub), date/time, speed buttons, overlay switcher and
/// the always-day lighting toggle. A speed click pushes `SetSpeed` into `outbox`.
pub fn top_bar(
    root: &mut egui::Ui,
    view: &SimView,
    outbox: &mut CommandOutbox,
    overlay: &mut OverlayState,
    lighting: &mut LightingSettings,
    state: &mut UiState,
    files: &mut FileMenu,
) -> TopRects {
    let mut rects = TopRects::default();
    egui::Panel::top("top_bar").show(root, |ui| {
        ui.horizontal(|ui| {
            crate::budget::money_label(ui, view);
            for w in UiWindow::ALL {
                let on = match w {
                    UiWindow::Budget => state.budget_open,
                    UiWindow::Demand => state.demand_open,
                    UiWindow::Metrics => state.metrics_open,
                    UiWindow::Saves => files.open,
                    UiWindow::Debug => state.debug_open,
                };
                let r = ui
                    .add(egui::Button::image(w.icon().sized(20.0)).selected(on))
                    .on_hover_text(w.label());
                rects.windows.push((w, r.rect));
                if r.clicked() {
                    match w {
                        UiWindow::Budget => state.budget_open = !on,
                        UiWindow::Demand => state.demand_open = !on,
                        UiWindow::Metrics => state.metrics_open = !on,
                        UiWindow::Saves => toggle_saves(files),
                        UiWindow::Debug => state.debug_open = !on,
                    }
                }
            }
            ui.separator();
            crate::budget::demand_bars(ui, &view.demand);
            ui.separator();
            ui.label(format!("👥 {}", view.city.population))
                .on_hover_text("Residents");
            ui.separator();
            ui.label(format_time(view));
            ui.separator();
            for (speed, label) in SPEEDS {
                let r = ui.selectable_label(view.speed == speed, label);
                rects.speeds.push((speed, r.rect));
                if r.clicked() && view.speed != speed {
                    outbox.push(set_speed(speed));
                }
            }
            ui.separator();
            overlay_buttons(ui, overlay);
            ui.separator();
            // Render only: the game clock and everyone's routine keep running at night.
            ui.checkbox(&mut lighting.always_day, "Всегда день");
        });
    });
    rects
}

/// An icon + label button of a tool panel.
pub fn tool_button(ui: &mut egui::Ui, icon: Icon, label: &str, selected: bool) -> egui::Response {
    ui.add(egui::Button::image_and_text(icon.sized(22.0), label).selected(selected))
}

/// A titled section of a tool panel.
fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    ui.weak(title);
    add(ui);
}

/// The pinned "cost · verdict" block: a ghost that can be built or placed shows its cost, one
/// that cannot shows why; with no ghost, a hint for the open category.
pub fn verdict(ui: &mut egui::Ui, category: Category, tools: &ToolState) {
    let ok = |ui: &mut egui::Ui, text: &str, cost: Option<cp_core::Cost>, extra: String| {
        let cost = cost.map_or(String::new(), |c| format!("{} · ", format_money(c)));
        ui.colored_label(egui::Color32::LIGHT_GREEN, format!("{cost}✔ {text}{extra}"));
    };
    let bad = |ui: &mut egui::Ui, errors: &[String], fallback: &str| {
        if errors.is_empty() {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("✖ {fallback}"));
        }
        for e in errors {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("✖ {e}"));
        }
    };
    match (category, tools.active) {
        (Category::Roads, ActiveTool::Road) => match &tools.ghost {
            Some(g) if g.valid => {
                let snap = snap_label(g.snap.kind).map_or(String::new(), |l| format!("  ⌖ {l}"));
                ok(ui, "can build", g.cost, snap);
            }
            Some(g) => bad(ui, &g.errors, "cannot build here"),
            None => {
                ui.weak("Click to start a road. Esc cancels.");
            }
        },
        (Category::Buildings, ActiveTool::Place) => match &tools.place_ghost {
            Some(g) if g.valid => ok(ui, "can place", g.cost, String::new()),
            Some(g) => bad(
                ui,
                &g.error.iter().cloned().collect::<Vec<_>>(),
                "cannot place here",
            ),
            None => {
                ui.weak("Hover a spot beside a road. T rotates, Esc leaves the tool.");
            }
        },
        (Category::Water | Category::Power, ActiveTool::Utility) => match &tools.utility_ghost {
            Some(g) if g.valid => ok(ui, "can build", g.cost, String::new()),
            Some(g) => bad(ui, &g.errors, "cannot build here"),
            None => {
                ui.weak("Click to start. Buildings within 40 m of the network are connected. Esc cancels.");
            }
        },
        (Category::Water | Category::Power, ActiveTool::UtilityRemove) => {
            ui.weak("Hover a piece and click to remove it. Esc leaves the tool.");
        }
        (Category::Zones, ActiveTool::Zone) => {
            let n = tools.zone_preview.len();
            let what = match tools.zone.kind {
                ZoneToolKind::Rectangle if tools.zone.corner.is_some() => {
                    "click the opposite corner".to_string()
                }
                ZoneToolKind::Rectangle => "click the first corner".to_string(),
                ZoneToolKind::FillBlock => "click a block to fill it".to_string(),
                _ => "hold and drag to paint".to_string(),
            };
            ui.weak(format!("{what} — {n} cells under the cursor. Esc cancels."));
        }
        (Category::Zones, _) => {
            ui.weak("Pick a zoning tool or a zone type to show the zone grid.");
        }
        (Category::Bulldozer, ActiveTool::Bulldozer) => {
            ui.weak("Hover a building or road segment, click to remove it. Esc leaves the tool.");
        }
        (Category::Water | Category::Power, _) => {
            ui.weak("Pick a tool; the overlay shows the network and which buildings lack supply.");
        }
        _ => {
            ui.weak("Pick a tool.");
        }
    }
}

/// The Buildings category: the placeable service buildings of the content. T rotates the
/// building.
pub fn services_panel(ui: &mut egui::Ui, view: Option<&SimView>, tools: &mut ToolState) {
    section(ui, "Service buildings", |ui| {
        ui.horizontal_wrapped(|ui| {
            if let Some(content) = view.and_then(|v| v.content.as_deref()) {
                for (id, def) in content.buildings().filter(|(_, d)| d.service.is_some()) {
                    let selected =
                        tools.active == ActiveTool::Place && tools.building.def == Some(id);
                    if tool_button(ui, Icon::Building, &def.name, selected).clicked() {
                        if tools.active != ActiveTool::Place {
                            tools.select(ActiveTool::Place);
                        }
                        tools.building.def = Some(id);
                    }
                }
            }
        });
    });
}

/// The Bulldozer category: the one tool.
pub fn bulldozer_panel(ui: &mut egui::Ui, tools: &mut ToolState) {
    let on = tools.active == ActiveTool::Bulldozer;
    if tool_button(ui, Icon::Bulldozer, "Bulldozer", on).clicked() {
        tools.select(if on {
            ActiveTool::None
        } else {
            ActiveTool::Bulldozer
        });
    }
}

/// The Water and Power categories (`tab` picks which): the pipe or power-line tools, the remove
/// tool and the modes. Picking a tool also switches the map overlay to that network (pipes
/// are underground and only the Water overlay shows them).
pub fn utility_panel(
    ui: &mut egui::Ui,
    view: Option<&SimView>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    tab: Category,
) {
    if let Some(view) = view {
        section(ui, "Network", |ui| {
            crate::net_summary::network_summary(ui, view, tab);
        });
    }
    let (kinds, net, shown): (&[(UtilityKind, Icon, &str)], _, _) = if tab == Category::Power {
        (
            &[(UtilityKind::Power, Icon::PowerLine, "Power line")],
            UtilityNet::Power,
            Overlay::Power,
        )
    } else {
        (
            &[
                (UtilityKind::Water, Icon::PipeWater, "Water pipe"),
                (UtilityKind::Sewage, Icon::PipeSewage, "Sewage pipe"),
            ],
            UtilityNet::Pipe,
            Overlay::Water,
        )
    };
    section(ui, "Tools", |ui| {
        ui.horizontal_wrapped(|ui| {
            for &(kind, icon, label) in kinds {
                let selected = tools.active == ActiveTool::Utility && tools.utility.kind == kind;
                if tool_button(ui, icon, label, selected).clicked() {
                    if selected {
                        tools.select(ActiveTool::None);
                    } else {
                        tools.select(ActiveTool::Utility);
                        tools.utility.set_kind(kind);
                        overlay.active = shown;
                    }
                }
            }
            let removing =
                tools.active == ActiveTool::UtilityRemove && tools.remove_net == Some(net);
            if tool_button(ui, Icon::Remove, "Remove", removing).clicked() {
                if removing {
                    tools.select(ActiveTool::None);
                } else {
                    tools.select(ActiveTool::UtilityRemove);
                    tools.remove_net = Some(net);
                    overlay.active = shown;
                }
            }
        });
    });
    if tools.active == ActiveTool::Utility {
        section(ui, "Mode", |ui| {
            ui.horizontal_wrapped(|ui| {
                for (mode, label) in [
                    (UtilityMode::Straight, "Straight (2 clicks)"),
                    (UtilityMode::Polyline, "Polyline (Enter ends)"),
                ] {
                    if ui
                        .selectable_label(tools.utility.mode == mode, label)
                        .clicked()
                    {
                        tools.utility.set_mode(mode);
                    }
                }
            });
        });
    }
}

fn zone_kind_icon(kind: ZoneToolKind) -> Icon {
    match kind {
        ZoneToolKind::FillBlock => Icon::ZoneFill,
        ZoneToolKind::BrushSmall => Icon::ZoneBrushSmall,
        ZoneToolKind::BrushLarge => Icon::ZoneBrushLarge,
        ZoneToolKind::Rectangle => Icon::ZoneRect,
        ZoneToolKind::Eraser => Icon::ZoneEraser,
    }
}

/// The Zones category: tool kinds and the zone palette. Picking either activates the zoning
/// tool (which shows the zone grid); "Off" leaves it.
pub fn zones_panel(
    ui: &mut egui::Ui,
    view: Option<&SimView>,
    tools: &mut ToolState,
    state: &mut UiState,
) {
    section(ui, "Tool", |ui| {
        ui.horizontal_wrapped(|ui| {
            for kind in ZoneToolKind::ALL {
                let selected = tools.active == ActiveTool::Zone && tools.zone.kind == kind;
                if tool_button(ui, zone_kind_icon(kind), kind.label(), selected).clicked() {
                    if tools.active != ActiveTool::Zone {
                        tools.select(ActiveTool::Zone);
                    }
                    tools.zone.set_kind(kind);
                }
            }
            if ui
                .add_enabled(tools.active == ActiveTool::Zone, egui::Button::new("✖ Off"))
                .clicked()
            {
                tools.select(ActiveTool::None);
            }
        });
    });
    section(ui, "Zone", |ui| {
        for zone in PALETTE {
            if zone_card(ui, zone, view, tools.zone.zone == zone).clicked() {
                tools.zone.zone = zone;
                if tools.active != ActiveTool::Zone {
                    tools.select(ActiveTool::Zone);
                }
            }
        }
    });
    if ui.button("Спрос по зонам ▸").clicked() {
        state.demand_open = true;
    }
}

/// The demand group a zone type belongs to: the letter and its value (R for `ResLow`/`ResHigh`,
/// C for `Com*`, I, O). Demand is per group in M1.
pub fn zone_group_demand(zone: cp_content::ZoneType, d: &Demand) -> (&'static str, f32) {
    use cp_content::ZoneType as Z;
    match zone {
        Z::ResLow | Z::ResHigh => ("R", d.r),
        Z::ComLow | Z::ComHigh => ("C", d.c),
        Z::Industrial => ("I", d.i),
        Z::Office | Z::None => ("O", d.o),
    }
}

/// A zone card: the colour square with the group letter, the name and the group's demand bar.
fn zone_card(
    ui: &mut egui::Ui,
    zone: cp_content::ZoneType,
    view: Option<&SimView>,
    selected: bool,
) -> egui::Response {
    let [r, g, b] = zone_color(zone);
    let color = egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8);
    let inner = egui::Frame::group(ui.style())
        .fill(if selected {
            ui.visuals().selection.bg_fill
        } else {
            ui.visuals().faint_bg_color
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let letter = zone_group_demand(zone, &Demand::default()).0;
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 4.0, color);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    letter,
                    egui::FontId::proportional(15.0),
                    egui::Color32::BLACK,
                );
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(zone_name(zone)).strong());
                    if let Some(view) = view {
                        let (letter, v) = zone_group_demand(zone, &view.demand);
                        ui.add(
                            egui::ProgressBar::new(crate::budget::demand_fraction(v))
                                .desired_width(120.0)
                                .desired_height(8.0)
                                .fill(color),
                        )
                        .on_hover_text(format!("{letter} demand {v:+.0}"));
                    }
                });
            });
        });
    ui.interact(
        inner.response.rect,
        inner.response.id.with("card"),
        egui::Sense::click(),
    )
}

/// The Roads category: the road tool, road mode, road type (with its profile) and elevation.
pub fn roads_panel(ui: &mut egui::Ui, view: Option<&SimView>, tools: &mut ToolState) {
    section(ui, "Tool", |ui| {
        let on = tools.active == ActiveTool::Road;
        if tool_button(ui, Icon::Road, "Road", on).clicked() {
            tools.select(if on {
                ActiveTool::None
            } else {
                ActiveTool::Road
            });
        }
    });
    if tools.active != ActiveTool::Road {
        return;
    }
    section(ui, "Mode", |ui| {
        ui.horizontal_wrapped(|ui| {
            for (mode, icon, label) in [
                (RoadMode::Straight, Icon::Road, "Straight"),
                (RoadMode::Curve, Icon::Curve, "Curve (3 clicks)"),
                (RoadMode::Free, Icon::FreeRoad, "Free (Enter ends)"),
            ] {
                if tool_button(ui, icon, label, tools.road.mode == mode).clicked() {
                    tools.road.set_mode(mode);
                }
            }
        });
    });
    if let Some(content) = view.and_then(|v| v.content.as_ref()) {
        if tools.road.road.is_none() {
            tools.road.road = content.roads().next().map(|(id, _)| id);
        }
        section(ui, "Road type", |ui| {
            let current = tools
                .road
                .road
                .map_or("—".to_string(), |id| content.road(id).name.clone());
            egui::ComboBox::from_id_salt("road_type")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (id, def) in content.roads() {
                        ui.selectable_value(&mut tools.road.road, Some(id), &def.name);
                    }
                });
            if let Some(id) = tools.road.road {
                icons::road_profile(ui, content.road(id), egui::vec2(180.0, 28.0));
            }
        });
    }
    section(ui, "Elevation", |ui| {
        ui.label(match tools.road.elevation() {
            ElevationMode::Ground => "Ground (PgUp raises)".to_string(),
            ElevationMode::Elevated(h) => format!("Raised {h:.0} m (PgUp / PgDn)"),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_sim::{SimCommand, SimRunner};

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

    /// Runs three frames: layout, press, release on `speed`'s button.
    fn click_speed(view: &SimView, speed: SimSpeed, outbox: &mut CommandOutbox) {
        let ctx = egui::Context::default();
        let mut rects = TopRects::default();
        crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| {
            rects = top_bar(
                ui,
                view,
                &mut CommandOutbox::default(),
                &mut OverlayState::default(),
                &mut LightingSettings::default(),
                &mut UiState::default(),
                &mut FileMenu::default(),
            );
        });
        let pos = rects
            .speeds
            .iter()
            .find(|(s, _)| *s == speed)
            .unwrap()
            .1
            .center();
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for (t, events) in [
            (0.1, vec![egui::Event::PointerMoved(pos), button(true)]),
            (0.2, vec![button(false)]),
        ] {
            crate::testutil::frame(&ctx, input(t, events), |ui| {
                top_bar(
                    ui,
                    view,
                    outbox,
                    &mut OverlayState::default(),
                    &mut LightingSettings::default(),
                    &mut UiState::default(),
                    &mut FileMenu::default(),
                );
            });
        }
    }

    #[test]
    fn clicking_a_speed_button_sends_set_speed_into_the_command_log() {
        let mut sim = SimRunner::new(1);
        let mut outbox = CommandOutbox::default();
        click_speed(&sim.view(), SimSpeed::X3, &mut outbox);
        assert_eq!(outbox.len(), 1);
        assert_eq!(
            outbox.iter().next(),
            Some(&SimCommand::SetSpeed {
                speed: SimSpeed::X3
            })
        );

        // The app's submit path: queue -> dispatcher -> log.
        outbox.flush_into(&mut sim);
        sim.tick_once();
        let log = sim.world().resource::<cp_core::CommandLog<SimCommand>>();
        assert_eq!(log.len(), 1);
        assert_eq!(
            log.entries()[0].command,
            SimCommand::SetSpeed {
                speed: SimSpeed::X3
            }
        );
        assert_eq!(sim.view().speed, SimSpeed::X3);

        // Pause, then resume from the paused state, both via buttons.
        click_speed(&sim.view(), SimSpeed::Pause, &mut outbox);
        outbox.flush_into(&mut sim);
        assert_eq!(sim.view().speed, SimSpeed::Pause);
        click_speed(&sim.view(), SimSpeed::X1, &mut outbox);
        outbox.flush_into(&mut sim);
        assert_eq!(sim.view().speed, SimSpeed::X1);
        assert_eq!(
            sim.world()
                .resource::<cp_core::CommandLog<SimCommand>>()
                .len(),
            3
        );
    }

    #[test]
    fn clicking_the_active_speed_sends_nothing() {
        let sim = SimRunner::new(1);
        let mut outbox = CommandOutbox::default();
        click_speed(&sim.view(), SimSpeed::X1, &mut outbox);
        assert!(outbox.is_empty());
    }

    #[test]
    fn time_is_formatted_from_game_time() {
        let mut sim = SimRunner::new(1);
        sim.world_mut().insert_resource(cp_core::Tick(
            cp_core::time::TICKS_PER_DAY * 8 + cp_core::time::TICKS_PER_DAY / 2,
        ));
        assert_eq!(format_time(&sim.view()), "Tue 12:00  ·  day 9");
    }

    #[test]
    fn roads_panel_picks_a_default_road_and_shows_the_ghost_verdict() {
        use cp_core::Cost;
        use cp_tools::Ghost;
        let mut sim = SimRunner::new(1);
        sim.set_content(std::sync::Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        ));
        let view = sim.view();
        let ctx = egui::Context::default();
        let mut tools = ToolState::default();
        tools.select(ActiveTool::Road);
        assert!(tools.road.road.is_none());
        let draw = |tools: &mut ToolState| {
            crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| {
                roads_panel(ui, Some(&view), tools);
            });
        };
        draw(&mut tools);
        assert!(
            tools.road.road.is_some(),
            "the first road type is preselected"
        );

        // Valid and invalid ghosts both render.
        tools.ghost = Some(Ghost {
            polylines: vec![],
            valid: true,
            cost: Some(Cost(123_456)),
            errors: vec![],
            half_width: 0.0,
            snap: Default::default(),
        });
        draw(&mut tools);
        tools.ghost = Some(Ghost {
            polylines: vec![],
            valid: false,
            cost: None,
            errors: vec!["too steep (20%, max 12%)".into()],
            half_width: 6.0,
            snap: Default::default(),
        });
        draw(&mut tools);
        tools.select(ActiveTool::Bulldozer);
        draw(&mut tools);
        assert_eq!(tools.active, ActiveTool::Bulldozer);
        assert!(tools.ghost.is_none(), "switching tools clears the ghost");
    }

    #[test]
    fn clicking_an_overlay_label_selects_it() {
        let ctx = egui::Context::default();
        let mut state = OverlayState::default();
        let mut rects = Vec::new();
        crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| {
            rects = overlay_buttons(ui, &mut state);
        });
        assert_eq!(rects.len(), Overlay::ALL.len());
        for want in [Overlay::Zones, Overlay::Pollution, Overlay::Off] {
            let pos = rects.iter().find(|(o, _)| *o == want).unwrap().1.center();
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            for (t, events) in [
                (1.0, vec![egui::Event::PointerMoved(pos), button(true)]),
                (1.1, vec![button(false)]),
            ] {
                crate::testutil::frame(&ctx, input(t, events), |ui| {
                    overlay_buttons(ui, &mut state);
                });
            }
            assert_eq!(state.active, want);
        }
    }

    #[test]
    fn zones_panel_activates_the_zoning_tool_and_shows_the_state() {
        let ctx = egui::Context::default();
        let mut tools = ToolState::default();
        let mut state = UiState::default();
        let mut draw = |tools: &mut ToolState| {
            crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| {
                zones_panel(ui, None, tools, &mut state);
            });
        };
        draw(&mut tools);
        assert_eq!(tools.active, ActiveTool::None);
        tools.select(ActiveTool::Zone);
        tools.zone.set_kind(ZoneToolKind::Rectangle);
        draw(&mut tools);
        tools.zone.corner = Some(bevy::math::Vec2::ZERO);
        draw(&mut tools);
        tools.select(ActiveTool::None);
        draw(&mut tools);
        assert!(
            tools.zone.corner.is_none(),
            "leaving the tool cancels the rectangle"
        );
    }

    #[test]
    fn utility_tabs_activate_their_tools_and_switch_the_overlay() {
        let ctx = egui::Context::default();
        let mut tools = ToolState::default();
        let mut overlay = OverlayState::default();
        let view = SimRunner::new(1).view();
        let draw = |tools: &mut ToolState, overlay: &mut OverlayState, tab| {
            crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| {
                utility_panel(ui, Some(&view), tools, overlay, tab);
            });
        };
        draw(&mut tools, &mut overlay, Category::Water);
        assert_eq!(tools.active, ActiveTool::None);
        assert_eq!(overlay.active, Overlay::Off);

        // What a click on "Power line" does, then the panel draws with and without a ghost.
        tools.select(ActiveTool::Utility);
        tools.utility.set_kind(cp_content::UtilityKind::Power);
        overlay.active = Overlay::Power;
        draw(&mut tools, &mut overlay, Category::Power);
        tools.utility_ghost = Some(cp_tools::Ghost {
            polylines: vec![],
            valid: false,
            cost: None,
            errors: vec!["outside the land you own".into()],
            half_width: 0.0,
            snap: Default::default(),
        });
        draw(&mut tools, &mut overlay, Category::Power);
        tools.select(ActiveTool::UtilityRemove);
        tools.remove_net = Some(UtilityNet::Power);
        draw(&mut tools, &mut overlay, Category::Power);
        tools.select(ActiveTool::None);
        assert!(
            tools.utility_ghost.is_none(),
            "leaving the tool clears the ghost"
        );
    }

    #[test]
    fn the_top_bar_icons_switch_their_windows() {
        let ctx = egui::Context::default();
        let sim = SimRunner::new(1);
        let view = sim.view();
        let mut state = UiState::default();
        let mut files = FileMenu::default();
        let mut outbox = CommandOutbox::default();
        let mut t = 0.0;
        let mut draw = |events: Vec<egui::Event>, state: &mut UiState, files: &mut FileMenu| {
            t += 0.1;
            let mut rects = TopRects::default();
            crate::testutil::frame(&ctx, input(t, events), |ui| {
                rects = top_bar(
                    ui,
                    &view,
                    &mut outbox,
                    &mut OverlayState::default(),
                    &mut LightingSettings::default(),
                    state,
                    files,
                );
            });
            rects
        };
        draw(vec![], &mut state, &mut files);
        let rects = draw(vec![], &mut state, &mut files);
        assert_eq!(rects.windows.len(), UiWindow::ALL.len());
        for (w, rect) in rects.windows {
            let [down, up] = crate::testutil::click(rect.center());
            draw(down, &mut state, &mut files);
            draw(up, &mut state, &mut files);
            let on = match w {
                UiWindow::Budget => state.budget_open,
                UiWindow::Demand => state.demand_open,
                UiWindow::Metrics => state.metrics_open,
                UiWindow::Saves => files.open,
                UiWindow::Debug => state.debug_open,
            };
            assert!(on, "{w:?} opens on a click");
        }
        // A second click on each closes it again.
        let rects = draw(vec![], &mut state, &mut files);
        for (w, rect) in rects.windows {
            let [down, up] = crate::testutil::click(rect.center());
            draw(down, &mut state, &mut files);
            draw(up, &mut state, &mut files);
            let on = match w {
                UiWindow::Budget => state.budget_open,
                UiWindow::Demand => state.demand_open,
                UiWindow::Metrics => state.metrics_open,
                UiWindow::Saves => files.open,
                UiWindow::Debug => state.debug_open,
            };
            assert!(!on, "{w:?} closes on a second click");
        }
        assert_eq!(
            files.request.take(),
            Some(crate::files::FileRequest::Refresh)
        );
    }

    #[test]
    fn the_top_bar_shows_the_population_from_the_city_stats() {
        let ctx = egui::Context::default();
        let sim = SimRunner::new(1);
        let mut view = sim.view();
        std::sync::Arc::make_mut(&mut view.city).population = 12_345;
        let texts = crate::testutil::frame_texts(&ctx, input(0.0, vec![]), |ui| {
            top_bar(
                ui,
                &view,
                &mut CommandOutbox::default(),
                &mut OverlayState::default(),
                &mut LightingSettings::default(),
                &mut UiState::default(),
                &mut FileMenu::default(),
            );
        });
        assert!(texts.iter().any(|(t, _)| t == "👥 12345"), "{texts:?}");
    }
}
