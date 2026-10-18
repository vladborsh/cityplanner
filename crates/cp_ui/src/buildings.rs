//! The Buildings category: the service buildings of the content as cards, grouped by the kind
//! of service they provide. Read-only over the content; a click only selects the building in
//! the existing [`ToolState`].

use bevy_egui::egui;
use cp_content::{BuildingDefId, ContentDb, ServiceKind, ServiceSite, UtilityKind};
use cp_core::Cost;
use cp_render::{Overlay, OverlayState};
use cp_sim::SimView;
use cp_tools::{ActiveTool, ToolState, UtilityMode, format_money};
use cp_utility::UtilityNet;

use crate::icons::Icon;
use crate::style::Sizes;
use crate::tile::{TileIcon, tool_tile, tool_tile_enabled};

/// What a card shows, taken from the building and its service definition.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: BuildingDefId,
    pub name: String,
    pub cost: Cost,
    pub upkeep_week: Cost,
    /// Supply per game day, in the unit of the buildings' `consumption`.
    pub capacity: u32,
    pub lot: (u8, u8),
    /// The icon of the building (`BuildingDef.icon`).
    pub icon: String,
    /// Where the building may stand (`ServiceDef.site`).
    pub site: ServiceSite,
}

/// The cards of one service kind.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub kind: ServiceKind,
    pub cards: Vec<Card>,
}

/// Every building with a service, grouped by [`ServiceKind`] (groups in kind order, cards in
/// definition order). A building whose service id is unknown is left out.
pub fn groups(content: &ContentDb) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for (id, def) in content.buildings() {
        let Some(service) = def.service.as_deref().and_then(|s| content.service_id(s)) else {
            continue;
        };
        let service = content.service(service);
        let card = Card {
            id,
            name: def.name.clone(),
            cost: Cost::from_units(def.cost),
            upkeep_week: Cost::from_units(def.upkeep_week),
            capacity: service.capacity,
            lot: def.lot,
            icon: def.icon.clone(),
            site: service.site,
        };
        match out.iter_mut().find(|g| g.kind == service.kind) {
            Some(g) => g.cards.push(card),
            None => out.push(Group {
                kind: service.kind,
                cards: vec![card],
            }),
        }
    }
    out.sort_by_key(|g| g.kind);
    out
}

fn kind_label(kind: ServiceKind) -> String {
    match kind {
        ServiceKind::Park => "Parks".to_string(),
        _ => format!("{kind:?}"),
    }
}

fn kind_icon(kind: ServiceKind) -> Icon {
    match kind {
        ServiceKind::Water => Icon::Water,
        ServiceKind::Power => Icon::Power,
        ServiceKind::Sewage => Icon::Pipe,
        ServiceKind::Roads => Icon::Roads,
        ServiceKind::Garbage => Icon::OverlayGarbage,
        ServiceKind::Health => Icon::OverlayHealth,
        ServiceKind::Deathcare => Icon::OverlayDeathcare,
        ServiceKind::Fire => Icon::OverlayFire,
        ServiceKind::Police => Icon::OverlayPolice,
        ServiceKind::Education => Icon::OverlayEducation,
        ServiceKind::Transit => Icon::LineBus,
        ServiceKind::Landmark => Icon::Buildings,
        ServiceKind::Industry => Icon::Buildings,
        ServiceKind::Park => Icon::Parks,
        ServiceKind::Cargo => Icon::Buildings,
        ServiceKind::Post => Icon::CategoryPost,
    }
}

/// The text of a card: name, then price and upkeep, then capacity and lot.
pub fn card_text(c: &Card) -> String {
    let capacity = if c.capacity > 0 {
        format!("capacity {} · ", c.capacity)
    } else {
        String::new()
    };
    format!(
        "{}\n{} · {}/wk\n{capacity}lot {}×{}",
        c.name,
        format_money(c.cost),
        format_money(c.upkeep_week),
        c.lot.0,
        c.lot.1
    )
}

/// Why `id` cannot be placed yet: the milestone it waits for (T-149); `None` when it is open.
pub fn lock_text(view: &SimView, content: &ContentDb, id: BuildingDefId) -> Option<String> {
    let m = view.progress.locked_by(content, id)?;
    let ms = &content.milestones()[m];
    Some(format!(
        "Opens at {} residents ({})",
        ms.population, ms.name
    ))
}

/// Selects `id` for placing.
pub fn select(tools: &mut ToolState, id: BuildingDefId) {
    if tools.active != ActiveTool::Place {
        tools.select(ActiveTool::Place);
    }
    tools.building.def = Some(id);
}

/// Which network a section of the panel builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetTab {
    /// Water and sewage (one pipe network).
    Water,
    Power,
}

/// One section of the panel: the buildings of a service kind (water and sewage share one)
/// and, for the networks, their tools.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub kind: ServiceKind,
    pub cards: Vec<Card>,
    /// The network this section builds (`Water` also carries the sewage buildings).
    pub net: Option<NetTab>,
}

impl Section {
    pub fn title(&self) -> String {
        kind_label(self.kind)
    }
}

/// The sections in [`ServiceKind`] order. A section without buildings and without a network
/// is not drawn (a kind with no content has none).
pub fn sections(content: &ContentDb) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    // Transit buildings live in the Transport category.
    for g in groups(content).into_iter().filter(|g| {
        !matches!(
            g.kind,
            ServiceKind::Transit
                | ServiceKind::Landmark
                | ServiceKind::Industry
                | ServiceKind::Cargo
        )
    }) {
        let mut g = g;
        // The buildings of the park areas have their own category (T-226): the Parks section
        // here keeps the four basic city parks.
        if g.kind == ServiceKind::Park {
            g.cards.retain(|c| {
                content
                    .building(c.id)
                    .park
                    .as_ref()
                    .is_none_or(|p| p.park.is_none() && p.role == cp_content::ParkRole::Attraction)
            });
        }
        // Unique buildings get their own sections (T-201).
        // One pipe network: sewage buildings sit in the Water section (§13.7).
        let kind = if g.kind == ServiceKind::Sewage {
            ServiceKind::Water
        } else {
            g.kind
        };
        match out.iter_mut().find(|s| s.kind == kind) {
            Some(s) => s.cards.extend(g.cards),
            None => out.push(Section {
                kind,
                cards: g.cards,
                net: None,
            }),
        }
    }
    for (kind, net, util) in [
        (ServiceKind::Water, NetTab::Water, UtilityKind::Water),
        (ServiceKind::Power, NetTab::Power, UtilityKind::Power),
    ] {
        if content.utility_for_kind(util).is_none() {
            continue;
        }
        match out.iter_mut().find(|s| s.kind == kind) {
            Some(s) => s.net = Some(net),
            None => out.push(Section {
                kind,
                cards: Vec::new(),
                net: Some(net),
            }),
        }
    }
    out.sort_by_key(|s| s.kind);
    out
}

/// Where a unique building or monument stands in the city's progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UniqueState {
    /// Its unlock conditions do not all hold yet.
    Locked,
    /// Open to place.
    Open,
    /// One already stands: there is one per city.
    Built,
}

/// The cards of the unique buildings and of the monuments (`UniqueSpec.monument`), each in
/// definition order; a list without cards is left out. Unique buildings come first.
pub fn unique_sections(content: &ContentDb) -> Vec<(bool, Vec<Card>)> {
    let cards: Vec<Card> = groups(content)
        .into_iter()
        .filter(|g| g.kind == ServiceKind::Landmark)
        .flat_map(|g| g.cards)
        .collect();
    [false, true]
        .into_iter()
        .map(|monument| {
            let of_kind = cards
                .iter()
                .filter(|c| {
                    content
                        .building(c.id)
                        .unique
                        .as_ref()
                        .is_some_and(|u| u.monument == monument)
                })
                .cloned()
                .collect::<Vec<_>>();
            (monument, of_kind)
        })
        .filter(|(_, c)| !c.is_empty())
        .collect()
}

pub fn unique_title(monument: bool) -> &'static str {
    if monument {
        "Monuments"
    } else {
        "Unique buildings"
    }
}

/// `3200` as `3 200`.
pub fn spaced(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

pub fn unique_state(view: &SimView, id: BuildingDefId) -> UniqueState {
    match view.uniques.iter().find(|u| u.def == id) {
        Some(u) if u.built => UniqueState::Built,
        Some(u) if !u.unlocked => UniqueState::Locked,
        _ => UniqueState::Open,
    }
}

/// The tooltip of a unique tile: the state, each condition with its progress
/// ("Population 3 200 / 5 000"), then the card text.
pub fn unique_hover(view: &SimView, card: &Card) -> String {
    let state = unique_state(view, card.id);
    let mut out = String::new();
    match state {
        UniqueState::Built => out.push_str("Built — one per city\n"),
        UniqueState::Locked => {
            out.push_str("Locked\n");
            if let Some(u) = view.uniques.iter().find(|u| u.def == card.id) {
                for (cond, cur, need) in &u.conditions {
                    out.push_str(&format!(
                        "{} {} {} / {}\n",
                        if cur >= need { "✓" } else { "•" },
                        cp_sim::progress::cond_name(cond),
                        spaced(*cur),
                        spaced(*need)
                    ));
                }
            }
        }
        UniqueState::Open => {}
    }
    out.push_str(&card_text(card));
    out
}

/// The inspector rows of a unique building: what it does to the land and the tourists.
pub fn unique_rows(content: &ContentDb, def: &str) -> Vec<(String, String)> {
    let Some(d) = content.building_id(def).map(|id| content.building(id)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(spec) = d.spec.and_then(|k| content.spec(k)) {
        out.push((
            "Specialisation".to_string(),
            format!("{}: {}", spec.name, crate::specs::effect_text(spec)),
        ));
    }
    if let Some(e) = d.land_effect {
        out.push((
            "Land value".to_string(),
            format!("{:+.0} within {:.0} m", e.add, e.radius_m),
        ));
        if e.green > 0.0 {
            // T-237: what the neighbours get besides the price of their land.
            out.push((
                "Wellbeing".to_string(),
                format!(
                    "{:.0} % fewer sick, {:.0} % likelier to move in, within {:.0} m",
                    e.green * cp_building::GREEN_SICK_CUT * 100.0,
                    e.green * cp_building::GREEN_MOVE_IN_BONUS * 100.0,
                    e.radius_m
                ),
            ));
        }
    }
    if let Some(u) = &d.unique {
        out.push((
            "Tourists".to_string(),
            format!("{:.0} a day", u.tourists_per_day),
        ));
    }
    out
}

/// A padlock (locked) or a tick (built) in the corner of a tile.
pub(crate) fn badge(ui: &egui::Ui, tile: egui::Rect, state: UniqueState) {
    let (colour, size) = match state {
        UniqueState::Open => return,
        UniqueState::Locked => (egui::Color32::from_gray(210), 14.0),
        UniqueState::Built => (egui::Color32::from_rgb(90, 200, 110), 16.0),
    };
    let r = egui::Rect::from_min_size(
        tile.right_top() + egui::vec2(-size - 3.0, 3.0),
        egui::vec2(size, size),
    );
    let p = ui.painter();
    p.circle_filled(
        r.center(),
        size * 0.62,
        egui::Color32::from_black_alpha(170),
    );
    match state {
        UniqueState::Built => p.add(egui::Shape::line(
            vec![
                r.lerp_inside(egui::vec2(0.15, 0.55)),
                r.lerp_inside(egui::vec2(0.4, 0.8)),
                r.lerp_inside(egui::vec2(0.85, 0.2)),
            ],
            egui::Stroke::new(2.2, colour),
        )),
        _ => {
            // Body and shackle.
            let body = egui::Rect::from_min_max(
                r.lerp_inside(egui::vec2(0.2, 0.5)),
                r.lerp_inside(egui::vec2(0.8, 0.95)),
            );
            p.rect_filled(body, 1.5, colour);
            p.add(egui::Shape::line(
                vec![
                    r.lerp_inside(egui::vec2(0.33, 0.5)),
                    r.lerp_inside(egui::vec2(0.33, 0.25)),
                    r.lerp_inside(egui::vec2(0.67, 0.25)),
                    r.lerp_inside(egui::vec2(0.67, 0.5)),
                ],
                egui::Stroke::new(1.8, colour),
            ))
        }
    };
}

/// What a network section builds with.
fn net_parts(net: NetTab) -> (UtilityKind, UtilityNet, Overlay) {
    match net {
        NetTab::Water => (UtilityKind::Water, UtilityNet::Pipe, Overlay::Water),
        NetTab::Power => (UtilityKind::Power, UtilityNet::Power, Overlay::Power),
    }
}

/// The plate under a section's tiles for the selected building's site (`ServiceDef.site`).
pub fn site_note(site: ServiceSite) -> String {
    match site {
        ServiceSite::OnWater => "Stands on water: at least one lot cell over water".to_string(),
        ServiceSite::Shore => format!(
            "Stands on the shore: lot on land, water no farther than {:.0} m",
            ServiceSite::SHORE_REACH_M
        ),
        ServiceSite::Land => "Land only".to_string(),
    }
}

/// The panel: a section per service kind with a header, one row of tiles
/// (network tool, the buildings, remove tool) and the site plate of the selected building.
/// Picking a tile of a network section also switches the map overlay to that network.
pub fn buildings_panel(
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
            kind_icon(section.kind).show(ui, Sizes::SECTION_ICON);
            ui.strong(section.title());
        });
        let net = section.net.map(net_parts);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            if let Some((kind, _, shown)) = net
                && let Some((_, def)) = content.utility_for_kind(kind)
            {
                let on = tools.active == ActiveTool::Utility && tools.utility.kind == kind;
                if tool_tile(ui, TileIcon::Def(&def.icon), &def.name, on)
                    .on_hover_text(format!("{}: click to start, click to end", def.name))
                    .clicked()
                {
                    if on {
                        tools.select(ActiveTool::None);
                    } else {
                        tools.select(ActiveTool::Utility);
                        tools.utility.set_kind(kind);
                        overlay.active = shown;
                    }
                }
            }
            for card in &section.cards {
                let on = tools.active == ActiveTool::Place && tools.building.def == Some(card.id);
                let lock = view.and_then(|v| lock_text(v, content, card.id));
                let hover = match &lock {
                    Some(l) => format!("{l}\n{}", card_text(card)),
                    None => card_text(card),
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
                    if let Some((_, _, shown)) = net {
                        overlay.active = shown;
                    } else if let Some(shown) = Overlay::for_service(section.kind) {
                        overlay.active = shown;
                    }
                }
            }
            if let Some((_, remove_net, shown)) = net {
                let on = tools.active == ActiveTool::UtilityRemove
                    && tools.remove_net == Some(remove_net);
                let (icon, name) = match section.net {
                    Some(NetTab::Power) => (Icon::RemoveLine, "Remove line"),
                    _ => (Icon::RemovePipe, "Remove pipe"),
                };
                if tool_tile(ui, TileIcon::Icon(icon), name, on)
                    .on_hover_text("Hover a piece and click to remove it")
                    .clicked()
                {
                    if on {
                        tools.select(ActiveTool::None);
                    } else {
                        tools.select(ActiveTool::UtilityRemove);
                        tools.remove_net = Some(remove_net);
                        overlay.active = shown;
                    }
                }
            }
        });
        // The drawing mode of the network tool, under the row.
        if let Some((kind, _, _)) = net
            && tools.active == ActiveTool::Utility
            && tools.utility.kind == kind
        {
            ui.weak("Mode");
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
                for (mode, icon, label, hover) in [
                    (UtilityMode::Straight, Icon::Road, "Straight", "Two clicks"),
                    (
                        UtilityMode::Polyline,
                        Icon::FreeRoad,
                        "Polyline",
                        "Enter ends",
                    ),
                ] {
                    if tool_tile(ui, TileIcon::Icon(icon), label, tools.utility.mode == mode)
                        .on_hover_text(hover)
                        .clicked()
                    {
                        tools.utility.set_mode(mode);
                    }
                }
            });
        }
        // The site of the selected building of this section.
        if tools.active == ActiveTool::Place
            && let Some(card) = section
                .cards
                .iter()
                .find(|c| Some(c.id) == tools.building.def)
        {
            ui.colored_label(egui::Color32::from_rgb(150, 190, 240), site_note(card.site));
        }
    }
    if let Some(view) = view {
        for (monument, cards) in unique_sections(content) {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                Icon::Buildings.show(ui, Sizes::SECTION_ICON);
                ui.strong(unique_title(monument));
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
                for card in &cards {
                    let state = unique_state(view, card.id);
                    let on =
                        tools.active == ActiveTool::Place && tools.building.def == Some(card.id);
                    let r = tool_tile_enabled(
                        ui,
                        TileIcon::Def(&card.icon),
                        &card.name,
                        on,
                        state == UniqueState::Open,
                    )
                    .on_hover_text(unique_hover(view, card));
                    badge(ui, r.rect, state);
                    if r.clicked() {
                        select(tools, card.id);
                    }
                }
            });
        }
        if tools.active == ActiveTool::Place
            && let Some(def) = tools.building.def
            && content.building(def).unique.is_some()
        {
            ui.colored_label(
                egui::Color32::from_rgb(150, 190, 240),
                site_note(ServiceSite::Land),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{click, frame_texts, input};
    use cp_sim::SimRunner;

    fn view() -> SimView {
        let mut sim = SimRunner::new(1);
        sim.set_content(std::sync::Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        ));
        sim.view()
    }

    /// T-223: the Parks section of the Buildings panel keeps the four basic city parks; the
    /// gates, attractions and props of the park areas are not in it.
    #[test]
    fn the_buildings_parks_section_holds_only_the_four_city_parks() {
        let view = view();
        let content = view.content.as_deref().unwrap();
        let parks = sections(content)
            .into_iter()
            .find(|s| s.kind == ServiceKind::Park)
            .unwrap();
        let mut ids: Vec<&str> = parks
            .cards
            .iter()
            .map(|c| content.building(c.id).id.as_str())
            .collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            ["playground", "plaza_fountain", "small_park", "sports_field"]
        );
    }

    #[test]
    fn one_card_per_service_building_and_groups_follow_the_kind_order() {
        let view = view();
        let content = view.content.as_deref().unwrap();
        let groups = groups(content);
        let cards: usize = groups.iter().map(|g| g.cards.len()).sum();
        let services = content
            .buildings()
            .filter(|(_, d)| d.service.is_some())
            .count();
        assert!(services > 0);
        assert_eq!(cards, services);
        let kinds: Vec<_> = groups.iter().map(|g| g.kind).collect();
        let mut sorted = kinds.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(kinds, sorted, "ordered by ServiceKind, one group per kind");
        assert!(kinds.contains(&ServiceKind::Water) && kinds.contains(&ServiceKind::Power));
        // The numbers come from the content.
        let pump = groups
            .iter()
            .flat_map(|g| &g.cards)
            .find(|c| c.name == "Water pump")
            .unwrap();
        assert_eq!(pump.cost, Cost::from_units(12000.0));
        assert_eq!(pump.upkeep_week, Cost::from_units(512.0));
        assert_eq!(pump.capacity, 1000);
        assert_eq!(pump.lot, (2, 2));
    }

    #[test]
    fn clicking_a_card_selects_its_building_and_activates_place() {
        let view = view();
        let first = groups(view.content.as_deref().unwrap())[0].cards[0].clone();
        let ctx = egui::Context::default();
        let mut tools = ToolState::default();
        let mut overlay = OverlayState::default();
        let draw = |tools: &mut ToolState, overlay: &mut OverlayState, events| {
            frame_texts(&ctx, input(0.0, events), |ui| {
                buildings_panel(ui, Some(&view), tools, overlay)
            })
        };
        let texts = draw(&mut tools, &mut overlay, vec![]);
        let (_, at) = texts
            .iter()
            .find(|(t, _)| t.starts_with(&first.name))
            .expect("the card is drawn");
        assert_eq!(tools.active, ActiveTool::None);
        let [down, up] = click(*at + egui::vec2(4.0, 4.0));
        draw(&mut tools, &mut overlay, down);
        draw(&mut tools, &mut overlay, up);
        assert_eq!(tools.active, ActiveTool::Place);
        assert_eq!(tools.building.def, Some(first.id));
        // The text of a card carries the price, upkeep and lot.
        let text = card_text(&first);
        assert!(text.contains("/wk") && text.contains("lot 2×2"), "{text}");
    }

    #[test]
    fn the_verdict_draws_for_a_valid_and_an_invalid_ghost() {
        use crate::palette::Category;
        use cp_tools::PlacementGhost;
        let ctx = egui::Context::default();
        let mut tools = ToolState::default();
        tools.select(ActiveTool::Place);
        let drawn = |tools: &ToolState| -> String {
            frame_texts(&ctx, input(0.0, vec![]), |ui| {
                crate::panels::verdict(ui, Category::Buildings, tools)
            })
            .into_iter()
            .map(|(t, _)| t)
            .collect::<Vec<_>>()
            .join("\n")
        };
        assert!(
            drawn(&tools).contains("T rotates"),
            "the hint without a ghost"
        );
        tools.place_ghost = Some(PlacementGhost {
            cells: vec![],
            valid: true,
            cost: Some(Cost(1_200_000)),
            error: None,
            free: None,
            richness: None,
            access: None,
        });
        let ok = drawn(&tools);
        assert!(ok.contains("can place") && ok.contains("12,000"), "{ok}");
        assert!(ok.contains("T rotates"), "{ok}");
        // A service ghost's road coverage (T-254) under the verdict.
        tools.coverage = Some(cp_render::CoverageCount {
            reached: 3,
            newly: 2,
            capacity: 1000,
        });
        let cov = drawn(&tools);
        assert!(
            cov.contains("Covers 3 buildings, 2 newly · capacity 1000")
                && cov.contains("Blue: newly covered"),
            "{cov}"
        );
        tools.coverage = None;
        tools.place_ghost = Some(PlacementGhost {
            cells: vec![],
            valid: false,
            cost: None,
            error: Some("not on water".into()),
            free: None,
            richness: None,
            access: None,
        });
        let bad = drawn(&tools);
        assert!(
            bad.contains("not on water") && !bad.contains("can place"),
            "{bad}"
        );
        assert!(bad.contains("T rotates"), "{bad}");
    }
}
