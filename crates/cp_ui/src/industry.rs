//! The Industry category (T-207, M5): the area brush, "New area" with the choice of its
//! resource, and the list of the industry areas. The Resources and Industry areas overlays show
//! automatically while a tool of the category is on. The building sections come with T-211.

use bevy_egui::egui;
use cp_content::{BuildingDefId, CargoKind, ContentDb, IndustryRole, ResourceKind};
use cp_district::{AreaId, AreaKind, DistrictCommand};
use cp_locale::{tr, trf};
use cp_render::{Overlay, OverlayState, resource_color};
use cp_sim::{SimCommand, SimView};
use cp_tools::{ActiveTool, CommandOutbox, DistrictToolKind, ToolState};

use crate::buildings::{Card, UniqueState, badge, card_text, groups, select};
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

/// The id the next `CreateArea` will get: the lowest free slot (the rule of `cp_district`).
pub fn next_id(view: &SimView) -> Option<AreaId> {
    cp_tools::area::next_area(&view.areas)
}

/// "Forestry area 3" with `3` the id the new area gets.
pub fn new_name(kind: ResourceKind, id: AreaId) -> String {
    cp_tools::area::new_area_name(AreaKind::Industry(kind), id)
}

/// The name of a cargo as the player reads it (the key is the variant's name).
fn cargo_name(cargo: CargoKind) -> String {
    tr(&format!("{cargo:?}"))
}

/// Picking a resource gives the brush an area of that kind (BUG-062): the selected one if it is
/// of that kind, else the first existing one, else a new area is made and chosen. The brush is
/// switched on.
fn pick_resource_area(
    view: &SimView,
    kind: ResourceKind,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    outbox: &mut CommandOutbox,
) {
    let of_kind = |id: AreaId| {
        view.areas
            .get(id)
            .is_some_and(|a| a.kind == AreaKind::Industry(kind))
    };
    let existing = tools
        .area
        .selected
        .filter(|&a| of_kind(a))
        .or_else(|| view.areas.iter().map(|(id, _)| id).find(|&id| of_kind(id)));
    let chosen = match existing {
        Some(id) => Some(id),
        None => next_id(view).inspect(|&id| {
            send(
                outbox,
                DistrictCommand::CreateArea {
                    kind: AreaKind::Industry(kind),
                    name: new_name(kind, id),
                },
            );
        }),
    };
    if chosen.is_some() {
        tools.area.selected = chosen;
        if tools.active != ActiveTool::Area {
            tools.select(ActiveTool::Area);
        }
        show_overlays(overlay);
    }
}

fn send(outbox: &mut CommandOutbox, cmd: DistrictCommand) {
    outbox.push(SimCommand::District(cmd));
}

/// Shows the overlays of the category: the resources under the area tints.
pub(crate) fn show_overlays(overlay: &mut OverlayState) {
    overlay.open_auto(Overlay::Resources);
}

/// The chain buildings of one resource, in role order (main, extractors, processors,
/// warehouse), as cards. Factories (no resource) are not here (T-215).
pub fn chain_sections(db: &ContentDb) -> Vec<(ResourceKind, Vec<Card>)> {
    let cards: Vec<Card> = groups(db)
        .into_iter()
        .filter(|g| g.kind == cp_content::ServiceKind::Industry)
        .flat_map(|g| g.cards)
        .collect();
    ResourceKind::ALL
        .into_iter()
        .filter_map(|res| {
            let mut mine: Vec<(u8, Card)> = cards
                .iter()
                .filter_map(|c| {
                    let spec = db.building(c.id).industry.as_ref()?;
                    (spec.resource == Some(res)).then(|| (spec.role as u8, c.clone()))
                })
                .collect();
            mine.sort_by_key(|(role, _)| *role);
            (!mine.is_empty()).then(|| (res, mine.into_iter().map(|(_, c)| c).collect()))
        })
        .collect()
}

/// Why a chain building cannot be placed yet, from the view alone: no main building in an area
/// of its resource, or no such area high enough. `None` when it is open (or not a chain
/// building). The same rules as `cp_building::validate_industry_site`, read from the snapshot.
pub fn industry_lock(view: &SimView, db: &ContentDb, def: BuildingDefId) -> Option<String> {
    let spec = db.building(def).industry.as_ref()?;
    if spec.role == IndustryRole::Factory {
        return crate::industry_data::factory_lock(view, db, def);
    }
    let res = spec.resource?;
    if spec.role == IndustryRole::Main {
        return None;
    }
    // The areas of the resource that have a main building.
    let mut best: Option<u8> = None;
    for (id, area) in view.areas.iter() {
        if area.kind != AreaKind::Industry(res) {
            continue;
        }
        let has_main = view.buildings.iter().any(|(_, b)| {
            db.building(b.def)
                .industry
                .as_ref()
                .is_some_and(|s| s.role == IndustryRole::Main)
                && cp_district::area_at(&view.area_map, b.pos) == Some(id)
        });
        if has_main {
            best = Some(best.map_or(area.level, |l| l.max(area.level)));
        }
    }
    match best {
        None => Some(tr("needs main building")),
        Some(level) if level < spec.min_level => {
            Some(trf!("area level {level}", level = spec.min_level))
        }
        Some(_) => None,
    }
}

/// What the Area panel says about one area, counted from the view: only its own buildings.
#[derive(Clone, Debug, PartialEq)]
pub struct AreaSummary {
    pub name: String,
    pub resource: ResourceKind,
    pub level: u8,
    pub xp: u32,
    /// The xp of the next level, `None` at the top.
    pub next_xp: Option<u32>,
    pub buildings: u32,
    /// Job slots of the buildings (capacity).
    pub jobs: u32,
    /// What the buildings hold, per cargo in kind order.
    pub stock: Vec<(CargoKind, f32)>,
    /// The last week's money (T-316).
    pub economy: AreaEconomy,
}

/// One area's last settled week in money (T-316): what its buildings made, what the trucks
/// brought in and cost, and what its chain buildings paid and cost the city.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AreaEconomy {
    /// Units made by cargo, in kind order.
    pub produced: Vec<(CargoKind, f32)>,
    pub exports: cp_core::Cost,
    /// The cost of the orders from the outside, transport included.
    pub imports: cp_core::Cost,
    pub upkeep: cp_core::Cost,
    /// The workers' taxes of the area's buildings.
    pub taxes: cp_core::Cost,
}

impl AreaEconomy {
    /// Export income and taxes minus imports and upkeep.
    pub fn net(&self) -> cp_core::Cost {
        self.exports + self.taxes - self.imports - self.upkeep
    }
}

/// The last settled week of area `id`.
pub fn area_economy(view: &SimView, id: AreaId) -> AreaEconomy {
    let books = &view.area_books;
    let book = books.last_of(id.0).cloned().unwrap_or_default();
    let (taxes, upkeep) = books.money.get(&id.0).copied().unwrap_or_default();
    AreaEconomy {
        produced: book.produced.into_iter().collect(),
        exports: book.exports,
        imports: book.imports,
        upkeep,
        taxes,
    }
}

pub fn area_summary(view: &SimView, id: AreaId) -> Option<AreaSummary> {
    let db = view.content.as_deref()?;
    let area = view.areas.get(id)?;
    let AreaKind::Industry(resource) = area.kind else {
        return None;
    };
    let mut out = AreaSummary {
        name: area.name.clone(),
        resource,
        level: area.level,
        xp: area.xp,
        next_xp: db
            .balance()
            .industry_levels
            .get(usize::from(area.level.saturating_sub(1)))
            .copied(),
        buildings: 0,
        jobs: 0,
        stock: Vec::new(),
        economy: area_economy(view, id),
    };
    for (bid, b) in view.buildings.iter() {
        if cp_district::area_at(&view.area_map, b.pos) != Some(id) {
            continue;
        }
        let def = db.building(b.def);
        if def.industry.is_none() {
            continue;
        }
        out.buildings += 1;
        out.jobs += def.jobs.iter().map(|&j| u32::from(j)).sum::<u32>();
        for &(cargo, v) in view.industry.stock.get(&bid).map_or(&[][..], Vec::as_slice) {
            match out.stock.iter_mut().find(|(k, _)| *k == cargo) {
                Some(e) => e.1 += v,
                None => out.stock.push((cargo, v)),
            }
        }
    }
    out.stock.sort_by_key(|(k, _)| *k as u8);
    Some(out)
}

/// "Out of input", "Storage full"…: the words of an industry problem.
pub fn problem_text(p: cp_industry::IndustryProblem) -> Option<&'static str> {
    use cp_industry::IndustryProblem as P;
    match p {
        P::None => None,
        P::NoMain => Some("no main building in the area"),
        P::NoInput => Some("waiting for input"),
        P::Full => Some("storage full"),
        P::Depleted => Some("the ground is spent"),
    }
}

/// Inspector rows of a chain building that the block does not draw: its cargo's prices.
pub fn industry_rows(view: &SimView, id: cp_building::BuildingId) -> Vec<(String, String)> {
    crate::trade::price_rows(view, id)
}

/// A cargo mini card: its picture, name, the rate of the day and the stock against the
/// building's cap.
fn cargo_mini(
    ui: &mut egui::Ui,
    db: &ContentDb,
    cargo: CargoKind,
    rate: Option<f32>,
    stock: f32,
    cap: f32,
) {
    egui::Frame::new()
        .fill(egui::Color32::from_gray(36))
        .corner_radius(5.0)
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            ui.set_width(150.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::Image::new(Icon::cargo(cargo).image())
                        .fit_to_exact_size(egui::vec2(18.0, 18.0)),
                );
                ui.label(egui::RichText::new(cargo_name(cargo)).size(12.0));
                if let Some(r) = rate {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (sign, colour) = if r < 0.0 {
                            ("", egui::Color32::from_rgb(235, 150, 70))
                        } else {
                            ("+", egui::Color32::from_rgb(120, 200, 120))
                        };
                        ui.label(
                            egui::RichText::new(format!("{sign}{r:.0}/{}", tr("day")))
                                .size(11.0)
                                .color(colour),
                        );
                    });
                }
            });
            let frac = if cap > 0.0 {
                (stock / cap).min(1.0)
            } else {
                0.0
            };
            let (rect, resp) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 5.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 2.5, egui::Color32::from_gray(56));
            ui.painter().rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * frac, 5.0)),
                2.5,
                crate::industry_win::cargo_color(db, cargo),
            );
            resp.on_hover_text(format!("{stock:.0} / {cap:.0}"));
        });
}

/// The block of a chain building in the inspector (T-337 §2.6): what it is, its state, the
/// input and output with their stock and speed, the load and the prices. Nothing for a
/// building outside the chains. A warehouse's cargo is [`warehouse_controls`].
pub fn industry_block(ui: &mut egui::Ui, view: &SimView, id: cp_building::BuildingId) {
    let Some(db) = view.content.as_deref() else {
        return;
    };
    let Some(b) = view.buildings.get(id) else {
        return;
    };
    let def = db.building(b.def);
    let Some(spec) = def.industry.as_ref() else {
        return;
    };
    let area = cp_district::area_at(&view.area_map, b.pos);
    let me = area.and_then(|a| {
        crate::industry_data::area_buildings(view, a)
            .into_iter()
            .find(|c| c.id == id)
    });
    ui.horizontal(|ui| {
        ui.add(
            egui::Image::new(crate::icons::def_icon(&def.icon))
                .fit_to_exact_size(egui::vec2(28.0, 28.0)),
        );
        ui.vertical(|ui| {
            let product = spec.outputs.first().map(|&(c, _)| cargo_name(c));
            ui.strong(match product {
                Some(p) => format!("{} · {p}", tr(&def.name)),
                None => tr(&def.name),
            });
            if let Some(a) = area.and_then(|a| view.areas.get(a)) {
                ui.weak(format!(
                    "{} · {}",
                    a.name,
                    trf!("Lvl {level}", level = a.level)
                ));
            }
        });
        let problem = view.industry.problem_of(id);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            match problem_text(problem) {
                Some(t) => ui.colored_label(
                    egui::Color32::from_rgb(235, 150, 70),
                    format!("▲ {}", tr(t)),
                ),
                None => ui.colored_label(egui::Color32::from_rgb(120, 200, 120), "✓"),
            };
        });
    });
    let (load, filled, slots, cap) = me
        .as_ref()
        .map_or((1.0, 0, 0, 0.0), |c| (c.load(), c.filled, c.slots, c.cap));
    if matches!(
        spec.role,
        IndustryRole::Extractor | IndustryRole::Processor | IndustryRole::Factory
    ) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
            ui.vertical(|ui| {
                for &(c, r) in &spec.inputs {
                    cargo_mini(
                        ui,
                        db,
                        c,
                        Some(-r * load),
                        view.industry.stock_of(id, c),
                        cap,
                    );
                }
            });
            if !spec.inputs.is_empty() {
                ui.label("▸");
            }
            ui.vertical(|ui| {
                for &(c, r) in &spec.outputs {
                    cargo_mini(
                        ui,
                        db,
                        c,
                        Some(r * load),
                        view.industry.stock_of(id, c),
                        cap,
                    );
                }
            });
        });
    }
    if slots > 0 {
        ui.weak(trf!(
            "Production {pct}% · {filled} / {slots} workers",
            pct = (load * 100.0).round() as u32,
            filled = filled,
            slots = slots
        ));
    }
}

/// The mode and cargo buttons of a warehouse's inspector (T-219): Balanced / Fill / Empty and
/// "All cargo" or one cargo of its chain; each click sends `SetWarehouse`. Nothing for any
/// other building. The buttons' rectangles go to `actions` (tests click them).
pub fn warehouse_controls(
    ui: &mut egui::Ui,
    id: cp_building::BuildingId,
    view: &SimView,
    outbox: &mut CommandOutbox,
    actions: &mut Vec<(String, egui::Rect)>,
) {
    use cp_industry::{IndustryCommand, WarehouseMode};
    let Some(db) = view.content.as_deref() else {
        return;
    };
    let Some(b) = view.buildings.get(id) else {
        return;
    };
    let Some(spec) = db
        .building(b.def)
        .industry
        .as_ref()
        .filter(|i| i.role == IndustryRole::Warehouse)
    else {
        return;
    };
    let now = view.warehouses.get(id);
    let mut send = |cargo: Option<CargoKind>, mode: WarehouseMode| {
        outbox.push(SimCommand::Industry(IndustryCommand::SetWarehouse {
            building: id,
            cargo,
            mode,
        }));
    };
    ui.separator();
    // The fill of the warehouse and the mark of what its mode aims at.
    let held = view.industry.total(id);
    let cap = crate::industry_data::area_buildings(
        view,
        cp_district::area_at(&view.area_map, b.pos).unwrap_or(cp_district::AreaId(0)),
    )
    .into_iter()
    .find(|c| c.id == id)
    .map_or(spec.storage, |c| c.cap);
    let target = match now.mode {
        WarehouseMode::Balanced => 0.5,
        WarehouseMode::Fill => 1.0,
        WarehouseMode::Empty => 0.0,
    };
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(220.0), 10.0),
        egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, 4.0, egui::Color32::from_gray(48));
    let frac = if cap > 0.0 {
        (held / cap).min(1.0)
    } else {
        0.0
    };
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * frac, 10.0)),
        4.0,
        egui::Color32::from_rgb(90, 160, 230),
    );
    let x = rect.left() + rect.width() * target;
    ui.painter().line_segment(
        [
            egui::pos2(x, rect.top() - 2.0),
            egui::pos2(x, rect.bottom() + 2.0),
        ],
        egui::Stroke::new(2.0, egui::Color32::WHITE),
    );
    resp.on_hover_text(format!("{held:.0} / {cap:.0}"));
    ui.horizontal(|ui| {
        for (mode, icon, label, hint) in [
            (
                WarehouseMode::Balanced,
                Icon::WarehouseBalanced,
                "Balanced",
                "Keeps about half full",
            ),
            (
                WarehouseMode::Fill,
                Icon::WarehouseFill,
                "Fill",
                "Takes cargo in; gives it out only in a shortage",
            ),
            (
                WarehouseMode::Empty,
                Icon::WarehouseEmpty,
                "Empty",
                "Gives cargo out and exports it, takes none",
            ),
        ] {
            let r = ui
                .add(egui::Button::selectable(
                    now.mode == mode,
                    (icon.sized(18.0), tr(label)),
                ))
                .on_hover_text(tr(hint));
            actions.push((label.to_string(), r.rect));
            if r.clicked() && now.mode != mode {
                send(now.cargo, mode);
            }
        }
    });
    if let Some(hint) = crate::trade::warehouse_hint(view, id) {
        ui.weak(hint);
    }
    ui.horizontal_wrapped(|ui| {
        let options = std::iter::once((None, "All cargo".to_string())).chain(
            cp_industry::chain_cargo(db, spec.resource)
                .into_iter()
                .map(|c| (Some(c), format!("{c:?}"))),
        );
        // `label` is the English key (tests click by it); the button shows its translation.
        for (cargo, label) in options {
            let r = ui.selectable_label(now.cargo == cargo, {
                let text = egui::RichText::new(tr(&label));
                match cargo {
                    Some(c) => text.color(crate::industry_win::cargo_color(db, c)),
                    None => text,
                }
            });
            actions.push((label, r.rect));
            if r.clicked() && now.cargo != cargo {
                send(cargo, now.mode);
            }
        }
    });
}

/// A short text on the corner of a tile (a count, a level).
fn text_badge(ui: &egui::Ui, tile: egui::Rect, text: &str, left: bool, colour: egui::Color32) {
    let p = ui.painter();
    let galley = p.layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(11.0),
        egui::Color32::WHITE,
    );
    let size = galley.size() + egui::vec2(8.0, 2.0);
    let at = if left {
        tile.left_top() + egui::vec2(3.0, 3.0)
    } else {
        tile.right_top() + egui::vec2(-size.x - 3.0, 3.0)
    };
    let r = egui::Rect::from_min_size(at, size);
    p.rect_filled(r, 7.0, colour);
    p.galley(r.min + egui::vec2(4.0, 1.0), galley, egui::Color32::WHITE);
}

fn clip_name(name: &str) -> String {
    if name.chars().count() <= 11 {
        name.to_string()
    } else {
        format!("{}…", name.chars().take(10).collect::<String>())
    }
}

/// "Build in zone": the zone of this resource the badges and locks count by, with its level, a
/// drop-down of the zones of the resource and the brushes (T-337 §2.5).
fn build_zone_block(
    ui: &mut egui::Ui,
    view: &SimView,
    res: ResourceKind,
    zone: Option<AreaId>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
) {
    ui.vertical(|ui| {
        ui.set_width(180.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.weak(tr("Building in the zone"));
        let zones: Vec<(AreaId, String, u8)> = view
            .areas
            .iter()
            .filter(|(_, a)| a.kind == AreaKind::Industry(res))
            .map(|(id, a)| (id, a.name.clone(), a.level))
            .collect();
        match zone.and_then(|z| zones.iter().find(|(id, ..)| *id == z)) {
            Some((_, name, level)) => {
                let label =
                    egui::RichText::new(format!("■ {} ▾", clip_name(name))).color(swatch(res));
                ui.horizontal(|ui| {
                    ui.menu_button(label, |ui| {
                        for (id, name, level) in &zones {
                            ui.horizontal(|ui| {
                                if ui
                                    .selectable_label(zone == Some(*id), clip_name(name))
                                    .on_hover_text(name)
                                    .clicked()
                                {
                                    tools.area.selected = Some(*id);
                                    show_overlays(overlay);
                                    ui.close();
                                }
                                ui.weak(trf!("Lvl {level}", level = level));
                            });
                        }
                    });
                    ui.weak(trf!("Lvl {level}", level = level));
                });
                if let Some(a) = zone.and_then(|z| view.areas.get(z)) {
                    let next = view
                        .content
                        .as_deref()
                        .and_then(|db| {
                            db.balance()
                                .industry_levels
                                .get(usize::from(a.level.saturating_sub(1)))
                                .copied()
                        })
                        .unwrap_or(0);
                    let frac = if next == 0 {
                        1.0
                    } else {
                        (a.xp as f32 / next as f32).min(1.0)
                    };
                    ui.add(egui::ProgressBar::new(frac).desired_height(5.0));
                }
            }
            None => {
                ui.weak(tr("no area of this resource"));
            }
        }
        ui.horizontal(|ui| {
            for kind in DistrictToolKind::ALL {
                let on = tools.active == ActiveTool::Area && tools.area.kind == kind;
                let icon = match kind {
                    DistrictToolKind::BrushSmall => Icon::DistrictBrushSmall,
                    DistrictToolKind::BrushLarge => Icon::DistrictBrushLarge,
                    DistrictToolKind::Erase => Icon::ZoneEraser,
                };
                let r = icon
                    .button(ui, 16.0, 2.0, on)
                    .on_hover_text(tr(kind.label()));
                if r.clicked() {
                    if tools.active != ActiveTool::Area {
                        tools.select(ActiveTool::Area);
                    }
                    tools.area.set_kind(kind);
                    show_overlays(overlay);
                }
            }
        });
    });
}

/// The chain sections of the panel: for each resource a tab with the zone block and the chain
/// as columns Main → Extraction → Processing → Storage → Next; closed tiles are grey with the
/// level that opens them.
fn chain_panel(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
) {
    for (res, cards) in chain_sections(db) {
        if !crate::tray::is_active(ui.ctx(), &tr(resource_name(res))) {
            continue;
        }
        if !crate::tray::in_tray(ui.ctx()) {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                resource_icon(res).show(ui, Sizes::SECTION_ICON);
                ui.strong(tr(resource_name(res)));
            });
        }
        // The zone the badges and locks count by: the selected one of this resource, else the
        // first.
        let zone = tools
            .area
            .selected
            .filter(|&a| {
                view.areas
                    .get(a)
                    .is_some_and(|x| x.kind == AreaKind::Industry(res))
            })
            .or_else(|| {
                view.areas
                    .iter()
                    .find(|(_, a)| a.kind == AreaKind::Industry(res))
                    .map(|(id, _)| id)
            });
        let built = zone.map(|z| crate::industry_data::area_buildings(view, z));
        ui.horizontal_top(|ui| {
            build_zone_block(ui, view, res, zone, tools, overlay);
            ui.separator();
            let columns: [(&str, &[IndustryRole]); 4] = [
                ("Main building", &[IndustryRole::Main]),
                ("Extraction", &[IndustryRole::Extractor]),
                ("Processing", &[IndustryRole::Processor]),
                ("Storage", &[IndustryRole::Warehouse]),
            ];
            for (n, (title, roles)) in columns.into_iter().enumerate() {
                if n > 0 {
                    ui.label(egui::RichText::new("▸").color(egui::Color32::from_gray(120)))
                        .on_hover_text(tr(title));
                }
                let _ = title;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, 2.0);
                    for card in cards.iter().filter(|c| {
                        db.building(c.id)
                            .industry
                            .as_ref()
                            .is_some_and(|s| roles.contains(&s.role))
                    }) {
                        chain_tile(ui, view, db, card, zone, built.as_deref(), tools, overlay);
                    }
                });
            }
            ui.label(egui::RichText::new("▸").color(egui::Color32::from_gray(120)))
                .on_hover_text(tr("Next"));
            ui.horizontal_top(|ui| {
                next_tiles(ui, view, db, res, tools, overlay);
            });
        });
    }
}

/// One tile of a chain building: the output chip under it, a count and a lock badge.
#[allow(clippy::too_many_arguments)]
fn chain_tile(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    card: &Card,
    zone: Option<AreaId>,
    built: Option<&[crate::industry_data::ChainBuilding]>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
) {
    use crate::industry_data::{ChainLock, chain_lock};
    let lock = chain_lock(view, db, card.id, zone);
    let on = tools.active == ActiveTool::Place && tools.building.def == Some(card.id);
    let hover = match &lock {
        Some(l) => format!(
            "{}\n{}",
            trf!("Locked: {reason}", reason = l.text()),
            card_text(card)
        ),
        None => card_text(card),
    };
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let r = tool_tile_enabled(
            ui,
            TileIcon::Def(&card.icon),
            &tr(&card.name),
            on,
            lock.is_none(),
        )
        .on_hover_text(hover);
        match lock {
            Some(ChainLock::Level(level)) => text_badge(
                ui,
                r.rect,
                &trf!("Lvl {level}", level = level),
                false,
                egui::Color32::from_gray(90),
            ),
            Some(ChainLock::NoMain) => badge(ui, r.rect, UniqueState::Locked),
            None => {}
        }
        let count = built.map_or(0, |b| b.iter().filter(|c| c.def == card.id).count());
        if count > 0 {
            text_badge(
                ui,
                r.rect,
                &format!("×{count}"),
                true,
                egui::Color32::from_rgb(40, 110, 70),
            );
        }
        // The output beside the icon: a square in the cargo's colour and the rate.
        let spec = db.building(card.id).industry.as_ref();
        let (dot, text) = match spec {
            Some(sp) => match sp.outputs.first() {
                Some(&(c, rate)) => (
                    Some(crate::industry_win::cargo_color(db, c)),
                    format!("{rate:.0}"),
                ),
                None if sp.storage > 0.0 => (None, format!("{:.0}", sp.storage)),
                None => (None, String::new()),
            },
            None => (None, String::new()),
        };
        if !text.is_empty() {
            let icon_right = r.rect.center().x + Sizes::TILE_ICON * 0.5 + 3.0;
            let p = ui.painter();
            if let Some(c) = dot {
                p.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(icon_right, r.rect.top() + 12.0),
                        egui::vec2(8.0, 8.0),
                    ),
                    2.0,
                    c,
                );
            }
            p.text(
                egui::pos2(icon_right, r.rect.top() + 24.0),
                egui::Align2::LEFT_TOP,
                text,
                egui::FontId::proportional(10.0),
                egui::Color32::from_gray(200),
            );
        }
        if r.clicked() {
            select(tools, card.id);
            show_overlays(overlay);
        }
    });
}

/// "Next": the factories that take the products of the chain; a click opens the Factories tab
/// with the factory chosen.
fn next_tiles(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    res: ResourceKind,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
) {
    let made: Vec<CargoKind> = db
        .buildings()
        .filter_map(|(_, d)| d.industry.as_ref())
        .filter(|s| s.resource == Some(res) && s.role == IndustryRole::Processor)
        .flat_map(|s| s.outputs.iter().map(|&(c, _)| c))
        .collect();
    for (id, d) in db.buildings() {
        let Some(s) = d
            .industry
            .as_ref()
            .filter(|s| s.role == IndustryRole::Factory)
        else {
            continue;
        };
        if !s.inputs.iter().any(|&(c, _)| made.contains(&c)) {
            continue;
        }
        let lock = crate::industry_data::factory_lock(view, db, id);
        let r = tool_tile_enabled(ui, TileIcon::Def(&d.icon), &tr(&d.name), false, true)
            .on_hover_text(match &lock {
                Some(l) => format!(
                    "{}\n{}",
                    trf!("Locked: {reason}", reason = l),
                    tr("Open the Factories tab")
                ),
                None => tr("Open the Factories tab"),
            });
        if lock.is_some() {
            text_badge(
                ui,
                r.rect,
                &trf!("Lvl {level}", level = s.min_level),
                false,
                egui::Color32::from_gray(90),
            );
        }
        if r.clicked() {
            if lock.is_none() {
                select(tools, id);
                show_overlays(overlay);
            }
            ui.ctx().data_mut(|m| {
                m.insert_temp(egui::Id::new(TRAY_GOTO), tr("Factories"));
            });
        }
    }
}

/// The tray tab a click inside a panel wants to go to (read by the tray after it drew).
pub(crate) const TRAY_GOTO: &str = "cp_tray_goto";

/// The Factories tab: the four factories as wide tiles.
fn factories_panel(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
) {
    let status = crate::industry_data::cargo_status(view);
    let mut none = None;
    ui.horizontal_top(|ui| {
        for (id, d) in db.buildings() {
            if d.industry
                .as_ref()
                .is_none_or(|s| s.role != IndustryRole::Factory)
            {
                continue;
            }
            ui.allocate_ui_with_layout(
                egui::vec2(340.0, 80.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    crate::industry_city::factory_card(
                        ui, view, db, id, &status, tools, overlay, &mut none, true,
                    );
                },
            );
        }
    });
}

/// The Industry panel.
pub fn industry_panel(
    ui: &mut egui::Ui,
    view: Option<&SimView>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    outbox: &mut CommandOutbox,
) {
    let Some(view) = view else { return };
    section(ui, &tr("Areas"), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            for kind in ResourceKind::ALL {
                let on = tools.area.new_resource == Some(kind);
                if tool_tile(
                    ui,
                    TileIcon::Icon(resource_icon(kind)),
                    &tr(resource_name(kind)),
                    on,
                )
                .on_hover_text(trf!(
                    "{resource}: the colour of its area and the overlay",
                    resource = tr(resource_name(kind))
                ))
                .clicked()
                {
                    tools.area.new_resource = Some(kind);
                    pick_resource_area(view, kind, tools, overlay, outbox);
                }
            }
            let next = next_id(view);
            let can = next.zip(tools.area.new_resource);
            if tool_tile_enabled(
                ui,
                TileIcon::Icon(Icon::DistrictNew),
                &tr("New area"),
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
    if let Some(db) = view.content.as_deref() {
        chain_panel(ui, view, db, tools, overlay);
        section(ui, &tr("Factories"), |ui| {
            factories_panel(ui, view, db, tools, overlay)
        });
    }
}

/// The options strip of the Industry tray: the brush, Off, the list of areas and what is painted.
pub fn industry_options(
    ui: &mut egui::Ui,
    view: Option<&SimView>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    industry_open: &mut bool,
) {
    for kind in DistrictToolKind::ALL {
        let selected = tools.active == ActiveTool::Area && tools.area.kind == kind;
        if ui.selectable_label(selected, tr(kind.label())).clicked() {
            if tools.active != ActiveTool::Area {
                tools.select(ActiveTool::Area);
            }
            tools.area.set_kind(kind);
            // A brush with no area under it paints nothing (BUG-062): take the first.
            if let Some(view) = view
                && tools
                    .area
                    .selected
                    .is_none_or(|a| view.areas.get(a).is_none())
            {
                tools.area.selected = view.areas.iter().next().map(|(id, _)| id);
            }
            show_overlays(overlay);
        }
    }
    if ui
        .add_enabled(
            tools.active == ActiveTool::Area,
            egui::Button::new(tr("Off")),
        )
        .clicked()
    {
        tools.select(ActiveTool::None);
    }
    ui.separator();
    if ui.selectable_label(*industry_open, tr("Window")).clicked() {
        *industry_open = !*industry_open;
    }
    if tools.active == ActiveTool::Area
        && let Some(a) = view.and_then(|v| tools.area.selected.and_then(|id| v.areas.get(id)))
    {
        ui.label(trf!("Painting: {name}", name = a.name));
    }
}

#[cfg(test)]
pub(crate) mod tests {
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

    /// One frame of the options strip; every text with where it starts.
    fn options_frame(
        ctx: &egui::Context,
        view: &SimView,
        tools: &mut ToolState,
        overlay: &mut OverlayState,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Pos2)> {
        let mut open = false;
        crate::testutil::frame_texts(ctx, input(0.0, events), |ui| {
            ui.horizontal(|ui| industry_options(ui, Some(view), tools, overlay, &mut open));
        })
    }

    fn press_label(
        ctx: &egui::Context,
        view: &SimView,
        tools: &mut ToolState,
        overlay: &mut OverlayState,
        label: &str,
    ) {
        let texts = options_frame(ctx, view, tools, overlay, vec![]);
        let (_, at) = texts
            .iter()
            .find(|(t, _)| t == label)
            .unwrap_or_else(|| panic!("no {label:?} in {texts:?}"));
        let [down, up] = click(*at + egui::vec2(4.0, 4.0));
        options_frame(ctx, view, tools, overlay, down);
        options_frame(ctx, view, tools, overlay, up);
    }

    /// BUG-062: a brush chosen before any area exists paints nothing; with an area in the view it
    /// takes that area. T-317: the brushes are buttons of the options strip.
    #[test]
    fn a_brush_with_no_area_gets_the_first_area() {
        let ctx = egui::Context::default();
        let (mut tools, mut overlay) = (ToolState::default(), OverlayState::default());
        let empty = view();
        press_label(&ctx, &empty, &mut tools, &mut overlay, "Brush S");
        assert_eq!(tools.active, ActiveTool::Area);
        assert_eq!(tools.area.selected, None);
        let (sim, _, _) = world_with_areas(1);
        let view = sim.view();
        press_label(&ctx, &view, &mut tools, &mut overlay, "Brush L");
        assert_eq!(tools.area.kind, DistrictToolKind::BrushLarge);
        assert!(tools.area.selected.is_some(), "the first area");
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
        // Resources, New area, the 24 chain tiles and the "Next" shortcuts to the factories.
        assert!(tiles.len() >= 4 + 1 + 24, "{}", tiles.len());
        // New area is dead until a resource is chosen.
        press(
            &ctx,
            &view,
            &mut tools,
            &mut overlay,
            &mut outbox,
            tiles[4].center(),
        );
        assert_eq!(outbox.iter().count(), 0);
        // Forestry is the second resource tile.
        press(
            &ctx,
            &view,
            &mut tools,
            &mut overlay,
            &mut outbox,
            tiles[1].center(),
        );
        assert_eq!(tools.area.new_resource, Some(ResourceKind::Forestry));
        // BUG-062: picking a resource with no area of it makes one and switches the brush on.
        let create = SimCommand::District(DistrictCommand::CreateArea {
            kind: AreaKind::Industry(ResourceKind::Forestry),
            name: "Forestry area 1".into(),
        });
        assert_eq!(outbox.iter().cloned().collect::<Vec<_>>(), [create]);
        assert_eq!(tools.area.selected, Some(AreaId(1)));
        assert_eq!(tools.active, ActiveTool::Area);
        // "New area" makes one more.
        outbox = CommandOutbox::default();
        press(
            &ctx,
            &view,
            &mut tools,
            &mut overlay,
            &mut outbox,
            tiles[4].center(),
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
    }

    /// A world with a Forestry area and a Farming area, painted over two spots.
    pub(crate) fn world_with_areas(
        level: u8,
    ) -> (SimRunner, Arc<cp_content::ContentDb>, [bevy::math::Vec2; 2]) {
        let db = Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        );
        let mut sim = SimRunner::new(1);
        sim.set_content(Arc::clone(&db));
        let at = [
            bevy::math::Vec2::new(1000.0, 1000.0),
            bevy::math::Vec2::new(2000.0, 1000.0),
        ];
        for (k, res) in [ResourceKind::Forestry, ResourceKind::Farming]
            .into_iter()
            .enumerate()
        {
            let cell = cp_district::DistrictMap::cell_of(at[k]).unwrap();
            for c in [
                DistrictCommand::CreateArea {
                    kind: AreaKind::Industry(res),
                    name: format!("{res:?}"),
                },
                DistrictCommand::PaintArea {
                    cells: vec![cell],
                    area: AreaId(k as u8 + 1),
                },
            ] {
                sim.push_command(SimCommand::District(c), sim.tick());
            }
        }
        sim.tick_once();
        sim.world_mut()
            .resource_mut::<cp_district::Areas>()
            .get_mut(AreaId(1))
            .unwrap()
            .level = level;
        (sim, db, at)
    }

    pub(crate) fn put(
        sim: &mut SimRunner,
        db: &cp_content::ContentDb,
        name: &str,
        at: bevy::math::Vec2,
    ) -> cp_building::BuildingId {
        use cp_building::{AccessPoint, Building, BuildingFlags, BuildingState, Lot};
        let def = db.building_id(name).unwrap();
        let d = db.building(def);
        sim.world_mut()
            .resource_mut::<cp_building::BuildingGrid>()
            .insert(Building {
                def,
                footprint: cp_building::Footprint::Lot(Lot {
                    block: cp_core::Id::new(0, 0),
                    col: 0,
                    w: d.lot.0,
                    d: d.lot.1,
                }),
                level: 1,
                state: BuildingState::Active,
                ready_at: cp_core::Tick(0),
                pos: at,
                rot: 0.0,
                access: Some(AccessPoint {
                    segment: cp_core::Id::new(0, 0),
                    t: 0.5,
                }),
                flags: BuildingFlags::default(),
                level_progress: 0.0,
            })
    }

    /// T-211: a chain tile is closed without a main building in an area of its resource, and
    /// then by the area level, and opens when both hold; the main building is never locked.
    #[test]
    fn an_extractor_tile_is_locked_without_a_main_building_and_by_area_level_and_opens_with_them() {
        let (mut sim, db, at) = world_with_areas(1);
        let id = |n: &str| db.building_id(n).unwrap();
        let lock = |sim: &SimRunner, n: &str| industry_lock(&sim.view(), &db, id(n));
        assert_eq!(lock(&sim, "forest_main"), None);
        assert_eq!(
            lock(&sim, "forest_extractor_s").as_deref(),
            Some("needs main building")
        );
        // A main building in the Farming area does not open the forest chain.
        put(&mut sim, &db, "farm_main", at[1]);
        assert_eq!(
            lock(&sim, "forest_extractor_s").as_deref(),
            Some("needs main building")
        );
        put(&mut sim, &db, "forest_main", at[0]);
        assert_eq!(lock(&sim, "forest_extractor_s"), None);
        // The large extractor wants area level 2.
        assert_eq!(
            lock(&sim, "forest_extractor_l").as_deref(),
            Some("area level 2")
        );
        sim.world_mut()
            .resource_mut::<cp_district::Areas>()
            .get_mut(AreaId(1))
            .unwrap()
            .level = 2;
        assert_eq!(lock(&sim, "forest_extractor_l"), None);
    }

    /// T-211: the area panel counts only its own buildings and stock; the inspector rows list
    /// the stock and the problem of one building.
    #[test]
    fn the_area_summary_adds_up_only_its_own_buildings_and_the_inspector_shows_stock_and_problem() {
        use cp_industry::{IndustryProblem, IndustryState};
        let (mut sim, db, at) = world_with_areas(1);
        let a = put(&mut sim, &db, "forest_extractor_s", at[0]);
        let b = put(&mut sim, &db, "forest_main", at[0]);
        let c = put(&mut sim, &db, "farm_extractor_s", at[1]);
        let mut st = IndustryState::default();
        st.stock.insert(a, vec![(CargoKind::Logs, 30.0)]);
        st.stock.insert(b, vec![(CargoKind::Logs, 12.0)]);
        st.stock.insert(c, vec![(CargoKind::Crops, 99.0)]);
        st.problems.insert(a, IndustryProblem::Full);
        sim.world_mut().insert_resource(st);
        let view = sim.view();
        let s = area_summary(&view, AreaId(1)).unwrap();
        assert_eq!(s.resource, ResourceKind::Forestry);
        assert_eq!(s.buildings, 2);
        assert_eq!(s.stock, vec![(CargoKind::Logs, 42.0)]);
        assert_eq!(s.level, 1);
        assert_eq!(s.next_xp, Some(db.balance().industry_levels[0]));
        let farm = area_summary(&view, AreaId(2)).unwrap();
        assert_eq!((farm.buildings, farm.stock.len()), (1, 1));
        // T-337: the inspector block shows the stock and the problem of a building.
        let ctx = egui::Context::default();
        let texts = |id| {
            crate::testutil::frame_texts(&ctx, input(0.0, vec![]), |ui| {
                industry_block(ui, &view, id)
            })
            .into_iter()
            .map(|(t, _)| t)
            .collect::<Vec<_>>()
        };
        let sawmill = texts(a);
        assert!(sawmill.iter().any(|t| t == "▲ storage full"), "{sawmill:?}");
        assert!(
            sawmill.iter().any(|t| t.starts_with("Forest Extractor S")),
            "{sawmill:?}"
        );
        assert!(sawmill.iter().any(|t| t == "Logs"), "{sawmill:?}");
        let main = texts(b);
        assert!(main.iter().all(|t| !t.contains("storage full")), "{main:?}");
    }

    /// T-211: the chain sections list the four resources with their six buildings in role order,
    /// and the ghost verdict shows the lot richness (red under the least).
    #[test]
    fn the_chain_sections_come_from_content_in_role_order_and_the_verdict_shows_the_richness() {
        use crate::testutil::frame_texts;
        let db = cp_content::ContentDb::load_dir(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/content"
        ))
        .unwrap();
        let sections = chain_sections(&db);
        assert_eq!(sections.len(), 4);
        for (res, cards) in &sections {
            assert_eq!(cards.len(), 6, "{res:?}");
            let roles: Vec<_> = cards
                .iter()
                .map(|c| db.building(c.id).industry.as_ref().unwrap().role as u8)
                .collect();
            assert!(roles.windows(2).all(|w| w[0] <= w[1]), "{roles:?}");
        }
        let ctx = egui::Context::default();
        let mut tools = ToolState::default();
        tools.select(ActiveTool::Place);
        tools.place_ghost = Some(cp_tools::PlacementGhost {
            valid: false,
            error: Some("too poor".into()),
            richness: Some((12, 30)),
            ..Default::default()
        });
        let text = frame_texts(&ctx, crate::testutil::input(0.0, vec![]), |ui| {
            crate::panels::verdict(ui, crate::palette::Category::Industry, &tools)
        })
        .into_iter()
        .map(|(t, _)| t)
        .collect::<Vec<_>>()
        .join("\n");
        assert!(text.contains("Lot richness 12% (needs 30%)"), "{text}");
    }

    fn texts_of(
        ctx: &egui::Context,
        view: &SimView,
        tools: &mut ToolState,
        tab: &str,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Pos2)> {
        let mut overlay = OverlayState::default();
        let mut outbox = CommandOutbox::default();
        crate::tray::enter(ctx, tab);
        let out = crate::testutil::frame_texts(ctx, input(0.0, events), |ui| {
            industry_panel(ui, Some(view), tools, &mut overlay, &mut outbox)
        });
        crate::tray::leave(ctx);
        out
    }

    /// T-337: the tray has a Factories tab and no Trade tab (the market is in the window).
    #[test]
    fn the_tray_has_a_factories_tab_and_no_trade_tab() {
        let view = view();
        let labels: Vec<String> =
            crate::palette::tabs(crate::palette::Category::Industry, Some(&view))
                .into_iter()
                .map(|(l, _)| l)
                .collect();
        assert!(labels.contains(&"Factories".to_string()), "{labels:?}");
        assert!(!labels.contains(&"Trade".to_string()), "{labels:?}");
    }

    /// T-337: the Factories tab shows the four factories; a click on an open one picks its tool.
    #[test]
    fn the_factories_tab_shows_four_factories_and_a_click_picks_an_open_one() {
        let (mut sim, db, at) = world_with_areas(2);
        put(&mut sim, &db, "forest_main", at[0]);
        let view = sim.view();
        let ctx = egui::Context::default();
        let mut tools = ToolState::default();
        let texts = texts_of(&ctx, &view, &mut tools, "Factories", vec![]);
        for name in [
            "Food factory",
            "Furniture factory",
            "Electronics factory",
            "Car factory",
        ] {
            assert!(texts.iter().any(|(t, _)| t == name), "{name} in {texts:?}");
        }
        let texts = texts_of(&ctx, &view, &mut tools, "Factories", vec![]);
        let (_, at) = texts.iter().find(|(t, _)| t == "Food factory").unwrap();
        let [down, up] = click(*at + egui::vec2(4.0, 4.0));
        texts_of(&ctx, &view, &mut tools, "Factories", down);
        texts_of(&ctx, &view, &mut tools, "Factories", up);
        assert_eq!(tools.active, ActiveTool::Place);
        assert_eq!(tools.building.def, db.building_id("food_factory"));
        // A factory the zone is too low for stays shut.
        let mut tools = ToolState::default();
        let texts = texts_of(&ctx, &view, &mut tools, "Factories", vec![]);
        let (_, at) = texts
            .iter()
            .find(|(t, _)| t == "Electronics factory")
            .unwrap();
        let [down, up] = click(*at + egui::vec2(4.0, 4.0));
        texts_of(&ctx, &view, &mut tools, "Factories", down);
        texts_of(&ctx, &view, &mut tools, "Factories", up);
        assert_eq!(tools.building.def, None);
    }

    /// T-337: the count badge of a tile is the number of such buildings in the selected zone,
    /// and a tile the zone's level does not open says which level does.
    #[test]
    fn a_resource_tab_counts_the_built_in_the_selected_zone_and_names_the_level_that_opens_a_tile()
    {
        let (mut sim, db, at) = world_with_areas(1);
        put(&mut sim, &db, "forest_main", at[0]);
        put(&mut sim, &db, "forest_extractor_s", at[0]);
        put(&mut sim, &db, "forest_extractor_s", at[0]);
        let view = sim.view();
        let ctx = egui::Context::default();
        let mut tools = ToolState::default();
        tools.area.selected = Some(AreaId(1));
        let texts = texts_of(&ctx, &view, &mut tools, "Forestry", vec![]);
        let has = |s: &str| texts.iter().filter(|(t, _)| t == s).count();
        assert_eq!(has("×2"), 1, "{texts:?}");
        assert_eq!(has("×1"), 1, "the main building: {texts:?}");
        assert!(
            has("Lvl 2") >= 1,
            "the large extractor wants level 2: {texts:?}"
        );
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

#[cfg(test)]
mod economy_tests {
    use super::*;
    use cp_core::Cost;

    #[test]
    fn the_net_of_an_area_is_exports_and_taxes_less_imports_and_upkeep() {
        let e = AreaEconomy {
            exports: Cost(100_000),
            imports: Cost(40_000),
            upkeep: Cost(30_000),
            taxes: Cost(5_000),
            ..AreaEconomy::default()
        };
        assert_eq!(e.net(), Cost(35_000));
        let loss = AreaEconomy {
            imports: Cost(10),
            ..AreaEconomy::default()
        };
        assert!(loss.net().0 < 0);
    }
}
