//! The Industry category (T-207, M5): the area brush, "New area" with the choice of its
//! resource, and the list of the industry areas. The Resources and Industry areas overlays show
//! automatically while a tool of the category is on. The building sections come with T-211.

use bevy_egui::egui;
use cp_content::ResourceKind;
use cp_district::{AreaId, AreaKind, DistrictCommand, MAX_AREAS, NAME_MAX};
use cp_render::{Overlay, OverlayState, resource_color};
use cp_sim::{SimCommand, SimView};
use cp_tools::{ActiveTool, CommandOutbox, DistrictToolKind, ToolState};

use crate::icons::Icon;
use crate::panels::section;
use crate::style::Sizes;
use crate::tile::{TileIcon, tool_tile, tool_tile_enabled};

pub fn resource_name(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Farming => "Farming",
        ResourceKind::Forestry => "Forestry",
        ResourceKind::Ore => "Ore",
        ResourceKind::Oil => "Oil",
    }
}

pub fn resource_icon(kind: ResourceKind) -> Icon {
    match kind {
        ResourceKind::Farming => Icon::ResourceFarming,
        ResourceKind::Forestry => Icon::ResourceForestry,
        ResourceKind::Ore => Icon::ResourceOre,
        ResourceKind::Oil => Icon::ResourceOil,
    }
}

/// The colour of a resource for swatches.
pub fn swatch(kind: ResourceKind) -> egui::Color32 {
    let [r, g, b] = resource_color(kind);
    egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

fn kind_icon(kind: DistrictToolKind) -> Icon {
    match kind {
        DistrictToolKind::BrushSmall => Icon::DistrictBrushSmall,
        DistrictToolKind::BrushLarge => Icon::DistrictBrushLarge,
        DistrictToolKind::Erase => Icon::DistrictErase,
    }
}

/// The id the next `CreateArea` will get: the lowest free slot (the rule of `cp_district`).
pub fn next_id(view: &SimView) -> Option<AreaId> {
    let list = &view.areas.list;
    let slot = list.iter().position(Option::is_none).unwrap_or(list.len());
    (slot < MAX_AREAS).then(|| AreaId(slot as u8 + 1))
}

/// "Forestry area 3" with `3` the id the new area gets.
pub fn new_name(kind: ResourceKind, id: AreaId) -> String {
    format!("{} area {}", resource_name(kind), id.0)
}

fn send(outbox: &mut CommandOutbox, cmd: DistrictCommand) {
    outbox.push(SimCommand::District(cmd));
}

/// Shows the overlays of the category: the resources under the area tints.
fn show_overlays(overlay: &mut OverlayState) {
    overlay.active = Overlay::Resources;
}

/// The Industry panel.
pub fn industry_panel(
    ui: &mut egui::Ui,
    view: Option<&SimView>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    outbox: &mut CommandOutbox,
) {
    section(ui, "Tool", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            for kind in DistrictToolKind::ALL {
                let selected = tools.active == ActiveTool::Area && tools.area.kind == kind;
                if tool_tile(ui, TileIcon::Icon(kind_icon(kind)), kind.label(), selected).clicked()
                {
                    if tools.active != ActiveTool::Area {
                        tools.select(ActiveTool::Area);
                    }
                    tools.area.set_kind(kind);
                    show_overlays(overlay);
                }
            }
            if tool_tile_enabled(
                ui,
                TileIcon::Icon(Icon::Remove),
                "Off",
                false,
                tools.active == ActiveTool::Area,
            )
            .clicked()
            {
                tools.select(ActiveTool::None);
            }
        });
    });
    let Some(view) = view else { return };
    section(ui, "New area", |ui| {
        ui.weak("Choose what the area works, then press New area.");
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            for kind in ResourceKind::ALL {
                let on = tools.area.new_resource == Some(kind);
                if tool_tile(
                    ui,
                    TileIcon::Icon(resource_icon(kind)),
                    resource_name(kind),
                    on,
                )
                .on_hover_text(format!(
                    "{}: the colour of its area and the overlay",
                    resource_name(kind)
                ))
                .clicked()
                {
                    tools.area.new_resource = Some(kind);
                }
            }
            let next = next_id(view);
            let can = next.zip(tools.area.new_resource);
            if tool_tile_enabled(
                ui,
                TileIcon::Icon(Icon::DistrictNew),
                "New area",
                false,
                can.is_some(),
            )
            .clicked()
                && let Some((id, kind)) = can
            {
                send(
                    outbox,
                    DistrictCommand::CreateArea {
                        kind: AreaKind::Industry(kind),
                        name: new_name(kind, id),
                    },
                );
                tools.area.selected = Some(id);
                if tools.active != ActiveTool::Area {
                    tools.select(ActiveTool::Area);
                }
                show_overlays(overlay);
            }
        });
    });
    section(ui, "Areas", |ui| {
        if view.areas.iter().next().is_none() {
            ui.weak("No areas yet: pick a resource, press New area, then paint with a brush.");
        }
        for (id, a) in view.areas.iter() {
            let AreaKind::Industry(kind) = a.kind else {
                continue;
            };
            let selected = tools.area.selected == Some(id);
            ui.horizontal(|ui| {
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                ui.painter().rect_filled(rect, 3.0, swatch(kind));
                if selected {
                    ui.painter().rect_stroke(
                        rect,
                        3.0,
                        egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                        egui::StrokeKind::Outside,
                    );
                }
                if resp.on_hover_text("Choose this area").clicked() {
                    tools.area.selected = Some(id);
                    if tools.active != ActiveTool::Area {
                        tools.select(ActiveTool::Area);
                    }
                    show_overlays(overlay);
                }
                let key = egui::Id::new(("area_name", id.0));
                let mut buf: String = ui
                    .data(|m| m.get_temp(key))
                    .unwrap_or_else(|| a.name.clone());
                if !ui.memory(|m| m.has_focus(key)) {
                    buf = a.name.clone();
                }
                let r = ui.add(
                    egui::TextEdit::singleline(&mut buf)
                        .id(key)
                        .desired_width(130.0)
                        .char_limit(NAME_MAX),
                );
                if r.lost_focus() && buf.trim() != a.name && !buf.trim().is_empty() {
                    send(
                        outbox,
                        DistrictCommand::RenameArea {
                            area: id,
                            name: buf.trim().to_string(),
                        },
                    );
                }
                ui.data_mut(|m| m.insert_temp(key, buf));
                ui.label(format!("L{}", a.level));
                if ui.small_button("Delete").clicked() {
                    send(outbox, DistrictCommand::DeleteArea { area: id });
                    if selected {
                        tools.area.selected = None;
                    }
                }
            });
            ui.weak(format!(
                "    {} · {} cells",
                resource_name(kind),
                view.area_map.cell_count(id)
            ));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{click, frame, input};
    use cp_sim::SimRunner;
    use std::sync::Arc;

    fn view() -> SimView {
        let db = Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        );
        let mut sim = SimRunner::new(1);
        sim.set_content(db);
        sim.view()
    }

    fn draw(
        ctx: &egui::Context,
        view: &SimView,
        tools: &mut ToolState,
        overlay: &mut OverlayState,
        outbox: &mut CommandOutbox,
        events: Vec<egui::Event>,
    ) -> Vec<egui::Rect> {
        crate::tile::take_rects(ctx);
        frame(ctx, input(0.0, events), |ui| {
            industry_panel(ui, Some(view), tools, overlay, outbox)
        });
        crate::tile::take_rects(ctx)
            .0
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    fn press(
        ctx: &egui::Context,
        view: &SimView,
        tools: &mut ToolState,
        overlay: &mut OverlayState,
        outbox: &mut CommandOutbox,
        at: egui::Pos2,
    ) {
        let [down, up] = click(at);
        draw(ctx, view, tools, overlay, outbox, down);
        draw(ctx, view, tools, overlay, outbox, up);
    }

    /// Tiles: Brush S, Brush L, Erase, Off | four resources, New area.
    #[test]
    fn new_area_with_forestry_sends_create_area_and_selects_it_and_a_brush_tile_picks_the_tool() {
        let view = view();
        let ctx = egui::Context::default();
        let (mut tools, mut overlay, mut outbox) = (
            ToolState::default(),
            OverlayState::default(),
            CommandOutbox::default(),
        );
        let tiles = draw(&ctx, &view, &mut tools, &mut overlay, &mut outbox, vec![]);
        assert_eq!(tiles.len(), 4 + 4 + 1);
        // New area is dead until a resource is chosen.
        press(
            &ctx,
            &view,
            &mut tools,
            &mut overlay,
            &mut outbox,
            tiles[8].center(),
        );
        assert_eq!(outbox.iter().count(), 0);
        // Forestry is the second resource tile.
        press(
            &ctx,
            &view,
            &mut tools,
            &mut overlay,
            &mut outbox,
            tiles[5].center(),
        );
        assert_eq!(tools.area.new_resource, Some(ResourceKind::Forestry));
        press(
            &ctx,
            &view,
            &mut tools,
            &mut overlay,
            &mut outbox,
            tiles[8].center(),
        );
        let sent: Vec<SimCommand> = outbox.iter().cloned().collect();
        assert_eq!(
            sent,
            [SimCommand::District(DistrictCommand::CreateArea {
                kind: AreaKind::Industry(ResourceKind::Forestry),
                name: "Forestry area 1".into()
            })]
        );
        assert_eq!(tools.area.selected, Some(AreaId(1)));
        assert_eq!(tools.active, ActiveTool::Area);
        assert_eq!(overlay.active, Overlay::Resources);
        // A brush tile switches the brush kind.
        press(
            &ctx,
            &view,
            &mut tools,
            &mut overlay,
            &mut outbox,
            tiles[1].center(),
        );
        assert_eq!(tools.area.kind, DistrictToolKind::BrushLarge);
    }

    #[test]
    fn resources_have_their_own_icon_colour_and_name() {
        let mut names: Vec<_> = ResourceKind::ALL
            .iter()
            .map(|k| resource_name(*k))
            .collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 4);
        let mut colours: Vec<_> = ResourceKind::ALL.iter().map(|k| swatch(*k)).collect();
        colours.dedup();
        assert_eq!(colours.len(), 4);
        assert_eq!(new_name(ResourceKind::Ore, AreaId(7)), "Ore area 7");
    }
}
