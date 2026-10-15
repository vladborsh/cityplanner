//! The Roads category (T-083): the tools, the drawing mode, the elevation buttons and the
//! road types as cards ordered by class and lanes. Read-only over the content; a click only
//! changes the existing [`ToolState`].

use bevy_egui::egui;
use cp_content::{ContentDb, LaneKind, RoadClass, RoadDefId};
use cp_core::Cost;
use cp_tools::{
    ActiveTool, ELEVATION_STEP, MAX_ELEVATION_LEVEL, RoadMode, ToolState, format_money,
};

use crate::icons::{self, Icon};

/// What a card shows, taken from the road definition.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: RoadDefId,
    pub name: String,
    pub class: RoadClass,
    /// Car lanes.
    pub lanes: usize,
    pub speed_kmh: f32,
    pub one_way: bool,
    pub cost_per_m: Cost,
    pub upkeep_per_m_week: Cost,
    pub allows_zoning: bool,
}

/// One card per road type, ordered by class, then lanes, then name.
pub fn cards(content: &ContentDb) -> Vec<Card> {
    let mut out: Vec<Card> = content
        .roads()
        .map(|(id, def)| Card {
            id,
            name: def.name.clone(),
            class: def.class,
            lanes: def.lanes.iter().filter(|l| l.kind == LaneKind::Car).count(),
            speed_kmh: def.speed_kmh,
            one_way: def.is_one_way(),
            cost_per_m: Cost::from_units(def.cost_per_m),
            upkeep_per_m_week: Cost::from_units(def.upkeep_per_m_week),
            allows_zoning: def.allows_zoning,
        })
        .collect();
    out.sort_by(|a, b| (a.class, a.lanes, &a.name).cmp(&(b.class, b.lanes, &b.name)));
    out
}

/// "2 lanes · 50 km/h · one-way".
pub fn card_summary(c: &Card) -> String {
    format!(
        "{} {} · {:.0} km/h{}",
        c.lanes,
        if c.lanes == 1 { "lane" } else { "lanes" },
        c.speed_kmh,
        if c.one_way { " · one-way" } else { "" }
    )
}

fn class_label(class: RoadClass) -> &'static str {
    match class {
        RoadClass::Local => "Local",
        RoadClass::Collector => "Collector",
        RoadClass::Arterial => "Arterial",
        RoadClass::Highway => "Highway",
    }
}

/// The elevation as the panel states it.
pub fn elevation_text(level: u8) -> String {
    if level == 0 {
        "Ground".to_string()
    } else {
        format!("Raised {:.0} m", f32::from(level) * ELEVATION_STEP)
    }
}

fn card(ui: &mut egui::Ui, content: &ContentDb, c: &Card, selected: bool) -> egui::Response {
    let fill = if selected {
        ui.visuals().selection.bg_fill
    } else {
        ui.visuals().faint_bg_color
    };
    let inner = egui::Frame::group(ui.style()).fill(fill).show(ui, |ui| {
        ui.set_width(ui.available_width());
        icons::road_profile(
            ui,
            content.road(c.id),
            egui::vec2(ui.available_width(), 22.0),
        );
        ui.strong(&c.name);
        ui.label(card_summary(c));
        ui.label(format!(
            "{}/m · {}/m/wk",
            format_money(c.cost_per_m),
            format_money(c.upkeep_per_m_week)
        ));
        if !c.allows_zoning {
            ui.colored_label(egui::Color32::from_rgb(230, 180, 60), "no zoning");
        }
    });
    ui.interact(
        inner.response.rect,
        inner.response.id.with("road_card"),
        egui::Sense::click(),
    )
}

/// The Roads category panel.
pub fn roads_panel(ui: &mut egui::Ui, content: Option<&ContentDb>, tools: &mut ToolState) {
    ui.add_space(4.0);
    ui.weak("Tools");
    ui.horizontal_wrapped(|ui| {
        let on = tools.active == ActiveTool::Road;
        if crate::panels::tool_button(ui, Icon::Road, "Build", on).clicked() {
            tools.select(if on {
                ActiveTool::None
            } else {
                ActiveTool::Road
            });
        }
        let on = tools.active == ActiveTool::Bulldozer;
        if crate::panels::tool_button(ui, Icon::Bulldozer, "Bulldozer", on).clicked() {
            tools.select(if on {
                ActiveTool::None
            } else {
                ActiveTool::Bulldozer
            });
        }
    });
    if tools.active != ActiveTool::Road {
        return;
    }
    ui.add_space(4.0);
    ui.weak("Mode");
    ui.horizontal_wrapped(|ui| {
        for (mode, icon, label) in [
            (RoadMode::Straight, Icon::Road, "Straight"),
            (RoadMode::Curve, Icon::Curve, "Curve (3 clicks)"),
            (RoadMode::Free, Icon::FreeRoad, "Free (Enter ends)"),
        ] {
            if crate::panels::tool_button(ui, icon, label, tools.road.mode == mode).clicked() {
                tools.road.set_mode(mode);
            }
        }
    });
    ui.add_space(4.0);
    ui.weak("Elevation (PgUp / PgDn)");
    ui.horizontal(|ui| {
        if ui
            .add_enabled(tools.road.level > 0, egui::Button::new("−"))
            .clicked()
        {
            tools.road.lower();
        }
        ui.label(elevation_text(tools.road.level));
        if ui
            .add_enabled(
                tools.road.level < MAX_ELEVATION_LEVEL,
                egui::Button::new("+"),
            )
            .clicked()
        {
            tools.road.raise();
        }
    });
    let Some(content) = content else {
        return;
    };
    let cards = cards(content);
    if tools.road.road.is_none() {
        tools.road.road = cards.first().map(|c| c.id);
    }
    ui.add_space(4.0);
    ui.weak("Road type");
    let mut last = None;
    for c in &cards {
        if last != Some(c.class) {
            ui.strong(class_label(c.class));
            last = Some(c.class);
        }
        if card(ui, content, c, tools.road.road == Some(c.id)).clicked() {
            tools.road.road = Some(c.id);
        }
    }
}
