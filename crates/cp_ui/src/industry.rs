//! The Industry category (T-207, M5): the area brush, "New area" with the choice of its
//! resource, and the list of the industry areas. The Resources and Industry areas overlays show
//! automatically while a tool of the category is on. The building sections come with T-211.

use bevy_egui::egui;
use cp_content::{BuildingDefId, CargoKind, ContentDb, IndustryRole, ResourceKind};
use cp_district::{AreaId, AreaKind, DistrictCommand, MAX_AREAS, NAME_MAX};
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
        None => Some("needs main building".to_string()),
        Some(level) if level < spec.min_level => Some(format!("area level {}", spec.min_level)),
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

/// Inspector rows of a chain building: what it holds per cargo and its problem.
pub fn industry_rows(view: &SimView, id: cp_building::BuildingId) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for &(cargo, v) in view.industry.stock.get(&id).map_or(&[][..], Vec::as_slice) {
        out.push((format!("Stock: {cargo:?}"), format!("{v:.0}")));
    }
    if let Some(t) = problem_text(view.industry.problem_of(id)) {
        out.push(("Problem".to_string(), t.to_string()));
    }
    out
}

fn area_panel(ui: &mut egui::Ui, view: &SimView, id: AreaId) {
    let Some(s) = area_summary(view, id) else {
        return;
    };
    section(ui, &s.name, |ui| {
        ui.label(format!("{} · level {}", resource_name(s.resource), s.level));
        let (frac, text) = match s.next_xp {
            Some(next) => (
                (s.xp as f32 / next.max(1) as f32).min(1.0),
                format!("{} / {} xp", s.xp, next),
            ),
            None => (1.0, format!("{} xp (top level)", s.xp)),
        };
        ui.add(egui::ProgressBar::new(frac).text(text));
        ui.label(format!("{} buildings · {} jobs", s.buildings, s.jobs));
        if s.stock.is_empty() {
            ui.weak("Nothing stored yet.");
        }
        for (cargo, v) in &s.stock {
            ui.label(format!("{cargo:?}: {v:.0}"));
        }
    });
}

/// The chain sections of the panel: a header per resource and a row of tiles; closed ones are
/// grey with a padlock and say why.
fn chain_panel(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
) {
    for (res, cards) in chain_sections(db) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            resource_icon(res).show(ui, Sizes::SECTION_ICON);
            ui.strong(resource_name(res));
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            for card in &cards {
                let lock = industry_lock(view, db, card.id);
                let on = tools.active == ActiveTool::Place && tools.building.def == Some(card.id);
                let hover = match &lock {
                    Some(l) => format!("Locked: {l}\n{}", card_text(card)),
                    None => card_text(card),
                };
                let r = tool_tile_enabled(
                    ui,
                    TileIcon::Def(&card.icon),
                    &card.name,
                    on,
                    lock.is_none(),
                )
                .on_hover_text(hover);
                if lock.is_some() {
                    badge(ui, r.rect, UniqueState::Locked);
                }
                if r.clicked() {
                    select(tools, card.id);
                    show_overlays(overlay);
                }
            }
        });
    }
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
    if let Some(id) = tools.area.selected {
        area_panel(ui, view, id);
    }
    if let Some(db) = view.content.as_deref() {
        chain_panel(ui, view, db, tools, overlay);
    }
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
        assert_eq!(
            tiles.len(),
            4 + 4 + 1 + 24,
            "tools, resources, New area, then the chain tiles"
        );
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

    /// A world with a Forestry area and a Farming area, painted over two spots.
    fn world_with_areas(
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

    fn put(
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
        let rows = industry_rows(&view, a);
        assert!(
            rows.contains(&("Stock: Logs".to_string(), "30".to_string())),
            "{rows:?}"
        );
        assert!(
            rows.iter()
                .any(|(l, v)| l == "Problem" && v == "storage full"),
            "{rows:?}"
        );
        assert!(industry_rows(&view, b).iter().all(|(l, _)| l != "Problem"));
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
