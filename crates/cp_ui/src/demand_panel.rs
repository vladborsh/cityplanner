//! The bottom demand panel (T-099, `architecture/system-design.md` §13.7): a real panel under
//! the 3D view, right of the palette and the tool panel, with one column per zone type — its
//! icon and a vertical demand bar from −100 to +100 with zero in the middle. Demand is per
//! group in M1, so the two residential and the two commercial columns show one value and are
//! joined by a bracket with the group's letter. It folds into a 28 px strip.

use bevy_egui::egui;
use cp_content::ZoneType;
use cp_sim::SimView;
use cp_sim::stats::ZONES;
use cp_tools::{ActiveTool, ToolState};

use crate::budget::demand_fraction;
use crate::icons::zone_icon;
use crate::palette::Category;
use crate::panels::{UiState, zone_group_demand};
use crate::style::Sizes;
use crate::windows::{color, zone_tooltip};

/// Where one column was drawn.
#[derive(Clone, Copy, Debug)]
pub struct Column {
    pub zone: ZoneType,
    /// The whole clickable column.
    pub rect: egui::Rect,
    /// The bar's track (−100 at the bottom, +100 at the top).
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
}

/// Draws the panel into `root` (after the left panels, so it takes the rest of the width).
pub fn demand_panel(
    root: &mut egui::Ui,
    view: &SimView,
    state: &mut UiState,
    tools: &mut ToolState,
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
                    let r = ui.small_button("▴").on_hover_text("Show demand");
                    rects.fold = Some(r.rect);
                    if r.clicked() {
                        state.demand_collapsed = false;
                    }
                    ui.label("Demand");
                });
                return;
            }
            ui.horizontal(|ui| {
                // The controls, left of the columns.
                ui.vertical(|ui| {
                    ui.set_width(Sizes::DEMAND_COL_W);
                    ui.strong("Demand");
                    let r = ui.small_button("▾").on_hover_text("Fold the panel");
                    rects.fold = Some(r.rect);
                    if r.clicked() {
                        state.demand_collapsed = true;
                    }
                    let r = ui
                        .small_button("▸")
                        .on_hover_text("Demand by zone (window)");
                    rects.details = Some(r.rect);
                    if r.clicked() {
                        state.demand_open = true;
                    }
                });
                for zone in ZONES {
                    let col = column(ui, view, zone);
                    if col.1.clicked() {
                        state.category = Some(Category::Zones);
                        if tools.active != ActiveTool::Zone {
                            tools.select(ActiveTool::Zone);
                        }
                        tools.zone.zone = zone;
                    }
                    rects.columns.push(col.0);
                }
            });
        });
    rects.panel = Some(out.response.rect);
    // Brackets under the paired groups (R: the two residential, C: the two commercial).
    if !state.demand_collapsed {
        bracket(
            root,
            &rects.columns,
            ZoneType::ResLow,
            ZoneType::ResHigh,
            "R",
        );
        bracket(
            root,
            &rects.columns,
            ZoneType::ComLow,
            ZoneType::ComHigh,
            "C",
        );
    }
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
    egui::Image::new(zone_icon(zone).image()).paint_at(ui, icon);
    let track = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.bottom() - Sizes::DEMAND_BAR_H * 0.5),
        egui::vec2(14.0, Sizes::DEMAND_BAR_H),
    );
    let (_, value) = zone_group_demand(zone, &view.demand);
    let level_y = track.bottom() - demand_fraction(value) * track.height();
    painter.rect_filled(track, 2.0, ui.visuals().extreme_bg_color);
    // Fill from the zero line (the middle) to the level.
    let mid = track.center().y;
    let fill = egui::Rect::from_x_y_ranges(track.x_range(), level_y.min(mid)..=level_y.max(mid));
    painter.rect_filled(fill, 0.0, color(zone));
    painter.line_segment(
        [track.left_center(), track.right_center()],
        egui::Stroke::new(1.0, ui.visuals().text_color()),
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

/// A bracket with `letter` under the columns of `a` and `b`.
fn bracket(root: &egui::Ui, cols: &[Column], a: ZoneType, b: ZoneType, letter: &str) {
    let (Some(a), Some(b)) = (
        cols.iter().find(|c| c.zone == a),
        cols.iter().find(|c| c.zone == b),
    ) else {
        return;
    };
    let y = a.rect.bottom() + 2.0;
    let (x0, x1) = (a.rect.center().x, b.rect.center().x);
    let painter = root.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("demand_brackets"),
    ));
    let stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(150));
    painter.line_segment([egui::pos2(x0, y - 3.0), egui::pos2(x0, y)], stroke);
    painter.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], stroke);
    painter.line_segment([egui::pos2(x1, y), egui::pos2(x1, y - 3.0)], stroke);
    painter.text(
        egui::pos2((x0 + x1) * 0.5, y + 1.0),
        egui::Align2::CENTER_TOP,
        letter,
        egui::TextStyle::Small.resolve(&root.ctx().global_style()),
        egui::Color32::from_gray(190),
    );
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
    fn six_columns_in_zone_order_with_bars_proportional_to_their_groups_demand() {
        let rig = Rig::new(view_with(60.0, -30.0, 100.0, -100.0));
        let zones: Vec<_> = rig.rects.columns.iter().map(|c| c.zone).collect();
        assert_eq!(zones, ZONES.to_vec());
        for col in &rig.rects.columns {
            let (_, v) = zone_group_demand(col.zone, &rig.view.demand);
            let frac = (col.track.bottom() - col.level_y) / col.track.height();
            assert!((frac - demand_fraction(v)).abs() < 1e-4, "{:?}", col.zone);
        }
        // The pairs share one value, so one height.
        let level = |z| {
            rig.rects
                .columns
                .iter()
                .find(|c| c.zone == z)
                .unwrap()
                .level_y
        };
        assert_eq!(level(ZoneType::ResLow), level(ZoneType::ResHigh));
        assert_eq!(level(ZoneType::ComLow), level(ZoneType::ComHigh));
        assert_ne!(level(ZoneType::ResLow), level(ZoneType::ComLow));
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
            .find(|c| c.zone == ZoneType::ComHigh)
            .unwrap()
            .rect
            .center();
        rig.click(at);
        assert_eq!(rig.state.category, Some(Category::Zones));
        assert_eq!(rig.tools.active, ActiveTool::Zone);
        assert_eq!(rig.tools.zone.zone, ZoneType::ComHigh);
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
        assert_eq!(rig.rects.columns.len(), 6);
        assert!(!zone_tooltip(&rig.view, ZoneType::Industrial).is_empty());
    }
}
