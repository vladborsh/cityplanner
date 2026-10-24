//! The Roads category (T-083): the tools, the drawing mode, the elevation buttons and the
//! road types as cards ordered by class and lanes. Read-only over the content; a click only
//! changes the existing [`ToolState`].

use bevy_egui::egui;
use cp_content::{ContentDb, LaneKind, RoadClass, RoadDefId};
use cp_core::Cost;
use cp_locale::{tr, trf, trn};
use cp_tools::{
    ActiveTool, ELEVATION_STEP, MAX_ELEVATION_LEVEL, RoadMode, ToolState, format_money,
};

use crate::Icon;
use crate::icons;
use crate::style::Sizes;
use crate::tile::{TileIcon, tool_tile};

/// The tab and section a road type is shown under: its class, or the Bike section for every
/// road with a bike lane (T-322, §23.3), the last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoadTab {
    Class(RoadClass),
    Bike,
}

/// What a card shows, taken from the road definition.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: RoadDefId,
    pub name: String,
    pub class: RoadClass,
    pub tab: RoadTab,
    /// A path of bike lanes only (T-322).
    pub bike_path: bool,
    /// Car lanes.
    pub lanes: usize,
    pub speed_kmh: f32,
    pub one_way: bool,
    pub cost_per_m: Cost,
    pub upkeep_per_m_week: Cost,
    pub allows_zoning: bool,
}

/// One card per road type, ordered by class, then lanes, then name. Track (class `Rail`) is not
/// a road: it comes with the metro tool (T-164).
pub fn cards(content: &ContentDb) -> Vec<Card> {
    let mut out: Vec<Card> = content
        .roads()
        .filter(|(_, def)| def.class != RoadClass::Rail)
        .map(|(id, def)| Card {
            id,
            name: def.name.clone(),
            class: def.class,
            tab: if def.lanes.iter().any(|l| l.kind == LaneKind::Bike) {
                RoadTab::Bike
            } else {
                RoadTab::Class(def.class)
            },
            bike_path: def.class == RoadClass::Path
                && def.lanes.iter().all(|l| l.kind == LaneKind::Bike),
            lanes: def.lanes.iter().filter(|l| l.kind == LaneKind::Car).count(),
            speed_kmh: def.speed_kmh,
            one_way: def.is_one_way(),
            cost_per_m: Cost::from_units(def.cost_per_m),
            upkeep_per_m_week: Cost::from_units(def.upkeep_per_m_week),
            allows_zoning: def.allows_zoning,
        })
        .collect();
    out.sort_by(|a, b| (a.tab, a.class, a.lanes, &a.name).cmp(&(b.tab, b.class, b.lanes, &b.name)));
    out
}

/// "2 lanes · 50 km/h · one-way".
pub fn card_summary(c: &Card) -> String {
    if c.bike_path {
        return trf!(
            "Bike lanes only · {speed} km/h",
            speed = format!("{:.0}", c.speed_kmh)
        );
    }
    if c.class == RoadClass::Path {
        return trf!(
            "Pedestrian lanes only · {speed} km/h",
            speed = format!("{:.0}", c.speed_kmh)
        );
    }
    let mut out = format!(
        "{} · {}",
        trn("{n} lanes", c.lanes as i64),
        trf!("{speed} km/h", speed = format!("{:.0}", c.speed_kmh))
    );
    if c.one_way {
        out.push_str(" · ");
        out.push_str(&tr("one-way"));
    }
    out
}

fn class_label(tab: RoadTab) -> &'static str {
    match tab {
        RoadTab::Class(RoadClass::Local) => "Local",
        RoadTab::Class(RoadClass::Collector) => "Collector",
        RoadTab::Class(RoadClass::Arterial) => "Arterial",
        RoadTab::Class(RoadClass::Highway) => "Highway",
        RoadTab::Class(RoadClass::Rail) => "Rail",
        RoadTab::Class(RoadClass::Path) => "Paths",
        RoadTab::Bike => "Bike",
    }
}

/// The height of the road tool as the panel states it: under the ground, on it or raised.
pub fn road_height_text(tool: &cp_tools::RoadTool) -> String {
    if tool.underground {
        trf!(
            "Tunnel {m} m",
            m = format!("{:.0}", cp_tools::ROAD_TUNNEL_DEPTH)
        )
    } else {
        elevation_text(tool.level)
    }
}

/// The elevation as the panel states it.
pub fn elevation_text(level: u8) -> String {
    if level == 0 {
        tr("Ground")
    } else {
        trf!(
            "Raised {m} m",
            m = format!("{:.0}", f32::from(level) * ELEVATION_STEP)
        )
    }
}

/// The hover text of a road tile: everything the tile itself leaves out.
pub fn tile_tooltip(c: &Card) -> String {
    let mut text = format!(
        "{}\n{}\n{}",
        tr(&c.name),
        card_summary(c),
        trf!(
            "{cost}/m · {upkeep}/m/wk",
            cost = format_money(c.cost_per_m),
            upkeep = format_money(c.upkeep_per_m_week)
        )
    );
    if !c.allows_zoning {
        text.push('\n');
        text.push_str(&tr("no zoning"));
    }
    text
}

/// One tile of the grid: the procedural icon, the name, the numbers on hover.
fn road_tile_widget(
    ui: &mut egui::Ui,
    content: &ContentDb,
    widest: f32,
    c: &Card,
    selected: bool,
) -> egui::Response {
    let def = content.road(c.id);
    let draw = |painter: &egui::Painter, rect: egui::Rect| {
        icons::road_tile_scaled(painter, def, rect, widest);
    };
    tool_tile(ui, TileIcon::Draw(&draw), &tr(&c.name), selected).on_hover_text(tile_tooltip(c))
}

/// The footpath tile for the Parks category (T-226): the same tool and tile as the Paths row of
/// the Roads panel (the walkers' path, not the bike path of T-322). Does nothing without a path
/// road type.
pub fn path_tile(ui: &mut egui::Ui, content: &ContentDb, tools: &mut ToolState) {
    let Some(c) = cards(content).into_iter().find(|c| {
        c.class == RoadClass::Path
            && content
                .road(c.id)
                .lanes
                .iter()
                .all(|l| l.kind == cp_content::LaneKind::Pedestrian)
    }) else {
        return;
    };
    let widest = content.roads().map(|(_, d)| d.width).fold(0.0, f32::max);
    let on = tools.active == ActiveTool::Road && tools.road.road == Some(c.id);
    if road_tile_widget(ui, content, widest, &c, on).clicked() {
        if tools.active != ActiveTool::Road {
            tools.select(ActiveTool::Road);
        }
        tools.road.road = Some(c.id);
    }
}

/// The tabs that have a tile, in order: the tabs of the Roads tray.
pub fn tab_classes(content: &ContentDb) -> Vec<RoadTab> {
    let mut tabs: Vec<RoadTab> = cards(content).iter().map(|c| c.tab).collect();
    tabs.dedup();
    tabs
}

/// The label of a tab (T-317: Small / Medium / Large / Highway, the footpaths; T-322: Bike).
pub fn tab_label(tab: RoadTab) -> String {
    tr(match tab {
        RoadTab::Class(RoadClass::Local) => "Small",
        RoadTab::Class(RoadClass::Collector) => "Medium",
        RoadTab::Class(RoadClass::Arterial) => "Large",
        RoadTab::Class(RoadClass::Highway) => "Highway",
        RoadTab::Class(RoadClass::Rail) => "Rail",
        RoadTab::Class(RoadClass::Path) => "Paths",
        RoadTab::Bike => "Bike",
    })
}

/// The Roads tray: the road types of the active tab as one row of tiles, the most used first.
/// A tile picks the road type; with no road tool in hand it also takes the road tool.
pub fn roads_panel(ui: &mut egui::Ui, content: Option<&ContentDb>, tools: &mut ToolState) {
    let Some(content) = content else {
        return;
    };
    let cards = cards(content);
    // The metro track is the Transport panel's (T-164): coming back here drops it.
    if tools.road.tunnel_depth.take().is_some() {
        tools.road.road = None;
    }
    if tools.road.road.is_none() {
        tools.road.road = cards.first().map(|c| c.id);
    }
    let upgrading = tools.active == ActiveTool::Upgrade;
    let building = tools.active == ActiveTool::Road;
    let widest = content.roads().map(|(_, d)| d.width).fold(0.0, f32::max);
    let in_tray = crate::tray::in_tray(ui.ctx());
    let mut first = true;
    for class in tab_classes(content) {
        if !crate::tray::is_active(ui.ctx(), &tab_label(class)) {
            continue;
        }
        if !in_tray {
            ui.strong(tr(class_label(class)));
        } else if !std::mem::take(&mut first) {
            // No filter (T-317f): every class in the one row, a line between them.
            ui.separator();
        }
        let tiles = |ui: &mut egui::Ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            for c in cards.iter().filter(|c| c.tab == class) {
                if road_tile_widget(ui, content, widest, c, tools.road.road == Some(c.id)).clicked()
                {
                    tools.road.road = Some(c.id);
                    if !upgrading && !building {
                        tools.select(ActiveTool::Road);
                    }
                }
            }
        };
        if in_tray {
            ui.horizontal(tiles);
        } else {
            ui.horizontal_wrapped(tiles);
        }
    }
    // Decoration (T-323): the street trees tool.
    ui.separator();
    let on = tools.active == ActiveTool::Trees;
    if Icon::StreetTrees
        .button(ui, Sizes::PALETTE_ICON, Sizes::BUTTON_PAD, on)
        .on_hover_text(tr("Street trees"))
        .clicked()
    {
        tools.select(if on {
            ActiveTool::None
        } else {
            ActiveTool::Trees
        });
    }
}

/// The options strip of the Roads tray: mode, snaps and elevation of the road tool; a word for
/// the upgrade tool (the tiles are what it upgrades to).
pub fn road_options(ui: &mut egui::Ui, tools: &mut ToolState) {
    if tools.active == ActiveTool::Upgrade {
        ui.label(tr("Upgrade to"));
        return;
    }
    if tools.active == ActiveTool::Trees {
        ui.label(tr("Street trees"));
        return;
    }
    if tools.active != ActiveTool::Road {
        return;
    }
    for (mode, label) in [
        (RoadMode::Straight, "Straight"),
        (RoadMode::Curve, "Curve"),
        (RoadMode::Free, "Free"),
    ] {
        if ui
            .selectable_label(tools.road.mode == mode, tr(label))
            .clicked()
        {
            tools.road.set_mode(mode);
        }
    }
    ui.separator();
    // Snaps (§6.8.3 item 9): off is the same as holding Alt.
    let mut snaps = !tools.snaps_off;
    if ui
        .checkbox(&mut snaps, tr("Snaps"))
        .on_hover_text(tr(
            "Nodes, guides, angles and the 8 m step. Hold Alt for free placement.",
        ))
        .changed()
    {
        tools.snaps_off = !snaps;
    }
    // T-323 (§24.2): the plan plants a tree row on its ground segments.
    ui.checkbox(&mut tools.road.with_trees, tr("With trees"));
    ui.separator();
    ui.label(tr("Elevation (PgUp / PgDn, Ctrl+Up / Down)"));
    if ui
        .add_enabled(
            tools.road.level > 0 || !tools.road.underground,
            egui::Button::new("−"),
        )
        .clicked()
    {
        tools.road.lower();
    }
    ui.label(road_height_text(&tools.road));
    if ui
        .add_enabled(
            tools.road.underground || tools.road.level < MAX_ELEVATION_LEVEL,
            egui::Button::new("+"),
        )
        .clicked()
    {
        tools.road.raise();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{click, frame_texts, input};
    use crate::tile::take_rects;

    /// T-322 (§23.6 item 7): the three bike roads are a Bike tab of their own, the last, with
    /// the bike path said to be bike lanes only.
    #[test]
    fn the_bike_roads_are_a_bike_tab_of_three_tiles() {
        let db = content();
        assert_eq!(tab_classes(&db).last(), Some(&RoadTab::Bike));
        let bikes: Vec<String> = cards(&db)
            .into_iter()
            .filter(|c| c.tab == RoadTab::Bike)
            .map(|c| c.name)
            .collect();
        assert_eq!(bikes, ["Bike street", "Bike collector", "Bike path"]);
        let path = cards(&db).into_iter().find(|c| c.bike_path).unwrap();
        assert!(card_summary(&path).starts_with("Bike lanes only"));
        assert_eq!(tab_label(RoadTab::Bike), "Bike");
    }

    fn content() -> ContentDb {
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap()
    }

    /// One frame of the Roads panel (road tool active) in a 300 px panel on a tall screen.
    fn draw(
        ctx: &egui::Context,
        t: f64,
        events: Vec<egui::Event>,
        content: &ContentDb,
        tools: &mut ToolState,
    ) -> Vec<(String, egui::Pos2)> {
        let mut raw = input(t, events);
        raw.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 3000.0),
        ));
        frame_texts(ctx, raw, |ui| {
            let mut root = crate::panels::root_ui(ui.ctx());
            egui::Panel::left("p")
                .exact_size(300.0)
                .show(&mut root, |ui| roads_panel(ui, Some(content), tools));
        })
    }

    fn road_tiles(ctx: &egui::Context, n: usize) -> Vec<(egui::Rect, egui::Rect)> {
        let all = take_rects(ctx).0;
        all[all.len() - n..].to_vec()
    }

    #[test]
    fn the_types_are_tiles_in_card_order_three_to_a_row_under_one_header_per_class() {
        let content = content();
        let cards = cards(&content);
        let mut tools = ToolState::default();
        tools.select(ActiveTool::Road);
        let ctx = egui::Context::default();
        draw(&ctx, 0.0, vec![], &content, &mut tools);
        take_rects(&ctx);
        let texts = draw(&ctx, 0.1, vec![], &content, &mut tools);
        let tiles = road_tiles(&ctx, cards.len());
        // The square rule (§13.7).
        for (tile, icon) in &tiles {
            assert_eq!(tile.size(), Sizes::tile());
            assert_eq!(icon.size(), Sizes::tile_icon());
        }
        // Order: left to right, rows top to bottom; at most three to a row.
        let mut rows: Vec<Vec<egui::Rect>> = Vec::new();
        for (tile, _) in &tiles {
            match rows.last_mut() {
                Some(row) if (row[0].top() - tile.top()).abs() < 1.0 => row.push(*tile),
                _ => rows.push(vec![*tile]),
            }
        }
        assert!(rows.iter().all(|r| r.len() <= 3), "{rows:?}");
        assert!(
            rows.iter().any(|r| r.len() == 3) || cards.len() < 3,
            "no row holds three tiles: {rows:?}"
        );
        for row in &rows {
            assert!(row.windows(2).all(|w| w[1].left() > w[0].right() - 0.5));
        }
        // Names follow the card order.
        let names: Vec<&str> = texts
            .iter()
            .filter(|(t, _)| cards.iter().any(|c| &c.name == t))
            .map(|(t, _)| t.as_str())
            .collect();
        let expect: Vec<&str> = cards.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, expect);
        // One header per class.
        let classes = {
            let mut c: Vec<_> = cards.iter().map(|c| c.tab).collect();
            c.dedup();
            c
        };
        let headers = texts
            .iter()
            .filter(|(t, _)| classes.iter().any(|c| t == class_label(*c)))
            .count();
        assert_eq!(headers, classes.len());
    }

    #[test]
    fn a_click_on_a_tile_selects_the_road_type() {
        let content = content();
        let cards = cards(&content);
        let mut tools = ToolState::default();
        tools.select(ActiveTool::Road);
        let ctx = egui::Context::default();
        draw(&ctx, 0.0, vec![], &content, &mut tools);
        take_rects(&ctx);
        draw(&ctx, 0.1, vec![], &content, &mut tools);
        let tiles = road_tiles(&ctx, cards.len());
        for (i, c) in cards.iter().enumerate().rev() {
            let [down, up] = click(tiles[i].0.center());
            draw(&ctx, 0.2 + i as f64, down, &content, &mut tools);
            draw(&ctx, 0.25 + i as f64, up, &content, &mut tools);
            assert_eq!(tools.road.road, Some(c.id), "{}", c.name);
        }
    }

    #[test]
    fn the_tooltip_has_the_numbers_the_tile_leaves_out() {
        let content = content();
        for c in cards(&content) {
            let text = tile_tooltip(&c);
            assert!(text.contains(&c.name));
            assert!(text.contains("km/h"), "{text}");
            assert!(text.contains("lane"), "{text}");
            assert!(text.contains(&format_money(c.cost_per_m)), "{text}");
            assert_eq!(text.contains("no zoning"), !c.allows_zoning);
        }
    }

    /// How many filled polygons (the arrows) a road tile paints.
    fn arrows(def: &cp_content::RoadDef) -> usize {
        fn count(shape: &egui::Shape) -> usize {
            match shape {
                egui::Shape::Path(_) => 1,
                egui::Shape::Vec(v) => v.iter().map(count).sum(),
                _ => 0,
            }
        }
        let ctx = egui::Context::default();
        let mut n = 0;
        let mut out = ctx.run_ui(input(0.0, vec![]), |ui| {
            let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), Sizes::tile_icon());
            icons::road_tile(ui.painter(), def, rect);
        });
        out.textures_delta.clear();
        for clipped in &out.shapes {
            n += count(&clipped.shape);
        }
        n
    }

    #[test]
    fn one_way_roads_show_arrows_and_two_way_streets_none() {
        let content = content();
        let road = |id: &str| content.road(content.road_id(id).unwrap());
        assert_eq!(arrows(road("basic_2l")), 0);
        // T-172: one-way streets with sidewalks show the arrows of their cars lanes too.
        assert!(arrows(road("basic_1w_2l")) > 0 && arrows(road("medium_1w_3l")) > 0);
        assert!(arrows(road("highway_1w_1l")) > 0);
        assert!(arrows(road("highway_1w_4l")) > arrows(road("highway_1w_1l")));
    }

    /// T-172: every road type paints its own picture (the lane count shows), in a square tile.
    #[test]
    fn every_road_tile_is_square_and_different_from_all_the_others() {
        let content = content();
        let size = Sizes::tile_icon();
        assert_eq!(size.x, size.y);
        let widest = content.roads().map(|(_, d)| d.width).fold(0.0, f32::max);
        let mut seen: Vec<(String, String)> = Vec::new();
        for (_, def) in content.roads() {
            let ctx = egui::Context::default();
            let mut out = ctx.run_ui(input(0.0, vec![]), |ui| {
                let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), size);
                icons::road_tile_scaled(ui.painter(), def, rect, widest);
            });
            out.textures_delta.clear();
            let picture = format!("{:?}", out.shapes);
            if let Some((other, _)) = seen.iter().find(|(_, p)| *p == picture) {
                panic!("{} and {other} have the same tile", def.id);
            }
            seen.push((def.id.clone(), picture));
        }
        assert!(seen.iter().any(|(id, _)| id == "basic_1w_2l"));
    }
}
