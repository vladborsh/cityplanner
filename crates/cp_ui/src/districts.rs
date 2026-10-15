//! The Districts category (T-133): the brush tools, the district list (name, colour,
//! residents, delete), the tax block of the chosen district, and the map labels.

use bevy_egui::egui;
use cp_content::ZoneType;
use cp_district::{DistrictCommand, DistrictId, MAX_DISTRICTS, PALETTE_RGB, TAX_MAX, TAX_MIN};
use cp_sim::{SimCommand, SimView};
use cp_tools::{ActiveTool, CommandOutbox, DistrictToolKind, PALETTE, ToolState, zone_name};

use crate::icons::Icon;
use crate::panels::section;
use crate::style::Sizes;
use crate::tile::{TileIcon, tool_tile, tool_tile_enabled};

fn kind_icon(kind: DistrictToolKind) -> Icon {
    match kind {
        DistrictToolKind::BrushSmall => Icon::DistrictBrushSmall,
        DistrictToolKind::BrushLarge => Icon::DistrictBrushLarge,
        DistrictToolKind::Erase => Icon::DistrictErase,
    }
}

/// The colour of a palette index.
pub fn swatch(color: u8) -> egui::Color32 {
    let [r, g, b] = PALETTE_RGB[usize::from(color) % PALETTE_RGB.len()];
    egui::Color32::from_rgb(r, g, b)
}

/// The id the next `Create` will get: the lowest free slot (the rule of `cp_district::apply`).
pub fn next_id(view: &SimView) -> Option<DistrictId> {
    let list = &view.districts.list;
    let slot = list.iter().position(Option::is_none).unwrap_or(list.len());
    (slot < MAX_DISTRICTS).then(|| DistrictId(slot as u8 + 1))
}

/// "District N" with `N` the id the new district gets.
pub fn new_name(id: DistrictId) -> String {
    format!("District {}", id.0)
}

fn send(outbox: &mut CommandOutbox, cmd: DistrictCommand) {
    outbox.push(SimCommand::District(cmd));
}

/// The Districts panel.
pub fn districts_panel(
    ui: &mut egui::Ui,
    view: Option<&SimView>,
    tools: &mut ToolState,
    outbox: &mut CommandOutbox,
) {
    section(ui, "Tool", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            for kind in DistrictToolKind::ALL {
                let selected = tools.active == ActiveTool::District && tools.district.kind == kind;
                let tile = tool_tile(ui, TileIcon::Icon(kind_icon(kind)), kind.label(), selected);
                if tile.clicked() {
                    if tools.active != ActiveTool::District {
                        tools.select(ActiveTool::District);
                    }
                    tools.district.set_kind(kind);
                }
            }
            let next = view.and_then(next_id);
            if tool_tile_enabled(
                ui,
                TileIcon::Icon(Icon::DistrictNew),
                "New district",
                false,
                next.is_some(),
            )
            .clicked()
                && let Some(id) = next
            {
                send(outbox, DistrictCommand::Create { name: new_name(id) });
                tools.district.selected = Some(id);
                if tools.active != ActiveTool::District {
                    tools.select(ActiveTool::District);
                }
            }
            if tool_tile_enabled(
                ui,
                TileIcon::Icon(Icon::Remove),
                "Off",
                false,
                tools.active == ActiveTool::District,
            )
            .clicked()
            {
                tools.select(ActiveTool::None);
            }
        });
    });
    let Some(view) = view else { return };
    section(ui, "Districts", |ui| {
        if view.districts.iter().next().is_none() {
            ui.weak("No districts yet: press New district, then paint with a brush.");
        }
        for (id, d) in view.districts.iter() {
            district_row(ui, view, tools, outbox, id, d);
        }
    });
    if let Some(id) = tools.district.selected
        && let Some(d) = view.districts.get(id)
    {
        section(ui, &format!("Taxes of {}", d.name), |ui| {
            tax_block(ui, view, outbox, id, d);
        });
    }
}

fn district_row(
    ui: &mut egui::Ui,
    view: &SimView,
    tools: &mut ToolState,
    outbox: &mut CommandOutbox,
    id: DistrictId,
    d: &cp_district::District,
) {
    let selected = tools.district.selected == Some(id);
    ui.horizontal(|ui| {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
        ui.painter().rect_filled(rect, 3.0, swatch(d.color));
        if selected {
            ui.painter().rect_stroke(
                rect,
                3.0,
                egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Outside,
            );
        }
        if resp.on_hover_text("Choose this district").clicked() {
            tools.district.selected = Some(id);
            if tools.active != ActiveTool::District {
                tools.select(ActiveTool::District);
            }
        }
        // The name: edited in a buffer kept in egui memory; sent when the field loses focus.
        let key = egui::Id::new(("district_name", id.0));
        let mut buf: String = ui
            .data(|m| m.get_temp(key))
            .unwrap_or_else(|| d.name.clone());
        if !ui.memory(|m| m.has_focus(key)) {
            buf = d.name.clone();
        }
        let r = ui.add(
            egui::TextEdit::singleline(&mut buf)
                .id(key)
                .desired_width(150.0)
                .char_limit(cp_district::NAME_MAX),
        );
        let commit = r.lost_focus() && buf.trim() != d.name && !buf.trim().is_empty();
        if commit {
            send(
                outbox,
                DistrictCommand::Rename {
                    district: id,
                    name: buf.trim().to_string(),
                },
            );
        }
        ui.data_mut(|m| m.insert_temp(key, buf));
        if ui.small_button("Delete").clicked() {
            send(outbox, DistrictCommand::Delete { district: id });
            if selected {
                tools.district.selected = None;
            }
        }
    });
    if let Some(s) = view.city.districts.iter().find(|s| s.id == id) {
        ui.weak(format!(
            "    {} residents · {} buildings · tax {} last week",
            s.population,
            s.buildings,
            cp_tools::format_money(s.income_last_week)
        ));
    }
}

/// One row per zone type: "own rate" switch, the slider, and the city rate beside it.
fn tax_block(
    ui: &mut egui::Ui,
    view: &SimView,
    outbox: &mut CommandOutbox,
    id: DistrictId,
    d: &cp_district::District,
) {
    for zone in PALETTE {
        let Some(slot) = cp_district::zone_slot(zone) else {
            continue;
        };
        let city = view.economy.taxes.get(zone).unwrap_or(0);
        let own = d.tax[slot];
        ui.horizontal(|ui| {
            let mut on = own.is_some();
            if ui.checkbox(&mut on, zone_short(zone)).changed() {
                send(
                    outbox,
                    DistrictCommand::SetTax {
                        district: id,
                        zone,
                        pct: on.then_some(city.clamp(TAX_MIN, TAX_MAX)),
                    },
                );
            }
            match own {
                Some(mut pct) => {
                    let r = ui.add(egui::Slider::new(&mut pct, TAX_MIN..=TAX_MAX).suffix(" %"));
                    if r.drag_stopped() || (r.changed() && !r.dragged()) {
                        send(
                            outbox,
                            DistrictCommand::SetTax {
                                district: id,
                                zone,
                                pct: Some(pct),
                            },
                        );
                    }
                    ui.weak(format!("city {city} %"));
                }
                None => {
                    ui.weak(format!("city rate {city} %"));
                }
            }
        })
        .response
        .on_hover_text(zone_name(zone));
    }
}

fn zone_short(z: ZoneType) -> &'static str {
    match z {
        ZoneType::ResLow => "R low",
        ZoneType::ResHigh => "R high",
        ZoneType::ComLow => "C low",
        ZoneType::ComHigh => "C high",
        ZoneType::Industrial => "Industry",
        ZoneType::Office => "Office",
        ZoneType::None => "",
    }
}

/// The name labels of the districts, at the centre of mass of their cells.
#[derive(Default)]
pub struct Centroids {
    key: (usize, usize),
    pub labels: Vec<(String, bevy::math::Vec2)>,
}

impl Centroids {
    /// Recomputed only when the map or the records changed (compared by `Arc`).
    pub fn update(&mut self, view: &SimView) -> &[(String, bevy::math::Vec2)] {
        let key = (
            std::sync::Arc::as_ptr(&view.district_map) as usize,
            std::sync::Arc::as_ptr(&view.districts) as usize,
        );
        if key != self.key {
            self.key = key;
            self.labels = view
                .districts
                .iter()
                .filter_map(|(id, d)| {
                    let (mut sum, mut n) = (bevy::math::Vec2::ZERO, 0u32);
                    for c in view.district_map.cells_of(id) {
                        sum += cp_district::DistrictMap::cell_center(c);
                        n += 1;
                    }
                    (n > 0).then(|| (d.name.clone(), sum / n as f32))
                })
                .collect();
        }
        &self.labels
    }
}

/// Draws the district names on the map while the overlay or the brush is on.
pub fn draw_labels(
    mut contexts: bevy_egui::EguiContexts,
    view: Option<bevy::prelude::Res<SimView>>,
    overlay: bevy::prelude::Res<cp_render::OverlayState>,
    tools: bevy::prelude::Res<ToolState>,
    cameras: bevy::prelude::Query<(&bevy::prelude::Camera, &bevy::prelude::GlobalTransform)>,
    mut centroids: bevy::prelude::Local<Centroids>,
) {
    let Some(view) = view else { return };
    if overlay.active != cp_render::Overlay::Districts && tools.active != ActiveTool::District {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let Ok((camera, transform)) = cameras.single() else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("district_labels"),
    ));
    for (name, at) in centroids.update(&view) {
        let y = view.terrain.as_deref().map_or(0.0, |t| t.height(*at));
        let Ok(p) = camera.world_to_viewport(transform, bevy::math::Vec3::new(at.x, y, at.y))
        else {
            continue;
        };
        let pos = egui::pos2(p.x, p.y);
        painter.text(
            pos + egui::vec2(1.0, 1.0),
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::proportional(16.0),
            egui::Color32::BLACK,
        );
        painter.text(
            pos,
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::proportional(16.0),
            egui::Color32::WHITE,
        );
    }
}
