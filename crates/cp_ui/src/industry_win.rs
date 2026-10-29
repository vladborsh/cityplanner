//! The Industry window (T-337, `architecture/industry-ui.md`): the areas with their chain, buildings
//! and money, the chains of the whole city and the market, under a KPI header. UI only: it reads
//! the [`SimView`]; renaming and deleting an area and a new area are commands.

use bevy_egui::egui;
use cp_content::{BuildingDefId, CargoKind, ContentDb, IndustryRole, ResourceKind};
use cp_core::time::{TICKS_PER_DAY, TICKS_PER_WEEK};
use cp_district::{AreaId, AreaKind, DistrictCommand, DistrictMap, NAME_MAX};
use cp_locale::{tr, trf};
use cp_render::OverlayState;
use cp_sim::{Selection, SimCommand, SimView};
use cp_tools::{ActiveTool, CommandOutbox, Picked, ToolState, format_money};

use crate::icons::Icon;
use crate::industry::{
    AreaEconomy, area_economy, area_summary, new_name, next_id, resource_icon, resource_name,
    show_overlays, swatch,
};
use crate::industry_data::{
    CargoStatus, ChainBuilding, area_buildings, area_flows, area_problem, area_tip, area_unlocks,
    cargo_status, has_main, level_eta_days,
};
use crate::panels::UiState;
use crate::power::{bar, chip, tile};

const WIDTH: f32 = 760.0;
const LIST_W: f32 = 236.0;
const GOOD: egui::Color32 = egui::Color32::from_rgb(120, 200, 120);
const COST: egui::Color32 = egui::Color32::from_rgb(235, 150, 70);
const BLUE: egui::Color32 = egui::Color32::from_rgb(90, 160, 230);

/// The tabs of the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IndustryTab {
    #[default]
    Areas,
    Chains,
    Market,
}

/// The sub-tabs of the selected area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AreaSub {
    #[default]
    Chain,
    Buildings,
    Money,
}

impl AreaSub {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "chain" => Self::Chain,
            "buildings" => Self::Buildings,
            "money" => Self::Money,
            _ => return None,
        })
    }
}

/// Which week the Money sub-tab shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MoneyWeek {
    #[default]
    Last,
    Now,
}

impl UiState {
    /// Opens the Industry window on `tab`.
    pub fn show_industry(&mut self, tab: IndustryTab) {
        self.industry_open = true;
        self.industry_tab = tab;
    }
}

// ----- small pieces ---------------------------------------------------------------------------

pub(crate) fn cargo_color(db: &ContentDb, c: CargoKind) -> egui::Color32 {
    db.cargo(c).map_or(egui::Color32::GRAY, |d| {
        egui::Color32::from_rgb(d.color[0], d.color[1], d.color[2])
    })
}

fn cargo_name(c: CargoKind) -> String {
    tr(&format!("{c:?}"))
}

/// The picture of a building def.
pub(crate) fn def_image(ui: &mut egui::Ui, db: &ContentDb, def: BuildingDefId, size: f32) {
    ui.add(
        egui::Image::new(crate::icons::def_icon(&db.building(def).icon))
            .fit_to_exact_size(egui::vec2(size, size)),
    );
}

/// A pill with a building's picture and name.
fn def_chip(ui: &mut egui::Ui, db: &ContentDb, def: BuildingDefId, tint: Option<bool>) {
    let fill = match tint {
        Some(true) => egui::Color32::from_rgb(34, 62, 40),
        _ => egui::Color32::from_gray(44),
    };
    egui::Frame::new()
        .fill(fill)
        .corner_radius(9.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                def_image(ui, db, def, 16.0);
                ui.label(
                    egui::RichText::new(tr(&db.building(def).name))
                        .size(12.0)
                        .color(egui::Color32::from_gray(210)),
                );
            });
        })
        .response
        .on_hover_text(tr(&db.building(def).name));
}

fn money_color(net: cp_core::Cost) -> egui::Color32 {
    if net.0 >= 0 { GOOD } else { COST }
}

fn arrow_money(net: cp_core::Cost) -> String {
    format!(
        "{} {}",
        if net.0 >= 0 { "▲" } else { "▼" },
        format_money(net)
    )
}

/// The central cell of an area, as a world position.
fn area_center(view: &SimView, id: AreaId) -> Option<bevy::math::Vec2> {
    let cells: Vec<_> = view.area_map.cells_of(id).collect();
    if cells.is_empty() {
        return None;
    }
    let n = cells.len() as f32;
    let sum = cells
        .iter()
        .map(|&c| DistrictMap::cell_center(c))
        .fold(bevy::math::Vec2::ZERO, |a, b| a + b);
    Some(sum / n)
}

fn industry_areas(view: &SimView) -> Vec<(AreaId, String, ResourceKind, u8)> {
    view.areas
        .iter()
        .filter_map(|(id, a)| match a.kind {
            AreaKind::Industry(k) => Some((id, a.name.clone(), k, a.level)),
            _ => None,
        })
        .collect()
}

/// The Main building of a resource, for "Place a main building".
fn main_def(db: &ContentDb, res: ResourceKind) -> Option<BuildingDefId> {
    db.buildings().find_map(|(id, d)| {
        d.industry
            .as_ref()
            .is_some_and(|s| s.role == IndustryRole::Main && s.resource == Some(res))
            .then_some(id)
    })
}

// ----- the window -------------------------------------------------------------------------------

/// The Industry window.
#[allow(clippy::too_many_arguments)]
pub fn industry_window(
    ctx: &egui::Context,
    st: &mut UiState,
    view: &SimView,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    picked: &mut Picked,
    outbox: &mut CommandOutbox,
) {
    let UiState {
        industry_open,
        industry_tab,
        industry_sub,
        industry_money,
        industry_confirm,
        industry_focus,
        camera_goal,
        ..
    } = st;
    crate::frame::window("industry", Icon::Industry, &tr("Industry"), industry_open)
        .default_pos(egui::pos2(16.0, 52.0))
        .show(ctx, |ui| {
            let Some(db) = view.content.as_deref() else {
                return;
            };
            ui.set_width(WIDTH);
            header(ui, view, db);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for (t, icon, label) in [
                    (IndustryTab::Areas, Icon::Industry, tr("Areas")),
                    (IndustryTab::Chains, Icon::IndustryChains, tr("Chains")),
                    (IndustryTab::Market, Icon::IndustryMarket, tr("Market")),
                ] {
                    let on = *industry_tab == t;
                    if ui
                        .add(egui::Button::selectable(on, (icon.sized(18.0), label)))
                        .clicked()
                    {
                        *industry_tab = t;
                    }
                }
            });
            ui.separator();
            match *industry_tab {
                IndustryTab::Areas => areas_tab(
                    ui,
                    view,
                    db,
                    tools,
                    overlay,
                    picked,
                    outbox,
                    AreasState {
                        sub: industry_sub,
                        money: industry_money,
                        confirm: industry_confirm,
                        camera_goal,
                    },
                ),
                IndustryTab::Chains => crate::industry_city::chains_tab(
                    ui,
                    view,
                    db,
                    tools,
                    overlay,
                    outbox,
                    industry_tab,
                    industry_focus,
                ),
                IndustryTab::Market => {
                    crate::industry_city::market_tab(ui, view, db, industry_focus)
                }
            }
        });
}

struct AreasState<'a> {
    sub: &'a mut AreaSub,
    money: &'a mut MoneyWeek,
    confirm: &'a mut Option<AreaId>,
    camera_goal: &'a mut Option<bevy::math::Vec2>,
}

fn header(ui: &mut egui::Ui, view: &SimView, db: &ContentDb) {
    let areas = industry_areas(view);
    let (filled, slots) = view
        .industry_jobs
        .values()
        .fold((0u32, 0u32), |(f, s), &(a, b)| {
            (f + u32::from(a), s + u32::from(b))
        });
    let net = areas
        .iter()
        .map(|(id, ..)| area_economy(view, *id).net())
        .fold(cp_core::Cost(0), |a, b| a + b);
    let f = view.freight;
    let _ = db;
    ui.columns(4, |cols| {
        tile(&mut cols[0], Icon::Industry, &tr("Areas"), |ui| {
            ui.label(
                egui::RichText::new(areas.len().to_string())
                    .size(18.0)
                    .strong(),
            );
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (_, _, k, _) in areas.iter().take(8) {
                    resource_icon(*k).show(ui, 14.0);
                }
            });
        })
        .on_hover_text(tr("Industrial areas of the city and their resources."));
        tile(&mut cols[1], Icon::Jobs, &tr("Workers"), |ui| {
            ui.label(
                egui::RichText::new(format!("{filled} / {slots}"))
                    .size(18.0)
                    .strong(),
            );
            let frac = if slots == 0 {
                0.0
            } else {
                filled as f32 / slots as f32
            };
            bar(ui, frac, None, BLUE);
        })
        .on_hover_text(tr(
            "Workers in the chain buildings: filled jobs of all job slots.",
        ));
        tile(&mut cols[2], Icon::Budget, &tr("Profit a week"), |ui| {
            ui.label(
                egui::RichText::new(arrow_money(net))
                    .size(18.0)
                    .strong()
                    .color(money_color(net)),
            );
        })
        .on_hover_text(tr(
            "Net profit of all industrial areas in the last settled week.",
        ));
        tile(&mut cols[3], Icon::Transit, &tr("Trucks"), |ui| {
            ui.label(
                egui::RichText::new(f.trucks.to_string())
                    .size(18.0)
                    .strong(),
            );
            ui.weak(format!(
                "▲ {}  ▼ {}",
                f.trucks_exporting, f.trucks_importing
            ));
        })
        .on_hover_text(tr(
            "Goods trucks under way now; ▲ carry cargo out, ▼ bring it in.",
        ));
    });
}

// ----- the Areas tab ----------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn areas_tab(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    picked: &mut Picked,
    outbox: &mut CommandOutbox,
    state: AreasState,
) {
    let areas = industry_areas(view);
    // The selection falls back to the first area, so the right side is never empty.
    if tools
        .area
        .selected
        .is_none_or(|a| !areas.iter().any(|(id, ..)| *id == a))
        && let Some((id, ..)) = areas.first()
    {
        tools.area.selected = Some(*id);
    }
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(LIST_W, 560.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| area_list_column(ui, view, db, tools, overlay, outbox, &areas),
        );
        ui.separator();
        ui.vertical(|ui| {
            ui.set_width(WIDTH - LIST_W - 24.0);
            match tools.area.selected {
                Some(id) => area_detail(ui, view, db, tools, overlay, picked, outbox, id, state),
                None => {
                    ui.weak(tr(
                        "No areas yet: pick a resource, press New area, then paint with a brush.",
                    ));
                }
            }
        });
    });
}

fn area_list_column(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    outbox: &mut CommandOutbox,
    areas: &[(AreaId, String, ResourceKind, u8)],
) {
    egui::ScrollArea::vertical()
        .id_salt("industry_areas")
        .max_height(480.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (id, name, kind, level) in areas {
                let selected = tools.area.selected == Some(*id);
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(LIST_W - 12.0, 44.0), egui::Sense::click());
                let p = ui.painter();
                let fill = if selected {
                    ui.visuals().selection.bg_fill
                } else if resp.hovered() {
                    egui::Color32::from_gray(50)
                } else {
                    egui::Color32::from_gray(36)
                };
                p.rect_filled(rect, 5.0, fill);
                let sq = egui::Rect::from_min_size(
                    rect.min + egui::vec2(5.0, 5.0),
                    egui::vec2(34.0, 34.0),
                );
                p.rect_filled(sq, 4.0, swatch(*kind));
                egui::Image::new(resource_icon(*kind).image()).paint_at(ui, sq.shrink(3.0));
                let net = area_economy(view, *id).net();
                let text = egui::Color32::from_gray(225);
                p.text(
                    rect.min + egui::vec2(46.0, 6.0),
                    egui::Align2::LEFT_TOP,
                    clip(name, 20),
                    egui::FontId::proportional(13.0),
                    text,
                );
                p.text(
                    rect.min + egui::vec2(46.0, 24.0),
                    egui::Align2::LEFT_TOP,
                    trf!("Lvl {level}", level = level),
                    egui::FontId::proportional(12.0),
                    egui::Color32::from_gray(160),
                );
                p.text(
                    rect.min + egui::vec2(100.0, 24.0),
                    egui::Align2::LEFT_TOP,
                    arrow_money(net),
                    egui::FontId::proportional(12.0),
                    money_color(net),
                );
                if let Some(problem) = area_problem(view, db, *id) {
                    let c = egui::pos2(rect.right() - 16.0, rect.center().y);
                    p.circle_filled(c, 8.0, COST);
                    p.text(
                        c,
                        egui::Align2::CENTER_CENTER,
                        "!",
                        egui::FontId::proportional(12.0),
                        egui::Color32::BLACK,
                    );
                    ui.interact(
                        egui::Rect::from_center_size(c, egui::vec2(18.0, 18.0)),
                        egui::Id::new(("area_problem", id.0)),
                        egui::Sense::hover(),
                    )
                    .on_hover_text(problem);
                }
                if resp.on_hover_text(tr("Choose this area")).clicked() {
                    tools.area.selected = Some(*id);
                    if tools.active != ActiveTool::Area {
                        tools.select(ActiveTool::Area);
                    }
                    show_overlays(overlay);
                }
                ui.add_space(2.0);
            }
        });
    ui.separator();
    ui.weak(tr("New area"));
    ui.horizontal(|ui| {
        for kind in ResourceKind::ALL {
            let r = resource_icon(kind)
                .button(ui, 24.0, 4.0, false)
                .on_hover_text(trf!(
                    "New {resource} area",
                    resource = tr(resource_name(kind))
                ));
            if r.clicked()
                && let Some(id) = next_id(view)
            {
                outbox.push(SimCommand::District(DistrictCommand::CreateArea {
                    kind: AreaKind::Industry(kind),
                    name: new_name(kind, id),
                }));
                tools.area.new_resource = Some(kind);
                tools.area.selected = Some(id);
                if tools.active != ActiveTool::Area {
                    tools.select(ActiveTool::Area);
                }
                show_overlays(overlay);
            }
        }
    });
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

#[allow(clippy::too_many_arguments)]
fn area_detail(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    picked: &mut Picked,
    outbox: &mut CommandOutbox,
    id: AreaId,
    state: AreasState,
) {
    let Some(s) = area_summary(view, id) else {
        return;
    };
    let Some(area) = view.areas.get(id) else {
        return;
    };
    // The name, the cells and the three buttons.
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 5.0, swatch(s.resource));
        egui::Image::new(resource_icon(s.resource).image()).paint_at(ui, rect.shrink(4.0));
        ui.vertical(|ui| {
            let key = egui::Id::new(("area_name", id.0));
            let mut buf: String = ui
                .data(|m| m.get_temp(key))
                .unwrap_or_else(|| area.name.clone());
            if !ui.memory(|m| m.has_focus(key)) {
                buf = area.name.clone();
            }
            let r = ui.add(
                egui::TextEdit::singleline(&mut buf)
                    .id(key)
                    .desired_width(220.0)
                    .char_limit(NAME_MAX),
            );
            if r.lost_focus() && buf.trim() != area.name && !buf.trim().is_empty() {
                outbox.push(SimCommand::District(DistrictCommand::RenameArea {
                    area: id,
                    name: buf.trim().to_string(),
                }));
            }
            ui.data_mut(|m| m.insert_temp(key, buf));
            let (filled, slots) = chain_workers(&area_buildings(view, id));
            ui.weak(format!(
                "{} · {} · {}",
                tr(resource_name(s.resource)),
                trf!(plural "{n} cells", n = view.area_map.cell_count(id)),
                trf!("{filled} / {slots} workers", filled = filled, slots = slots)
            ));
        });
    });
    ui.add_space(4.0);
    level_block(
        ui,
        db,
        &s,
        area_flows(view, db, id).produced.iter().sum(),
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if *state.confirm == Some(id) {
                    if ui.button(tr("No")).clicked() {
                        *state.confirm = None;
                    }
                    if ui
                        .button(egui::RichText::new(tr("Yes, delete")).color(COST))
                        .clicked()
                    {
                        outbox.push(SimCommand::District(DistrictCommand::DeleteArea {
                            area: id,
                        }));
                        tools.area.selected = None;
                        *state.confirm = None;
                    }
                    ui.label(tr("Delete the area?"));
                    return;
                }
                if ui
                    .add(egui::Button::image_and_text(
                        egui::Image::new(Icon::Remove.image())
                            .fit_to_exact_size(egui::vec2(14.0, 14.0)),
                        tr("Delete"),
                    ))
                    .on_hover_text(tr("Delete the area (its buildings stay)"))
                    .clicked()
                {
                    *state.confirm = Some(id);
                }
                if ui
                    .add(egui::Button::image_and_text(
                        egui::Image::new(Icon::View.image())
                            .fit_to_exact_size(egui::vec2(14.0, 14.0)),
                        tr("Show"),
                    ))
                    .on_hover_text(tr("Show the area on the map"))
                    .clicked()
                    && let Some(at) = area_center(view, id)
                {
                    *state.camera_goal = Some(at);
                }
                if ui
                    .add(egui::Button::image_and_text(
                        egui::Image::new(Icon::DistrictBrushSmall.image())
                            .fit_to_exact_size(egui::vec2(14.0, 14.0)),
                        tr("Brush"),
                    ))
                    .on_hover_text(tr("Paint cells into this area"))
                    .clicked()
                {
                    if tools.active != ActiveTool::Area {
                        tools.select(ActiveTool::Area);
                    }
                    tools.area.selected = Some(id);
                    show_overlays(overlay);
                }
            });
        },
    );
    ui.add_space(4.0);
    if !has_main(view, db, id) {
        no_main(ui, db, tools, overlay, s.resource);
        return;
    }
    ui.horizontal(|ui| {
        for (t, label) in [
            (AreaSub::Chain, tr("Chain")),
            (AreaSub::Buildings, tr("Buildings")),
            (AreaSub::Money, tr("Money")),
        ] {
            if ui.selectable_label(*state.sub == t, label).clicked() {
                *state.sub = t;
            }
        }
    });
    ui.separator();
    match *state.sub {
        AreaSub::Chain => chain_sub(ui, view, db, id),
        AreaSub::Buildings => buildings_sub(ui, view, db, id, picked, state.camera_goal),
        AreaSub::Money => money_sub(ui, view, id, state.money),
    }
}

fn chain_workers(b: &[ChainBuilding]) -> (u32, u32) {
    b.iter()
        .filter(|c| c.role != IndustryRole::Main)
        .fold((0, 0), |(f, s), c| {
            (f + u32::from(c.filled), s + u32::from(c.slots))
        })
}

/// The five steps of the level, the xp bar and what the next level opens.
fn level_block(
    ui: &mut egui::Ui,
    db: &ContentDb,
    s: &crate::industry::AreaSummary,
    made: f32,
    actions: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        for i in 1..=5u8 {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(26.0, 10.0), egui::Sense::hover());
            let on = i <= s.level;
            ui.painter().rect_filled(
                rect,
                2.0,
                if on {
                    BLUE
                } else {
                    egui::Color32::from_gray(48)
                },
            );
        }
        ui.label(trf!("Lvl {level}", level = s.level));
        actions(ui);
    });
    let (frac, text) = match s.next_xp {
        Some(next) => (
            (s.xp as f32 / next.max(1) as f32).min(1.0),
            trf!("{xp} / {next} xp", xp = s.xp, next = next),
        ),
        None => (1.0, trf!("{xp} xp (top level)", xp = s.xp)),
    };
    ui.add(egui::ProgressBar::new(frac).text(text).desired_height(12.0));
    if s.next_xp.is_none() {
        ui.weak(tr("Top level"));
        return;
    }
    let next_level = s.level + 1;
    let eta = match level_eta_days(s, made) {
        Some(d) => trf!(plural "Level {level} in about {n} days", n = d, level = next_level),
        None => trf!(
            "Level {level}: {n} more xp needed",
            level = next_level,
            n = s.next_xp.unwrap_or(0).saturating_sub(s.xp)
        ),
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 3.0);
        ui.weak(format!("{eta} — {}", tr("opens")));
        for def in area_unlocks(db, s.resource, next_level) {
            def_chip(ui, db, def, Some(true));
        }
        chip(ui, Icon::Industry, &tr("storage +10%"), Some(true));
    });
}

fn no_main(
    ui: &mut egui::Ui,
    db: &ContentDb,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    res: ResourceKind,
) {
    ui.add_space(20.0);
    ui.vertical_centered(|ui| {
        if let Some(def) = main_def(db, res) {
            def_image(ui, db, def, 48.0);
        }
        ui.strong(tr("The area has no main building"));
        ui.weak(tr(
            "Nothing works without one: it opens the other chain buildings.",
        ));
        if let Some(def) = main_def(db, res)
            && ui.button(tr("Place a main building")).clicked()
        {
            crate::buildings::select(tools, def);
            show_overlays(overlay);
        }
    });
}

// ----- the Chain sub-tab ---------------------------------------------------------------------------

fn daily(
    db: &ContentDb,
    def: BuildingDefId,
    buildings: &[ChainBuilding],
    flows: &crate::industry_data::AreaFlows,
) -> (usize, f32, f32) {
    let spec = db.building(def).industry.as_ref().expect("a chain def");
    let mine: Vec<&ChainBuilding> = buildings.iter().filter(|b| b.def == def).collect();
    let load = flows
        .load
        .iter()
        .find(|(d, _)| *d == def)
        .map_or(1.0, |&(_, l)| l);
    let rate: f32 = spec.outputs.iter().map(|&(_, r)| r).sum();
    (mine.len(), rate * mine.len() as f32 * load, load)
}

fn chain_sub(ui: &mut egui::Ui, view: &SimView, db: &ContentDb, id: AreaId) {
    let Some(area) = view.areas.get(id) else {
        return;
    };
    let AreaKind::Industry(res) = area.kind else {
        return;
    };
    let buildings = area_buildings(view, id);
    let flows = area_flows(view, db, id);
    let econ = area_economy(view, id);
    let open_defs = |roles: &[IndustryRole]| -> Vec<BuildingDefId> {
        let mut v: Vec<(u8, String, BuildingDefId)> = db
            .buildings()
            .filter_map(|(def, d)| {
                let s = d.industry.as_ref()?;
                let here = buildings.iter().any(|b| b.def == def);
                let mine = (s.resource == Some(res) && s.min_level <= area.level) || here;
                (roles.contains(&s.role) && mine).then(|| (s.role as u8, d.id.clone(), def))
            })
            .collect();
        v.sort();
        v.into_iter().map(|(_, _, d)| d).collect()
    };
    let col0 = open_defs(&[IndustryRole::Extractor]);
    let col1 = open_defs(&[IndustryRole::Processor, IndustryRole::Factory]);
    const CARD_W: f32 = 150.0;
    const CARD_H: f32 = 56.0;
    const GAP: f32 = 8.0;
    let rows = col0.len().max(col1.len()).max(3);
    let width = WIDTH - LIST_W - 24.0;
    let col_x = [0.0, (width - CARD_W) / 2.0, width - CARD_W];
    let height = rows as f32 * (CARD_H + GAP) + 18.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let at = |c: usize, r: usize| {
        egui::Rect::from_min_size(
            rect.min + egui::vec2(col_x[c], 18.0 + r as f32 * (CARD_H + GAP)),
            egui::vec2(CARD_W, CARD_H),
        )
    };
    let p = ui.painter().clone();
    for (c, title) in [tr("Extraction"), tr("Processing"), tr("Where it goes")]
        .into_iter()
        .enumerate()
    {
        p.text(
            rect.min + egui::vec2(col_x[c], 0.0),
            egui::Align2::LEFT_TOP,
            title,
            egui::FontId::proportional(12.0),
            egui::Color32::from_gray(150),
        );
    }
    // The ribbons first, under the cards.
    let shares = |c: CargoKind| -> Vec<(usize, f32)> {
        let all: Vec<(usize, f32)> = col0
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                db.building(**d)
                    .industry
                    .as_ref()
                    .is_some_and(|s| s.outputs.iter().any(|&(o, _)| o == c))
            })
            .map(|(i, d)| (i, daily(db, *d, &buildings, &flows).1))
            .collect();
        let total: f32 = all.iter().map(|&(_, v)| v).sum();
        all.into_iter()
            .map(|(i, v)| (i, if total > 0.0 { v / total } else { 0.0 }))
            .collect()
    };
    let mut ribbons: Vec<(egui::Pos2, egui::Pos2, f32, egui::Color32)> = Vec::new();
    for (pi, pdef) in col1.iter().enumerate() {
        let Some(spec) = db.building(*pdef).industry.as_ref() else {
            continue;
        };
        let runs = spec.outputs.first().map_or(0.0, |&(c, r)| {
            flows.produced[c as usize] / r.max(f32::EPSILON)
        });
        for &(c_in, rate) in &spec.inputs {
            let vol = runs * rate;
            for (ei, share) in shares(c_in) {
                ribbons.push((
                    at(0, ei).right_center(),
                    at(1, pi).left_center(),
                    vol * share,
                    cargo_color(db, c_in),
                ));
            }
        }
    }
    let export_rect = at(2, 0);
    for (ei, d) in col0.iter().enumerate() {
        let Some(spec) = db.building(*d).industry.as_ref() else {
            continue;
        };
        for &(c, _) in &spec.outputs {
            let produced = flows.produced[c as usize];
            if produced > 0.0 {
                let share = shares(c)
                    .into_iter()
                    .find(|&(i, _)| i == ei)
                    .map_or(0.0, |(_, s)| s);
                ribbons.push((
                    at(0, ei).right_center(),
                    export_rect.left_center(),
                    flows.surplus[c as usize] * share,
                    cargo_color(db, c),
                ));
            }
        }
    }
    for (pi, d) in col1.iter().enumerate() {
        let Some(spec) = db.building(*d).industry.as_ref() else {
            continue;
        };
        for &(c, _) in &spec.outputs {
            ribbons.push((
                at(1, pi).right_center(),
                export_rect.left_center(),
                flows.surplus[c as usize],
                cargo_color(db, c),
            ));
        }
    }
    let max = ribbons.iter().map(|r| r.2).fold(1.0f32, f32::max);
    for (a, b, vol, colour) in ribbons.into_iter().filter(|r| r.2 > 0.5) {
        let w = (vol / max * 14.0).clamp(2.0, 14.0);
        let dx = (b.x - a.x) / 2.0;
        p.add(egui::epaint::CubicBezierShape::from_points_stroke(
            [a, a + egui::vec2(dx, 0.0), b - egui::vec2(dx, 0.0), b],
            false,
            egui::Color32::TRANSPARENT,
            egui::Stroke::new(w, colour.gamma_multiply(0.65)),
        ));
    }
    // The cards of extraction and processing.
    for (c, defs) in [(0usize, &col0), (1, &col1)] {
        for (r, def) in defs.iter().enumerate() {
            let card = at(c, r);
            building_card(ui, db, *def, &buildings, &flows, card);
        }
    }
    // Where it goes: export, storage, factories.
    let ex = at(2, 0);
    let exported: Vec<String> = CargoKind::ALL
        .iter()
        .filter(|&&c| flows.surplus[c as usize] >= 1.0)
        .map(|&c| format!("{} {:.0}", cargo_name(c), flows.surplus[c as usize]))
        .collect();
    node(
        ui,
        ex,
        &tr("Export"),
        &format!("≈ {}", format_money(econ.exports)),
        &if exported.is_empty() {
            tr("Nothing leaves.")
        } else {
            format!("{}\n{}", tr("Surplus last week (≈):"), exported.join("\n"))
        },
    );
    let stores: Vec<&ChainBuilding> = buildings
        .iter()
        .filter(|b| b.role == IndustryRole::Warehouse)
        .collect();
    let (have, cap): (f32, f32) = stores
        .iter()
        .fold((0.0, 0.0), |(h, c), b| (h + b.stock_total(), c + b.cap));
    let st = at(2, 1);
    let mode = stores
        .first()
        .map(|b| match view.warehouses.get(b.id).mode {
            cp_industry::WarehouseMode::Balanced => tr("Balanced"),
            cp_industry::WarehouseMode::Fill => tr("Fill"),
            cp_industry::WarehouseMode::Empty => tr("Empty"),
        });
    node(
        ui,
        st,
        &tr("Storage"),
        &if cap > 0.0 {
            format!("{:.0} / {:.0}", have, cap)
        } else {
            "—".to_string()
        },
        &mode.unwrap_or_else(|| tr("No warehouse in the area")),
    );
    if cap > 0.0 {
        let strip = egui::Rect::from_min_size(
            egui::pos2(st.min.x + 6.0, st.max.y - 9.0),
            egui::vec2(CARD_W - 12.0, 4.0),
        );
        p.rect_filled(strip, 2.0, egui::Color32::from_gray(50));
        p.rect_filled(
            egui::Rect::from_min_size(
                strip.min,
                egui::vec2(strip.width() * (have / cap).min(1.0), 4.0),
            ),
            2.0,
            BLUE,
        );
    }
    let fa = at(2, 2);
    let chain = cp_industry::chain_cargo(db, Some(res));
    let city_factories: Vec<(BuildingDefId, usize)> = db
        .buildings()
        .filter_map(|(def, d)| {
            let s = d.industry.as_ref()?;
            (s.role == IndustryRole::Factory && s.inputs.iter().any(|&(c, _)| chain.contains(&c)))
                .then(|| {
                    let n = view.buildings.iter().filter(|(_, b)| b.def == def).count();
                    (def, n)
                })
        })
        .collect();
    let taking: usize = city_factories.iter().map(|&(_, n)| n).sum();
    let hint = city_factories
        .iter()
        .find(|&&(_, n)| n == 0)
        .map(|&(d, _)| tr(&db.building(d).name));
    node(
        ui,
        fa,
        &tr("City factories"),
        &taking.to_string(),
        &match (taking, hint) {
            (0, Some(name)) => trf!("{name} would take this chain's cargo", name = name),
            _ => tr("Factories that take this chain's cargo."),
        },
    );
    ui.add_space(6.0);
    // Stocks of the area's buildings, one bar in the cargo colours.
    let mut by_cargo = [0.0f32; 13];
    for b in &buildings {
        for &(c, v) in &b.stock {
            by_cargo[c as usize] += v;
        }
    }
    let cap_all: f32 = buildings.iter().map(|b| b.cap).sum();
    let used: f32 = by_cargo.iter().sum();
    ui.weak(tr("Stocks in the area's buildings"));
    let (sr, sresp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 12.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(sr, 3.0, egui::Color32::from_gray(40));
    let mut x = sr.min.x;
    for &c in &CargoKind::ALL {
        let w = if cap_all > 0.0 {
            sr.width() * by_cargo[c as usize] / cap_all
        } else {
            0.0
        };
        if w >= 0.5 {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, sr.min.y), egui::vec2(w, sr.height())),
                0.0,
                cargo_color(db, c),
            );
            x += w;
        }
    }
    sresp.on_hover_text(format!("{used:.0} / {cap_all:.0}"));
    ui.weak(format!("{used:.0} / {cap_all:.0}"));
    if let Some(tip) = area_tip(view, db, id) {
        ui.add_space(4.0);
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(28, 44, 66))
            .corner_radius(5.0)
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    chip(ui, Icon::Info, &tr("Tip"), None);
                    ui.label(tip);
                });
            });
    }
}

/// A plain node of the "where it goes" column: a title, a value and a tooltip.
fn node(ui: &mut egui::Ui, rect: egui::Rect, title: &str, value: &str, tip: &str) {
    let p = ui.painter();
    p.rect_filled(rect, 5.0, egui::Color32::from_gray(36));
    p.text(
        rect.min + egui::vec2(8.0, 6.0),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::proportional(12.0),
        egui::Color32::from_gray(150),
    );
    p.text(
        rect.min + egui::vec2(8.0, 22.0),
        egui::Align2::LEFT_TOP,
        value,
        egui::FontId::proportional(15.0),
        egui::Color32::from_gray(230),
    );
    ui.interact(rect, egui::Id::new(("node", title)), egui::Sense::hover())
        .on_hover_text(tip);
}

fn building_card(
    ui: &mut egui::Ui,
    db: &ContentDb,
    def: BuildingDefId,
    buildings: &[ChainBuilding],
    flows: &crate::industry_data::AreaFlows,
    rect: egui::Rect,
) {
    let spec = db.building(def).industry.as_ref().expect("a chain def");
    let (count, per_day, load) = daily(db, def, buildings, flows);
    let mine: Vec<&ChainBuilding> = buildings.iter().filter(|b| b.def == def).collect();
    let stuck = mine
        .iter()
        .filter(|b| b.problem != cp_industry::IndustryProblem::None)
        .count();
    let p = ui.painter().clone();
    p.rect_filled(
        rect,
        5.0,
        if count == 0 {
            egui::Color32::from_gray(30)
        } else {
            egui::Color32::from_gray(40)
        },
    );
    let img = egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 4.0), egui::vec2(30.0, 30.0));
    egui::Image::new(crate::icons::def_icon(&db.building(def).icon)).paint_at(ui, img);
    p.text(
        rect.min + egui::vec2(38.0, 4.0),
        egui::Align2::LEFT_TOP,
        format!("{} ×{count}", clip(&tr(&db.building(def).name), 13)),
        egui::FontId::proportional(12.0),
        egui::Color32::from_gray(225),
    );
    let flow = match spec.role {
        IndustryRole::Extractor => format!("+{per_day:.0}/{}", tr("day")),
        _ => {
            let ins: Vec<String> = spec
                .inputs
                .iter()
                .map(|&(_, r)| format!("{r:.0}"))
                .collect();
            let outs: Vec<String> = spec
                .outputs
                .iter()
                .map(|&(_, r)| format!("{:.0}", r * count as f32 * load))
                .collect();
            format!("{} ▸ {}/{}", ins.join("+"), outs.join("+"), tr("day"))
        }
    };
    p.text(
        rect.min + egui::vec2(38.0, 20.0),
        egui::Align2::LEFT_TOP,
        flow,
        egui::FontId::proportional(11.0),
        egui::Color32::from_gray(170),
    );
    // Cargo squares under the icon: what goes in and out.
    let mut x = rect.min.x + 6.0;
    for &(c, _) in spec.inputs.iter().chain(&spec.outputs) {
        p.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, rect.min.y + 38.0), egui::vec2(8.0, 8.0)),
            2.0,
            cargo_color(db, c),
        );
        x += 11.0;
    }
    let (state, colour) = if count == 0 {
        (tr("not built"), egui::Color32::from_gray(130))
    } else if stuck > 0 {
        (
            trf!("▲ {n} stuck of {total}", n = stuck, total = count),
            COST,
        )
    } else {
        (
            trf!("✓ working · {pct}%", pct = (load * 100.0).round() as u32),
            GOOD,
        )
    };
    p.text(
        rect.min + egui::vec2(38.0, 38.0),
        egui::Align2::LEFT_TOP,
        state,
        egui::FontId::proportional(11.0),
        colour,
    );
    let tip = mine
        .iter()
        .filter_map(|b| {
            crate::industry::problem_text(b.problem)
                .map(|t| format!("{} #{}: {}", tr(&db.building(b.def).name), b.number, tr(t)))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let r = ui.interact(
        rect,
        egui::Id::new(("chain_card", def.0)),
        egui::Sense::hover(),
    );
    if !tip.is_empty() {
        r.on_hover_text(tip);
    }
}

// ----- the Buildings sub-tab -------------------------------------------------------------------

fn buildings_sub(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    id: AreaId,
    picked: &mut Picked,
    camera_goal: &mut Option<bevy::math::Vec2>,
) {
    let buildings = area_buildings(view, id);
    egui::ScrollArea::vertical()
        .id_salt("industry_buildings")
        .max_height(380.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for b in &buildings {
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 34.0),
                    egui::Sense::click(),
                );
                let p = ui.painter().clone();
                p.rect_filled(
                    rect,
                    4.0,
                    if resp.hovered() {
                        egui::Color32::from_gray(50)
                    } else {
                        egui::Color32::from_gray(36)
                    },
                );
                let img = egui::Rect::from_min_size(
                    rect.min + egui::vec2(4.0, 3.0),
                    egui::vec2(28.0, 28.0),
                );
                egui::Image::new(crate::icons::def_icon(&db.building(b.def).icon))
                    .paint_at(ui, img);
                p.text(
                    rect.min + egui::vec2(38.0, 9.0),
                    egui::Align2::LEFT_TOP,
                    format!("{} #{}", clip(&tr(&db.building(b.def).name), 18), b.number),
                    egui::FontId::proportional(12.0),
                    egui::Color32::from_gray(225),
                );
                if b.role != IndustryRole::Main {
                    p.text(
                        rect.min + egui::vec2(200.0, 9.0),
                        egui::Align2::LEFT_TOP,
                        format!("{} / {}", b.filled, b.slots),
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_gray(190),
                    );
                    meter(&p, rect.min + egui::vec2(245.0, 15.0), 50.0, b.load(), BLUE);
                    if b.cap > 0.0 {
                        let total = b.stock_total();
                        p.text(
                            rect.min + egui::vec2(305.0, 9.0),
                            egui::Align2::LEFT_TOP,
                            format!("{total:.0} / {:.0}", b.cap),
                            egui::FontId::proportional(12.0),
                            egui::Color32::from_gray(190),
                        );
                        let colour = b
                            .stock
                            .first()
                            .map_or(egui::Color32::GRAY, |&(c, _)| cargo_color(db, c));
                        meter(
                            &p,
                            rect.min + egui::vec2(385.0, 15.0),
                            50.0,
                            (total / b.cap).min(1.0),
                            colour,
                        );
                    }
                }
                let (word, colour) = match crate::industry::problem_text(b.problem) {
                    Some(t) => (format!("▲ {}", tr(t)), COST),
                    None => ("✓".to_string(), GOOD),
                };
                p.text(
                    egui::pos2(rect.right() - 6.0, rect.min.y + 9.0),
                    egui::Align2::RIGHT_TOP,
                    clip(&word, 18),
                    egui::FontId::proportional(11.0),
                    colour,
                );
                if resp.on_hover_text(tr("Show the building")).clicked() {
                    if let Some(bd) = view.buildings.get(b.id) {
                        *camera_goal = Some(bd.pos);
                    }
                    picked.0 = Some(Selection::Building(b.id));
                }
            }
        });
}

fn meter(p: &egui::Painter, at: egui::Pos2, w: f32, frac: f32, colour: egui::Color32) {
    let r = egui::Rect::from_min_size(at, egui::vec2(w, 5.0));
    p.rect_filled(r, 2.5, egui::Color32::from_gray(56));
    p.rect_filled(
        egui::Rect::from_min_size(at, egui::vec2(w * frac.clamp(0.0, 1.0), 5.0)),
        2.5,
        colour,
    );
}

// ----- the Money sub-tab -----------------------------------------------------------------------

fn money_sub(ui: &mut egui::Ui, view: &SimView, id: AreaId, week: &mut MoneyWeek) {
    ui.horizontal(|ui| {
        let day = (view.tick.0 % TICKS_PER_WEEK) / TICKS_PER_DAY + 1;
        if ui
            .selectable_label(*week == MoneyWeek::Last, tr("Last week"))
            .clicked()
        {
            *week = MoneyWeek::Last;
        }
        if ui
            .selectable_label(
                *week == MoneyWeek::Now,
                trf!("This week (day {d} of 7)", d = day),
            )
            .clicked()
        {
            *week = MoneyWeek::Now;
        }
    });
    let last = area_economy(view, id);
    let e = match week {
        MoneyWeek::Last => last,
        MoneyWeek::Now => {
            let b = view.area_books.now.get(&id.0).cloned().unwrap_or_default();
            AreaEconomy {
                produced: b.produced.into_iter().collect(),
                exports: b.exports,
                imports: b.imports,
                ..AreaEconomy::default()
            }
        }
    };
    let settled = *week == MoneyWeek::Last;
    let rows: [(String, cp_core::Cost, bool, bool); 4] = [
        (tr("Export income"), e.exports, true, true),
        (tr("Workers' taxes"), e.taxes, true, settled),
        (tr("Imports incl. transport"), e.imports, false, true),
        (tr("Building upkeep"), e.upkeep, false, settled),
    ];
    let scale = rows
        .iter()
        .map(|r| r.1.0.abs())
        .max()
        .unwrap_or(0)
        .max(e.net().0.abs())
        .max(1) as f32;
    let width = ui.available_width() - 190.0;
    for (label, value, up, known) in rows {
        ui.horizontal(|ui| {
            ui.allocate_ui(egui::vec2(140.0, 18.0), |ui| ui.label(label));
            let (r, resp) = ui.allocate_exact_size(egui::vec2(width, 14.0), egui::Sense::hover());
            if known {
                let w = (width * value.0.abs() as f32 / scale).max(1.0);
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(r.min, egui::vec2(w, r.height())),
                    3.0,
                    if up { GOOD } else { COST },
                );
                ui.label(format_money(value));
            } else {
                resp.on_hover_text(tr("Accrued at the end of the week"));
                ui.weak("—");
            }
        });
    }
    ui.horizontal(|ui| {
        ui.allocate_ui(egui::vec2(140.0, 18.0), |ui| ui.strong(tr("Net profit")));
        let (r, _) = ui.allocate_exact_size(egui::vec2(width, 14.0), egui::Sense::hover());
        let net = e.net();
        let w = (width * net.0.abs() as f32 / scale).max(1.0);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(r.min, egui::vec2(w, r.height())),
            3.0,
            BLUE,
        );
        ui.colored_label(money_color(net), format_money(net));
    });
    ui.add_space(4.0);
    ui.weak(tr(
        "Exports and workers' taxes raise the profit; imports and upkeep lower it.",
    ));
}

/// The kinds of the cargo status, for the Chains tab (part b).
#[allow(dead_code)]
fn status_of(view: &SimView) -> [CargoStatus; 13] {
    cargo_status(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::industry::tests::{put, world_with_areas};
    use crate::testutil::{click, frame_texts, input};

    pub(super) struct Rig {
        ctx: egui::Context,
        pub(super) st: UiState,
        pub(super) tools: ToolState,
        overlay: OverlayState,
        picked: Picked,
        pub(super) outbox: CommandOutbox,
        pub(super) view: SimView,
    }

    impl Rig {
        pub(super) fn new(view: SimView) -> Self {
            let mut tools = ToolState::default();
            tools.area.selected = Some(AreaId(1));
            let mut rig = Self {
                ctx: egui::Context::default(),
                st: UiState {
                    industry_open: true,
                    ..UiState::default()
                },
                tools,
                overlay: OverlayState::default(),
                picked: Picked::default(),
                outbox: CommandOutbox::default(),
                view,
            };
            rig.frame(vec![]);
            rig
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<(String, egui::Pos2)> {
            let Self {
                ctx,
                st,
                tools,
                overlay,
                picked,
                outbox,
                view,
            } = self;
            frame_texts(ctx, input(0.0, events), |ui| {
                industry_window(ui.ctx(), st, view, tools, overlay, picked, outbox)
            })
        }

        pub(super) fn texts(&mut self) -> Vec<(String, egui::Pos2)> {
            self.frame(vec![]);
            self.frame(vec![])
        }

        pub(super) fn click_text(&mut self, label: &str) {
            let texts = self.texts();
            let at = texts
                .iter()
                .find(|(t, _)| t == label)
                .map(|(_, p)| *p + egui::vec2(4.0, 4.0))
                .unwrap_or_else(|| panic!("no {label:?} in {texts:?}"));
            let [down, up] = click(at);
            self.frame(down);
            self.frame(up);
        }
    }

    #[test]
    fn the_window_opens_on_every_tab_and_sub_tab_without_panic() {
        let (mut sim, db, at) = world_with_areas(2);
        put(&mut sim, &db, "forest_main", at[0]);
        put(&mut sim, &db, "forest_extractor_s", at[0]);
        let mut rig = Rig::new(sim.view());
        for tab in [IndustryTab::Areas, IndustryTab::Chains, IndustryTab::Market] {
            rig.st.industry_tab = tab;
            for sub in [AreaSub::Chain, AreaSub::Buildings, AreaSub::Money] {
                rig.st.industry_sub = sub;
                for week in [MoneyWeek::Last, MoneyWeek::Now] {
                    rig.st.industry_money = week;
                    assert!(!rig.texts().is_empty(), "{tab:?} {sub:?}");
                }
            }
        }
    }

    #[test]
    fn a_click_on_the_second_area_selects_it() {
        let (sim, _, _) = world_with_areas(1);
        let mut rig = Rig::new(sim.view());
        assert_eq!(rig.tools.area.selected, Some(AreaId(1)));
        rig.click_text("Farming");
        assert_eq!(rig.tools.area.selected, Some(AreaId(2)));
    }

    #[test]
    fn delete_sends_the_command_only_after_the_confirmation() {
        let (sim, _, _) = world_with_areas(1);
        let mut rig = Rig::new(sim.view());
        rig.click_text("Delete");
        assert_eq!(rig.outbox.iter().count(), 0, "asked first");
        assert_eq!(rig.st.industry_confirm, Some(AreaId(1)));
        rig.click_text("No");
        assert_eq!(rig.st.industry_confirm, None);
        rig.click_text("Delete");
        rig.click_text("Yes, delete");
        assert_eq!(
            rig.outbox.iter().cloned().collect::<Vec<_>>(),
            [SimCommand::District(DistrictCommand::DeleteArea {
                area: AreaId(1)
            })]
        );
    }

    #[test]
    fn renaming_an_area_sends_the_name_when_the_field_loses_focus() {
        let (sim, _, _) = world_with_areas(1);
        let mut rig = Rig::new(sim.view());
        let texts = rig.texts();
        // The first "Forestry" is the card of the list, the second the name field.
        let field = texts
            .iter()
            .filter(|(t, _)| t == "Forestry")
            .nth(1)
            .map(|(_, p)| *p + egui::vec2(150.0, 4.0))
            .expect("the name field");
        let [down, up] = click(field);
        rig.frame(down);
        rig.frame(up);
        rig.frame(vec![egui::Event::Text("2".into())]);
        rig.click_text("0 / 2000 xp");
        assert_eq!(
            rig.outbox.iter().cloned().collect::<Vec<_>>(),
            [SimCommand::District(DistrictCommand::RenameArea {
                area: AreaId(1),
                name: "Forestry2".into()
            })]
        );
    }

    #[test]
    fn an_area_without_a_main_building_offers_to_place_one() {
        let (sim, db, _) = world_with_areas(1);
        let mut rig = Rig::new(sim.view());
        rig.click_text("Place a main building");
        assert_eq!(rig.tools.active, ActiveTool::Place);
        assert_eq!(rig.tools.building.def, db.building_id("forest_main"));
    }

    #[test]
    fn a_new_area_button_creates_the_area_and_selects_it() {
        let (sim, _, _) = world_with_areas(1);
        let mut rig = Rig::new(sim.view());
        let id = next_id(&rig.view).unwrap();
        rig.st.industry_tab = IndustryTab::Areas;
        let _ = rig.texts();
        // The four resource buttons stand in a row under "New area"; take the first.
        let texts = rig.texts();
        let (_, label) = texts.iter().find(|(t, _)| t == "New area").unwrap();
        let [down, up] = click(*label + egui::vec2(12.0, 28.0));
        rig.frame(down);
        rig.frame(up);
        let sent: Vec<_> = rig.outbox.iter().cloned().collect();
        assert!(
            matches!(
                sent.as_slice(),
                [SimCommand::District(DistrictCommand::CreateArea { .. })]
            ),
            "{sent:?}"
        );
        assert_eq!(rig.tools.area.selected, Some(id));
    }
}

#[cfg(test)]
mod city_tests {
    use super::*;
    use crate::industry::tests::{put, world_with_areas};
    use cp_industry::{AreaBook, AreaBooks, IndustryLedger};

    #[test]
    fn the_chains_tab_marks_what_the_city_makes_and_buys_and_links_to_the_market() {
        let (mut sim, db, at) = world_with_areas(2);
        put(&mut sim, &db, "forest_main", at[0]);
        let mut b = AreaBook::default();
        b.produced.insert(CargoKind::Logs, 450.0);
        sim.world_mut()
            .resource_mut::<AreaBooks>()
            .last
            .insert(1, b);
        sim.world_mut()
            .resource_mut::<IndustryLedger>()
            .last_by_cargo[CargoKind::Logs as usize]
            .import = 80;
        let mut rig = super::tests::Rig::new(sim.view());
        rig.st.industry_tab = IndustryTab::Chains;
        let texts = rig.texts();
        assert!(texts.iter().any(|(t, _)| t == "Logs 450 +80"), "{texts:?}");
        // A product the area's level does not open says so.
        assert!(
            texts.iter().any(|(t, _)| t.starts_with("Planks")),
            "{texts:?}"
        );
        rig.click_text("Logs 450 +80");
        assert_eq!(rig.st.industry_tab, IndustryTab::Market);
        assert_eq!(rig.st.industry_focus, Some(CargoKind::Logs));
    }

    #[test]
    fn a_click_on_a_resource_row_opens_its_area_and_a_factory_card_picks_its_tool() {
        let (mut sim, db, at) = world_with_areas(2);
        put(&mut sim, &db, "forest_main", at[0]);
        let mut rig = super::tests::Rig::new(sim.view());
        rig.st.industry_tab = IndustryTab::Chains;
        rig.click_text("Farming");
        assert_eq!(rig.st.industry_tab, IndustryTab::Areas);
        assert_eq!(rig.tools.area.selected, Some(AreaId(2)));
        rig.st.industry_tab = IndustryTab::Chains;
        rig.click_text("Food factory");
        assert_eq!(rig.tools.building.def, db.building_id("food_factory"));
    }

    #[test]
    fn the_market_tab_shows_the_season_the_cargo_cards_and_the_rest_as_chips() {
        let (mut sim, db, at) = world_with_areas(1);
        put(&mut sim, &db, "forest_processor_a", at[0]);
        let mut rig = super::tests::Rig::new(sim.view());
        rig.st.industry_tab = IndustryTab::Market;
        let texts = rig.texts();
        let has = |s: &str| texts.iter().any(|(t, _)| t == s);
        assert!(
            has("The city's cargo") && has("The city neither makes nor buys"),
            "{texts:?}"
        );
        assert!(has("Logs") && has("Planks"), "{texts:?}");
        assert!(
            texts
                .iter()
                .any(|(t, _)| t.starts_with("Spring") || t.starts_with("Summer")),
            "{texts:?}"
        );
    }
}
