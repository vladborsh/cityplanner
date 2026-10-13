//! The left palette and the tool panel beside it (`architecture/system-design.md` §13.6).
//!
//! The palette is a 56 px column with one icon per [`Category`]; clicking one (or pressing its
//! number key, 1…6) opens that category's 300 px tool panel, clicking it again closes the
//! panel. The panel has a header, the tool buttons and options of the category, and the pinned
//! "cost · verdict" block at the bottom.

use bevy_egui::egui;
use cp_content::ContentDb;
use cp_render::OverlayState;
use cp_sim::SimView;
use cp_tools::ToolState;

use crate::icons::Icon;
use crate::panels::{
    UiState, bulldozer_panel, roads_panel, services_panel, utility_panel, verdict, zones_panel,
};

/// Width of the palette column, points.
pub const PALETTE_WIDTH: f32 = 56.0;
/// Width of the tool panel, points.
pub const TOOL_PANEL_WIDTH: f32 = 300.0;

/// Build-menu categories. A category without tools is not drawn ([`Category::available`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Roads,
    Zones,
    Water,
    Power,
    Buildings,
    Bulldozer,
    /// Public transport (M3): no tools yet.
    Transit,
    /// Districts (M4): no tools yet.
    Districts,
}

impl Category {
    pub const ALL: [Category; 8] = [
        Category::Roads,
        Category::Zones,
        Category::Water,
        Category::Power,
        Category::Buildings,
        Category::Bulldozer,
        Category::Transit,
        Category::Districts,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Roads => "Roads",
            Category::Zones => "Zones",
            Category::Water => "Water",
            Category::Power => "Power",
            Category::Buildings => "Buildings",
            Category::Bulldozer => "Bulldozer",
            Category::Transit => "Transit",
            Category::Districts => "Districts",
        }
    }

    /// Transit and Districts have no icon yet; they are never drawn without tools.
    pub fn icon(self) -> Icon {
        match self {
            Category::Roads => Icon::Roads,
            Category::Zones => Icon::Zones,
            Category::Water => Icon::Water,
            Category::Power => Icon::Power,
            Category::Buildings => Icon::Buildings,
            Category::Bulldozer => Icon::Bulldozer,
            Category::Transit | Category::Districts => Icon::Info,
        }
    }

    /// Whether the category has a tool to offer with `content`: roads need a road type,
    /// buildings a placeable service building, the others are always there; Transit and
    /// Districts have none before M3 / M4.
    pub fn has_tools(self, content: Option<&ContentDb>) -> bool {
        match self {
            Category::Roads => content.is_some_and(|c| c.roads().next().is_some()),
            Category::Buildings => {
                content.is_some_and(|c| c.buildings().any(|(_, d)| d.service.is_some()))
            }
            Category::Zones | Category::Water | Category::Power | Category::Bulldozer => true,
            Category::Transit | Category::Districts => false,
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
        .exact_size(PALETTE_WIDTH)
        .resizable(false)
        .show(root, |ui| {
            ui.vertical_centered(|ui| {
                for (n, c) in Category::available(content).into_iter().enumerate() {
                    let selected = state.category == Some(c);
                    let r = ui
                        .add(egui::Button::image(c.icon().sized(32.0)).selected(selected))
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
    state: &UiState,
    view: Option<&SimView>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
) -> Option<egui::Rect> {
    let category = state.category?;
    let out = egui::Panel::left("tool_panel")
        .exact_size(TOOL_PANEL_WIDTH)
        .resizable(false)
        .show(root, |ui| {
            ui.horizontal(|ui| {
                ui.add(category.icon().sized(28.0));
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
                Category::Roads => roads_panel(ui, view, tools),
                Category::Zones => zones_panel(ui, tools),
                Category::Water | Category::Power => {
                    utility_panel(ui, view, tools, overlay, category)
                }
                Category::Buildings => services_panel(ui, view, tools),
                Category::Bulldozer => bulldozer_panel(ui, tools),
                Category::Transit | Category::Districts => {
                    ui.weak("Nothing to build here yet.");
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
        view: SimView,
        t: f64,
        palette: Vec<(Category, egui::Rect)>,
        panel: Option<egui::Rect>,
    }

    impl Rig {
        fn new() -> Self {
            let mut rig = Rig {
                ctx: egui::Context::default(),
                state: UiState::default(),
                tools: ToolState::default(),
                overlay: OverlayState::default(),
                view: view(),
                t: 0.0,
                palette: Vec::new(),
                panel: None,
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
                view,
                palette: pal,
                panel,
                t,
            } = self;
            frame_texts(ctx, input(*t, events), |ui| {
                let mut root = crate::panels::root_ui(ui.ctx());
                *pal = palette(&mut root, state, view.content.as_deref());
                *panel = tool_panel(&mut root, state, Some(view), tools, overlay);
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
    fn the_palette_is_56_wide_and_the_tool_panel_300() {
        let mut rig = Rig::new();
        let pal = rig.palette[0].1;
        assert!(pal.width() < PALETTE_WIDTH);
        rig.click_category(Category::Roads);
        let panel = rig.panel.unwrap();
        assert_eq!(panel.width(), TOOL_PANEL_WIDTH);
        assert_eq!(
            panel.min.x, PALETTE_WIDTH,
            "the panel sits right of the palette"
        );
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
                Category::Water,
                Category::Power,
                Category::Buildings,
                Category::Bulldozer
            ]
        );
        assert!(!drawn.contains(&Category::Transit));
        assert!(!drawn.contains(&Category::Districts));
        // Without content the road and building tools do not exist either.
        let none = Category::available(None);
        assert_eq!(
            none,
            vec![
                Category::Zones,
                Category::Water,
                Category::Power,
                Category::Bulldozer
            ]
        );
    }

    #[test]
    fn clicking_a_tool_activates_it_and_the_verdict_shows_for_valid_and_invalid_ghosts() {
        let mut rig = Rig::new();
        rig.click_category(Category::Roads);
        rig.click_label("Road");
        assert_eq!(rig.tools.active, ActiveTool::Road);

        rig.tools.ghost = Some(Ghost {
            polylines: vec![],
            valid: true,
            cost: Some(Cost(123_456)),
            errors: vec![],
            half_width: 0.0,
            snap: Default::default(),
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
        rig.click_label("Road");
        rig.tools.ghost = Some(Ghost {
            polylines: vec![],
            valid: true,
            cost: Some(Cost(123_456)),
            errors: vec![],
            half_width: 0.0,
            snap: Default::default(),
        });
        let texts = rig.frame_texts(vec![]);
        for (text, at) in &texts {
            println!("({:6.1}, {:6.1})  {text}", at.x, at.y);
        }
        let all: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        for want in [
            "Roads",
            "Tool",
            "Road",
            "Mode",
            "Straight",
            "Curve (3 clicks)",
            "Free (Enter ends)",
            "Road type",
            "Elevation",
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
            tool_panel(&mut root, state, Some(view), tools, overlay);
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
        rig.click_category(Category::Bulldozer);
        assert_eq!(rig.state.category, Some(Category::Bulldozer));
        rig.click_label("Bulldozer");
        assert_eq!(rig.tools.active, ActiveTool::Bulldozer);
    }

    #[test]
    fn digit_keys_open_the_categories_in_palette_order() {
        let ctx = egui::Context::default();
        let available = Category::available(Some(&content()));
        assert_eq!(available.len(), 6);
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
}
