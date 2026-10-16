//! The left palette and the tool panel beside it (`architecture/system-design.md` §13.6).
//!
//! The palette is a 72 px column with one icon per [`Category`]; clicking one (or pressing its
//! number key, 1…5) opens that category's 300 px tool panel, clicking it again closes the
//! panel. The panel has a header, the tool buttons and options of the category, and the pinned
//! "cost · verdict" block at the bottom.

use bevy_egui::egui;
use cp_content::ContentDb;
use cp_render::OverlayState;
use cp_sim::SimView;
use cp_tools::{CommandOutbox, ToolState};

use crate::icons::Icon;
use crate::panels::{UiState, bulldozer_panel, roads_panel, services_panel, verdict, zones_panel};
use crate::style::Sizes;

/// Build-menu categories. A category without tools is not drawn ([`Category::available`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Roads,
    Zones,
    Buildings,
    Bulldozer,
    /// Districts: brushes, the district list and its taxes (M2).
    Districts,
    /// Public transport (M3): the line tool and the depot. Last, so the hotkeys of the
    /// others do not move.
    Transit,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Roads,
        Category::Zones,
        Category::Buildings,
        Category::Bulldozer,
        Category::Districts,
        Category::Transit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Roads => "Roads",
            Category::Zones => "Zones",
            Category::Buildings => "Buildings",
            Category::Bulldozer => "Bulldozer",
            Category::Districts => "Districts",
            Category::Transit => "Transport",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Category::Roads => Icon::Roads,
            Category::Zones => Icon::Zones,
            Category::Buildings => Icon::Buildings,
            Category::Bulldozer => Icon::Bulldozer,
            Category::Districts => Icon::Districts,
            Category::Transit => Icon::Transit,
        }
    }

    /// Whether the category has a tool to offer with `content`: roads need a road type,
    /// buildings a placeable service building, transport a mode with something to build; the
    /// others are always there.
    pub fn has_tools(self, content: Option<&ContentDb>) -> bool {
        match self {
            Category::Roads => content.is_some_and(|c| !crate::roads::cards(c).is_empty()),
            Category::Buildings => {
                content.is_some_and(|c| c.buildings().any(|(_, d)| d.service.is_some()))
            }
            Category::Zones | Category::Bulldozer | Category::Districts => true,
            Category::Transit => crate::transit::has_tools(content),
        }
    }

    /// The categories to draw, in palette order. Hotkey `n` opens the `n`-th of them.
    pub fn available(content: Option<&ContentDb>) -> Vec<Category> {
        Category::ALL
            .into_iter()
            .filter(|c| c.has_tools(content))
            .collect()
    }
}

/// The palette column. Returns each icon's rectangle (tests click them). A click toggles
/// `state.category`.
pub fn palette(
    root: &mut egui::Ui,
    state: &mut UiState,
    content: Option<&ContentDb>,
) -> Vec<(Category, egui::Rect)> {
    let mut rects = Vec::new();
    egui::Panel::left("palette")
        .exact_size(Sizes::PALETTE_W)
        .resizable(false)
        .show(root, |ui| {
            ui.vertical_centered(|ui| {
                for (n, c) in Category::available(content).into_iter().enumerate() {
                    let selected = state.category == Some(c);
                    let r = c
                        .icon()
                        .button(ui, Sizes::PALETTE_ICON, Sizes::BUTTON_PAD, selected)
                        .on_hover_text(format!("{} ({})", c.label(), n + 1));
                    rects.push((c, r.rect));
                    if r.clicked() {
                        toggle(state, c);
                    }
                }
            });
        });
    rects
}

/// Opens `c`'s panel, or closes it when it is open.
pub fn toggle(state: &mut UiState, c: Category) {
    state.category = if state.category == Some(c) {
        None
    } else {
        Some(c)
    };
}

/// The category a digit key press opens, if any: `1` is the first available category. Keys with
/// Ctrl, Alt or Cmd held, and keys typed into a text field, do not count.
pub fn hotkey(ctx: &egui::Context, available: &[Category]) -> Option<Category> {
    if ctx.egui_wants_keyboard_input() {
        return None;
    }
    ctx.input(|i| {
        const KEYS: [egui::Key; 9] = [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
        ];
        // Read the events, not `key_pressed`: that one ignores which modifiers were held.
        i.events
            .iter()
            .find_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if !modifiers.any() => KEYS.iter().position(|k| k == key),
                _ => None,
            })
            .and_then(|n| available.get(n).copied())
    })
}

/// The tool panel of the open category (nothing when none is open). Returns its rectangle.
pub fn tool_panel(
    root: &mut egui::Ui,
    state: &mut UiState,
    view: Option<&SimView>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    outbox: &mut CommandOutbox,
) -> Option<egui::Rect> {
    let category = state.category?;
    let out = egui::Panel::left("tool_panel")
        .exact_size(Sizes::TOOL_PANEL_W)
        .resizable(false)
        .show(root, |ui| {
            ui.horizontal(|ui| {
                category.icon().show(ui, Sizes::PANEL_HEADER_ICON);
                ui.heading(category.label());
            });
            ui.separator();
            // The pinned block goes first so that the content above leaves room for it.
            egui::Panel::bottom("tool_verdict")
                .resizable(false)
                .show(ui, |ui| {
                    ui.add_space(2.0);
                    verdict(ui, category, tools);
                });
            egui::ScrollArea::vertical().show(ui, |ui| match category {
                Category::Roads => roads_panel(ui, view, tools, state),
                Category::Zones => zones_panel(ui, view, tools, state),
                Category::Buildings => services_panel(ui, view, tools, overlay),
                Category::Bulldozer => bulldozer_panel(ui, tools),
                Category::Districts => crate::districts::districts_panel(ui, view, tools, outbox),
                Category::Transit => {
                    crate::transit::transit_panel(ui, view, tools, overlay, &mut state.lines_open)
                }
            });
        });
    Some(out.response.rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{click, frame, frame_texts, input, key};
    use cp_core::Cost;
    use cp_sim::SimRunner;
    use cp_tools::{ActiveTool, Ghost};

    fn content() -> std::sync::Arc<ContentDb> {
        std::sync::Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        )
    }

    fn view() -> SimView {
        let mut sim = SimRunner::new(1);
        sim.set_content(content());
        sim.view()
    }

    /// The UI of a frame: palette and tool panel on a root `Ui`, like `draw_ui`.
    struct Rig {
        ctx: egui::Context,
        state: UiState,
        tools: ToolState,
        overlay: OverlayState,
        outbox: CommandOutbox,
        view: SimView,
        t: f64,
        palette: Vec<(Category, egui::Rect)>,
        panel: Option<egui::Rect>,
        /// Screen size of the frames.
        screen: egui::Vec2,
    }

    impl Rig {
        fn new() -> Self {
            let mut rig = Rig {
                ctx: egui::Context::default(),
                state: UiState::default(),
                tools: ToolState::default(),
                overlay: OverlayState::default(),
                outbox: CommandOutbox::default(),
                view: view(),
                t: 0.0,
                palette: Vec::new(),
                panel: None,
                screen: egui::vec2(1280.0, 720.0),
            };
            rig.frame(vec![]);
            rig.frame(vec![]);
            rig
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> String {
            self.frame_texts(events)
                .into_iter()
                .map(|(t, _)| t)
                .collect::<Vec<_>>()
                .join("\n")
        }

        /// A frame; every painted text with where it starts.
        fn frame_texts(&mut self, events: Vec<egui::Event>) -> Vec<(String, egui::Pos2)> {
            self.t += 0.1;
            let Rig {
                ctx,
                state,
                tools,
                overlay,
                outbox,
                view,
                palette: pal,
                panel,
                screen,
                t,
            } = self;
            let mut raw = input(*t, events);
            raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, *screen));
            frame_texts(ctx, raw, |ui| {
                let mut root = crate::panels::root_ui(ui.ctx());
                *pal = palette(&mut root, state, view.content.as_deref());
                *panel = tool_panel(&mut root, state, Some(view), tools, overlay, outbox);
            })
        }

        fn click_at(&mut self, pos: egui::Pos2) {
            let [down, up] = click(pos);
            self.frame(down);
            self.frame(up);
            self.frame(vec![]);
        }

        /// Clicks the button whose label is `label` in the open panel.
        fn click_label(&mut self, label: &str) {
            let texts = self.frame_texts(vec![]);
            let (_, at) = texts
                .iter()
                .find(|(t, p)| t == label && p.y > 30.0)
                .unwrap_or_else(|| panic!("no {label:?} in {texts:?}"));
            self.click_at(*at + egui::vec2(4.0, 4.0));
        }

        fn click_category(&mut self, c: Category) {
            let rect = self.palette.iter().find(|(k, _)| *k == c).unwrap().1;
            self.click_at(rect.center());
        }
    }

    #[test]
    fn clicking_a_category_icon_opens_its_panel_and_clicking_again_closes_it() {
        let mut rig = Rig::new();
        assert_eq!(rig.state.category, None);
        assert!(rig.panel.is_none());
        rig.click_category(Category::Roads);
        assert_eq!(rig.state.category, Some(Category::Roads));
        assert!(rig.panel.is_some());
        rig.click_category(Category::Zones);
        assert_eq!(rig.state.category, Some(Category::Zones));
        rig.click_category(Category::Zones);
        assert_eq!(rig.state.category, None);
        assert!(rig.panel.is_none());
    }

    #[test]
    fn zone_tools_and_cards_set_the_tool_kind_and_zone_and_esc_keeps_the_panel() {
        use cp_content::ZoneType;
        use cp_tools::ZoneToolKind;
        let mut rig = Rig::new();
        rig.click_category(Category::Zones);
        for (label, kind) in [
            ("Rectangle", ZoneToolKind::Rectangle),
            ("Eraser", ZoneToolKind::Eraser),
        ] {
            rig.click_label(label);
            assert_eq!(rig.tools.active, ActiveTool::Zone);
            assert_eq!(rig.tools.zone.kind, kind);
        }
        rig.click_label("Office");
        assert_eq!(rig.tools.zone.zone, ZoneType::Office);
        rig.click_label("Industrial");
        assert_eq!(rig.tools.zone.zone, ZoneType::Industrial);
        // Esc leaves the tool (cp_tools); the panel stays open.
        rig.tools.select(ActiveTool::None);
        rig.frame(vec![]);
        assert_eq!(rig.state.category, Some(Category::Zones));
        assert!(rig.panel.is_some());
        // A card alone also activates the tool.
        rig.click_label("Residential (high)");
        assert_eq!(rig.tools.active, ActiveTool::Zone);
        assert_eq!(rig.tools.zone.zone, ZoneType::ResHigh);
    }

    #[test]
    fn zone_cards_show_their_group_demand_and_the_demand_button_opens_the_window() {
        use cp_content::ZoneType as Z;
        let d = cp_building::Demand {
            r: 40.0,
            c: -20.0,
            i: 10.0,
            o: -90.0,
            ..Default::default()
        };
        for (zone, want) in [
            (Z::ResLow, ("R", 40.0)),
            (Z::ResHigh, ("R", 40.0)),
            (Z::ComLow, ("C", -20.0)),
            (Z::ComHigh, ("C", -20.0)),
            (Z::Industrial, ("I", 10.0)),
            (Z::Office, ("O", -90.0)),
        ] {
            assert_eq!(crate::panels::zone_group_demand(zone, &d), want);
        }
        let mut rig = Rig::new();
        rig.click_category(Category::Zones);
        let text = rig.frame(vec![]);
        assert!(text.contains("Demand by zone"), "{text}");
        assert!(!rig.state.demand_open);
        rig.click_label("Demand by zone ▸");
        assert!(rig.state.demand_open);
    }

    #[test]
    fn road_cards_match_the_content_in_class_and_lane_order_and_a_click_selects() {
        let db = content();
        let cards = crate::roads::cards(&db);
        assert_eq!(
            cards.len(),
            db.roads()
                .filter(|(_, d)| d.class != cp_content::RoadClass::Rail)
                .count()
        );
        assert!(
            cards
                .windows(2)
                .all(|w| (w[0].class, w[0].lanes) <= (w[1].class, w[1].lanes)),
            "{cards:?}"
        );
        let mut rig = Rig::new();
        // Tall enough for every card (the list grows until T-098 makes it a grid).
        rig.screen = egui::vec2(1280.0, 3000.0);
        rig.click_category(Category::Roads);
        rig.click_label("Build");
        assert_eq!(rig.tools.active, ActiveTool::Road);
        // Click the first two.
        for c in &cards[..2] {
            rig.click_label(&c.name);
            assert_eq!(rig.tools.road.road, Some(c.id));
        }
        // ("no zoning" is in the tile's tooltip now: `roads::tile_tooltip`, tested there.)
    }

    #[test]
    fn the_upgrade_button_selects_the_tool_and_its_verdict_shows_cost_warning_and_errors() {
        use cp_tools::UpgradePreview;
        let mut rig = Rig::new();
        rig.click_category(Category::Roads);
        rig.click_label("Upgrade");
        assert_eq!(rig.tools.active, ActiveTool::Upgrade);
        assert!(rig.frame(vec![]).contains("Upgrade to"));
        rig.tools.upgrade_preview = Some(UpgradePreview {
            run: vec![],
            changes: vec![],
            cost: Cost(32_100),
            dropped_building_cells: 4,
            errors: vec![],
        });
        // Nothing changes: the run is already of that type.
        rig.frame(vec![]);
        assert!(
            rig.frame(vec![])
                .contains("already has the chosen road type")
        );
        rig.tools.upgrade_preview.as_mut().unwrap().changes = vec![cp_net::SegmentId::new(1, 0)];
        rig.frame(vec![]);
        let text = rig.frame(vec![]);
        assert!(text.contains("can upgrade"), "{text}");
        assert!(
            text.contains(&cp_tools::format_money(Cost(32_100))),
            "{text}"
        );
        assert!(text.contains("NO_ROAD"), "{text}");
        rig.tools.upgrade_preview.as_mut().unwrap().errors = vec!["no elevated variant".into()];
        rig.frame(vec![]);
        assert!(rig.frame(vec![]).contains("no elevated variant"));
        // Build and Upgrade are exclusive; the cards stay for both.
        rig.click_label("Build");
        assert_eq!(rig.tools.active, ActiveTool::Road);
        assert!(rig.tools.upgrade_preview.is_none());
    }

    #[test]
    fn elevation_buttons_stay_within_the_levels() {
        let mut rig = Rig::new();
        rig.click_category(Category::Roads);
        rig.click_label("Build");
        rig.click_label("−");
        assert_eq!(rig.tools.road.level, 0);
        for _ in 0..cp_tools::MAX_ELEVATION_LEVEL + 2 {
            rig.click_label("+");
        }
        assert_eq!(rig.tools.road.level, cp_tools::MAX_ELEVATION_LEVEL);
        rig.click_label("−");
        assert_eq!(rig.tools.road.level, cp_tools::MAX_ELEVATION_LEVEL - 1);
    }

    #[test]
    fn the_road_verdict_shows_length_cost_and_errors() {
        let mut rig = Rig::new();
        rig.click_category(Category::Roads);
        rig.click_label("Build");
        rig.tools.ghost = Some(Ghost {
            polylines: vec![vec![
                bevy::math::Vec3::ZERO,
                bevy::math::Vec3::new(30.0, 0.0, 40.0),
            ]],
            valid: true,
            cost: Some(Cost(5_000)),
            errors: vec![],
            half_width: 3.0,
            snap: Default::default(),
            chevrons: Vec::new(),
            warnings: vec!["one-way: arms face the same way".into()],
        });
        rig.frame(vec![]);
        let text = rig.frame(vec![]);
        assert!(text.contains("one-way: arms"), "{text}");
        assert!(text.contains("50 m"), "{text}");
        assert!(text.contains("can build"), "{text}");
        rig.tools.ghost = Some(Ghost {
            polylines: vec![],
            valid: false,
            cost: None,
            errors: vec!["crosses water".into()],
            half_width: 3.0,
            snap: Default::default(),
            chevrons: Vec::new(),
            warnings: vec![],
        });
        let text = rig.frame(vec![]);
        assert!(text.contains("crosses water"), "{text}");
    }

    #[test]
    fn the_palette_is_72_wide_with_52_icons_and_the_tool_panel_300() {
        let mut rig = Rig::new();
        for (c, r) in &rig.palette {
            assert!(
                r.width() >= Sizes::PALETTE_ICON && r.height() >= Sizes::PALETTE_ICON,
                "{c:?}: {r:?}"
            );
            assert!(r.width() <= Sizes::PALETTE_W, "{c:?}: {r:?}");
        }
        rig.click_category(Category::Roads);
        let panel = rig.panel.unwrap();
        assert_eq!(panel.width(), Sizes::TOOL_PANEL_W);
        // (The panel's rectangle starts half a stroke inside its outer edge.)
        assert!(
            (panel.min.x - Sizes::PALETTE_W).abs() <= 1.0,
            "the panel sits right of the palette: {}",
            panel.min.x
        );
    }

    /// The tiles one frame of the open panel draws: `(tile, icon frame)`.
    fn tiles(rig: &mut Rig) -> Vec<(egui::Rect, egui::Rect)> {
        crate::tile::take_rects(&rig.ctx);
        rig.frame(vec![]);
        crate::tile::take_rects(&rig.ctx).0
    }

    /// §13.7: every tile of every construction panel is one size, with a square icon frame.
    #[test]
    fn every_tile_of_the_roads_zones_buildings_and_bulldozer_panels_is_88_with_a_64_square_icon() {
        for (category, tool) in [
            (Category::Roads, ActiveTool::Road),
            (Category::Roads, ActiveTool::Upgrade),
            (Category::Zones, ActiveTool::Zone),
            (Category::Buildings, ActiveTool::Utility),
            (Category::Buildings, ActiveTool::Place),
            (Category::Buildings, ActiveTool::UtilityRemove),
            (Category::Buildings, ActiveTool::None),
            (Category::Bulldozer, ActiveTool::Bulldozer),
            (Category::Districts, ActiveTool::District),
            (Category::Transit, ActiveTool::Line),
        ] {
            let mut rig = Rig::new();
            rig.click_category(category);
            rig.tools.select(tool);
            let tiles = tiles(&mut rig);
            assert!(!tiles.is_empty(), "{category:?}/{tool:?}: no tiles");
            for (tile, icon) in tiles {
                assert_eq!(tile.size(), Sizes::tile(), "{category:?}/{tool:?}");
                assert_eq!(icon.size(), Sizes::tile_icon(), "{category:?}/{tool:?}");
                assert_eq!(icon.width(), icon.height());
            }
        }
    }

    #[test]
    fn the_top_bar_is_44_high() {
        let ctx = egui::Context::default();
        let mut sim = SimRunner::new(1);
        sim.set_content(content());
        let view = sim.view();
        let mut bar = None;
        for t in [0.0, 0.1] {
            frame(&ctx, input(t, vec![]), |ui| {
                let mut root = crate::panels::root_ui(ui.ctx());
                let rects = crate::panels::top_bar(
                    &mut root,
                    &view,
                    &mut cp_tools::CommandOutbox::default(),
                    &mut OverlayState::default(),
                    &mut cp_render::LightingSettings::default(),
                    &mut UiState::default(),
                    &mut crate::files::FileMenu::default(),
                );
                bar = rects.bar;
            });
        }
        assert_eq!(bar.unwrap().height(), Sizes::TOP_BAR_H);
    }

    #[test]
    fn categories_without_tools_are_not_drawn() {
        let rig = Rig::new();
        let drawn: Vec<_> = rig.palette.iter().map(|(c, _)| *c).collect();
        assert_eq!(
            drawn,
            vec![
                Category::Roads,
                Category::Zones,
                Category::Buildings,
                Category::Bulldozer,
                Category::Districts,
                Category::Transit
            ]
        );
        // Without content the road and building tools do not exist either.
        let none = Category::available(None);
        assert_eq!(
            none,
            vec![Category::Zones, Category::Bulldozer, Category::Districts]
        );
    }

    #[test]
    fn clicking_a_tool_activates_it_and_the_verdict_shows_for_valid_and_invalid_ghosts() {
        let mut rig = Rig::new();
        rig.click_category(Category::Roads);
        rig.click_label("Build");
        assert_eq!(rig.tools.active, ActiveTool::Road);

        rig.tools.ghost = Some(Ghost {
            polylines: vec![],
            valid: true,
            cost: Some(Cost(123_456)),
            errors: vec![],
            half_width: 0.0,
            snap: Default::default(),
            warnings: Vec::new(),
            chevrons: Vec::new(),
        });
        let text = rig.frame(vec![]);
        assert!(text.contains("can build"), "{text}");
        assert!(
            text.contains(&cp_tools::format_money(Cost(123_456))),
            "{text}"
        );

        rig.tools.ghost = Some(Ghost {
            polylines: vec![],
            valid: false,
            cost: None,
            errors: vec!["too steep (20%, max 12%)".into()],
            half_width: 6.0,
            snap: Default::default(),
            warnings: Vec::new(),
            chevrons: Vec::new(),
        });
        let text = rig.frame(vec![]);
        assert!(text.contains("too steep (20%, max 12%)"), "{text}");
        assert!(!text.contains("can build"), "{text}");
    }

    /// What the Roads panel paints (icons are images; their texts here are the "⚠" egui draws
    /// for an image without a loader). Run with `--nocapture` to see the dump.
    #[test]
    fn the_roads_panel_paints_its_sections() {
        let mut rig = Rig::new();
        rig.click_category(Category::Roads);
        rig.click_label("Build");
        rig.tools.ghost = Some(Ghost {
            polylines: vec![],
            valid: true,
            cost: Some(Cost(123_456)),
            errors: vec![],
            half_width: 0.0,
            snap: Default::default(),
            warnings: Vec::new(),
            chevrons: Vec::new(),
        });
        let texts = rig.frame_texts(vec![]);
        for (text, at) in &texts {
            println!("({:6.1}, {:6.1})  {text}", at.x, at.y);
        }
        let all: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        for want in [
            "Roads",
            "Tools",
            "Build",
            "Mode",
            "Straight",
            "Curve",
            "Free",
            "▾ Road type",
            "Elevation (PgUp / PgDn)",
        ] {
            assert!(all.contains(&want), "{want:?} missing from {all:?}");
        }
        assert!(all.iter().any(|t| t.contains("can build")));
    }

    #[test]
    fn the_verdict_is_pinned_below_the_content() {
        let mut rig = Rig::new();
        rig.click_category(Category::Roads);
        rig.tools.select(ActiveTool::Road);
        rig.tools.ghost = Some(Ghost {
            polylines: vec![],
            valid: false,
            cost: None,
            errors: vec!["PINNED-VERDICT".into()],
            half_width: 6.0,
            snap: Default::default(),
            warnings: Vec::new(),
            chevrons: Vec::new(),
        });
        rig.frame(vec![]);
        // The panel fills the height under the top bar; the verdict is in its lower part.
        let mut at = None;
        rig.t += 0.1;
        let Rig {
            ctx,
            state,
            tools,
            overlay,
            view,
            t,
            ..
        } = &mut rig;
        let mut out = ctx.run_ui(input(*t, vec![]), |ui| {
            let mut root = crate::panels::root_ui(ui.ctx());
            tool_panel(
                &mut root,
                state,
                Some(view),
                tools,
                overlay,
                &mut CommandOutbox::default(),
            );
        });
        out.textures_delta.clear();
        fn walk(s: &egui::epaint::Shape, at: &mut Option<f32>) {
            match s {
                egui::epaint::Shape::Text(t) if t.galley.text().contains("PINNED-VERDICT") => {
                    *at = Some(t.pos.y);
                }
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, at)),
                _ => {}
            }
        }
        for c in &out.shapes {
            walk(&c.shape, &mut at);
        }
        let y = at.expect("verdict text painted");
        assert!(y > 720.0 * 0.7, "verdict at y={y} is near the bottom");
    }

    #[test]
    fn each_category_panel_draws_and_its_tools_work() {
        let mut rig = Rig::new();
        for c in Category::available(rig.view.content.as_deref()) {
            rig.click_category(c);
            let text = rig.frame(vec![]);
            assert!(text.contains(c.label()), "{c:?}: {text}");
        }
        rig.click_category(Category::Bulldozer);
        assert_eq!(rig.state.category, Some(Category::Bulldozer));
        rig.click_label("Bulldozer");
        assert_eq!(rig.tools.active, ActiveTool::Bulldozer);
    }

    #[test]
    fn digit_keys_open_the_categories_in_palette_order() {
        let ctx = egui::Context::default();
        let available = Category::available(Some(&content()));
        assert_eq!(
            available,
            [
                Category::Roads,
                Category::Zones,
                Category::Buildings,
                Category::Bulldozer,
                Category::Districts,
                Category::Transit
            ]
        );
        let keys = [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
        ];
        for (n, k) in keys.into_iter().enumerate() {
            let mut got = None;
            frame(&ctx, input(n as f64, vec![key(k)]), |ui| {
                got = hotkey(ui.ctx(), &available);
            });
            assert_eq!(got, Some(available[n]));
        }
        // Past the last category, and with a modifier held, nothing happens.
        let mut got = Some(Category::Roads);
        frame(&ctx, input(10.0, vec![key(egui::Key::Num7)]), |ui| {
            got = hotkey(ui.ctx(), &available);
        });
        assert_eq!(got, None);
        let ctrl_one = egui::Event::Key {
            key: egui::Key::Num1,
            physical_key: Some(egui::Key::Num1),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        };
        let with_ctrl = input(11.0, vec![ctrl_one]);
        let mut got = Some(Category::Roads);
        frame(&ctx, with_ctrl, |ui| {
            got = hotkey(ui.ctx(), &available);
        });
        assert_eq!(got, None);
    }

    /// T-102: the Buildings panel has a section per service kind (water and sewage share one)
    /// in kind order, each a row of tiles: the network tool, the buildings, remove.
    #[test]
    fn the_buildings_panel_has_water_then_power_sections_with_network_tools_beside_the_buildings() {
        use cp_content::ServiceKind;
        let mut rig = Rig::new();
        rig.screen = egui::vec2(1280.0, 3000.0);
        rig.click_category(Category::Buildings);
        let content = rig.view.content.clone().unwrap();
        let sections = crate::buildings::sections(&content);
        let kinds: Vec<ServiceKind> = sections.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            [
                ServiceKind::Water,
                ServiceKind::Power,
                ServiceKind::Garbage,
                ServiceKind::Health,
                ServiceKind::Deathcare,
                ServiceKind::Fire,
                ServiceKind::Police,
                ServiceKind::Education,
            ],
            "the depot is in the Transport category"
        );
        // Water: the pipe, every Water and Sewage service building, remove; Power likewise.
        let service = |kinds: &[ServiceKind]| {
            content
                .buildings()
                .filter(|(_, d)| {
                    d.service
                        .as_deref()
                        .and_then(|s| content.service_id(s))
                        .is_some_and(|s| kinds.contains(&content.service(s).kind))
                })
                .count()
        };
        assert_eq!(
            sections[0].cards.len(),
            service(&[ServiceKind::Water, ServiceKind::Sewage])
        );
        assert_eq!(sections[1].cards.len(), service(&[ServiceKind::Power]));
        let tiles = tiles(&mut rig);
        // The networks add their tool and remove; the Garbage section is just its buildings.
        let expect: usize = sections
            .iter()
            .map(|s| s.cards.len() + if s.net.is_some() { 2 } else { 0 })
            .sum();
        assert_eq!(tiles.len(), expect);
        let texts = rig.frame_texts(vec![]);
        let names: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        for want in [
            "Water",
            "Power",
            "Pipe",
            "Water pump",
            "Sewage outflow",
            "Remove pipe",
            "Power line",
            "Coal power plant",
            "Wind turbine",
            "Remove line",
        ] {
            assert!(names.contains(&want), "{want:?} missing from {names:?}");
        }
    }

    #[test]
    fn the_tiles_of_the_buildings_panel_set_the_tools_and_the_overlay() {
        use cp_render::Overlay;
        use cp_utility::UtilityNet;
        let mut rig = Rig::new();
        rig.screen = egui::vec2(1280.0, 3000.0);
        rig.click_category(Category::Buildings);
        rig.click_label("Water pump");
        let pump = rig
            .view
            .content
            .as_ref()
            .unwrap()
            .building_id("water_pump_station");
        assert_eq!(rig.tools.active, ActiveTool::Place);
        assert_eq!(rig.tools.building.def, pump);
        assert_eq!(rig.overlay.active, Overlay::Water);

        rig.overlay.active = Overlay::Off;
        rig.click_label("Pipe");
        assert_eq!(rig.tools.active, ActiveTool::Utility);
        assert_eq!(rig.tools.utility.kind, cp_content::UtilityKind::Water);
        assert_eq!(rig.overlay.active, Overlay::Water);

        rig.click_label("Remove line");
        assert_eq!(rig.tools.active, ActiveTool::UtilityRemove);
        assert_eq!(rig.tools.remove_net, Some(UtilityNet::Power));
        assert_eq!(rig.overlay.active, Overlay::Power);

        rig.click_label("Coal power plant");
        assert_eq!(rig.tools.active, ActiveTool::Place);
        assert_eq!(rig.overlay.active, Overlay::Power);
    }

    #[test]
    fn the_site_plate_names_the_site_of_the_selected_building() {
        use cp_content::ServiceSite;
        let mut rig = Rig::new();
        rig.screen = egui::vec2(1280.0, 3000.0);
        rig.click_category(Category::Buildings);
        for (label, site) in [
            ("Water pump", ServiceSite::OnWater),
            ("Sewage outflow", ServiceSite::Shore),
            ("Wind turbine", ServiceSite::Land),
        ] {
            rig.click_label(label);
            let text = rig.frame(vec![]);
            let note = crate::buildings::site_note(site);
            assert!(text.contains(&note), "{label}: {note:?} not in {text}");
        }
        assert_ne!(
            crate::buildings::site_note(ServiceSite::OnWater),
            crate::buildings::site_note(ServiceSite::Shore)
        );
        assert!(crate::buildings::site_note(ServiceSite::Shore).contains("16"));
    }

    #[test]
    fn the_verdict_for_a_missing_water_site_differs_for_the_pump_and_the_outflow() {
        use cp_building::BuildingError;
        use cp_content::ServiceSite;
        let pump = cp_tools::error_text(&BuildingError::NeedsWater, ServiceSite::OnWater);
        let outflow = cp_tools::error_text(&BuildingError::NeedsWater, ServiceSite::Shore);
        assert_ne!(pump, outflow);
        assert!(pump.contains("water") && outflow.contains("shore"));
        let wet = cp_tools::error_text(&BuildingError::Water, ServiceSite::Land);
        assert!(wet.contains("land only"), "{wet}");
        assert_eq!(
            cp_tools::error_text(&BuildingError::NoRoad, ServiceSite::Land),
            BuildingError::NoRoad.to_string()
        );
    }

    #[test]
    fn digit_keys_do_not_collide_with_camera_or_tool_keys() {
        // Camera: WASD, Q/E, R/F; tools: T, Enter, Esc, PgUp/PgDn; UI: F3. Digits are free.
        let digits = [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3];
        let others = [
            egui::Key::W,
            egui::Key::A,
            egui::Key::S,
            egui::Key::D,
            egui::Key::Q,
            egui::Key::E,
            egui::Key::R,
            egui::Key::F,
            egui::Key::T,
            egui::Key::Enter,
            egui::Key::Escape,
            egui::Key::PageUp,
            egui::Key::PageDown,
            egui::Key::F3,
        ];
        for d in digits {
            assert!(!others.contains(&d));
        }
    }

    /// A rig whose view has one district ("Hill", id 1) with the given ResLow override.
    fn rig_with_district(resl_tax: Option<u8>) -> Rig {
        use cp_district::{DistrictCommand, DistrictId};
        use cp_sim::SimCommand;
        let mut sim = SimRunner::new(1);
        sim.set_content(content());
        let cmds = [
            DistrictCommand::Create {
                name: "Hill".into(),
            },
            DistrictCommand::SetTax {
                district: DistrictId(1),
                zone: cp_content::ZoneType::ResLow,
                pct: resl_tax,
            },
        ];
        for c in cmds {
            sim.push_command(SimCommand::District(c), sim.tick());
            sim.tick_once();
        }
        let mut rig = Rig::new();
        rig.view = sim.view();
        rig.click_category(Category::Districts);
        rig.tools.district.selected = Some(DistrictId(1));
        rig.frame(vec![]);
        rig
    }

    #[test]
    fn the_districts_category_is_the_fifth_and_new_district_creates_and_selects_one() {
        use cp_district::{DistrictCommand, DistrictId};
        use cp_sim::SimCommand;
        let mut rig = Rig::new();
        assert_eq!(
            Category::available(rig.view.content.as_deref())[4],
            Category::Districts
        );
        rig.click_category(Category::Districts);
        rig.click_label("New district");
        assert_eq!(
            rig.outbox.iter().cloned().collect::<Vec<_>>(),
            vec![SimCommand::District(DistrictCommand::Create {
                name: "District 1".into()
            })]
        );
        assert_eq!(rig.tools.district.selected, Some(DistrictId(1)));
        assert_eq!(rig.tools.active, ActiveTool::District);
    }

    #[test]
    fn the_own_rate_switch_sends_set_tax_some_and_none() {
        use cp_district::{DistrictCommand, DistrictId};
        use cp_sim::SimCommand;
        let set = |pct| {
            SimCommand::District(DistrictCommand::SetTax {
                district: DistrictId(1),
                zone: cp_content::ZoneType::ResLow,
                pct,
            })
        };
        // Off -> on: starts at the city rate (9 %).
        let mut rig = rig_with_district(None);
        rig.click_label("R low");
        assert_eq!(
            rig.outbox.iter().cloned().collect::<Vec<_>>(),
            vec![set(Some(9))]
        );
        // On -> off.
        let mut rig = rig_with_district(Some(20));
        rig.click_label("R low");
        assert_eq!(
            rig.outbox.iter().cloned().collect::<Vec<_>>(),
            vec![set(None)]
        );
    }
}
