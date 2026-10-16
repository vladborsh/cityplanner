//! The Transport category (T-157): a section per mode of public transport with the tools of
//! that mode — for buses the line tool and the depot — and, in the Transit view, the labels on
//! the map: the riders waiting at each stop and the length and round trip of the line being
//! drawn.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use cp_content::{ContentDb, ServiceKind};
use cp_render::{Overlay, OverlayState, RtsCamera};
use cp_sim::SimView;
use cp_tools::{ActiveTool, ToolState, route_summary};
use cp_transit::TransitMode;

use crate::buildings::{Card, groups, lock_text, select};
use crate::icons::Icon;
use crate::style::Sizes;
use crate::tile::{TileIcon, tool_tile, tool_tile_enabled};

/// One section of the panel: a mode, its buildings and whether it has a line tool.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub mode: TransitMode,
    pub cards: Vec<Card>,
    pub line_tool: bool,
}

impl Section {
    pub fn title(&self) -> &'static str {
        match self.mode {
            TransitMode::Bus => "Bus",
            TransitMode::Metro => "Metro",
            TransitMode::Train => "Train",
        }
    }
}

/// The sections to draw. Buses have the line tool and the buildings of the `Transit` service
/// kind (the depot); metro and train come with their stations (T-162) and stay hidden while
/// the content has none.
pub fn sections(content: &ContentDb) -> Vec<Section> {
    let cards: Vec<Card> = groups(content)
        .into_iter()
        .filter(|g| g.kind == ServiceKind::Transit)
        .flat_map(|g| g.cards)
        .collect();
    let mut out = Vec::new();
    if !cards.is_empty() || content.roads().next().is_some() {
        out.push(Section {
            mode: TransitMode::Bus,
            cards,
            line_tool: true,
        });
    }
    out
}

/// Whether the category has anything to offer.
pub fn has_tools(content: Option<&ContentDb>) -> bool {
    content.is_some_and(|c| !sections(c).is_empty())
}

/// Riders waiting at a stop. Zero until the riders exist (T-159).
pub fn waiting_at(_view: &SimView, _stop: cp_transit::StopId) -> u32 {
    0
}

pub fn transit_panel(
    ui: &mut egui::Ui,
    view: Option<&SimView>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
) {
    let Some(content) = view.and_then(|v| v.content.as_deref()) else {
        return;
    };
    for section in sections(content) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            Icon::LineBus.show(ui, Sizes::SECTION_ICON);
            ui.strong(section.title());
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            if section.line_tool {
                let on = tools.active == ActiveTool::Line;
                if tool_tile(ui, TileIcon::Icon(Icon::LineBus), "Bus line", on)
                    .on_hover_text(
                        "Click roads to place stops, click the first stop to close the line. Backspace takes the last stop away.",
                    )
                    .clicked()
                {
                    if on {
                        tools.select(ActiveTool::None);
                    } else {
                        tools.select(ActiveTool::Line);
                        overlay.active = Overlay::Transit;
                    }
                }
            }
            for card in &section.cards {
                let on = tools.active == ActiveTool::Place && tools.building.def == Some(card.id);
                let lock = view.and_then(|v| lock_text(v, content, card.id));
                let hover = match &lock {
                    Some(l) => format!("{l}\n{}", crate::buildings::card_text(card)),
                    None => crate::buildings::card_text(card),
                };
                if tool_tile_enabled(
                    ui,
                    TileIcon::Def(&card.icon),
                    &card.name,
                    on,
                    lock.is_none(),
                )
                .on_hover_text(hover)
                .clicked()
                {
                    select(tools, card.id);
                    overlay.active = Overlay::Transit;
                }
            }
        });
    }
}

/// The map labels of the Transit view: the waiting count at every stop, and the length and
/// round trip of the line being drawn beside the cursor.
pub fn transit_labels(
    mut contexts: EguiContexts,
    view: Option<Res<SimView>>,
    overlay: Res<OverlayState>,
    tools: Res<ToolState>,
    cameras: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
) {
    let Some(view) = view else { return };
    if overlay.active != Overlay::Transit {
        return;
    }
    let (Ok(ctx), Ok((camera, transform))) = (contexts.ctx_mut(), cameras.single()) else {
        return;
    };
    let Some(terrain) = view.terrain.as_deref() else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("transit_labels"),
    ));
    let label = |at: Vec2, text: String, size: f32, lift: f32| {
        let Ok(p) =
            camera.world_to_viewport(transform, Vec3::new(at.x, terrain.height(at) + lift, at.y))
        else {
            return;
        };
        let pos = egui::pos2(p.x, p.y);
        let font = egui::FontId::proportional(size);
        painter.text(
            pos + egui::vec2(1.0, 1.0),
            egui::Align2::CENTER_BOTTOM,
            &text,
            font.clone(),
            egui::Color32::BLACK,
        );
        painter.text(
            pos,
            egui::Align2::CENTER_BOTTOM,
            text,
            font,
            egui::Color32::WHITE,
        );
    };
    for (id, stop) in view.transit.net.stops.iter() {
        label(stop.pos, waiting_at(&view, id).to_string(), 14.0, 6.0);
    }
    if tools.active == ActiveTool::Line
        && let Some(g) = &tools.line_ghost
        && let Some((at, _)) = g.hover
        && g.route.len() > 1
    {
        label(at, route_summary(g), 16.0, 14.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> ContentDb {
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap()
    }

    #[test]
    fn only_the_bus_section_exists_and_it_holds_the_line_tool_and_the_depot() {
        let c = content();
        let s = sections(&c);
        assert_eq!(s.len(), 1, "metro and train have no content yet");
        assert_eq!(s[0].mode, TransitMode::Bus);
        assert!(s[0].line_tool);
        assert!(s[0].cards.iter().any(|c| c.name == "Bus Depot"));
        assert!(has_tools(Some(&c)) && !has_tools(None));
    }
}
