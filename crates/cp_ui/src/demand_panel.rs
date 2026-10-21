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
use cp_render::OverlayState;
use cp_sim::SimView;
use cp_tools::{ActiveTool, BulldozeLayer, ToolState};

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

/// Where the parts of the panel were drawn (tests click them).
#[derive(Clone, Debug, Default)]
pub struct DemandRects {
    pub panel: Option<egui::Rect>,
    pub columns: Vec<Column>,
    /// The fold / unfold button.
    pub fold: Option<egui::Rect>,
    /// The button that opens the window "Demand by zone".
    pub details: Option<egui::Rect>,
    /// The "show" button of the not-connected notice, when the notice is visible.
    pub show_stub: Option<egui::Rect>,
    /// The notice is on screen.
    pub notice: bool,
    /// The balance (a click opens the budget) and the population (T-289: moved here from the
    /// top bar).
    pub money: Option<egui::Rect>,
    pub population: Option<egui::Rect>,
    /// The utility gauges (T-279).
    pub gauges: Vec<(cp_sim::city_services::Sector, egui::Rect)>,
    /// The bulldozer button, right of the demand columns (T-289).
    pub bulldozer: Option<egui::Rect>,
}

/// Width of the block of money, population and gauges at the left of the panel.
const STATS_W: f32 = 330.0;
/// Icon of the bulldozer button.
const BULLDOZER_ICON: f32 = 48.0;

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

/// Money, income, population and its change, and the utility gauges (T-289, T-279): the city's
/// status block at the left of the bottom panel. In the folded strip they stand in one row.
fn stats(
    ui: &mut egui::Ui,
    view: &SimView,
    state: &mut UiState,
    overlay: &mut OverlayState,
    rects: &mut DemandRects,
    one_row: bool,
) {
    if one_row {
        money_row(ui, view, state, rects);
        population_row(ui, view, rects);
        rects.gauges = crate::gauges::utility_gauges(ui, view, overlay);
        return;
    }
    ui.vertical(|ui| {
        ui.set_width(STATS_W);
        ui.horizontal(|ui| money_row(ui, view, state, rects));
        ui.horizontal(|ui| population_row(ui, view, rects));
        ui.horizontal(|ui| rects.gauges = crate::gauges::utility_gauges(ui, view, overlay));
    });
}

/// The bulldozer button (T-289): the tool of the open menu's layer — Power and Water take only
/// their own buildings and links (T-286) — on and off; B does the same.
fn bulldozer_button(ui: &mut egui::Ui, tools: &mut ToolState, size: f32) -> egui::Rect {
    let on = tools.active == ActiveTool::Bulldozer;
    let hover = match tools.menu_layer {
        BulldozeLayer::All => tr("Bulldozer (B)"),
        BulldozeLayer::Power => tr("Bulldozer (B) — power lines and buildings only"),
        BulldozeLayer::Water => tr("Bulldozer (B) — pipes and water buildings only"),
    };
    let r = crate::icons::Icon::Bulldozer
        .button(ui, size, Sizes::BUTTON_PAD, on)
        .on_hover_text(hover);
    if r.clicked() {
        tools.toggle_bulldozer();
    }
    r.rect
}

/// Draws the panel into `root` (after the left panels, so it takes the rest of the width): the
/// status block, the demand columns, the bulldozer button.
pub fn demand_panel(
    root: &mut egui::Ui,
    view: &SimView,
    state: &mut UiState,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
) -> DemandRects {
    let mut rects = DemandRects::default();
    let height = if state.demand_collapsed {
        Sizes::DEMAND_COLLAPSED_H
    } else {
        Sizes::DEMAND_H
    };
    let out = egui::Panel::bottom("demand")
        .exact_size(height)
        .resizable(false)
        .show(root, |ui| {
            if state.demand_collapsed {
                ui.horizontal_centered(|ui| {
                    stats(ui, view, state, overlay, &mut rects, true);
                    ui.separator();
                    let r = ui.small_button("▴").on_hover_text(tr("Show demand"));
                    rects.fold = Some(r.rect);
                    if r.clicked() {
                        state.demand_collapsed = false;
                    }
                    ui.label(tr("Demand"));
                    rects.bulldozer = Some(bulldozer_button(ui, tools, 20.0));
                    if unconnected_stub(view).is_some() {
                        rects.notice = true;
                        ui.colored_label(egui::Color32::YELLOW, format!("⚠ {}", tr(NOT_CONNECTED)));
                    }
                });
                return;
            }
            ui.horizontal(|ui| {
                stats(ui, view, state, overlay, &mut rects, false);
                ui.separator();
                // The controls, left of the columns.
                ui.vertical(|ui| {
                    ui.set_width(Sizes::DEMAND_COL_W);
                    ui.strong(tr("Demand"));
                    let r = ui.small_button("▾").on_hover_text(tr("Fold the panel"));
                    rects.fold = Some(r.rect);
                    if r.clicked() {
                        state.demand_collapsed = true;
                    }
                    let r = ui
                        .small_button("▸")
                        .on_hover_text(tr("Demand by zone (window)"));
                    rects.details = Some(r.rect);
                    if r.clicked() {
                        state.demand_open = true;
                    }
                });
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
                ui.separator();
                rects.bulldozer = Some(bulldozer_button(ui, tools, BULLDOZER_ICON));
                if let Some(stub) = unconnected_stub(view) {
                    rects.notice = true;
                    ui.vertical(|ui| {
                        ui.add_space(8.0);
                        ui.set_max_width(260.0);
                        ui.colored_label(egui::Color32::YELLOW, format!("⚠ {}", tr(NOT_CONNECTED)));
                        let r = ui
                            .button(tr("Show"))
                            .on_hover_text(tr("Centre the camera on the dead end"));
                        rects.show_stub = Some(r.rect);
                        if r.clicked() {
                            state.camera_goal = Some(stub);
                        }
                    });
                }
            });
        });
    rects.panel = Some(out.response.rect);
    rects
}

/// One column: the icon over the bar, sensitive to clicks, with the tooltip.
fn column(ui: &mut egui::Ui, view: &SimView, zone: ZoneType) -> (Column, egui::Response) {
    let size = egui::vec2(
        Sizes::DEMAND_COL_W,
        Sizes::DEMAND_ICON + 4.0 + Sizes::DEMAND_BAR_H,
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let icon = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + Sizes::DEMAND_ICON * 0.5),
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
        egui::pos2(rect.center().x, rect.bottom() - Sizes::DEMAND_BAR_H * 0.5),
        egui::vec2(14.0, Sizes::DEMAND_BAR_H),
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
        overlay: OverlayState,
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
                overlay: OverlayState::default(),
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
                overlay,
                view,
                rects,
                t,
            } = self;
            frame(ctx, input(*t, events), |ui| {
                let mut root = crate::panels::root_ui(ui.ctx());
                *rects = demand_panel(&mut root, view, state, tools, overlay);
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
    fn the_panel_is_120_high_and_sits_under_the_view() {
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
    fn view_with_utilities(figures: [(f32, f32); 3]) -> SimView {
        use cp_sim::city_services::{Sector, SectorBalance};
        let mut view = SimRunner::new(1).view();
        let mut stats = (*view.city).clone();
        stats.services = [Sector::Power, Sector::Water, Sector::Sewage]
            .into_iter()
            .zip(figures)
            .map(|(sector, (supply, demand))| SectorBalance {
                sector,
                supply,
                demand,
                unit: "units/day",
                locked: false,
            })
            .collect();
        view.city = std::sync::Arc::new(stats);
        view
    }

    fn texts(rig: &mut Rig, events: Vec<egui::Event>) -> Vec<String> {
        rig.t += 0.1;
        let Rig {
            ctx,
            state,
            tools,
            overlay,
            view,
            rects,
            t,
        } = rig;
        crate::testutil::frame_texts(ctx, input(*t, events), |ui| {
            let mut root = crate::panels::root_ui(ui.ctx());
            *rects = demand_panel(&mut root, view, state, tools, overlay);
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

    /// T-279: power, water and sewage as gauges with `supply / demand` on them, left of the
    /// demand columns; a click opens the overlay, a hover lists the networks.
    #[test]
    fn the_bottom_panel_has_the_utility_gauges_and_a_click_opens_their_overlay() {
        use cp_render::Overlay;
        use cp_sim::city_services::Sector;
        let mut rig = Rig::new(view_with_utilities([
            (340.0, 60.0),
            (100.0, 95.0),
            (10.0, 40.0),
        ]));
        let all = texts(&mut rig, vec![]);
        for t in ["340 / 60", "100 / 95", "10 / 40"] {
            assert!(all.iter().any(|s| s == t), "{t:?} in {all:?}");
        }
        let sectors: Vec<Sector> = rig.rects.gauges.iter().map(|(s, _)| *s).collect();
        assert_eq!(sectors, [Sector::Power, Sector::Water, Sector::Sewage]);
        let panel = rig.rects.panel.unwrap();
        for (s, r) in &rig.rects.gauges {
            assert!(panel.contains_rect(*r), "{s:?} {r:?} outside {panel:?}");
            assert!(r.right() < rig.rects.columns[0].rect.left());
        }
        let power = rig.rects.gauges[0].1.center();
        rig.click(power);
        assert_eq!(rig.overlay.active, Overlay::Power);
        let sewage = rig.rects.gauges[2].1.center();
        rig.click(sewage);
        assert_eq!(
            rig.overlay.active,
            Overlay::Water,
            "sewage runs in the pipes"
        );
        // Hover the power gauge: the tooltip shows after the pointer rests.
        let mut all = Vec::new();
        // Move the pointer once, then let it rest: the tooltip comes after its delay.
        texts(&mut rig, vec![egui::Event::PointerMoved(power)]);
        for _ in 0..12 {
            all = texts(&mut rig, vec![]);
        }
        assert!(
            all.iter().any(|t| t.starts_with("Electricity: 340 / 60")),
            "{all:?}"
        );
        assert!(all.iter().any(|t| t == "No network yet"), "{all:?}");
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
        let mut rig = Rig::new(view_with_utilities([
            (1_240_000.0, 1_100_000.0),
            (88_000.0, 99_000.0),
            (8888.0, 8888.0),
        ]));
        rig.frame(vec![]);
        let panel = rig.rects.panel.unwrap();
        let bull = rig.rects.bulldozer.unwrap();
        assert!(panel.contains_rect(bull), "{bull:?} not in {panel:?}");
        // Palette (72) and an open tool panel (300) leave 908: the bulldozer must still be in.
        assert!(bull.right() <= 1280.0 - 72.0 - 300.0, "{lang:?}: {bull:?}");
        assert!(
            bull.left() > rig.rects.columns[3].rect.right(),
            "right of the demand"
        );
        for (_, g) in &rig.rects.gauges {
            assert!(panel.contains_rect(*g), "{g:?} not in {panel:?}");
        }
    }

    /// T-289: the bulldozer button takes the bulldozer up and puts it down; with the Power or
    /// Water menu open it is that layer's.
    #[test]
    fn the_bulldozer_button_toggles_the_bulldozer_in_the_open_menus_layer() {
        use cp_tools::BulldozeLayer;
        let mut rig = Rig::new(SimRunner::new(1).view());
        assert_eq!(rig.tools.active, ActiveTool::None);
        rig.click(rig.rects.bulldozer.unwrap().center());
        assert_eq!(rig.tools.active, ActiveTool::Bulldozer);
        assert_eq!(rig.tools.bulldoze_layer, BulldozeLayer::All);
        rig.click(rig.rects.bulldozer.unwrap().center());
        assert_eq!(rig.tools.active, ActiveTool::None);
        rig.tools.menu_layer = BulldozeLayer::Power;
        rig.click(rig.rects.bulldozer.unwrap().center());
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

    #[test]
    fn it_folds_to_a_28_strip_and_unfolds() {
        let mut rig = Rig::new(view_with(0.0, 0.0, 0.0, 0.0));
        let at = rig.rects.fold.unwrap().center();
        rig.click(at);
        assert!(rig.state.demand_collapsed);
        assert_eq!(rig.rects.panel.unwrap().height(), Sizes::DEMAND_COLLAPSED_H);
        assert!(rig.rects.columns.is_empty());
        let at = rig.rects.fold.unwrap().center();
        rig.click(at);
        assert!(!rig.state.demand_collapsed);
        assert_eq!(rig.rects.panel.unwrap().height(), Sizes::DEMAND_H);
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
