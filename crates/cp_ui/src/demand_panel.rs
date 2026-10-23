//! The bottom demand panel (T-099, `architecture/system-design.md` §13.7): a real panel under
//! the 3D view, right of the palette and the tool panel, with one column per demand — its
//! icon and a vertical demand bar from 0 (empty) to 100 (full), filling up. Demand is one
//! scale each for residential, commercial, industrial and office (T-231), so there are four
//! columns, R, C, I and O, whatever the density; O is greyed with a padlock while offices are
//! locked. It folds into a 28 px strip. T-289: it is also the city's status bar — money, income,
//! population and the utility gauges (T-279) stand at its left — and the bulldozer button at its
//! right.

use bevy_egui::egui;
use cp_content::ZoneType;
use cp_locale::tr;
use cp_sim::SimView;
use cp_tools::{ActiveTool, ToolState};

use crate::budget::demand_fraction;
use crate::icons::zone_icon;
use crate::palette::Category;
use crate::panels::{UiState, zone_group_demand};
use crate::style::Sizes;
use crate::windows::{color, zone_tooltip};

/// The notice of §5.1 when new residents cannot arrive.
pub const NOT_CONNECTED: &str = "City not connected to the highway — new residents do not arrive";

/// Where the entry road's attachment ends (the stub's dead end), if the map has one and the
/// city is known not to be connected to it. Stats are published after the first 64 ticks;
/// before that the field is its default and says nothing.
pub fn unconnected_stub(view: &SimView) -> Option<bevy::math::Vec2> {
    if view.city.tick == 0 || view.city.connected_to_highway {
        return None;
    }
    Some(
        view.terrain
            .as_deref()?
            .meta()
            .entry_road
            .as_ref()?
            .stub_end(),
    )
}

/// The four columns and the zone type whose tool each one selects.
pub const COLUMNS: [ZoneType; 4] = [
    ZoneType::ResLow,
    ZoneType::ComLow,
    ZoneType::Industrial,
    ZoneType::Office,
];

/// Where one column was drawn.
#[derive(Clone, Copy, Debug)]
pub struct Column {
    pub zone: ZoneType,
    /// The whole clickable column.
    pub rect: egui::Rect,
    /// The bar's track (0 at the bottom, 100 at the top).
    pub track: egui::Rect,
    /// Y of the bar's level: `track.bottom() - demand_fraction * track.height()`.
    pub level_y: f32,
}

/// Where the parts of the bar were drawn (tests click them).
#[derive(Clone, Debug, Default)]
pub struct DemandRects {
    pub panel: Option<egui::Rect>,
    pub columns: Vec<Column>,
    /// The button that opens the window "Demand by zone".
    pub details: Option<egui::Rect>,
    /// The "show" button of the not-connected notice, when the notice is visible.
    pub show_stub: Option<egui::Rect>,
    /// The notice is on screen.
    pub notice: bool,
    /// The balance (a click opens the budget) and the population.
    pub money: Option<egui::Rect>,
    pub population: Option<egui::Rect>,
    /// The category buttons.
    pub categories: Vec<(Category, egui::Rect)>,
    /// The tools of every category, right of the categories.
    pub globals: Vec<(ActiveTool, egui::Rect)>,
}

impl DemandRects {
    /// The bulldozer button.
    pub fn bulldozer(&self) -> Option<egui::Rect> {
        self.globals
            .iter()
            .find(|(t, _)| *t == ActiveTool::Bulldozer)
            .map(|(_, r)| *r)
    }
}

/// The balance with its income (a click opens the budget).
fn money_row(ui: &mut egui::Ui, view: &SimView, state: &mut UiState, rects: &mut DemandRects) {
    let money = crate::budget::money_label(ui, view);
    crate::budget::trend_label(ui, view);
    let money = ui.interact(money.rect, money.id.with("open"), egui::Sense::click());
    rects.money = Some(money.rect);
    if money
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        state.budget_open = !state.budget_open;
    }
}

/// The population and its change over the last day.
fn population_row(ui: &mut egui::Ui, view: &SimView, rects: &mut DemandRects) {
    let r = ui
        .label(format!("☻ {}", view.city.population))
        .on_hover_text(tr("Residents"));
    rects.population = Some(r.rect);
    crate::budget::population_trend_label(ui, view);
}

/// The slim bottom bar (T-317), one row: money, income, population and the demand bars (a
/// click on a bar takes its zone tool) at the left; the category buttons and the tools that
/// work from any category (upgrade, flip, bulldozer) as one group in the middle of the screen
/// (T-317f), pushed right only when the left block would overlap it.
pub fn demand_panel(
    root: &mut egui::Ui,
    view: &SimView,
    state: &mut UiState,
    tools: &mut ToolState,
) -> DemandRects {
    let mut rects = DemandRects::default();
    let out = egui::Panel::bottom("demand")
        .exact_size(Sizes::DEMAND_H)
        .resizable(false)
        .show(root, |ui| {
            ui.horizontal_centered(|ui| {
                money_row(ui, view, state, &mut rects);
                ui.add_space(8.0);
                population_row(ui, view, &mut rects);
                ui.separator();
                let r = ui
                    .small_button("▸")
                    .on_hover_text(tr("Demand by zone (window)"));
                rects.details = Some(r.rect);
                if r.clicked() {
                    state.demand_open = true;
                }
                for zone in COLUMNS {
                    let col = column(ui, view, zone);
                    if col.1.clicked() && crate::windows::zone_lock(view, zone).is_none() {
                        state.category = Some(Category::Zones);
                        if tools.active != ActiveTool::Zone {
                            tools.select(ActiveTool::Zone);
                        }
                        tools.zone.zone = zone;
                    }
                    rects.columns.push(col.0);
                }
                let content = view.content.as_deref();
                let buttons = Category::available(content).len() + crate::palette::GLOBAL_TOOLS;
                let gap = ui.spacing().item_spacing.x;
                let side = Sizes::PALETTE_ICON + 2.0 * Sizes::BUTTON_PAD;
                // The buttons, their gaps and the separator between the two kinds.
                let group = buttons as f32 * (side + gap) + 2.0 * gap + 1.0;
                let start = (ui.max_rect().center().x - group * 0.5).max(ui.cursor().left());
                ui.add_space(start - ui.cursor().left());
                rects.categories = crate::palette::palette(ui, state, content);
                ui.separator();
                rects.globals = crate::palette::global_tools(ui, state, tools);
            });
        });
    rects.panel = Some(out.response.rect);
    notice(root.ctx(), view, state, &mut rects);
    rects
}

/// The notice of §5.1 above the bar's left end while the city is not connected.
fn notice(ctx: &egui::Context, view: &SimView, state: &mut UiState, rects: &mut DemandRects) {
    let Some(stub) = unconnected_stub(view) else {
        return;
    };
    rects.notice = true;
    egui::Area::new("not_connected".into())
        .anchor(
            egui::Align2::LEFT_BOTTOM,
            egui::vec2(
                8.0,
                -(Sizes::DEMAND_H
                    + if state.category.is_some() {
                        Sizes::TRAY_H
                    } else {
                        0.0
                    }
                    + 8.0),
            ),
        )
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::YELLOW, format!("⚠ {}", tr(NOT_CONNECTED)));
                    let r = ui
                        .button(tr("Show"))
                        .on_hover_text(tr("Centre the camera on the dead end"));
                    rects.show_stub = Some(r.rect);
                    if r.clicked() {
                        state.camera_goal = Some(stub);
                    }
                });
            });
        });
}

/// One column: the icon beside a thin bar, sensitive to clicks, with the tooltip.
fn column(ui: &mut egui::Ui, view: &SimView, zone: ZoneType) -> (Column, egui::Response) {
    let size = egui::vec2(Sizes::DEMAND_COL_W, Sizes::DEMAND_BAR_H);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let icon = egui::Rect::from_center_size(
        egui::pos2(
            rect.left() + 4.0 + Sizes::DEMAND_ICON * 0.5,
            rect.center().y,
        ),
        egui::vec2(Sizes::DEMAND_ICON, Sizes::DEMAND_ICON),
    );
    let locked = crate::windows::zone_lock(view, zone).is_some();
    let mut image = egui::Image::new(zone_icon(zone).image());
    if locked {
        image = image.tint(egui::Color32::from_gray(110));
    }
    image.paint_at(ui, icon);
    if locked {
        crate::buildings::badge(ui, icon, crate::buildings::UniqueState::Locked);
    }
    let track = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 8.0, rect.center().y),
        egui::vec2(8.0, Sizes::DEMAND_BAR_H - 4.0),
    );
    let (_, value) = zone_group_demand(zone, &view.demand);
    let level_y = track.bottom() - demand_fraction(value) * track.height();
    painter.rect_filled(track, 2.0, ui.visuals().extreme_bg_color);
    // Fill up from the bottom to the level; no demand = an empty bar.
    let fill = egui::Rect::from_x_y_ranges(track.x_range(), level_y..=track.bottom());
    painter.rect_filled(
        fill,
        0.0,
        if locked {
            egui::Color32::from_gray(110)
        } else {
            color(zone)
        },
    );
    painter.rect_stroke(
        track,
        2.0,
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    let response = response.on_hover_text(zone_tooltip(view, zone));
    (
        Column {
            zone,
            rect,
            track,
            level_y,
        },
        response,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{click, frame, input};
    use cp_sim::SimRunner;

    struct Rig {
        ctx: egui::Context,
        state: UiState,
        tools: ToolState,
        view: SimView,
        rects: DemandRects,
        t: f64,
    }

    impl Rig {
        fn new(view: SimView) -> Self {
            let mut rig = Rig {
                ctx: egui::Context::default(),
                state: UiState::default(),
                tools: ToolState::default(),
                view,
                rects: DemandRects::default(),
                t: 0.0,
            };
            rig.frame(vec![]);
            rig.frame(vec![]);
            rig
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            self.t += 0.1;
            let Rig {
                ctx,
                state,
                tools,
                view,
                rects,
                t,
            } = self;
            frame(ctx, input(*t, events), |ui| {
                let mut root = crate::panels::root_ui(ui.ctx());
                *rects = demand_panel(&mut root, view, state, tools);
            });
        }

        fn click(&mut self, pos: egui::Pos2) {
            let [down, up] = click(pos);
            self.frame(down);
            self.frame(up);
            self.frame(vec![]);
        }
    }

    fn view_with(r: f32, c: f32, i: f32, o: f32) -> SimView {
        let mut view = SimRunner::new(1).view();
        view.demand = cp_building::Demand {
            r,
            c,
            i,
            o,
            ..Default::default()
        };
        view
    }

    #[test]
    fn four_columns_r_c_i_o_with_bars_proportional_to_their_demand() {
        let rig = Rig::new(view_with(60.0, -30.0, 100.0, -100.0));
        let zones: Vec<_> = rig.rects.columns.iter().map(|c| c.zone).collect();
        assert_eq!(zones, COLUMNS.to_vec());
        for col in &rig.rects.columns {
            let (_, v) = zone_group_demand(col.zone, &rig.view.demand);
            let frac = (col.track.bottom() - col.level_y) / col.track.height();
            assert!((frac - demand_fraction(v)).abs() < 1e-4, "{:?}", col.zone);
        }
        let level = |z| {
            rig.rects
                .columns
                .iter()
                .find(|c| c.zone == z)
                .unwrap()
                .level_y
        };
        assert_ne!(level(ZoneType::ResLow), level(ZoneType::ComLow));
    }

    #[test]
    fn a_negative_demand_is_an_empty_bar_never_below_the_base() {
        let rig = Rig::new(view_with(-80.0, 0.0, 50.0, -1.0));
        for col in &rig.rects.columns {
            assert!(col.level_y <= col.track.bottom() + 1e-3, "{:?}", col.zone);
        }
        let r = &rig.rects.columns[0];
        assert_eq!(r.level_y, r.track.bottom());
        let i = &rig.rects.columns[2];
        assert!((i.track.bottom() - i.level_y - i.track.height() * 0.5).abs() < 1e-3);
    }

    #[test]
    fn the_tooltip_says_demand_out_of_100_and_why_it_is_low() {
        let mut view = view_with(-30.0, 60.0, 0.0, 0.0);
        std::sync::Arc::make_mut(&mut view.city).unemployed = 25;
        let t = zone_tooltip(&view, ZoneType::ResLow);
        assert!(t.contains("R demand 0/100"), "{t}");
        assert!(t.contains("No free jobs: 25 unemployed"), "{t}");
        let t = zone_tooltip(&view, ZoneType::ComLow);
        assert!(
            t.contains("C demand 60/100") && !t.contains("Enough shops"),
            "{t}"
        );
    }

    #[test]
    fn the_bar_is_slim_and_sits_under_the_view() {
        let rig = Rig::new(view_with(0.0, 0.0, 0.0, 0.0));
        let panel = rig.rects.panel.unwrap();
        assert_eq!(panel.height(), Sizes::DEMAND_H);
        assert_eq!(panel.bottom(), 720.0);
    }

    #[test]
    fn a_click_on_a_column_selects_the_zones_tool_with_that_type() {
        let mut rig = Rig::new(view_with(10.0, 20.0, 30.0, 40.0));
        let at = rig
            .rects
            .columns
            .iter()
            .find(|c| c.zone == ZoneType::ComLow)
            .unwrap()
            .rect
            .center();
        rig.click(at);
        assert_eq!(rig.state.category, Some(Category::Zones));
        assert_eq!(rig.tools.active, ActiveTool::Zone);
        assert_eq!(rig.tools.zone.zone, ZoneType::ComLow);
    }

    fn locked_view() -> SimView {
        let db = std::sync::Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        );
        let mut sim = SimRunner::new(1);
        sim.set_content(db);
        sim.world_mut().insert_resource(cp_sim::Progress::START);
        sim.view()
    }

    #[test]
    fn the_office_bar_is_locked_below_busy_town_and_its_click_selects_nothing() {
        let mut rig = Rig::new(locked_view());
        assert_eq!(
            crate::windows::zone_lock(&rig.view, ZoneType::Office).as_deref(),
            Some("Unlocks at 5,000 (Busy Town)")
        );
        assert!(crate::windows::zone_lock(&rig.view, ZoneType::ResLow).is_none());
        assert!(zone_tooltip(&rig.view, ZoneType::Office).contains("Unlocks at 5,000"));
        let at = |rig: &Rig, z| {
            rig.rects
                .columns
                .iter()
                .find(|c| c.zone == z)
                .unwrap()
                .rect
                .center()
        };
        let o = at(&rig, ZoneType::Office);
        rig.click(o);
        assert_eq!(rig.tools.active, ActiveTool::None);
        let r = at(&rig, ZoneType::ResLow);
        rig.click(r);
        assert_eq!(rig.tools.zone.zone, ZoneType::ResLow);

        // Busy Town reached: open.
        let mut view = rig.view.clone();
        view.progress = cp_sim::Progress::at(5);
        assert!(crate::windows::zone_lock(&view, ZoneType::Office).is_none());
    }

    /// A view whose city has these `(supply, demand)` for power, water and sewage.
    fn texts(rig: &mut Rig, events: Vec<egui::Event>) -> Vec<String> {
        rig.t += 0.1;
        let Rig {
            ctx,
            state,
            tools,
            view,
            rects,
            t,
        } = rig;
        crate::testutil::frame_texts(ctx, input(*t, events), |ui| {
            let mut root = crate::panels::root_ui(ui.ctx());
            *rects = demand_panel(&mut root, view, state, tools);
        })
        .into_iter()
        .map(|(t, _)| t)
        .collect()
    }

    /// T-289: the balance and the population stand at the left of the bottom panel; a click on
    /// the balance opens the budget.
    #[test]
    fn the_bottom_panel_shows_the_population_and_the_balance_opens_the_budget() {
        let mut view = SimRunner::new(1).view();
        std::sync::Arc::make_mut(&mut view.city).population = 12_345;
        let mut rig = Rig::new(view);
        let all = texts(&mut rig, vec![]);
        assert!(all.iter().any(|t| t == "☻ 12345"), "{all:?}");
        let panel = rig.rects.panel.unwrap();
        for r in [rig.rects.money.unwrap(), rig.rects.population.unwrap()] {
            assert!(panel.contains_rect(r) && r.left() < rig.rects.columns[0].rect.left());
        }
        assert!(!rig.state.budget_open);
        rig.click(rig.rects.money.unwrap().center());
        assert!(rig.state.budget_open);
    }

    /// The panel's content fits a 1280 wide window with the longest figures, the demand columns
    /// and the bulldozer button right of them.
    #[test]
    fn the_bottom_panel_fits_a_1280_wide_window_with_big_numbers() {
        for lang in cp_locale::Lang::ALL {
            let _lang = crate::testutil::in_lang(lang);
            bottom_panel_fits(lang);
        }
    }

    /// T-284: the check above in `lang`.
    fn bottom_panel_fits(lang: cp_locale::Lang) {
        let mut rig = Rig::new(SimRunner::new(1).view());
        rig.frame(vec![]);
        let panel = rig.rects.panel.unwrap();
        let bull = rig.rects.bulldozer().unwrap();
        assert!(panel.contains_rect(bull), "{bull:?} not in {panel:?}");
        assert!(bull.right() <= 1280.0, "{lang:?}: {bull:?}");
        let last = rig.rects.categories.last().unwrap().1;
        assert!(
            last.right() <= bull.left(),
            "{lang:?}: categories and tools overlap"
        );
        assert!(
            bull.left() > rig.rects.columns[3].rect.right(),
            "right of the demand"
        );
    }

    /// T-289: the bulldozer button takes the bulldozer up and puts it down; with the Power or
    /// Water menu open it is that layer's.
    #[test]
    fn the_bulldozer_button_toggles_the_bulldozer_in_the_open_menus_layer() {
        use cp_tools::BulldozeLayer;
        let mut rig = Rig::new(SimRunner::new(1).view());
        assert_eq!(rig.tools.active, ActiveTool::None);
        rig.click(rig.rects.bulldozer().unwrap().center());
        assert_eq!(rig.tools.active, ActiveTool::Bulldozer);
        assert_eq!(rig.tools.bulldoze_layer, BulldozeLayer::All);
        rig.click(rig.rects.bulldozer().unwrap().center());
        assert_eq!(rig.tools.active, ActiveTool::None);
        rig.tools.menu_layer = BulldozeLayer::Power;
        rig.click(rig.rects.bulldozer().unwrap().center());
        assert_eq!(rig.tools.active, ActiveTool::Bulldozer);
        assert_eq!(rig.tools.bulldoze_layer, BulldozeLayer::Power);
    }

    #[test]
    fn the_details_button_opens_the_window() {
        let mut rig = Rig::new(view_with(0.0, 0.0, 0.0, 0.0));
        assert!(!rig.state.demand_open);
        let at = rig.rects.details.unwrap().center();
        rig.click(at);
        assert!(rig.state.demand_open);
    }

    /// T-317: the categories and the global tools are in the bar; a click on a category opens
    /// its tray, a global tool works from any category.
    #[test]
    fn the_bar_holds_the_categories_and_the_global_tools() {
        let mut rig = Rig::new(SimRunner::new(1).view());
        // Without content: Zones and Districts only.
        assert_eq!(rig.rects.categories.len(), 2);
        let globals: Vec<_> = rig.rects.globals.iter().map(|(t, _)| *t).collect();
        assert_eq!(globals.len(), 3);
        assert!(
            !globals.contains(&ActiveTool::Junctions),
            "in the overlay panel"
        );
        // T-317f: categories and global tools are one group in the middle of the bar.
        let left = rig.rects.categories[0].1.left();
        let right = rig.rects.globals.last().unwrap().1.right();
        let mid = rig.rects.panel.unwrap().center().x;
        assert!(
            ((left + right) * 0.5 - mid).abs() < 6.0,
            "{left}..{right} vs {mid}"
        );
        for tool in [ActiveTool::Upgrade, ActiveTool::Flip] {
            assert!(globals.contains(&tool), "{tool:?}");
            let at = rig
                .rects
                .globals
                .iter()
                .find(|(t, _)| *t == tool)
                .unwrap()
                .1;
            rig.click(at.center());
            assert_eq!(rig.tools.active, tool);
            let at = rig
                .rects
                .globals
                .iter()
                .find(|(t, _)| *t == tool)
                .unwrap()
                .1;
            rig.click(at.center());
            assert_eq!(rig.tools.active, ActiveTool::None, "{tool:?} toggles");
        }
        let districts = rig.rects.categories[1];
        rig.click(districts.1.center());
        assert_eq!(rig.state.category, Some(districts.0));
    }

    #[test]
    fn an_empty_city_draws_without_panicking() {
        let rig = Rig::new(SimRunner::new(1).view());
        assert_eq!(rig.rects.columns.len(), 4);
        assert!(!zone_tooltip(&rig.view, ZoneType::Industrial).is_empty());
    }

    fn view_on_flatlands(connected: bool) -> SimView {
        let terrain = std::sync::Arc::new(
            cp_terrain::Terrain::load(
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../assets/maps/flatlands"),
            )
            .unwrap(),
        );
        let mut sim = SimRunner::new(1);
        sim.set_terrain(terrain);
        let mut view = sim.view();
        let city = std::sync::Arc::make_mut(&mut view.city);
        city.tick = 64;
        city.connected_to_highway = connected;
        view
    }

    #[test]
    fn the_not_connected_notice_shows_only_while_unconnected_and_show_centres_the_camera() {
        let rig = Rig::new(view_on_flatlands(true));
        assert!(!rig.rects.notice && rig.rects.show_stub.is_none());

        let mut rig = Rig::new(view_on_flatlands(false));
        assert!(rig.rects.notice);
        let btn = rig.rects.show_stub.expect("a show button");
        assert!(rig.state.camera_goal.is_none());
        rig.click(btn.center());
        let stub = rig
            .view
            .terrain
            .as_deref()
            .unwrap()
            .meta()
            .entry_road
            .as_ref()
            .unwrap()
            .stub_end();
        assert_eq!(rig.state.camera_goal, Some(stub));

        // A city that has not published stats yet, or a map without an entry road, says nothing.
        let mut view = view_on_flatlands(false);
        std::sync::Arc::make_mut(&mut view.city).tick = 0;
        assert!(!Rig::new(view).rects.notice);
        let mut view = view_with(0.0, 0.0, 0.0, 0.0);
        std::sync::Arc::make_mut(&mut view.city).tick = 64;
        assert!(!Rig::new(view).rects.notice);
    }
}
