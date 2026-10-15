//! The Buildings category: the service buildings of the content as cards, grouped by the kind
//! of service they provide. Read-only over the content; a click only selects the building in
//! the existing [`ToolState`].

use bevy_egui::egui;
use cp_content::{BuildingDefId, ContentDb, ServiceKind};
use cp_core::Cost;
use cp_sim::SimView;
use cp_tools::{ActiveTool, ToolState, format_money};

use crate::icons::Icon;

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
    format!("{kind:?}")
}

fn kind_icon(kind: ServiceKind) -> Icon {
    match kind {
        ServiceKind::Water => Icon::Water,
        ServiceKind::Power => Icon::Power,
        ServiceKind::Sewage => Icon::PipeSewage,
        ServiceKind::Roads => Icon::Roads,
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

/// Selects `id` for placing.
pub fn select(tools: &mut ToolState, id: BuildingDefId) {
    if tools.active != ActiveTool::Place {
        tools.select(ActiveTool::Place);
    }
    tools.building.def = Some(id);
}

/// The panel: a header with an icon per service kind, then its cards.
pub fn buildings_panel(ui: &mut egui::Ui, view: Option<&SimView>, tools: &mut ToolState) {
    let Some(content) = view.and_then(|v| v.content.as_deref()) else {
        return;
    };
    for group in groups(content) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            kind_icon(group.kind).show(ui, crate::style::Sizes::SECTION_ICON);
            ui.strong(kind_label(group.kind));
        });
        for card in &group.cards {
            let selected = tools.active == ActiveTool::Place && tools.building.def == Some(card.id);
            let button = egui::Button::image_and_text(
                egui::Image::new(crate::icons::def_icon(&card.icon)).fit_to_exact_size(egui::vec2(
                    crate::style::Sizes::ROW_ICON,
                    crate::style::Sizes::ROW_ICON,
                )),
                card_text(card),
            )
            .selected(selected)
            .min_size(egui::vec2(ui.available_width(), 0.0));
            if ui.add(button).clicked() {
                select(tools, card.id);
            }
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
        assert_eq!(pump.upkeep_week, Cost::from_units(300.0));
        assert_eq!(pump.capacity, 1000);
        assert_eq!(pump.lot, (2, 2));
    }

    #[test]
    fn clicking_a_card_selects_its_building_and_activates_place() {
        let view = view();
        let first = groups(view.content.as_deref().unwrap())[0].cards[0].clone();
        let ctx = egui::Context::default();
        let mut tools = ToolState::default();
        let draw = |tools: &mut ToolState, events| {
            frame_texts(&ctx, input(0.0, events), |ui| {
                buildings_panel(ui, Some(&view), tools)
            })
        };
        let texts = draw(&mut tools, vec![]);
        let (_, at) = texts
            .iter()
            .find(|(t, _)| t.starts_with(&first.name))
            .expect("the card is drawn");
        assert_eq!(tools.active, ActiveTool::None);
        let [down, up] = click(*at + egui::vec2(4.0, 4.0));
        draw(&mut tools, down);
        draw(&mut tools, up);
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
        });
        let ok = drawn(&tools);
        assert!(ok.contains("can place") && ok.contains("12,000"), "{ok}");
        assert!(ok.contains("T rotates"), "{ok}");
        tools.place_ghost = Some(PlacementGhost {
            cells: vec![],
            valid: false,
            cost: None,
            error: Some("not on water".into()),
        });
        let bad = drawn(&tools);
        assert!(
            bad.contains("not on water") && !bad.contains("can place"),
            "{bad}"
        );
        assert!(bad.contains("T rotates"), "{bad}");
    }
}
