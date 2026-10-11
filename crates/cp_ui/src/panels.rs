//! Top bar, bottom category bar.

use bevy::prelude::Resource;
use bevy_egui::egui;
use cp_core::SimSpeed;
use cp_net::ElevationMode;
use cp_render::{Overlay, OverlayState};
use cp_sim::SimView;
use cp_tools::{
    ActiveTool, CommandOutbox, PALETTE, RoadMode, ToolState, ZoneToolKind, format_money,
    zone_color, zone_name,
};

use crate::set_speed;

/// Build-menu categories along the bottom. Tabs are empty until their tools land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Roads,
    Zones,
    Water,
    Power,
    Services,
    Transit,
    Districts,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Roads,
        Category::Zones,
        Category::Water,
        Category::Power,
        Category::Services,
        Category::Transit,
        Category::Districts,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Roads => "Roads",
            Category::Zones => "Zones",
            Category::Water => "Water",
            Category::Power => "Power",
            Category::Services => "Services",
            Category::Transit => "Transit (M3)",
            Category::Districts => "Districts (M4)",
        }
    }
}

#[derive(Resource, Debug, Default)]
pub struct UiState {
    pub category: Option<Category>,
    pub debug_open: bool,
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

/// Top bar: money and population (stubs), date/time, speed buttons, overlay switcher.
/// Returns the on-screen rectangle of each speed button (tests use it to click them). A
/// click pushes `SetSpeed` into `outbox`.
pub fn top_bar(
    root: &mut egui::Ui,
    view: &SimView,
    outbox: &mut CommandOutbox,
    overlay: &mut OverlayState,
) -> Vec<(SimSpeed, egui::Rect)> {
    let mut rects = Vec::new();
    egui::Panel::top("top_bar").show(root, |ui| {
        ui.horizontal(|ui| {
            ui.label("💰 $0"); // stub until the economy lands
            ui.separator();
            ui.label("👥 —"); // stub until citizens land
            ui.separator();
            ui.label(format_time(view));
            ui.separator();
            for (speed, label) in SPEEDS {
                let r = ui.selectable_label(view.speed == speed, label);
                rects.push((speed, r.rect));
                if r.clicked() && view.speed != speed {
                    outbox.push(set_speed(speed));
                }
            }
            ui.separator();
            overlay_buttons(ui, overlay);
        });
    });
    rects
}

/// Bottom category bar. Clicking a tab selects it (click again to close). The Roads tab holds
/// the road tool and the bulldozer ([`roads_panel`]); the others are empty for now.
pub fn bottom_bar(
    root: &mut egui::Ui,
    state: &mut UiState,
    view: Option<&SimView>,
    tools: &mut ToolState,
) {
    egui::Panel::bottom("categories").show(root, |ui| {
        ui.horizontal(|ui| {
            for c in Category::ALL {
                let selected = state.category == Some(c);
                if ui.selectable_label(selected, c.label()).clicked() {
                    state.category = if selected { None } else { Some(c) };
                }
            }
        });
        match state.category {
            Some(Category::Roads) => {
                ui.separator();
                roads_panel(ui, view, tools);
            }
            Some(Category::Zones) => {
                ui.separator();
                zones_panel(ui, tools);
            }
            Some(Category::Services) => {
                ui.separator();
                services_panel(ui, view, tools);
            }
            Some(c) => {
                ui.separator();
                ui.weak(format!("{} — nothing to build here yet", c.label()));
            }
            None => {}
        }
    });
}

/// The Services tab: the placeable service buildings of the content, the bulldozer, and the
/// placement ghost's verdict. R rotates the building.
pub fn services_panel(ui: &mut egui::Ui, view: Option<&SimView>, tools: &mut ToolState) {
    ui.horizontal(|ui| {
        if let Some(content) = view.and_then(|v| v.content.as_deref()) {
            for (id, def) in content.buildings().filter(|(_, d)| d.service.is_some()) {
                let selected = tools.active == ActiveTool::Place && tools.building.def == Some(id);
                if ui.selectable_label(selected, &def.name).clicked() {
                    if tools.active != ActiveTool::Place {
                        tools.select(ActiveTool::Place);
                    }
                    tools.building.def = Some(id);
                }
            }
        }
        let bulldozing = tools.active == ActiveTool::Bulldozer;
        if ui.selectable_label(bulldozing, "🚜 Bulldozer").clicked() {
            tools.select(if bulldozing {
                ActiveTool::None
            } else {
                ActiveTool::Bulldozer
            });
        }
    });
    if tools.active == ActiveTool::Place {
        match &tools.place_ghost {
            Some(g) if g.valid => {
                let cost = g.cost.map_or(String::new(), format_money);
                ui.colored_label(egui::Color32::LIGHT_GREEN, format!("✔ can place — {cost}"));
            }
            Some(g) => {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    format!("✖ {}", g.error.as_deref().unwrap_or("cannot place here")),
                );
            }
            None => {
                ui.weak("Hover a spot beside a road. R rotates, Esc leaves the tool.");
            }
        }
    }
}

/// The Zones tab: tool kinds and the zone palette. Picking either activates the zoning tool
/// (which shows the zone grid); "Off" leaves it.
pub fn zones_panel(ui: &mut egui::Ui, tools: &mut ToolState) {
    ui.horizontal(|ui| {
        for kind in ZoneToolKind::ALL {
            let selected = tools.active == ActiveTool::Zone && tools.zone.kind == kind;
            if ui.selectable_label(selected, kind.label()).clicked() {
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
    ui.horizontal(|ui| {
        for zone in PALETTE {
            let [r, g, b] = zone_color(zone);
            let color =
                egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8);
            let selected = tools.zone.zone == zone;
            let text = egui::RichText::new(zone_name(zone)).color(color).strong();
            if ui.selectable_label(selected, text).clicked() {
                tools.zone.zone = zone;
                if tools.active != ActiveTool::Zone {
                    tools.select(ActiveTool::Zone);
                }
            }
        }
    });
    if tools.active == ActiveTool::Zone {
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
    } else {
        ui.weak("Pick a zoning tool or a zone type to show the zone grid.");
    }
}

/// The Roads tab: tool buttons, road mode, road type, elevation and the ghost's verdict.
pub fn roads_panel(ui: &mut egui::Ui, view: Option<&SimView>, tools: &mut ToolState) {
    ui.horizontal(|ui| {
        for (tool, label) in [
            (ActiveTool::Road, "🛣 Road"),
            (ActiveTool::Bulldozer, "🚜 Bulldozer"),
        ] {
            if ui.selectable_label(tools.active == tool, label).clicked() {
                let next = if tools.active == tool {
                    ActiveTool::None
                } else {
                    tool
                };
                tools.select(next);
            }
        }
        if tools.active == ActiveTool::Road {
            ui.separator();
            for (mode, label) in [
                (RoadMode::Straight, "Straight"),
                (RoadMode::Curve, "Curve (3 clicks)"),
                (RoadMode::Free, "Free (Enter ends)"),
            ] {
                if ui
                    .selectable_label(tools.road.mode == mode, label)
                    .clicked()
                {
                    tools.road.set_mode(mode);
                }
            }
            ui.separator();
            if let Some(content) = view.and_then(|v| v.content.as_ref()) {
                if tools.road.road.is_none() {
                    tools.road.road = content.roads().next().map(|(id, _)| id);
                }
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
            }
            ui.separator();
            ui.label(match tools.road.elevation() {
                ElevationMode::Ground => "Ground (PgUp raises)".to_string(),
                ElevationMode::Elevated(h) => format!("Raised {h:.0} m (PgUp / PgDn)"),
            });
        }
    });
    match tools.active {
        ActiveTool::Road => match &tools.ghost {
            Some(g) if g.valid => {
                let cost = g.cost.map_or(String::new(), format_money);
                ui.colored_label(egui::Color32::LIGHT_GREEN, format!("✔ can build — {cost}"));
            }
            Some(g) => {
                for e in &g.errors {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("✖ {e}"));
                }
            }
            None => {
                ui.weak("Click to start a road. Esc cancels.");
            }
        },
        ActiveTool::Bulldozer => {
            ui.weak("Hover a building or road segment, click to remove it. Esc leaves the tool.");
        }
        ActiveTool::None | ActiveTool::Zone | ActiveTool::Place => {
            ui.weak("Pick a tool.");
        }
    }
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
        let mut rects = Vec::new();
        crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| {
            rects = top_bar(
                ui,
                view,
                &mut CommandOutbox::default(),
                &mut OverlayState::default(),
            );
        });
        let pos = rects.iter().find(|(s, _)| *s == speed).unwrap().1.center();
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
                top_bar(ui, view, outbox, &mut OverlayState::default());
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
    fn bottom_bar_draws_with_no_category_selected() {
        let ctx = egui::Context::default();
        let mut state = UiState::default();
        crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| {
            bottom_bar(ui, &mut state, None, &mut ToolState::default())
        });
        assert_eq!(state.category, None);
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
        });
        draw(&mut tools);
        tools.ghost = Some(Ghost {
            polylines: vec![],
            valid: false,
            cost: None,
            errors: vec!["too steep (20%, max 12%)".into()],
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
        let draw = |tools: &mut ToolState| {
            crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| zones_panel(ui, tools));
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
}
