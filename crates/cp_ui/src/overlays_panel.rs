//! The permanent panel at the top right of the map (T-289): every map overlay as an icon in a
//! compact grid under its group's header, the active one lit, its name as the tooltip (T-317f).
//! The Junctions section holds the Junctions tool beside its overlay. A small arrow folds the
//! panel to a thin strip (O does the same). T-298: the buttons for the windows and the settings are one row at the right end of
//! the top bar ([`window_bar`]).

use bevy_egui::egui;
use cp_locale::tr;
use cp_render::{LightingSettings, Overlay, OverlayState};
use cp_tools::{ActiveTool, ToolState};

use crate::files::{FileMenu, toggle_saves};
use crate::icons::{Icon, overlay_icon};
use crate::panels::{UI_SCALE_RANGE, UiState, UiWindow};
use crate::style::Sizes;

/// Icon of an overlay button and of a window button.
const OVERLAY_ICON: f32 = 24.0;
const WINDOW_ICON: f32 = 24.0;
/// Height of the button row in the top bar.
const WINDOW_BAR_H: f32 = WINDOW_ICON + 2.0 * 4.0;
const OVERLAY_COLUMNS: usize = 4;
/// Side of one overlay cell: the icon button.
const CELL_W: f32 = OVERLAY_ICON + 2.0 * Sizes::BUTTON_PAD;

/// The groups of the panel (T-317): a header and its overlays. Every overlay but `Off` is in
/// exactly one. The last, Junctions, also holds the Junctions tool (T-317f).
pub const GROUPS: [(&str, &[Overlay]); 6] = [
    ("Utilities", &[Overlay::Power, Overlay::Water]),
    (
        "Services",
        &[
            Overlay::Health,
            Overlay::Police,
            Overlay::Fire,
            Overlay::Education,
            Overlay::Garbage,
            Overlay::Deathcare,
            Overlay::Mail,
        ],
    ),
    (
        "Environment",
        &[
            Overlay::Pollution,
            Overlay::Noise,
            Overlay::LandValue,
            Overlay::Resources,
        ],
    ),
    ("Traffic", &[Overlay::Traffic, Overlay::Transit]),
    (
        "City",
        &[
            Overlay::Happiness,
            Overlay::Zones,
            Overlay::Districts,
            Overlay::IndustryAreas,
            Overlay::Parks,
            Overlay::Land,
            Overlay::Underground,
        ],
    ),
    ("Junctions", &[Overlay::Junctions]),
];
/// Width of the folded strip, and of the open panel.
const STRIP_W: f32 = 22.0;
/// Margin from the screen's right edge.
const MARGIN: f32 = 8.0;

/// Where the parts of the panel were drawn (tests click them).
#[derive(Clone, Debug, Default)]
pub struct OverlayRects {
    pub panel: Option<egui::Rect>,
    /// The fold / unfold arrow.
    pub fold: Option<egui::Rect>,
    /// The window switches; the budget's too.
    pub windows: Vec<(UiWindow, egui::Rect)>,
    pub overlays: Vec<(Overlay, egui::Rect)>,
    /// The Junctions tool of the Junctions section.
    pub junctions_tool: Option<egui::Rect>,
    /// The settings button and (while its menu is open) the always-day switch.
    pub settings: Option<egui::Rect>,
    pub always_day: Option<egui::Rect>,
}

/// The windows of the row, in order (one row in the top bar): money, charts, services, lines; policies,
/// saves, debug; then the settings button follows.
const ROW: [UiWindow; 10] = [
    UiWindow::Budget,
    UiWindow::Metrics,
    UiWindow::Jobs,
    UiWindow::Services,
    UiWindow::Lines,
    UiWindow::Policies,
    UiWindow::Offers,
    UiWindow::Saves,
    UiWindow::Demand,
    UiWindow::Debug,
];

/// Width the open panel takes from the right edge, margin included; the overlay services panel
/// stands left of it.
pub fn width(state: &UiState) -> f32 {
    MARGIN
        + if state.overlays_collapsed {
            STRIP_W + 8.0
        } else {
            OVERLAY_COLUMNS as f32 * (CELL_W + Sizes::TILE_GAP) + 12.0
        }
}

fn window_on(state: &UiState, files: &FileMenu, w: UiWindow) -> bool {
    match w {
        UiWindow::Budget => state.budget_open,
        UiWindow::Demand => state.demand_open,
        UiWindow::Metrics => state.metrics_open,
        UiWindow::Jobs => state.jobs_open,
        UiWindow::Services => state.services_open,
        UiWindow::Lines => state.lines_open,
        UiWindow::Policies => state.policies_open,
        UiWindow::Offers => state.offers_open,
        UiWindow::Saves => files.open,
        UiWindow::Debug => state.debug_open,
    }
}

fn toggle_window(state: &mut UiState, files: &mut FileMenu, w: UiWindow) {
    let on = window_on(state, files, w);
    match w {
        UiWindow::Budget => state.budget_open = !on,
        UiWindow::Demand => state.demand_open = !on,
        UiWindow::Metrics => state.metrics_open = !on,
        UiWindow::Jobs => state.jobs_open = !on,
        UiWindow::Services => state.services_open = !on,
        UiWindow::Lines => state.lines_open = !on,
        UiWindow::Policies => state.policies_open = !on,
        UiWindow::Offers => state.offers_open = !on,
        UiWindow::Saves => toggle_saves(files),
        UiWindow::Debug => state.debug_open = !on,
    }
}

/// Draws the panel at the top right, under the top bar.
pub fn overlays_panel(
    ctx: &egui::Context,
    state: &mut UiState,
    overlay: &mut OverlayState,
    tools: &mut ToolState,
    lighting: &mut LightingSettings,
    files: &mut FileMenu,
    view: Option<&cp_sim::SimView>,
) -> OverlayRects {
    let mut rects = OverlayRects::default();
    let offers = view.map_or(0, |v| v.corruption.inbox.len());
    window_bar(ctx, state, files, offers, &mut rects);
    let area = egui::Area::new(egui::Id::new("overlays_panel"))
        .anchor(
            egui::Align2::RIGHT_TOP,
            egui::vec2(-MARGIN, Sizes::TOP_BAR_H + MARGIN),
        )
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    // The fold arrow, always there; "No overlay" beside it (T-317f: one row).
                    ui.horizontal(|ui| {
                        let arrow = if state.overlays_collapsed {
                            "◂"
                        } else {
                            "▸"
                        };
                        let r = ui
                            .small_button(arrow)
                            .on_hover_text(if state.overlays_collapsed {
                                tr("Show the overlays (O)")
                            } else {
                                tr("Fold the panel (O)")
                            });
                        rects.fold = Some(r.rect);
                        if r.clicked() {
                            state.overlays_collapsed = !state.overlays_collapsed;
                        }
                        if state.overlays_collapsed {
                            return;
                        }
                        let off = Overlay::Off;
                        let r = overlay_icon(off)
                            .button(ui, OVERLAY_ICON, Sizes::BUTTON_PAD, overlay.active == off)
                            .on_hover_text(tr(off.label()));
                        rects.overlays.push((off, r.rect));
                        if r.clicked() {
                            overlay.open(off);
                        }
                    });
                    if state.overlays_collapsed {
                        return;
                    }
                    ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
                    // The groups with their headers, icons only.
                    let mut cell = |ui: &mut egui::Ui, o: Overlay| {
                        let on = overlay.active == o;
                        let r = overlay_icon(o)
                            .button(ui, OVERLAY_ICON, Sizes::BUTTON_PAD, on)
                            .on_hover_ui(|ui| {
                                ui.strong(tr(o.label()));
                                if let Some(v) = view {
                                    crate::gauges::overlay_hover(ui, v, o);
                                }
                            });
                        rects.overlays.push((o, r.rect));
                        if r.clicked() {
                            overlay.open(o);
                        }
                    };
                    for (title, members) in GROUPS {
                        ui.weak(tr(title));
                        let junctions = members == [Overlay::Junctions];
                        for row in members.chunks(OVERLAY_COLUMNS) {
                            ui.horizontal(|ui| {
                                for &o in row {
                                    cell(ui, o);
                                }
                                if junctions {
                                    let on = tools.active == ActiveTool::Junctions;
                                    let r = Icon::Junctions
                                        .button(ui, OVERLAY_ICON, Sizes::BUTTON_PAD, on)
                                        .on_hover_text(tr("Junctions tool"));
                                    rects.junctions_tool = Some(r.rect);
                                    if r.clicked() {
                                        tools.select(if on {
                                            ActiveTool::None
                                        } else {
                                            ActiveTool::Junctions
                                        });
                                    }
                                    // Its mode: lights and signs, or lane turns (T-313).
                                    if on {
                                        crate::lane_rules::mode_buttons(
                                            ui,
                                            tools,
                                            OVERLAY_ICON,
                                            Sizes::BUTTON_PAD,
                                        );
                                    }
                                }
                            });
                        }
                    }
                });
        });
    rects.panel = Some(area.response.rect);
    if state.settings_open
        && let Some(anchor) = rects.settings
    {
        settings_popup(ctx, anchor, state, lighting, &mut rects);
    }
    rects
}

/// The window buttons and the settings button, one row at the right end of the top bar (T-298).
fn window_bar(
    ctx: &egui::Context,
    state: &mut UiState,
    files: &mut FileMenu,
    offers: usize,
    rects: &mut OverlayRects,
) {
    egui::Area::new(egui::Id::new("window_bar"))
        .anchor(
            egui::Align2::RIGHT_TOP,
            egui::vec2(-MARGIN, (Sizes::TOP_BAR_H - WINDOW_BAR_H) * 0.5),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.set_height(WINDOW_BAR_H);
                for w in ROW {
                    let on = window_on(state, files, w);
                    let r = w
                        .icon()
                        .button(ui, WINDOW_ICON, Sizes::BUTTON_PAD, on)
                        .on_hover_text(tr(w.label()));
                    // The offers waiting, a number in the corner of their button (T-325).
                    if w == UiWindow::Offers && offers > 0 {
                        let at = r.rect.right_top() + egui::vec2(-5.0, 5.0);
                        let p = ui.painter();
                        p.circle_filled(at, 7.0, egui::Color32::from_rgb(200, 60, 50));
                        p.text(
                            at,
                            egui::Align2::CENTER_CENTER,
                            offers.to_string(),
                            egui::FontId::proportional(10.0),
                            egui::Color32::WHITE,
                        );
                    }
                    rects.windows.push((w, r.rect));
                    if r.clicked() {
                        toggle_window(state, files, w);
                    }
                }
                let r = Icon::Settings
                    .button(ui, WINDOW_ICON, Sizes::BUTTON_PAD, state.settings_open)
                    .on_hover_text(tr("Settings"));
                rects.settings = Some(r.rect);
                if r.clicked() {
                    state.settings_open = !state.settings_open;
                }
            });
        });
}

/// The settings menu, under the settings button; a press anywhere else closes it.
fn settings_popup(
    ctx: &egui::Context,
    anchor: egui::Rect,
    state: &mut UiState,
    lighting: &mut LightingSettings,
    rects: &mut OverlayRects,
) {
    let area = egui::Area::new(egui::Id::new("settings_popup"))
        .order(egui::Order::Foreground)
        .anchor(
            egui::Align2::RIGHT_TOP,
            egui::vec2(-(width(state) + Sizes::TILE_GAP), anchor.top() - 0.0),
        )
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                // Render only: the game clock and everyone's routine keep running at night.
                let r = ui.checkbox(&mut lighting.always_day, tr("Always day"));
                rects.always_day = Some(r.rect);
                // Shadows of passing clouds (T-272).
                ui.checkbox(&mut lighting.cloud_shadows, tr("Cloud shadows"));
                // Parked cars along the kerbs (T-276).
                ui.checkbox(&mut lighting.parked_cars, tr("Parked cars"));
                let prefs = &mut state.prefs;
                // The language (T-284): its own name in each, so it reads in either; applies live.
                ui.horizontal(|ui| {
                    ui.label("Language / Язык");
                    for lang in cp_locale::Lang::ALL {
                        if ui
                            .selectable_label(prefs.lang == lang, lang.native_name())
                            .clicked()
                        {
                            prefs.lang = lang;
                            cp_locale::set_lang(lang);
                        }
                    }
                });
                ui.add(egui::Slider::new(&mut prefs.ui_scale, UI_SCALE_RANGE).text(tr("UI scale")));
                // Sound (T-189).
                ui.checkbox(&mut prefs.audio.mute, tr("Mute"));
                ui.add_enabled(
                    !prefs.audio.mute,
                    egui::Slider::new(&mut prefs.audio.master, 0.0..=1.0).text(tr("Volume")),
                );
                ui.add_enabled(
                    !prefs.audio.mute,
                    egui::Slider::new(&mut prefs.audio.effects, 0.0..=1.0).text(tr("Effects")),
                );
                let cam = &mut prefs.camera;
                ui.add(egui::Slider::new(&mut cam.pan_speed, 0.2..=3.0).text(tr("Pan speed")));
                ui.add(
                    egui::Slider::new(&mut cam.key_rotate_speed, 0.4..=4.0)
                        .text(tr("Rotation speed")),
                );
                ui.add(
                    egui::Slider::new(&mut cam.drag_rotate_sens, 0.001..=0.02)
                        .text(tr("Mouse sensitivity")),
                );
                // Distance factor per notch: smaller = faster, shown the other way round.
                let mut zoom = 1.0 - cam.zoom_step;
                if ui
                    .add(egui::Slider::new(&mut zoom, 0.02..=0.4).text(tr("Zoom step")))
                    .changed()
                {
                    cam.zoom_step = 1.0 - zoom;
                }
                ui.checkbox(&mut cam.edge_pan, tr("Pan at the screen edge"));
                ui.checkbox(&mut prefs.advisor, tr("City advisor"));
                ui.add_enabled(
                    prefs.advisor,
                    egui::Checkbox::new(&mut prefs.advisor_problems_only, tr("Only problems")),
                );
            });
        });
    let rect = area.response.rect;
    let outside = ctx.input(|i| i.pointer.any_pressed())
        && ctx
            .pointer_interact_pos()
            .is_some_and(|p| !rect.contains(p) && !anchor.contains(p));
    if outside {
        state.settings_open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{click, frame, input};

    /// The panel with its state kept between frames.
    struct Rig {
        ctx: egui::Context,
        state: UiState,
        overlay: OverlayState,
        tools: ToolState,
        lighting: LightingSettings,
        files: FileMenu,
        rects: OverlayRects,
        t: f64,
    }

    impl Rig {
        fn new() -> Self {
            let mut rig = Rig {
                ctx: egui::Context::default(),
                state: UiState::default(),
                overlay: OverlayState::default(),
                tools: ToolState::default(),
                lighting: LightingSettings::default(),
                files: FileMenu::default(),
                rects: OverlayRects::default(),
                t: 0.0,
            };
            egui_extras::install_image_loaders(&rig.ctx);
            rig.frame(vec![]);
            rig.frame(vec![]);
            rig.frame(vec![]);
            rig
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            self.t += 0.1;
            let Rig {
                ctx,
                state,
                overlay,
                tools,
                lighting,
                files,
                rects,
                t,
            } = self;
            frame(ctx, input(*t, events), |ui| {
                *rects = overlays_panel(ui.ctx(), state, overlay, tools, lighting, files, None);
            });
        }

        fn click(&mut self, pos: egui::Pos2) {
            let [down, up] = click(pos);
            self.frame(down);
            self.frame(up);
            self.frame(vec![]);
        }
    }

    #[test]
    fn every_overlay_is_an_icon_in_a_group_and_a_click_selects_it_and_the_panel_stays() {
        let mut rig = Rig::new();
        assert_eq!(rig.rects.overlays.len(), Overlay::ALL.len());
        // Every overlay but "No overlay" is in exactly one group.
        let mut grouped: Vec<Overlay> =
            GROUPS.iter().flat_map(|(_, m)| m.iter().copied()).collect();
        grouped.sort_by_key(|o| Overlay::ALL.iter().position(|a| a == o));
        let mut want: Vec<Overlay> = Overlay::ALL
            .into_iter()
            .filter(|o| *o != Overlay::Off)
            .collect();
        want.sort_by_key(|o| Overlay::ALL.iter().position(|a| a == o));
        assert_eq!(grouped, want);
        let cols: std::collections::BTreeSet<i32> = rig
            .rects
            .overlays
            .iter()
            .filter(|(o, _)| *o != Overlay::Off)
            .map(|(_, r)| r.left().round() as i32)
            .collect();
        assert_eq!(cols.len(), OVERLAY_COLUMNS);
        // T-317f: "No overlay" is in the row of the fold arrow.
        let off = rig.rects.overlays.iter().find(|(o, _)| *o == Overlay::Off);
        let fold = rig.rects.fold.unwrap();
        let off = off.unwrap().1;
        assert!(
            off.left() > fold.right() && off.top() < fold.bottom(),
            "{off:?} {fold:?}"
        );
        // Top right, under the top bar.
        let panel = rig.rects.panel.unwrap();
        assert!(panel.right() > 1200.0 && panel.top() >= Sizes::TOP_BAR_H);
        // A click picks the overlay, as the player's own (it stays until closed).
        let at = rig
            .rects
            .overlays
            .iter()
            .find(|(o, _)| *o == Overlay::Traffic)
            .unwrap()
            .1
            .center();
        rig.click(at);
        assert_eq!(rig.overlay.active, Overlay::Traffic);
        assert!(!rig.overlay.auto);
        assert_eq!(rig.rects.overlays.len(), Overlay::ALL.len(), "still there");
        let off = rig
            .rects
            .overlays
            .iter()
            .find(|(o, _)| *o == Overlay::Off)
            .unwrap()
            .1
            .center();
        rig.click(off);
        assert_eq!(rig.overlay.active, Overlay::Off);
    }

    #[test]
    fn the_junctions_section_holds_the_tool_beside_its_overlay() {
        let mut rig = Rig::new();
        let tool = rig.rects.junctions_tool.unwrap();
        let lights = rig
            .rects
            .overlays
            .iter()
            .find(|(o, _)| *o == Overlay::Junctions)
            .unwrap()
            .1;
        assert!((tool.center().y - lights.center().y).abs() < 1.0, "one row");
        rig.click(tool.center());
        assert_eq!(rig.tools.active, ActiveTool::Junctions);
        rig.click(tool.center());
        assert_eq!(rig.tools.active, ActiveTool::None);
    }

    #[test]
    fn the_buttons_row_switches_the_windows() {
        let mut rig = Rig::new();
        assert_eq!(rig.rects.windows.len(), UiWindow::ALL.len());
        for pass in [true, false] {
            for (w, rect) in rig.rects.windows.clone() {
                rig.click(rect.center());
                assert_eq!(
                    window_on(&rig.state, &rig.files, w),
                    pass,
                    "{w:?} {}",
                    if pass { "opens" } else { "closes" }
                );
            }
        }
        assert_eq!(
            rig.files.request.take(),
            Some(crate::files::FileRequest::Refresh)
        );
    }

    /// T-307: the Jobs & education button sits in the row and opens its window.
    #[test]
    fn the_jobs_button_opens_the_jobs_window() {
        let mut rig = Rig::new();
        let (_, rect) = *rig
            .rects
            .windows
            .iter()
            .find(|(w, _)| *w == UiWindow::Jobs)
            .expect("a Jobs button");
        assert!(!rig.state.jobs_open);
        rig.click(rect.center());
        assert!(rig.state.jobs_open);
    }

    #[test]
    fn the_settings_menu_holds_the_always_day_switch_and_a_press_elsewhere_closes_it() {
        let mut rig = Rig::new();
        assert!(!rig.lighting.always_day);
        rig.click(rig.rects.settings.unwrap().center());
        assert!(rig.state.settings_open);
        rig.frame(vec![]);
        rig.click(rig.rects.always_day.unwrap().center());
        assert!(rig.lighting.always_day);
        rig.click(rig.rects.always_day.unwrap().center());
        assert!(!rig.lighting.always_day);
        rig.click(egui::pos2(300.0, 400.0));
        assert!(!rig.state.settings_open);
    }

    #[test]
    fn the_arrow_folds_the_panel_to_a_strip_and_back() {
        let mut rig = Rig::new();
        let open = rig.rects.panel.unwrap();
        rig.click(rig.rects.fold.unwrap().center());
        assert!(rig.state.overlays_collapsed);
        let strip = rig.rects.panel.unwrap();
        assert!(strip.width() < open.width() / 2.0, "{strip:?} vs {open:?}");
        // T-298: the window buttons are in the top bar and stay when the panel folds.
        assert!(rig.rects.overlays.is_empty());
        assert_eq!(rig.rects.windows.len(), ROW.len());
        assert!(width(&rig.state) < 60.0);
        rig.click(rig.rects.fold.unwrap().center());
        assert!(!rig.state.overlays_collapsed);
        assert_eq!(rig.rects.overlays.len(), Overlay::ALL.len());
    }
}
