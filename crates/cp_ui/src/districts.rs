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

/// The tabs of the chosen district (T-191).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DistrictTab {
    #[default]
    Taxes,
    Policies,
    Specialisation,
    Stats,
}

impl DistrictTab {
    pub const ALL: [DistrictTab; 4] = [
        DistrictTab::Taxes,
        DistrictTab::Policies,
        DistrictTab::Specialisation,
        DistrictTab::Stats,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DistrictTab::Taxes => "Taxes",
            DistrictTab::Policies => "Policies",
            DistrictTab::Specialisation => "Special.",
            DistrictTab::Stats => "Stats",
        }
    }

    fn icon(self) -> Icon {
        match self {
            DistrictTab::Taxes => Icon::DistrictTabTaxes,
            DistrictTab::Policies => Icon::DistrictTabPolicies,
            DistrictTab::Specialisation => Icon::DistrictTabSpecialisation,
            DistrictTab::Stats => Icon::DistrictTabStats,
        }
    }
}

fn tab_key() -> egui::Id {
    egui::Id::new("district_tab")
}

/// The tab in use; it is kept across frames and across other districts being chosen.
pub fn current_tab(ctx: &egui::Context) -> DistrictTab {
    ctx.data(|d| d.get_temp(tab_key())).unwrap_or_default()
}

/// What the Stats tab shows for one district, counted from the view alone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DistrictStats {
    pub residents: u32,
    pub buildings: u32,
    /// Households' homes the buildings offer, and the job slots (capacities).
    pub homes: u32,
    pub jobs: u32,
    /// Buildings per zone type, in `cp_sim::stats::ZONES` order.
    pub by_zone: [u32; 6],
    /// Average land value of the district's cells, if the view has the grid.
    pub land_value: Option<f32>,
    pub no_water: u32,
    pub no_power: u32,
}

/// The stats of district `id`: only the buildings and cells inside it count.
pub fn district_stats(view: &SimView, id: DistrictId) -> DistrictStats {
    use cp_building::BuildingFlags;
    let mut out = DistrictStats {
        residents: view
            .city
            .districts
            .iter()
            .find(|s| s.id == id)
            .map_or(0, |s| s.population),
        ..Default::default()
    };
    if let Some(db) = view.content.as_deref() {
        for (_, b) in view.buildings.iter() {
            if cp_district::district_at(&view.district_map, b.pos) != Some(id) {
                continue;
            }
            let def = db.building(b.def);
            out.buildings += 1;
            out.homes += u32::from(def.households);
            out.jobs += def.jobs.iter().map(|&j| u32::from(j)).sum::<u32>();
            if let Some(k) = cp_sim::stats::ZONES.iter().position(|z| *z == def.zone) {
                out.by_zone[k] += 1;
            }
            out.no_water += u32::from(b.flags.has(BuildingFlags::NO_WATER));
            out.no_power += u32::from(b.flags.has(BuildingFlags::NO_POWER));
        }
    }
    if let Some(grid) = view.overlays.land_value.as_deref() {
        let (mut sum, mut n) = (0.0, 0u32);
        for c in view.district_map.cells_of(id) {
            let p = cp_district::DistrictMap::cell_center(c);
            let at =
                |v: f32| ((v / cp_economy::CELL_M).floor().max(0.0) as usize).min(grid.side - 1);
            sum += grid.values[at(p.y) * grid.side + at(p.x)] * 100.0;
            n += 1;
        }
        out.land_value = (n > 0).then(|| sum / n as f32);
    }
    out
}

fn stats_tab(ui: &mut egui::Ui, view: &SimView, id: DistrictId) {
    let s = district_stats(view, id);
    egui::Grid::new(("district_stats", id.0))
        .num_columns(2)
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, label: &str, value: String| {
                ui.weak(label);
                ui.label(value);
                ui.end_row();
            };
            row(ui, "Residents", s.residents.to_string());
            row(ui, "Homes (capacity)", s.homes.to_string());
            row(ui, "Jobs (capacity)", s.jobs.to_string());
            row(ui, "Buildings", s.buildings.to_string());
            for (k, z) in cp_sim::stats::ZONES.iter().enumerate() {
                row(
                    ui,
                    &format!("  {}", zone_name(*z)),
                    s.by_zone[k].to_string(),
                );
            }
            row(
                ui,
                "Land value (avg)",
                s.land_value.map_or("—".into(), |v| format!("{v:.1}")),
            );
            row(ui, "Without water", s.no_water.to_string());
            row(ui, "Without power", s.no_power.to_string());
        });
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
        let tab = current_tab(ui.ctx());
        section(ui, &d.name, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
                for t in DistrictTab::ALL {
                    if tool_tile(ui, TileIcon::Icon(t.icon()), t.label(), tab == t).clicked() {
                        ui.ctx().data_mut(|m| m.insert_temp(tab_key(), t));
                    }
                }
            });
            match tab {
                DistrictTab::Taxes => tax_block(ui, view, outbox, id, d),
                DistrictTab::Policies => {
                    crate::policies::policy_tiles(ui, view, outbox, Some(id));
                }
                DistrictTab::Specialisation => {
                    ui.weak("Specialisation: coming soon.");
                }
                DistrictTab::Stats => stats_tab(ui, view, id),
            }
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

/// One district name on the map: where, and the icons of the district's policies in force
/// (T-193; the city's own are not repeated in every district).
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub name: String,
    pub at: bevy::math::Vec2,
    pub icons: Vec<String>,
}

/// The name labels of the districts, at the centre of mass of their cells.
#[derive(Default)]
pub struct Centroids {
    key: (usize, usize),
    pub labels: Vec<Label>,
}

impl Centroids {
    /// Recomputed only when the map or the records changed (compared by `Arc`).
    pub fn update(&mut self, view: &SimView) -> &[Label] {
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
                    let icons = view
                        .content
                        .as_deref()
                        .map(|db| {
                            cp_content::PolicyKind::ALL
                                .into_iter()
                                .filter(|k| d.policies & k.bit() != 0)
                                .filter_map(|k| db.policy(k))
                                .map(|p| p.icon.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    (n > 0).then(|| Label {
                        name: d.name.clone(),
                        at: sum / n as f32,
                        icons,
                    })
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
    for label in centroids.update(&view) {
        let (name, at) = (&label.name, &label.at);
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
        // A row of mini-icons of the policies in force under the name.
        if !label.icons.is_empty() {
            let side = 18.0;
            let width = label.icons.len() as f32 * (side + 2.0) - 2.0;
            egui::Area::new(egui::Id::new(("district_policy_icons", name)))
                .order(egui::Order::Background)
                .interactable(false)
                .fixed_pos(pos + egui::vec2(-width * 0.5, 12.0))
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for icon in &label.icons {
                            ui.add(
                                egui::Image::new(crate::icons::def_icon(icon))
                                    .fit_to_exact_size(egui::vec2(side, side)),
                            );
                        }
                    });
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_building::{AccessPoint, Building, BuildingFlags, BuildingState, Lot};
    use cp_core::{Id, Tick};
    use cp_sim::SimRunner;
    use std::sync::Arc;

    /// T-191: three buildings in the district and two outside; the counters are the district's.
    #[test]
    fn the_stats_count_only_the_buildings_inside_the_district() {
        let db = Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        );
        let mut sim = SimRunner::new(1);
        sim.set_content(Arc::clone(&db));
        let cell = cp_district::DistrictMap::cell_of(bevy::math::Vec2::new(500.0, 500.0)).unwrap();
        for c in [
            DistrictCommand::Create {
                name: "Hill".into(),
            },
            DistrictCommand::Paint {
                cells: vec![cell],
                district: DistrictId(1),
            },
        ] {
            sim.push_command(SimCommand::District(c), sim.tick());
        }
        sim.tick_once();
        let centre = cp_district::DistrictMap::cell_center(cell);
        let house = db.building_id("house_low_1").unwrap();
        let put = |sim: &mut SimRunner, at: bevy::math::Vec2, flags: u8| {
            sim.world_mut()
                .resource_mut::<cp_building::BuildingGrid>()
                .insert(Building {
                    def: house,
                    footprint: cp_building::Footprint::Lot(Lot {
                        block: Id::new(0, 0),
                        col: 0,
                        w: 1,
                        d: 1,
                    }),
                    level: 1,
                    state: BuildingState::Active,
                    ready_at: Tick(0),
                    pos: at,
                    rot: 0.0,
                    access: Some(AccessPoint {
                        segment: Id::new(0, 0),
                        t: 0.0,
                    }),
                    flags: BuildingFlags(flags),
                    level_progress: 0.0,
                });
        };
        put(&mut sim, centre, 0);
        put(&mut sim, centre, BuildingFlags::NO_WATER);
        put(&mut sim, centre, BuildingFlags::NO_POWER);
        put(
            &mut sim,
            bevy::math::Vec2::new(5000.0, 5000.0),
            BuildingFlags::NO_WATER,
        );
        put(&mut sim, bevy::math::Vec2::new(7000.0, 2000.0), 0);
        let view = sim.view();
        let s = district_stats(&view, DistrictId(1));
        assert_eq!((s.buildings, s.no_water, s.no_power), (3, 1, 1));
        assert_eq!(s.by_zone.iter().sum::<u32>(), 3);
        let homes = u32::from(db.building(house).households);
        assert_eq!(s.homes, 3 * homes);
        assert_eq!(s.residents, 0);
        // No district: nothing counts.
        assert_eq!(district_stats(&view, DistrictId(2)).buildings, 0);
    }
}
