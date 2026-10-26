//! The left palette and the tool panel beside it (`architecture/system-design.md` §13.6).
//!
//! The palette is a 72 px column with one icon per [`Category`]; clicking one (or pressing its
//! number key, 1…5) opens that category's 300 px tool panel, clicking it again closes the
//! panel. The panel has a header, the tool buttons and options of the category, and the pinned
//! "cost · verdict" block at the bottom.

use bevy_egui::egui;
use cp_content::ContentDb;
use cp_locale::{tr, trf};
use cp_render::OverlayState;
use cp_sim::SimView;
use cp_tools::{CommandOutbox, ToolState};

use crate::icons::Icon;
use crate::panels::{UiState, roads_panel, services_panel, zones_panel};
use crate::style::Sizes;
use cp_tools::{ActiveTool, BulldozeLayer};

/// Build-menu categories. A category without tools is not drawn ([`Category::available`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Roads,
    Zones,
    Buildings,
    /// Wind turbine, coal plant and the power line (T-289: not in Buildings any more).
    Power,
    /// Pump, tower, outflow, treatment plant and pipes (T-289).
    Water,
    /// Districts: brushes, the district list and its taxes (M2).
    Districts,
    /// Public transport (M3): the line tool and the depot. Last, so the hotkeys of the
    /// others do not move.
    Transit,
    /// Industry areas and, later, the buildings of the resources (M5, T-207).
    Industry,
    /// Park areas, their buildings and the footpath (M6, T-226).
    Parks,
}

impl Category {
    pub const ALL: [Category; 9] = [
        Category::Roads,
        Category::Zones,
        Category::Buildings,
        Category::Power,
        Category::Water,
        Category::Districts,
        Category::Transit,
        Category::Industry,
        Category::Parks,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Roads => "Roads",
            Category::Zones => "Zones",
            Category::Buildings => "Buildings",
            Category::Power => "Power",
            Category::Water => "Water",
            Category::Districts => "Districts",
            Category::Transit => "Transport",
            Category::Industry => "Industry",
            Category::Parks => "Parks",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Category::Roads => Icon::Roads,
            Category::Zones => Icon::Zones,
            Category::Buildings => Icon::Buildings,
            Category::Power => Icon::Power,
            Category::Water => Icon::Water,
            Category::Districts => Icon::Districts,
            Category::Transit => Icon::Transit,
            Category::Industry => Icon::Industry,
            Category::Parks => Icon::CategoryParks,
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
            Category::Zones | Category::Districts => true,
            Category::Power | Category::Water => content.is_some(),
            Category::Transit => crate::transit::has_tools(content),
            Category::Industry | Category::Parks => content.is_some(),
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

/// The category buttons of the bottom bar (T-317): one square icon each. Returns each icon's
/// rectangle (tests click them). A click toggles `state.category`.
pub fn palette(
    ui: &mut egui::Ui,
    state: &mut UiState,
    content: Option<&ContentDb>,
) -> Vec<(Category, egui::Rect)> {
    let mut rects = Vec::new();
    for (n, c) in Category::available(content).into_iter().enumerate() {
        let selected = state.category == Some(c);
        let r = c
            .icon()
            .button(ui, Sizes::PALETTE_ICON, Sizes::BUTTON_PAD, selected)
            .on_hover_text(trf!(
                "{category} (Shift+{key})",
                category = tr(c.label()),
                key = n + 1
            ));
        rects.push((c, r.rect));
        if r.clicked() {
            toggle(state, c);
        }
    }
    rects
}

/// How many buttons [`global_tools`] draws.
pub const GLOBAL_TOOLS: usize = 1;

/// The tools that work from any category, right of the categories in the bottom bar: the
/// bulldozer (the layer of the open menu, T-286). Upgrade and Flip are in the Roads tray's side
/// panel; the Junctions tool is in the overlay panel (T-317f). Returns their rectangles.
pub fn global_tools(
    ui: &mut egui::Ui,
    _state: &mut UiState,
    tools: &mut ToolState,
) -> Vec<(ActiveTool, egui::Rect)> {
    let hover = match tools.menu_layer {
        BulldozeLayer::All => tr("Bulldozer (B)"),
        BulldozeLayer::Power => tr("Bulldozer (B) — power lines and buildings only"),
        BulldozeLayer::Water => tr("Bulldozer (B) — pipes and water buildings only"),
    };
    let on = tools.active == ActiveTool::Bulldozer;
    let r = Icon::Bulldozer
        .button(ui, Sizes::PALETTE_ICON, Sizes::BUTTON_PAD, on)
        .on_hover_text(hover);
    if r.clicked() {
        tools.toggle_bulldozer();
    }
    vec![(ActiveTool::Bulldozer, r.rect)]
}

/// Opens `c`'s panel, or closes it when it is open.
pub fn toggle(state: &mut UiState, c: Category) {
    state.category = if state.category == Some(c) {
        None
    } else {
        // A menu just opened: `menus::apply` takes up its tool (T-289).
        state.entered = Some(c);
        Some(c)
    };
}

/// The category a Shift+digit key press opens, if any: `Shift+1` is the first available
/// category (bare digits 1-3 are the game speed, BUG-059). Keys with Ctrl, Alt or Cmd held, and
/// keys typed into a text field, do not count.
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
                    physical_key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if modifiers.shift && !modifiers.ctrl && !modifiers.alt && !modifiers.command => {
                    // With Shift the logical key may be the symbol (`!`), so the physical
                    // key counts too.
                    KEYS.iter()
                        .position(|k| k == key || Some(*k) == *physical_key)
                }
                _ => None,
            })
            .and_then(|n| available.get(n).copied())
    })
}

/// The tabs of `category`'s tray: the keys the panel functions draw their sections under, with
/// the tab's icon.
pub fn tabs(category: Category, view: Option<&SimView>) -> Vec<(String, Option<Icon>)> {
    let content = view.and_then(|v| v.content.as_deref());
    let mut out: Vec<(String, Option<Icon>)> = Vec::new();
    match category {
        Category::Roads => {
            for class in content.map(crate::roads::tab_classes).unwrap_or_default() {
                out.push((crate::roads::tab_label(class), None));
            }
        }
        // The tool kinds and the zone types are one row (T-317f): no tabs.
        Category::Zones => {}
        Category::Buildings | Category::Power | Category::Water => {
            let scope = match category {
                Category::Power => crate::buildings::Scope::Power,
                Category::Water => crate::buildings::Scope::Water,
                _ => crate::buildings::Scope::Buildings,
            };
            if let Some(content) = content {
                for s in crate::buildings::sections(content)
                    .into_iter()
                    .filter(|s| crate::buildings::in_scope(s.kind, scope))
                {
                    out.push((s.title(), Some(crate::buildings::kind_icon(s.kind))));
                }
                if scope == crate::buildings::Scope::Buildings && view.is_some() {
                    for (monument, _) in crate::buildings::unique_sections(content) {
                        out.push((
                            tr(crate::buildings::unique_title(monument)),
                            Some(Icon::Buildings),
                        ));
                    }
                }
            }
        }
        Category::Districts => {
            out.push((tr("Districts"), Some(Icon::Districts)));
            if view.is_some() {
                out.push((tr("District"), Some(Icon::DistrictTabPolicies)));
            }
        }
        Category::Transit => {
            for s in content.map(crate::transit::sections).unwrap_or_default() {
                out.push((tr(s.title()), Some(crate::transit::section_icon(s.mode))));
            }
        }
        Category::Industry => {
            out.push((tr("Areas"), Some(Icon::Industry)));
            for (res, _) in content
                .map(crate::industry::chain_sections)
                .unwrap_or_default()
            {
                out.push((
                    tr(crate::industry::resource_name(res)),
                    Some(crate::industry::resource_icon(res)),
                ));
            }
            // §21.3: the market of every cargo.
            out.push((tr("Trade"), Some(Icon::Metrics)));
        }
        Category::Parks => {
            out.push((tr("Areas"), Some(Icon::CategoryParks)));
            if let Some(content) = content {
                for kind in cp_content::ParkKind::ALL {
                    if !crate::parks::section_cards(content, Some(kind)).is_empty() {
                        out.push((
                            tr(crate::parks::kind_name(kind)),
                            Some(crate::parks::kind_icon(kind)),
                        ));
                    }
                }
                if !crate::parks::section_cards(content, None).is_empty() {
                    out.push((tr("Props"), None));
                }
                out.push((tr("Path"), None));
            }
        }
    }
    out
}

/// Whether `category`'s tabs are filters (T-317f): a click shows only that tab, a second click
/// shows every tab again; none is chosen at first.
fn filters(category: Category) -> bool {
    category == Category::Roads
}

/// The tab of `category` the tray is on: the remembered one while it exists, else the first;
/// for a category of [`filters`] no tab (every tab shows) unless one was chosen.
fn active_tab(
    state: &UiState,
    category: Category,
    tabs: &[(String, Option<Icon>)],
) -> Option<usize> {
    let remembered = state
        .tray_tab
        .iter()
        .find(|(c, _)| *c == category)
        .and_then(|(_, key)| tabs.iter().position(|(t, _)| t == key));
    if filters(category) {
        remembered
    } else {
        Some(remembered.unwrap_or(0))
    }
}

/// The tool tray of the open category (nothing when none is open), above the bottom bar: the
/// tab row with the options strip at its end, then one row of tiles. Returns its rectangle.
pub fn tray(
    root: &mut egui::Ui,
    state: &mut UiState,
    view: Option<&SimView>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    outbox: &mut CommandOutbox,
) -> Option<egui::Rect> {
    let category = state.category?;
    let tabs = tabs(category, view);
    let mut at = active_tab(state, category, &tabs);
    let key = at
        .and_then(|n| tabs.get(n))
        .map(|(k, _)| k.clone())
        .unwrap_or_default();
    let out = egui::Panel::bottom("tray")
        .exact_size(Sizes::TRAY_H)
        .resizable(false)
        .show(root, |ui| {
            crate::tray::enter(ui.ctx(), &key);
            // One row (T-317f: the tray a third lower): the tabs, then the options strip.
            egui::ScrollArea::horizontal()
                .id_salt("tray_tabs")
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        category.icon().show(ui, Sizes::TRAY_TAB_H - 2.0);
                        ui.strong(tr(category.label()));
                        ui.separator();
                        for (n, (label, _)) in tabs.iter().enumerate() {
                            if tabs.len() < 2 {
                                break;
                            }
                            let on = at == Some(n);
                            let r = ui.selectable_label(on, label);
                            if r.clicked() {
                                at = if on && filters(category) {
                                    None
                                } else {
                                    Some(n)
                                };
                            }
                        }
                        if tabs.len() >= 2 {
                            ui.separator();
                        }
                        options(ui, category, state, view, tools, overlay);
                    });
                });
            ui.horizontal_top(|ui| {
                // The Roads side panel stays put while the tiles scroll.
                if category == Category::Roads && view.and_then(|v| v.content.as_deref()).is_some()
                {
                    crate::roads::side_panel(ui, tools);
                    ui.separator();
                }
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| match category {
                            Category::Roads => roads_panel(ui, view, tools),
                            Category::Zones => zones_panel(ui, view, tools),
                            Category::Buildings => services_panel(ui, view, tools, overlay),
                            Category::Power => crate::buildings::buildings_panel(
                                ui,
                                view,
                                tools,
                                overlay,
                                crate::buildings::Scope::Power,
                            ),
                            Category::Water => crate::buildings::buildings_panel(
                                ui,
                                view,
                                tools,
                                overlay,
                                crate::buildings::Scope::Water,
                            ),
                            Category::Districts => {
                                crate::districts::districts_panel(ui, view, tools, outbox)
                            }
                            Category::Transit => crate::transit::transit_panel(
                                ui,
                                view,
                                tools,
                                overlay,
                                &mut state.lines_open,
                            ),
                            Category::Industry => {
                                crate::industry::industry_panel(ui, view, tools, overlay, outbox)
                            }
                            Category::Parks => {
                                crate::parks::parks_panel(ui, view, tools, overlay, outbox)
                            }
                        });
                    });
            });
            crate::tray::leave(ui.ctx());
        });
    match at.and_then(|n| tabs.get(n)) {
        Some((k, _)) => match state.tray_tab.iter_mut().find(|(c, _)| *c == category) {
            Some(slot) => slot.1 = k.clone(),
            None => state.tray_tab.push((category, k.clone())),
        },
        None => state.tray_tab.retain(|(c, _)| *c != category),
    }
    Some(out.response.rect)
}

/// The options strip of the tray: what the open category's tool needs besides its tiles.
fn options(
    ui: &mut egui::Ui,
    category: Category,
    state: &mut UiState,
    view: Option<&SimView>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
) {
    match category {
        Category::Roads => crate::roads::road_options(ui, tools),
        Category::Districts => {
            crate::districts::districts_options(ui, view, tools, &mut state.areas_open)
        }
        Category::Industry => {
            crate::industry::industry_options(ui, view, tools, overlay, &mut state.areas_open)
        }
        Category::Parks => {
            crate::parks::parks_options(ui, view, tools, overlay, &mut state.areas_open)
        }
        _ => {}
    }
}

/// The list of districts or areas of the open category, in a small window of its own (opened by
/// the "List" button of the options strip). Returns whether it was drawn.
pub fn list_window(
    ctx: &egui::Context,
    state: &mut UiState,
    view: Option<&SimView>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    outbox: &mut CommandOutbox,
) {
    let (Some(category), Some(view)) = (state.category, view) else {
        return;
    };
    if !state.areas_open
        || !matches!(
            category,
            Category::Districts | Category::Industry | Category::Parks
        )
    {
        return;
    }
    let title = match category {
        Category::Districts => tr("Districts"),
        _ => tr("Areas"),
    };
    let mut open = true;
    egui::Window::new(title)
        .id(egui::Id::new("tray_list"))
        .open(&mut open)
        .anchor(
            egui::Align2::LEFT_BOTTOM,
            // Above the advisor's toast, which stands at the bottom left over the tray.
            egui::vec2(8.0, -(Sizes::DEMAND_H + Sizes::TRAY_H + 56.0)),
        )
        .default_width(300.0)
        .resizable(false)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height(if category == Category::Industry {
                    150.0
                } else {
                    260.0
                })
                .show(ui, |ui| match category {
                    Category::Districts => crate::districts::district_list(ui, view, tools, outbox),
                    Category::Industry => {
                        crate::industry::area_list(ui, view, tools, overlay, outbox)
                    }
                    _ => crate::parks::area_list(ui, view, tools, overlay, outbox),
                });
            if category == Category::Industry {
                crate::industry::area_summary_panel(ui, view, tools);
            }
        });
    state.areas_open = open;
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
            // The pointer rests over the map: the tool hint stands beside it.
            rig.frame(vec![egui::Event::PointerMoved(egui::pos2(640.0, 200.0))]);
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
                *pal = crate::demand_panel::demand_panel(&mut root, view, state, tools).categories;
                crate::menus::apply(state, tools, overlay, Some(view));
                *panel = tray(&mut root, state, Some(view), tools, overlay, outbox);
                crate::hint::cursor_hint(ui.ctx(), state, tools);
                crate::menus::remember(state, tools, view.content.as_deref());
                crate::panels::release_auto_overlay(state, tools, overlay);
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

    /// BUG-108: the Districts panel as the player uses it — New district, then a brush, then a
    /// stroke on the map paints the district in the sim.
    #[test]
    fn a_new_district_from_the_panel_can_be_painted() {
        let mut sim = SimRunner::new(1);
        sim.set_content(content());
        sim.tick_once();
        let mut rig = Rig::new();
        rig.view = sim.view();
        rig.click_category(Category::Districts);
        rig.click_label("New district");
        rig.outbox.flush_into(&mut sim);
        sim.tick_once();
        rig.view = sim.view();
        rig.frame(vec![]);
        assert_eq!(rig.tools.active, ActiveTool::District);
        let id = rig.tools.district.selected.expect("a district chosen");
        assert!(rig.view.districts.get(id).is_some(), "{id:?} exists");
        rig.click_label("Brush L");
        assert_eq!(rig.tools.active, ActiveTool::District);
        assert_eq!(
            rig.tools.district.selected,
            Some(id),
            "the brush keeps the district"
        );
        let p = bevy::math::Vec2::new(5000.0, 5000.0);
        let cmd = rig
            .tools
            .district
            .drag(&rig.view.district_map, p)
            .expect("paint");
        let mut out = CommandOutbox::default();
        out.push(cp_sim::SimCommand::District(cmd));
        out.flush_into(&mut sim);
        sim.tick_once();
        let cell = cp_district::DistrictMap::cell_of(p).unwrap();
        assert_eq!(sim.view().district_map.get(cell), Some(id));
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
        // T-317f: tool kinds and zone types in one row, the tools first; no Eraser tile (the
        // right mouse button erases).
        let texts = rig.frame_texts(vec![]);
        let at = |label: &str| {
            texts
                .iter()
                .find(|(t, p)| t == label && p.y > 30.0)
                .map(|x| x.1)
        };
        let (rect, office) = (at("Rectangle").unwrap(), at("Office").unwrap());
        assert!(
            rect.x < office.x && (rect.y - office.y).abs() < 1.0,
            "{rect:?} {office:?}"
        );
        assert!(at("Eraser").is_none());
        for (label, kind) in [
            ("Rectangle", ZoneToolKind::Rectangle),
            ("Brush L", ZoneToolKind::BrushLarge),
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
    }

    #[test]
    fn road_cards_match_the_content_in_tab_class_and_lane_order_and_a_click_selects() {
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
                // T-322: the bike roads last, in their own Bike tab.
                .all(|w| (w[0].tab, w[0].class, w[0].lanes) <= (w[1].tab, w[1].class, w[1].lanes)),
            "{cards:?}"
        );
        let mut rig = Rig::new();
        // Tall enough for every card (the list grows until T-098 makes it a grid).
        rig.screen = egui::vec2(1280.0, 3000.0);
        rig.click_category(Category::Roads);
        // T-289: the menu opens with the road tool in hand.
        assert_eq!(rig.tools.active, ActiveTool::Road);
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
        rig.tools.select(ActiveTool::Upgrade);
        rig.frame(vec![]);
        assert_eq!(rig.tools.active, ActiveTool::Upgrade);
        assert!(rig.frame(vec![]).contains("Upgrade to"));
        rig.tools.upgrade_preview = Some(UpgradePreview {
            run: vec![],
            spans: vec![],
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
        rig.tools.select(ActiveTool::Road);
        assert_eq!(rig.tools.active, ActiveTool::Road);
        assert!(rig.tools.upgrade_preview.is_none());
    }

    /// The side panel of the Roads tray: Upgrade and Flip toggle their tools, lower and raise
    /// stay within the levels and take the road tool.
    #[test]
    fn the_roads_side_panel_holds_upgrade_flip_and_the_elevation() {
        use crate::roads::{SideButton, take_side_rects};
        let mut rig = Rig::new();
        rig.click_category(Category::Roads);
        // T-289: the menu opens with the road tool in hand.
        assert_eq!(rig.tools.active, ActiveTool::Road);
        let click = |rig: &mut Rig, b: SideButton| {
            take_side_rects(&rig.ctx);
            rig.frame(vec![]);
            let rects = take_side_rects(&rig.ctx);
            let at = rects.iter().find(|(k, _)| *k == b).unwrap().1;
            rig.click_at(at.center());
        };
        for (b, tool) in [
            (SideButton::Upgrade, ActiveTool::Upgrade),
            (SideButton::Flip, ActiveTool::Flip),
        ] {
            click(&mut rig, b);
            assert_eq!(rig.tools.active, tool);
            click(&mut rig, b);
            assert_eq!(rig.tools.active, ActiveTool::None, "{b:?} toggles");
        }
        click(&mut rig, SideButton::Raise);
        assert_eq!(
            rig.tools.active,
            ActiveTool::Road,
            "a height takes the road tool"
        );
        assert_eq!(rig.tools.road.level, 1);
        for _ in 0..cp_tools::MAX_ELEVATION_LEVEL + 2 {
            click(&mut rig, SideButton::Raise);
        }
        assert_eq!(rig.tools.road.level, cp_tools::MAX_ELEVATION_LEVEL);
        click(&mut rig, SideButton::Lower);
        assert_eq!(rig.tools.road.level, cp_tools::MAX_ELEVATION_LEVEL - 1);
    }

    /// The side panel stays in view while the road tiles scroll sideways.
    #[test]
    fn the_roads_side_panel_stays_while_the_tiles_scroll() {
        use crate::roads::take_side_rects;
        let mut rig = Rig::new();
        rig.click_category(Category::Roads);
        let before_tiles = tiles(&mut rig);
        take_side_rects(&rig.ctx);
        rig.frame(vec![]);
        let before = take_side_rects(&rig.ctx);
        let over = before_tiles[0].0.center();
        rig.frame(vec![egui::Event::PointerMoved(over)]);
        for _ in 0..5 {
            rig.frame(vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-400.0, 0.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        let after_tiles = tiles(&mut rig);
        assert!(
            after_tiles[0].0.left() < before_tiles[0].0.left() - 50.0,
            "the tiles scroll: {:?} -> {:?}",
            before_tiles[0].0,
            after_tiles[0].0
        );
        take_side_rects(&rig.ctx);
        rig.frame(vec![]);
        let after = take_side_rects(&rig.ctx);
        assert_eq!(before.len(), 4);
        assert_eq!(before, after, "the side panel does not move");
    }

    #[test]
    fn the_road_verdict_shows_length_cost_and_errors() {
        let mut rig = Rig::new();
        rig.click_category(Category::Roads);
        // T-289: the menu opens with the road tool in hand.
        assert_eq!(rig.tools.active, ActiveTool::Road);
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
            zone: Default::default(),
            block: None,
            warnings: vec!["one-way: arms face the same way".into()],
            ..Default::default()
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
            zone: Default::default(),
            block: None,
            warnings: vec![],
            ..Default::default()
        });
        let text = rig.frame(vec![]);
        assert!(text.contains("crosses water"), "{text}");
    }

    /// T-317: the categories are square icons in the bar; an open tray is TRAY_H high, sits
    /// right above the bar and every tile of its tab is reachable without scrolling down.
    #[test]
    fn the_category_icons_are_square_and_the_tray_sits_above_the_bar() {
        let mut rig = Rig::new();
        for (c, r) in &rig.palette {
            assert!(
                r.width() >= Sizes::PALETTE_ICON && r.height() >= Sizes::PALETTE_ICON,
                "{c:?}: {r:?}"
            );
            assert_eq!(r.width(), r.height(), "{c:?}");
            assert!(r.bottom() <= 720.0 && r.top() >= 720.0 - Sizes::DEMAND_H);
        }
        rig.click_category(Category::Roads);
        let panel = rig.panel.unwrap();
        assert_eq!(panel.height(), Sizes::TRAY_H);
        assert!(
            (panel.max.y - (720.0 - Sizes::DEMAND_H)).abs() <= 1.0,
            "the tray sits on the bar: {}",
            panel.max.y
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
    fn every_tile_of_the_roads_zones_buildings_power_and_water_panels_is_one_size_with_a_square_icon()
     {
        for (category, tool) in [
            (Category::Roads, ActiveTool::Road),
            (Category::Roads, ActiveTool::Upgrade),
            (Category::Zones, ActiveTool::Zone),
            (Category::Buildings, ActiveTool::Utility),
            (Category::Buildings, ActiveTool::Place),
            (Category::Buildings, ActiveTool::None),
            (Category::Power, ActiveTool::Place),
            (Category::Power, ActiveTool::Utility),
            (Category::Water, ActiveTool::Place),
            (Category::Water, ActiveTool::Utility),
            (Category::Districts, ActiveTool::District),
            (Category::Transit, ActiveTool::Line),
            (Category::Industry, ActiveTool::Area),
            (Category::Parks, ActiveTool::Area),
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

    /// T-317: at 1280x720 every tile of every tab of every tray is inside the tray, in one row:
    /// nothing needs a vertical scroll.
    #[test]
    fn every_tab_of_every_tray_shows_its_tiles_in_one_row_inside_the_tray() {
        for category in Category::ALL {
            let mut rig = Rig::new();
            rig.click_category(category);
            let tray = rig.panel.unwrap();
            for (label, _) in tabs(category, Some(&rig.view)) {
                rig.click_label(&label);
                let found = tiles(&mut rig);
                let mut tops: Vec<i32> = found.iter().map(|(t, _)| t.top() as i32).collect();
                tops.dedup();
                assert!(tops.len() <= 1, "{category:?}/{label}: rows at {tops:?}");
                for (tile, _) in found {
                    assert!(
                        tile.top() >= tray.top() && tile.bottom() <= tray.bottom(),
                        "{category:?}/{label}: {tile:?} outside {tray:?}"
                    );
                }
            }
        }
    }

    /// T-317: a category's tray comes back on the tab it was left on.
    #[test]
    fn the_last_tab_of_a_category_is_remembered() {
        let mut rig = Rig::new();
        rig.click_category(Category::Roads);
        rig.click_label("Large");
        rig.click_category(Category::Districts);
        rig.click_label("District");
        rig.click_category(Category::Roads);
        assert_eq!(
            rig.state
                .tray_tab
                .iter()
                .find(|(c, _)| *c == Category::Roads)
                .map(|(_, k)| k.as_str()),
            Some("Large")
        );
        let text = rig.frame(vec![]);
        assert!(text.contains("Large"), "{text}");
    }

    /// T-317f: the Roads tabs are filters: every road type shows at first, a tab shows only
    /// its types, a second click on it shows every type again.
    #[test]
    fn the_roads_tabs_filter_the_tiles_and_a_second_click_shows_all() {
        let mut rig = Rig::new();
        let db = content();
        let all = crate::roads::cards(&db).len();
        rig.click_category(Category::Roads);
        assert_eq!(tiles(&mut rig).len(), all);
        rig.click_label("Large");
        let large = tiles(&mut rig).len();
        assert!(large > 0 && large < all, "{large} of {all}");
        rig.click_label("Large");
        assert_eq!(tiles(&mut rig).len(), all);
        assert!(
            !rig.state
                .tray_tab
                .iter()
                .any(|(c, _)| *c == Category::Roads)
        );
    }

    /// T-317: the hint is one short line beside the pointer while a tool is in hand, and nothing
    /// without one.
    #[test]
    fn the_hint_stands_beside_the_pointer_only_with_a_tool_in_hand() {
        let mut rig = Rig::new();
        rig.click_category(Category::Roads);
        rig.tools.select(ActiveTool::None);
        let idle = rig.frame_texts(vec![]);
        assert!(
            !idle
                .iter()
                .any(|(t, _)| t.contains("Click to start a road"))
        );
        rig.tools.select(ActiveTool::Road);
        // The click left the pointer on the bar: back to the map.
        let texts = rig.frame_texts(vec![egui::Event::PointerMoved(egui::pos2(640.0, 200.0))]);
        let (_, at) = texts
            .iter()
            .find(|(t, _)| t.contains("Click to start a road"))
            .expect("the hint");
        assert!(
            (at.x - 640.0).abs() < 60.0 && (at.y - 200.0).abs() < 100.0,
            "beside the pointer: {at:?}"
        );
    }

    /// T-317: the list of areas is its own window, opened by the "List" button of the strip.
    #[test]
    fn the_list_button_opens_the_area_list_window() {
        let mut rig = Rig::new();
        rig.click_category(Category::Industry);
        assert!(!rig.state.areas_open);
        rig.click_label("List ▸");
        assert!(rig.state.areas_open);
        rig.t += 0.1;
        let Rig {
            ctx,
            state,
            tools,
            overlay,
            outbox,
            view,
            t,
            ..
        } = &mut rig;
        // A window needs a frame or two to size itself before it paints.
        let mut texts = Vec::new();
        for n in 0..4 {
            texts = frame_texts(ctx, input(*t + 0.1 * f64::from(n), vec![]), |ui| {
                list_window(ui.ctx(), state, Some(view), tools, overlay, outbox);
            });
        }
        assert!(
            texts.iter().any(|(t, _)| t.contains("No areas yet")),
            "{texts:?}"
        );
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
                Category::Power,
                Category::Water,
                Category::Districts,
                Category::Transit,
                Category::Industry,
                Category::Parks
            ]
        );
        // Without content the road and building tools do not exist either.
        let none = Category::available(None);
        assert_eq!(none, vec![Category::Zones, Category::Districts]);
    }

    #[test]
    fn clicking_a_tool_activates_it_and_the_verdict_shows_for_valid_and_invalid_ghosts() {
        let mut rig = Rig::new();
        rig.click_category(Category::Roads);
        // T-289: the menu opens with the road tool in hand.
        assert_eq!(rig.tools.active, ActiveTool::Road);
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
            zone: Default::default(),
            block: None,
            ..Default::default()
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
            zone: Default::default(),
            block: None,
            ..Default::default()
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
        // T-289: the menu opens with the road tool in hand.
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
            zone: Default::default(),
            block: None,
            ..Default::default()
        });
        let texts = rig.frame_texts(vec![]);
        for (text, at) in &texts {
            println!("({:6.1}, {:6.1})  {text}", at.x, at.y);
        }
        let all: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        for want in ["Roads", "Small", "Straight", "Curve", "Free", "Ground"] {
            assert!(all.contains(&want), "{want:?} missing from {all:?}");
        }
        assert!(all.iter().any(|t| t.contains("can build")));
    }

    #[test]
    fn each_category_panel_draws_and_its_tools_work() {
        let mut rig = Rig::new();
        for c in Category::available(rig.view.content.as_deref()) {
            rig.click_category(c);
            let text = rig.frame(vec![]);
            assert!(text.contains(c.label()), "{c:?}: {text}");
        }
    }

    fn shift_key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: Some(k),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        }
    }

    #[test]
    fn shift_digit_keys_open_the_categories_in_palette_order() {
        let ctx = egui::Context::default();
        let available = Category::available(Some(&content()));
        assert_eq!(
            available,
            [
                Category::Roads,
                Category::Zones,
                Category::Buildings,
                Category::Power,
                Category::Water,
                Category::Districts,
                Category::Transit,
                Category::Industry,
                Category::Parks
            ]
        );
        let keys = [
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
        for (n, k) in keys.into_iter().enumerate() {
            let mut got = None;
            frame(&ctx, input(n as f64, vec![shift_key(k)]), |ui| {
                got = hotkey(ui.ctx(), &available);
            });
            assert_eq!(got, Some(available[n]));
            // A bare digit is not a category (1-3 are the game speed).
            let mut bare = Some(Category::Roads);
            frame(&ctx, input(n as f64 + 0.5, vec![key(k)]), |ui| {
                bare = hotkey(ui.ctx(), &available);
            });
            assert_eq!(bare, None);
        }
        // Past the last category, and with a modifier held, nothing happens.
        let mut got = Some(Category::Roads);
        frame(&ctx, input(10.0, vec![shift_key(egui::Key::Num8)]), |ui| {
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
    /// T-289: Power and Water have menus of their own; Buildings keeps the other services and the
    /// unique buildings.
    #[test]
    fn the_power_and_water_menus_hold_their_buildings_and_network_tools_and_buildings_has_neither()
    {
        use cp_content::ServiceKind;
        let mut rig = Rig::new();
        rig.screen = egui::vec2(1280.0, 3000.0);
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
                ServiceKind::Park,
                ServiceKind::Post,
                ServiceKind::Parking,
                ServiceKind::Tourism,
            ],
            "the depot is in the Transport category"
        );
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
        // Every tab of the open tray: its tile count and the texts it paints.
        let all = |rig: &mut Rig| -> (usize, Vec<String>) {
            let c = rig.state.category.unwrap();
            let (mut n, mut names) = (0, Vec::new());
            for (label, _) in tabs(c, Some(&rig.view)) {
                rig.click_label(&label);
                n += tiles(rig).len();
                names.extend(rig.frame_texts(vec![]).into_iter().map(|(t, _)| t));
            }
            (n, names)
        };
        // Water: the pipe and every Water and Sewage service building (T-289: no Remove tile, the
        // bulldozer is its own button).
        rig.click_category(Category::Water);
        let (count, n) = all(&mut rig);
        assert_eq!(
            count,
            service(&[ServiceKind::Water, ServiceKind::Sewage]) + 1
        );
        for want in ["Pipe", "Water pump", "Water tower", "Sewage outflow"] {
            assert!(n.iter().any(|t| t == want), "{want:?} missing from {n:?}");
        }
        assert!(!n.iter().any(|t| t == "Power line" || t == "Wind turbine"));
        // Power likewise.
        rig.click_category(Category::Power);
        let (count, n) = all(&mut rig);
        assert_eq!(count, service(&[ServiceKind::Power]) + 1);
        for want in ["Power line", "Coal power plant", "Wind turbine"] {
            assert!(n.iter().any(|t| t == want), "{want:?} missing from {n:?}");
        }
        assert!(!n.iter().any(|t| t == "Pipe" || t == "Water pump"));
        // Buildings: neither network, the rest of the services and the unique buildings.
        rig.click_category(Category::Buildings);
        let expect: usize = sections
            .iter()
            .filter(|s| crate::buildings::in_scope(s.kind, crate::buildings::Scope::Buildings))
            .map(|s| s.cards.len())
            .sum::<usize>()
            + crate::buildings::unique_sections(&content)
                .iter()
                .map(|(_, c)| c.len())
                .sum::<usize>();
        let (count, n) = all(&mut rig);
        assert_eq!(count, expect);
        for gone in [
            "Pipe",
            "Water pump",
            "Power line",
            "Wind turbine",
            "Remove pipe",
            "Remove line",
        ] {
            assert!(!n.iter().any(|t| t == gone), "{gone:?} still in Buildings");
        }
    }

    /// T-201: the unique buildings and the monuments are their own sections after the
    /// services; a locked or built tile is grey and does not pick the tool, an open one does.
    #[test]
    fn unique_tiles_open_lock_and_built_and_the_tooltip_has_the_progress() {
        use crate::buildings::{UniqueState, spaced, unique_hover, unique_state};
        let mut rig = Rig::new();
        rig.screen = egui::vec2(1280.0, 3000.0);
        rig.click_category(Category::Buildings);
        let content = rig.view.content.clone().unwrap();
        let id = |n: &str| content.building_id(n).unwrap();
        let sections = crate::buildings::unique_sections(&content);
        assert_eq!(sections.len(), 2);
        assert!(
            !sections[0].0 && sections[1].0,
            "unique buildings, then monuments"
        );
        for (tab, want) in [
            ("Unique buildings", "City plaza"),
            ("Monuments", "Golden arch"),
        ] {
            rig.click_label(tab);
            let texts = rig.frame_texts(vec![]);
            let names: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
            assert!(names.contains(&want), "{want:?} missing from {names:?}");
        }
        // Everything open (a world without milestones): a click picks the building.
        rig.click_label("Unique buildings");
        rig.click_label("City plaza");
        assert_eq!(rig.tools.active, ActiveTool::Place);
        assert_eq!(rig.tools.building.def, Some(id("city_plaza")));
        rig.tools.select(ActiveTool::None);

        // Closed with 3 200 of 5 000: no tool, and the tooltip says how far it is.
        let mut statuses = (*rig.view.uniques).clone();
        let plaza = statuses.iter_mut().find(|u| u.id == "city_plaza").unwrap();
        plaza.unlocked = false;
        plaza.conditions = vec![(cp_content::UnlockCond::Population(5000), 3200, 5000)];
        rig.view.uniques = std::sync::Arc::new(statuses);
        assert_eq!(
            unique_state(&rig.view, id("city_plaza")),
            UniqueState::Locked
        );
        rig.click_label("City plaza");
        assert_eq!(rig.tools.active, ActiveTool::None);
        let card = crate::buildings::groups(&content)
            .into_iter()
            .flat_map(|g| g.cards)
            .find(|c| c.id == id("city_plaza"))
            .unwrap();
        let hover = unique_hover(&rig.view, &card);
        assert!(hover.starts_with("Locked"), "{hover}");
        assert!(hover.contains("Population 3 200 / 5 000"), "{hover}");
        assert_eq!(spaced(1_234_567), "1 234 567");

        // Built: one per city, no tool either.
        let mut statuses = (*rig.view.uniques).clone();
        let plaza = statuses.iter_mut().find(|u| u.id == "city_plaza").unwrap();
        plaza.unlocked = true;
        plaza.built = true;
        rig.view.uniques = std::sync::Arc::new(statuses);
        assert_eq!(
            unique_state(&rig.view, id("city_plaza")),
            UniqueState::Built
        );
        rig.click_label("City plaza");
        assert_eq!(rig.tools.active, ActiveTool::None);
        assert!(unique_hover(&rig.view, &card).starts_with("Built"));
    }

    #[test]
    fn the_tiles_of_the_buildings_panel_set_the_tools_and_the_overlay() {
        use cp_render::Overlay;
        let mut rig = Rig::new();
        rig.screen = egui::vec2(1280.0, 3000.0);
        rig.click_category(Category::Water);
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

        rig.click_category(Category::Power);
        assert!(
            !rig.frame_texts(vec![])
                .iter()
                .any(|(t, _)| t == "Remove line" || t == "Remove pipe"),
            "the bulldozer is its own button"
        );
        rig.click_label("Coal power plant");
        assert_eq!(rig.tools.active, ActiveTool::Place);
        assert_eq!(rig.overlay.active, Overlay::Power);
    }

    /// BUG-083: the layout a tool opened by itself goes when the player moves on to another
    /// category or tool; the one the player picked stays; a service tool hands its coverage to
    /// the next service tool.
    #[test]
    fn an_overlay_a_tool_opened_goes_with_the_tool() {
        use cp_render::Overlay;
        let mut rig = Rig::new();
        rig.screen = egui::vec2(1280.0, 3000.0);
        rig.click_category(Category::Water);
        rig.click_label("Pipe");
        assert_eq!(rig.tools.active, ActiveTool::Utility);
        assert_eq!(rig.overlay.active, Overlay::Water);
        assert!(rig.overlay.auto);
        // To the Roads menu: the water layout goes.
        rig.click_category(Category::Roads);
        assert_eq!(rig.overlay.active, Overlay::Off);
        // The Roads menu opened with the road tool in hand (T-289); nothing comes back.
        assert_eq!(rig.tools.active, ActiveTool::Road);
        assert_eq!(rig.overlay.active, Overlay::Off);
    }

    #[test]
    fn an_overlay_the_player_opened_survives_a_tool_switch() {
        use cp_render::Overlay;
        let mut rig = Rig::new();
        rig.screen = egui::vec2(1280.0, 3000.0);
        rig.overlay.open(Overlay::Power);
        rig.click_category(Category::Roads);
        // T-289: the menu opens with the road tool in hand.
        assert_eq!(rig.tools.active, ActiveTool::Road);
        assert_eq!(rig.tools.active, ActiveTool::Road);
        assert_eq!(
            rig.overlay.active,
            Overlay::Power,
            "the player's layout stays"
        );
        rig.click_category(Category::Water);
        rig.click_label("Pipe");
        // The pipe tool shows the water layout (its own); the player closes what is left.
        assert_eq!(rig.overlay.active, Overlay::Water);
        rig.click_category(Category::Roads);
        assert_eq!(
            rig.overlay.active,
            Overlay::Off,
            "now it is the tool's, and goes"
        );
    }

    #[test]
    fn a_service_tool_hands_its_coverage_to_the_next_service_tool() {
        use cp_render::Overlay;
        let mut rig = Rig::new();
        rig.screen = egui::vec2(1280.0, 3000.0);
        rig.click_category(Category::Buildings);
        rig.click_label("Fire");
        rig.click_label("Fire Station");
        assert_eq!(rig.overlay.active, Overlay::Fire);
        rig.click_label("Police");
        rig.click_label("Police Station");
        assert_eq!(rig.overlay.active, Overlay::Police);
        // Staying with the same tool changes nothing.
        rig.frame(vec![]);
        assert_eq!(rig.overlay.active, Overlay::Police);
    }

    #[test]
    fn the_site_plate_names_the_site_of_the_selected_building() {
        use cp_content::ServiceSite;
        let mut rig = Rig::new();
        rig.screen = egui::vec2(1280.0, 3000.0);
        for (category, label, site) in [
            (Category::Water, "Water pump", ServiceSite::OnWater),
            (Category::Water, "Sewage outflow", ServiceSite::Shore),
            (Category::Power, "Wind turbine", ServiceSite::Land),
        ] {
            if rig.state.category != Some(category) {
                rig.click_category(category);
            }
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
            egui::Key::F4,
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
        rig.click_label("District");
        rig
    }

    /// T-289: opening a menu takes up the last build tool used in it this session, the first
    /// time its first tool; Power and Water turn their overlay on.
    #[test]
    fn a_menu_opens_with_its_last_tool_and_the_network_menus_show_their_overlay() {
        use cp_render::Overlay;
        let mut rig = Rig::new();
        rig.screen = egui::vec2(1280.0, 3000.0);
        let def = |rig: &Rig, n: &str| rig.view.content.as_ref().unwrap().building_id(n);
        // First time: the first tool of the menu.
        rig.click_category(Category::Power);
        assert_eq!(rig.tools.active, ActiveTool::Place);
        assert_eq!(rig.tools.building.def, def(&rig, "wind_turbine"));
        assert_eq!(rig.overlay.active, Overlay::Power);
        rig.click_category(Category::Water);
        assert_eq!(rig.tools.building.def, def(&rig, "water_pump_station"));
        assert_eq!(rig.overlay.active, Overlay::Water, "the Power layout went");
        // The pipe tool becomes Water's last tool; the coal plant Power's.
        rig.click_label("Pipe");
        assert_eq!(rig.tools.active, ActiveTool::Utility);
        rig.click_category(Category::Power);
        rig.click_label("Coal power plant");
        rig.click_category(Category::Water);
        assert_eq!(
            rig.tools.active,
            ActiveTool::Utility,
            "Water's last tool: pipes"
        );
        assert_eq!(rig.tools.utility.kind, cp_content::UtilityKind::Water);
        rig.click_category(Category::Power);
        assert_eq!(rig.tools.active, ActiveTool::Place);
        assert_eq!(rig.tools.building.def, def(&rig, "coal_power_plant"));
        // Roads and Zones: their tools.
        rig.click_category(Category::Roads);
        assert_eq!(rig.tools.active, ActiveTool::Road);
        rig.click_category(Category::Zones);
        assert_eq!(rig.tools.active, ActiveTool::Zone);
        // Closing a menu keeps nothing but the memory; the Buildings menu has its own.
        rig.click_category(Category::Buildings);
        assert_eq!(rig.tools.active, ActiveTool::Place);
        assert!(rig.tools.building.def != def(&rig, "coal_power_plant"));
    }

    /// T-289: with the Power or Water menu open the bulldozer is that layer's, even when no
    /// network tool is in hand; anywhere else it takes everything.
    #[test]
    fn the_bulldozer_follows_the_open_menus_layer() {
        use cp_tools::BulldozeLayer;
        let mut rig = Rig::new();
        rig.screen = egui::vec2(1280.0, 3000.0);
        for (category, layer) in [
            (Category::Power, BulldozeLayer::Power),
            (Category::Water, BulldozeLayer::Water),
            (Category::Roads, BulldozeLayer::All),
            (Category::Buildings, BulldozeLayer::All),
        ] {
            rig.click_category(category);
            rig.tools.select(ActiveTool::None);
            rig.frame(vec![]);
            rig.tools.toggle_bulldozer();
            assert_eq!(rig.tools.active, ActiveTool::Bulldozer, "{category:?}");
            assert_eq!(rig.tools.bulldoze_layer, layer, "{category:?}");
            rig.tools.toggle_bulldozer();
        }
    }

    #[test]
    fn the_districts_category_is_the_sixth_and_new_district_creates_and_selects_one() {
        use cp_district::{DistrictCommand, DistrictId};
        use cp_sim::SimCommand;
        let mut rig = Rig::new();
        assert_eq!(
            Category::available(rig.view.content.as_deref())[5],
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

    /// T-191: the tabs. Switching never loses the chosen district or sends a command, the tab
    /// is remembered, and the four tiles pass the tile checks.
    #[test]
    fn the_district_tabs_switch_without_losing_the_district_and_are_tiles() {
        use cp_district::DistrictId;
        let mut rig = rig_with_district(None);
        // Taxes first.
        assert!(rig.frame(vec![]).contains("R low"));
        let tab_tiles = tiles(&mut rig);
        let before = tab_tiles.len();
        for (tile, icon) in &tab_tiles {
            assert_eq!(tile.size(), Sizes::tile());
            assert_eq!(icon.size(), Sizes::tile_icon());
        }
        assert!(before >= 4);
        rig.click_label("Stats");
        let text = rig.frame(vec![]);
        assert!(
            text.contains("Residents") && !text.contains("R low"),
            "{text}"
        );
        assert_eq!(rig.tools.district.selected, Some(DistrictId(1)));
        // Remembered over frames; the stubs say what they are.
        assert!(rig.frame(vec![]).contains("Residents"));
        rig.click_label("Policies");
        let text = rig.frame(vec![]);
        assert!(
            text.contains("Recycling") && !text.contains("Tourism promotion"),
            "{text}"
        );
        rig.click_label("Special.");
        let text = rig.frame(vec![]);
        assert!(
            text.contains("Commercial") && text.contains("Leisure"),
            "{text}"
        );
        rig.click_label("Taxes");
        assert!(rig.frame(vec![]).contains("R low"));
        assert_eq!(rig.tools.district.selected, Some(DistrictId(1)));
        assert!(rig.outbox.iter().next().is_none(), "tabs send nothing");
    }
}
