//! Top bar and the content of the tool panels (the palette that opens them is in
//! [`crate::palette`]).

use bevy::prelude::Resource;
use bevy_egui::egui;
use cp_building::Demand;
use cp_core::SimSpeed;
use cp_render::{LightingSettings, Overlay, OverlayState};
use cp_sim::SimView;
use cp_tools::{
    ActiveTool, CommandOutbox, PALETTE, ToolState, ZoneToolKind, format_money, snap_label,
    zone_name,
};

use crate::files::{FileMenu, toggle_saves};
use crate::icons::Icon;
use crate::palette::Category;
use crate::set_speed;
use crate::style::Sizes;
use crate::tile::{TileIcon, tool_tile, tool_tile_enabled};

/// What the settings menu edits besides the lighting switch (T-069). `cp_app` loads these
/// from `settings.ron` at start and saves them when they change.
/// Volumes of the sound effects and the mute switch (T-189); `cp_app` plays with them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioPrefs {
    pub master: f32,
    pub effects: f32,
    pub mute: bool,
}

impl Default for AudioPrefs {
    fn default() -> Self {
        Self {
            master: 1.0,
            effects: 0.6,
            mute: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    /// Scale of the whole UI (egui zoom factor).
    pub ui_scale: f32,
    /// Camera sensitivities; copied to the `CameraSettings` resource when edited.
    pub camera: cp_render::CameraSettings,
    /// The city advisor snackbar (T-188) and whether it keeps to problems (no needs).
    pub advisor: bool,
    pub advisor_problems_only: bool,
    pub audio: AudioPrefs,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            ui_scale: 1.0,
            camera: cp_render::CameraSettings::default(),
            advisor: true,
            advisor_problems_only: false,
            audio: AudioPrefs::default(),
        }
    }
}

/// Range of the UI scale slider.
pub const UI_SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.75..=2.0;

#[derive(Resource, Debug, Default)]
pub struct UiState {
    /// Settings edited in the settings menu.
    pub prefs: Prefs,
    /// The category whose tool panel is open.
    pub category: Option<Category>,
    pub debug_open: bool,
    /// The budget window is shown (button in the top bar).
    pub budget_open: bool,
    pub demand_open: bool,
    pub metrics_open: bool,
    pub services_open: bool,
    /// The Lines window (T-160).
    pub lines_open: bool,
    /// The City policies window (T-193).
    pub policies_open: bool,
    /// Name drafts of the Lines window.
    pub lines: crate::lines::LinesDraft,
    /// The bottom demand panel is folded into its 28 px strip (T-099; not saved).
    pub demand_collapsed: bool,
    /// The player folded the road-type section of the Roads panel (BUG-044; not saved).
    pub road_types_folded: bool,
    /// Where the "show" button of the not-connected notice wants the camera (XZ, metres);
    /// taken by `apply_camera_goal` (T-117; not saved).
    pub camera_goal: Option<bevy::math::Vec2>,
    /// The view (overlay) menu and the settings menu of the top bar are open.
    pub view_menu_open: bool,
    pub settings_open: bool,
    /// Slider drafts of the budget window.
    pub budget: crate::budget::BudgetDraft,
}

/// The windows the top bar switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiWindow {
    Budget,
    Demand,
    Metrics,
    Services,
    Lines,
    Policies,
    Saves,
    Debug,
}

impl UiWindow {
    pub const ALL: [UiWindow; 8] = [
        UiWindow::Budget,
        UiWindow::Demand,
        UiWindow::Metrics,
        UiWindow::Services,
        UiWindow::Lines,
        UiWindow::Policies,
        UiWindow::Saves,
        UiWindow::Debug,
    ];

    pub fn label(self) -> &'static str {
        match self {
            UiWindow::Budget => "Budget",
            UiWindow::Demand => "Demand",
            UiWindow::Metrics => "Metrics",
            UiWindow::Services => "City services (F4)",
            UiWindow::Lines => "Lines",
            UiWindow::Policies => "City policies",
            UiWindow::Saves => "Save / Load",
            UiWindow::Debug => "Debug (F3)",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            UiWindow::Budget => Icon::Budget,
            UiWindow::Demand => Icon::Demand,
            UiWindow::Metrics => Icon::Metrics,
            UiWindow::Services => Icon::Services,
            UiWindow::Lines => Icon::Lines,
            UiWindow::Policies => Icon::DistrictTabPolicies,
            UiWindow::Saves => Icon::Saves,
            UiWindow::Debug => Icon::Debug,
        }
    }
}

pub const SPEEDS: [(SimSpeed, &str); 4] = [
    (SimSpeed::Pause, "⏸"),
    (SimSpeed::X1, "×1"),
    (SimSpeed::X2, "×2"),
    (SimSpeed::X3, "×3"),
];

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// "Mon 12:30, day 4" from the game time.
pub fn format_time(view: &SimView) -> String {
    let t = view.game_time;
    let minutes = u64::from(t.tick_of_day) * 24 * 60 / cp_core::time::TICKS_PER_DAY;
    format!(
        "{} {:02}:{:02}  ·  day {}",
        WEEKDAYS[t.day_of_week() as usize],
        minutes / 60,
        minutes % 60,
        t.day + 1
    )
}

/// Full-screen background `Ui` that panels are docked into. Create one per frame.
pub fn root_ui(ctx: &egui::Context) -> egui::Ui {
    egui::Ui::new(
        ctx.clone(),
        "cp_ui_root".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    )
}

/// Where the clickable parts of the top bar were drawn (tests click them).
#[derive(Clone, Debug, Default)]
pub struct TopRects {
    /// The whole bar (its height is [`Sizes::TOP_BAR_H`]).
    pub bar: Option<egui::Rect>,
    pub speeds: Vec<(SimSpeed, egui::Rect)>,
    /// The window switches; the budget's is the balance.
    pub windows: Vec<(UiWindow, egui::Rect)>,
    /// The view button, and (while its menu is open) the overlay icons of the menu.
    pub view: Option<egui::Rect>,
    pub overlays: Vec<(Overlay, egui::Rect)>,
    /// The settings button and (while its menu is open) the always-day switch.
    pub settings: Option<egui::Rect>,
    pub always_day: Option<egui::Rect>,
    /// Width of what the bar draws: left group + centre + right group.
    pub content_width: f32,
}

/// Whether the O key was pressed (opens the view menu): not with a modifier, not while a text
/// field has the keyboard.
pub fn view_hotkey(ctx: &egui::Context) -> bool {
    if ctx.egui_wants_keyboard_input() {
        return false;
    }
    ctx.input(|i| {
        i.events.iter().any(|e| {
            matches!(
                e,
                egui::Event::Key {
                    key: egui::Key::O,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if !modifiers.any()
            )
        })
    })
}

/// A popup under `anchor`, shown while `*open`; a press anywhere else closes it. Returns the
/// popup's rectangle while it is open.
fn popup(
    ctx: &egui::Context,
    id: &str,
    anchor: egui::Rect,
    open: &mut bool,
    add: impl FnOnce(&mut egui::Ui),
) -> Option<egui::Rect> {
    if !*open {
        return None;
    }
    let area = egui::Area::new(egui::Id::new(("top_popup", id)))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor.left_bottom() + egui::vec2(0.0, Sizes::TILE_GAP))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, add);
        });
    let rect = area.response.rect;
    let outside = ctx.input(|i| i.pointer.any_pressed())
        && ctx
            .pointer_interact_pos()
            .is_some_and(|p| !rect.contains(p) && !anchor.contains(p));
    if outside {
        *open = false;
    }
    Some(rect)
}

/// Top bar, city status only: balance (a click opens the budget) and population on the left;
/// clock and the speed icons in the middle; on the right the view and settings menus and the
/// window icons (demand, metrics, saves, debug). A speed click pushes `SetSpeed` into
/// `outbox`. The overlays and the always-day switch live in the two popups.
pub fn top_bar(
    root: &mut egui::Ui,
    view: &SimView,
    outbox: &mut CommandOutbox,
    overlay: &mut OverlayState,
    lighting: &mut LightingSettings,
    state: &mut UiState,
    files: &mut FileMenu,
) -> TopRects {
    let mut rects = TopRects::default();
    let ctx = root.ctx().clone();
    let bar = egui::Panel::top("top_bar")
        .exact_size(Sizes::TOP_BAR_H)
        .show(root, |ui| {
            let full = ui.max_rect();
            let mid = egui::Rect::from_center_size(
                full.center(),
                egui::vec2(Sizes::TOP_CENTER_W, full.height()),
            );
            let scope = |ui: &mut egui::Ui, rect: egui::Rect, layout: egui::Layout| {
                ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout))
            };
            let centred = egui::Layout::left_to_right(egui::Align::Center);
            // Left: the balance (opens the budget) and the population.
            let mut left = scope(
                ui,
                egui::Rect::from_min_max(full.min, egui::pos2(mid.left(), full.max.y)),
                centred,
            );
            let money = crate::budget::money_label(&mut left, view);
            crate::budget::trend_label(&mut left, view);
            let money = left.interact(money.rect, money.id.with("open"), egui::Sense::click());
            rects.windows.push((UiWindow::Budget, money.rect));
            if money
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                state.budget_open = !state.budget_open;
            }
            left.label(format!("☻ {}", view.city.population))
                .on_hover_text("Residents");
            crate::budget::population_trend_label(&mut left, view);
            // Middle: clock and speeds.
            let mut centre = scope(ui, mid, centred);
            centre.label(format_time(view));
            for (speed, label) in SPEEDS {
                let r = crate::icons::speed_icon(speed)
                    .button(
                        &mut centre,
                        Sizes::TOP_BAR_ICON,
                        Sizes::BUTTON_PAD,
                        view.speed == speed,
                    )
                    .on_hover_text(label);
                rects.speeds.push((speed, r.rect));
                if r.clicked() && view.speed != speed {
                    outbox.push(set_speed(speed));
                }
            }
            // Right (laid out right to left): the window icons, settings, view.
            let mut right = scope(
                ui,
                egui::Rect::from_min_max(egui::pos2(mid.right(), full.min.y), full.max),
                egui::Layout::right_to_left(egui::Align::Center),
            );
            for w in UiWindow::ALL
                .into_iter()
                .rev()
                .filter(|w| *w != UiWindow::Budget)
            {
                let on = match w {
                    UiWindow::Budget => state.budget_open,
                    UiWindow::Demand => state.demand_open,
                    UiWindow::Metrics => state.metrics_open,
                    UiWindow::Services => state.services_open,
                    UiWindow::Lines => state.lines_open,
                    UiWindow::Policies => state.policies_open,
                    UiWindow::Saves => files.open,
                    UiWindow::Debug => state.debug_open,
                };
                let r = w
                    .icon()
                    .button(&mut right, Sizes::TOP_BAR_ICON, Sizes::BUTTON_PAD, on)
                    .on_hover_text(w.label());
                rects.windows.push((w, r.rect));
                if r.clicked() {
                    match w {
                        UiWindow::Budget => state.budget_open = !on,
                        UiWindow::Demand => state.demand_open = !on,
                        UiWindow::Metrics => state.metrics_open = !on,
                        UiWindow::Services => state.services_open = !on,
                        UiWindow::Lines => state.lines_open = !on,
                        UiWindow::Policies => state.policies_open = !on,
                        UiWindow::Saves => toggle_saves(files),
                        UiWindow::Debug => state.debug_open = !on,
                    }
                }
            }
            right.separator();
            let r = Icon::Settings
                .button(
                    &mut right,
                    Sizes::TOP_BAR_ICON,
                    Sizes::BUTTON_PAD,
                    state.settings_open,
                )
                .on_hover_text("Settings");
            rects.settings = Some(r.rect);
            if r.clicked() {
                state.settings_open = !state.settings_open;
                state.view_menu_open = false;
            }
            // The view button shows the active overlay.
            let r = crate::icons::overlay_icon(overlay.active)
                .button(
                    &mut right,
                    Sizes::TOP_BAR_ICON,
                    Sizes::BUTTON_PAD,
                    state.view_menu_open,
                )
                .on_hover_text(format!("View: {} (O)", overlay.active.label()));
            rects.view = Some(r.rect);
            if r.clicked() {
                state.view_menu_open = !state.view_menu_open;
                state.settings_open = false;
            }
            rects.content_width =
                left.min_rect().width() + Sizes::TOP_CENTER_W + right.min_rect().width();
        });
    rects.bar = Some(bar.response.rect);
    if let Some(anchor) = rects.view {
        let mut chosen = false;
        popup(&ctx, "view", anchor, &mut state.view_menu_open, |ui| {
            egui::Grid::new("view_grid")
                .spacing(egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP))
                .show(ui, |ui| {
                    for (n, o) in Overlay::ALL.into_iter().enumerate() {
                        let r = crate::icons::overlay_icon(o)
                            .button(
                                ui,
                                Sizes::VIEW_MENU_ICON,
                                Sizes::BUTTON_PAD,
                                overlay.active == o,
                            )
                            .on_hover_text(o.label());
                        rects.overlays.push((o, r.rect));
                        if r.clicked() {
                            overlay.active = o;
                            chosen = true;
                        }
                        if n % VIEW_COLUMNS == VIEW_COLUMNS - 1 {
                            ui.end_row();
                        }
                    }
                });
        });
        if chosen {
            state.view_menu_open = false;
        }
    }
    if let Some(anchor) = rects.settings {
        popup(&ctx, "settings", anchor, &mut state.settings_open, |ui| {
            // Render only: the game clock and everyone's routine keep running at night.
            let r = ui.checkbox(&mut lighting.always_day, "Всегда день");
            rects.always_day = Some(r.rect);
            let prefs = &mut state.prefs;
            ui.add(egui::Slider::new(&mut prefs.ui_scale, UI_SCALE_RANGE).text("Масштаб UI"));
            // Sound (T-189).
            ui.checkbox(&mut prefs.audio.mute, "Mute");
            ui.add_enabled(
                !prefs.audio.mute,
                egui::Slider::new(&mut prefs.audio.master, 0.0..=1.0).text("Volume"),
            );
            ui.add_enabled(
                !prefs.audio.mute,
                egui::Slider::new(&mut prefs.audio.effects, 0.0..=1.0).text("Effects"),
            );
            let cam = &mut prefs.camera;
            ui.add(egui::Slider::new(&mut cam.pan_speed, 0.2..=3.0).text("Скорость сдвига"));
            ui.add(
                egui::Slider::new(&mut cam.key_rotate_speed, 0.4..=4.0).text("Скорость поворота"),
            );
            ui.add(
                egui::Slider::new(&mut cam.drag_rotate_sens, 0.001..=0.02)
                    .text("Чувствительность мыши"),
            );
            // Distance factor per notch: smaller = faster, shown the other way round.
            let mut zoom = 1.0 - cam.zoom_step;
            if ui
                .add(egui::Slider::new(&mut zoom, 0.02..=0.4).text("Шаг зума"))
                .changed()
            {
                cam.zoom_step = 1.0 - zoom;
            }
            ui.checkbox(&mut cam.edge_pan, "Сдвиг у края экрана");
            ui.checkbox(&mut prefs.advisor, "City advisor");
            ui.add_enabled(
                prefs.advisor,
                egui::Checkbox::new(&mut prefs.advisor_problems_only, "Only problems"),
            );
        });
    }
    rects
}

/// Columns of the view menu's icon grid (5 rows of 4, the last with one).
const VIEW_COLUMNS: usize = 4;

/// A tool of a tool panel: the one tile of every construction tool (see [`crate::tile`]).
pub fn tool_button(ui: &mut egui::Ui, icon: Icon, label: &str, selected: bool) -> egui::Response {
    tool_tile(ui, TileIcon::Icon(icon), label, selected)
}

/// A titled section of a tool panel.
pub(crate) fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    ui.weak(title);
    add(ui);
}

/// Length of the planned road, metres.
pub fn plan_length(g: &cp_tools::Ghost) -> f32 {
    g.polylines
        .iter()
        .flat_map(|p| p.windows(2))
        .fold(0.0, |acc, w| acc + w[0].distance(w[1]))
}

/// The pinned "cost · verdict" block: a ghost that can be built or placed shows its cost, one
/// that cannot shows why; with no ghost, a hint for the open category.
pub fn verdict(ui: &mut egui::Ui, category: Category, tools: &ToolState) {
    let ok = |ui: &mut egui::Ui, text: &str, cost: Option<cp_core::Cost>, extra: String| {
        let cost = cost.map_or(String::new(), |c| format!("{} · ", format_money(c)));
        ui.colored_label(egui::Color32::LIGHT_GREEN, format!("{cost}✔ {text}{extra}"));
    };
    let bad = |ui: &mut egui::Ui, errors: &[String], fallback: &str| {
        if errors.is_empty() {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("✖ {fallback}"));
        }
        for e in errors {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("✖ {e}"));
        }
    };
    match (category, tools.active) {
        (Category::Roads, ActiveTool::Road) => match &tools.ghost {
            Some(g) if g.valid => {
                let snap = snap_label(g.snap.kind).map_or(String::new(), |l| format!("  ⌖ {l}"));
                // The 8-cell block guide's verdict (T-185).
                let block = g
                    .block
                    .as_ref()
                    .map_or(String::new(), |b| format!("  ▦ {}", b.note));
                ok(
                    ui,
                    "can build",
                    g.cost,
                    format!(" · {:.0} m{snap}{block}", plan_length(g)),
                );
                // One-way direction warnings (§6.6): yellow, they never block the build.
                for w in &g.warnings {
                    ui.colored_label(egui::Color32::YELLOW, format!("⚠ {w}"));
                }
            }
            Some(g) => bad(ui, &g.errors, "cannot build here"),
            None => {
                ui.weak("Click to start a road. Esc cancels.");
            }
        },
        (Category::Roads, ActiveTool::Flip) => match &tools.flip_preview {
            Some(f) if !f.errors.is_empty() => bad(ui, &f.errors, "cannot flip"),
            Some(f) => {
                let n = f.run.len();
                ok(
                    ui,
                    "can flip",
                    None,
                    format!(" · {n} segment{}", if n == 1 { "" } else { "s" }),
                );
            }
            None => {
                ui.weak("Click a one-way road to turn its whole run round. Esc leaves the tool.");
            }
        },
        (Category::Roads, ActiveTool::Upgrade) => match &tools.upgrade_preview {
            Some(u) if !u.errors.is_empty() => bad(ui, &u.errors, "cannot upgrade"),
            Some(u) if u.is_noop() => {
                ui.weak("This run already has the chosen road type.");
            }
            Some(u) => {
                let n = u.changes.len();
                ok(
                    ui,
                    "can upgrade",
                    Some(u.cost),
                    format!(" · {n} segment{}", if n == 1 { "" } else { "s" }),
                );
                if let Some(w) = u.warning() {
                    ui.colored_label(egui::Color32::YELLOW, format!("⚠ {w}"));
                }
            }
            None => {
                ui.weak("Pick a road type, then click a road to change its whole run. Esc leaves the tool.");
            }
        },
        (Category::Buildings | Category::Transit | Category::Industry, ActiveTool::Place) => {
            match &tools.place_ghost {
                Some(g) => {
                    if let Some((have, need)) = g.richness {
                        let color = if have >= need {
                            egui::Color32::LIGHT_GREEN
                        } else {
                            egui::Color32::from_rgb(230, 90, 80)
                        };
                        ui.colored_label(color, format!("Lot richness {have}% (needs {need}%)"));
                    }
                    if g.valid {
                        ok(ui, "can place", g.cost, String::new());
                    } else {
                        bad(
                            ui,
                            &g.error.iter().cloned().collect::<Vec<_>>(),
                            "cannot place here",
                        );
                    }
                    match &g.free {
                        Some(f) => {
                            if let Some(h) = &f.hint {
                                let color = if h.connects {
                                    egui::Color32::LIGHT_GREEN
                                } else {
                                    egui::Color32::YELLOW
                                };
                                ui.colored_label(color, &h.text);
                            }
                            ui.weak(
                                "T turns 15°, Shift+T along the nearest road, Esc leaves the tool.",
                            );
                        }
                        None => {
                            ui.weak("T rotates, Esc leaves the tool.");
                        }
                    }
                }
                None => {
                    ui.weak("Hover a spot beside a road. T rotates, Esc leaves the tool.");
                }
            }
        }
        (Category::Buildings, ActiveTool::Utility) => match &tools.utility_ghost {
            Some(g) if g.valid => {
                let snap = snap_label(g.snap.kind).map_or(String::new(), |l| format!("  ⌖ {l}"));
                ok(ui, "can build", g.cost, snap)
            }
            Some(g) => bad(ui, &g.errors, "cannot build here"),
            None => {
                ui.weak("Click to start. Alt switches the snaps off. Buildings within reach of the network are connected (the band shows the reach). Esc cancels.");
            }
        },
        (Category::Buildings, ActiveTool::UtilityRemove) => {
            ui.weak("Hover a piece and click to remove it. Esc leaves the tool.");
        }
        (Category::Transit, ActiveTool::Line) => {
            if let Some(notice) = &tools.line_notice {
                bad(ui, std::slice::from_ref(notice), "cannot place a stop");
            }
            match &tools.line_ghost {
                Some(g) if g.error.is_some() && g.hover.is_some_and(|h| !h.1) => {
                    bad(
                        ui,
                        &g.error.iter().cloned().collect::<Vec<_>>(),
                        "cannot stop here",
                    );
                }
                Some(g) if g.closes => ok(
                    ui,
                    "click to close the line",
                    Some(g.cost),
                    format!(" · {}", cp_tools::route_summary(g)),
                ),
                Some(g) if g.route.len() > 1 => {
                    if let Some(e) = &g.error {
                        ui.colored_label(egui::Color32::YELLOW, format!("⚠ {e}"));
                    }
                    ui.weak(format!(
                        "{} stop{} · {}. Click the first stop to close the line.",
                        g.stops.len(),
                        if g.stops.len() == 1 { "" } else { "s" },
                        cp_tools::route_summary(g)
                    ));
                }
                _ => {
                    ui.weak("Click a road to put the first stop. Backspace undoes, Esc cancels.");
                }
            }
        }
        (Category::Transit, _) => {
            ui.weak("Pick the bus line tool or the depot; the Transit view shows the lines.");
        }
        (Category::Zones, ActiveTool::Zone) => {
            let n = tools.zone_preview.len();
            let what = match tools.zone.kind {
                ZoneToolKind::Rectangle if tools.zone.corner.is_some() => {
                    "click the opposite corner".to_string()
                }
                ZoneToolKind::Rectangle => "click the first corner".to_string(),
                ZoneToolKind::FillBlock => "click a block to fill it".to_string(),
                _ => "hold and drag to paint".to_string(),
            };
            ui.weak(format!("{what} — {n} cells under the cursor. Esc cancels."));
        }
        (Category::Districts, ActiveTool::District) if tools.district.selected.is_none() => {
            ui.weak("Choose a district in the list (or press New district), then paint.");
        }
        (Category::Districts, ActiveTool::District) => {
            let n = tools.district_preview.len();
            ui.weak(format!(
                "hold and drag to paint — {n} cells under the cursor. Esc leaves the brush."
            ));
        }
        (Category::Districts, _) => {
            ui.weak("Pick a brush or create a district; the overlay shows the districts.");
        }
        (Category::Industry, ActiveTool::Area) if tools.area.selected.is_none() => {
            ui.weak("Choose an area in the list (or press New area), then paint.");
        }
        (Category::Industry, ActiveTool::Area) => {
            let n = tools.area_preview.len();
            ui.weak(format!(
                "hold and drag to paint — {n} cells under the cursor. Esc leaves the brush."
            ));
        }
        (Category::Industry, _) => {
            ui.weak(
                "Pick a brush or create an area; the overlays show the resources and the areas.",
            );
        }
        (Category::Zones, _) => {
            ui.weak("Pick a zoning tool or a zone type to show the zone grid.");
        }
        (Category::Bulldozer, ActiveTool::Bulldozer) if tools.highlight_outside => {
            bad(ui, &["outside your land".to_string()], "cannot remove");
        }
        (Category::Bulldozer, ActiveTool::Bulldozer) => {
            ui.weak("Hover a building, bus stop or road segment, click to remove it. Esc leaves the tool.");
        }
        (Category::Buildings, _) => {
            ui.weak("Pick a building or a network tool; the overlay shows the network and which buildings lack supply.");
        }
        _ => {
            ui.weak("Pick a tool.");
        }
    }
}

/// The Buildings category: the service buildings of the content as cards, grouped by service
/// kind (see [`crate::buildings`]). T rotates the building.
pub fn services_panel(
    ui: &mut egui::Ui,
    view: Option<&SimView>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
) {
    crate::buildings::buildings_panel(ui, view, tools, overlay);
}

/// The Bulldozer category: the one tool.
pub fn bulldozer_panel(ui: &mut egui::Ui, tools: &mut ToolState) {
    let on = tools.active == ActiveTool::Bulldozer;
    if tool_button(ui, Icon::Bulldozer, "Bulldozer", on).clicked() {
        tools.toggle_bulldozer();
    }
}

fn zone_kind_icon(kind: ZoneToolKind) -> Icon {
    match kind {
        ZoneToolKind::FillBlock => Icon::ZoneFill,
        ZoneToolKind::BrushSmall => Icon::ZoneBrushSmall,
        ZoneToolKind::BrushLarge => Icon::ZoneBrushLarge,
        ZoneToolKind::Rectangle => Icon::ZoneRect,
        ZoneToolKind::Eraser => Icon::ZoneEraser,
    }
}

/// The Zones category: tool kinds and the zone palette. Picking either activates the zoning
/// tool (which shows the zone grid); "Off" leaves it.
pub fn zones_panel(
    ui: &mut egui::Ui,
    view: Option<&SimView>,
    tools: &mut ToolState,
    state: &mut UiState,
) {
    section(ui, "Tool", |ui| {
        ui.horizontal_wrapped(|ui| {
            for kind in ZoneToolKind::ALL {
                let selected = tools.active == ActiveTool::Zone && tools.zone.kind == kind;
                if tool_button(ui, zone_kind_icon(kind), kind.label(), selected).clicked() {
                    if tools.active != ActiveTool::Zone {
                        tools.select(ActiveTool::Zone);
                    }
                    tools.zone.set_kind(kind);
                }
            }
            if tool_tile_enabled(
                ui,
                TileIcon::Icon(Icon::Remove),
                "Off",
                false,
                tools.active == ActiveTool::Zone,
            )
            .clicked()
            {
                tools.select(ActiveTool::None);
            }
        });
    });
    section(ui, "Zone", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            for zone in PALETTE {
                let locked = view.is_some_and(|v| crate::windows::zone_lock(v, zone).is_some());
                let tile = tool_tile_enabled(
                    ui,
                    TileIcon::Icon(crate::icons::zone_icon(zone)),
                    zone_name(zone),
                    tools.zone.zone == zone,
                    !locked,
                );
                if locked {
                    crate::buildings::badge(ui, tile.rect, crate::buildings::UniqueState::Locked);
                }
                let tile = match view {
                    Some(view) => tile.on_hover_text(crate::windows::zone_tooltip(view, zone)),
                    None => tile,
                };
                if tile.clicked() && !locked {
                    tools.zone.zone = zone;
                    if tools.active != ActiveTool::Zone {
                        tools.select(ActiveTool::Zone);
                    }
                }
            }
        });
    });
    if ui.button("Demand by zone ▸").clicked() {
        state.demand_open = true;
    }
}

/// The demand group a zone type belongs to: the letter and its value (R for `ResLow`/`ResHigh`,
/// C for `Com*`, I, O). Demand is per group in M1.
pub fn zone_group_demand(zone: cp_content::ZoneType, d: &Demand) -> (&'static str, f32) {
    use cp_content::ZoneType as Z;
    match zone {
        Z::ResLow | Z::ResHigh => ("R", d.r),
        Z::ComLow | Z::ComHigh => ("C", d.c),
        Z::Industrial => ("I", d.i),
        Z::Office | Z::None => ("O", d.o),
    }
}

/// The Roads category: tools, mode, elevation and the road types as cards (see [`crate::roads`]).
pub fn roads_panel(
    ui: &mut egui::Ui,
    view: Option<&SimView>,
    tools: &mut ToolState,
    state: &mut UiState,
) {
    let content = view.and_then(|v| v.content.as_deref());
    crate::roads::roads_panel(ui, content, tools, &mut state.road_types_folded);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_sim::{SimCommand, SimRunner};

    fn input(t: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            time: Some(t),
            events,
            ..Default::default()
        }
    }

    /// Runs three frames: layout, press, release on `speed`'s button.
    fn click_speed(view: &SimView, speed: SimSpeed, outbox: &mut CommandOutbox) {
        let ctx = egui::Context::default();
        let mut rects = TopRects::default();
        crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| {
            rects = top_bar(
                ui,
                view,
                &mut CommandOutbox::default(),
                &mut OverlayState::default(),
                &mut LightingSettings::default(),
                &mut UiState::default(),
                &mut FileMenu::default(),
            );
        });
        let pos = rects
            .speeds
            .iter()
            .find(|(s, _)| *s == speed)
            .unwrap()
            .1
            .center();
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for (t, events) in [
            (0.1, vec![egui::Event::PointerMoved(pos), button(true)]),
            (0.2, vec![button(false)]),
        ] {
            crate::testutil::frame(&ctx, input(t, events), |ui| {
                top_bar(
                    ui,
                    view,
                    outbox,
                    &mut OverlayState::default(),
                    &mut LightingSettings::default(),
                    &mut UiState::default(),
                    &mut FileMenu::default(),
                );
            });
        }
    }

    #[test]
    fn clicking_a_speed_button_sends_set_speed_into_the_command_log() {
        let mut sim = SimRunner::new(1);
        let mut outbox = CommandOutbox::default();
        click_speed(&sim.view(), SimSpeed::X3, &mut outbox);
        assert_eq!(outbox.len(), 1);
        assert_eq!(
            outbox.iter().next(),
            Some(&SimCommand::SetSpeed {
                speed: SimSpeed::X3
            })
        );

        // The app's submit path: queue -> dispatcher -> log.
        outbox.flush_into(&mut sim);
        sim.tick_once();
        let log = sim.world().resource::<cp_core::CommandLog<SimCommand>>();
        assert_eq!(log.len(), 1);
        assert_eq!(
            log.entries()[0].command,
            SimCommand::SetSpeed {
                speed: SimSpeed::X3
            }
        );
        assert_eq!(sim.view().speed, SimSpeed::X3);

        // Pause, then resume from the paused state, both via buttons.
        click_speed(&sim.view(), SimSpeed::Pause, &mut outbox);
        outbox.flush_into(&mut sim);
        assert_eq!(sim.view().speed, SimSpeed::Pause);
        click_speed(&sim.view(), SimSpeed::X1, &mut outbox);
        outbox.flush_into(&mut sim);
        assert_eq!(sim.view().speed, SimSpeed::X1);
        assert_eq!(
            sim.world()
                .resource::<cp_core::CommandLog<SimCommand>>()
                .len(),
            3
        );
    }

    #[test]
    fn clicking_the_active_speed_sends_nothing() {
        let sim = SimRunner::new(1);
        let mut outbox = CommandOutbox::default();
        click_speed(&sim.view(), SimSpeed::X1, &mut outbox);
        assert!(outbox.is_empty());
    }

    #[test]
    fn time_is_formatted_from_game_time() {
        let mut sim = SimRunner::new(1);
        sim.world_mut().insert_resource(cp_core::Tick(
            cp_core::time::TICKS_PER_DAY * 8 + cp_core::time::TICKS_PER_DAY / 2,
        ));
        assert_eq!(format_time(&sim.view()), "Tue 12:00  ·  day 9");
    }

    #[test]
    fn roads_panel_picks_a_default_road_and_shows_the_ghost_verdict() {
        use cp_core::Cost;
        use cp_tools::Ghost;
        let mut sim = SimRunner::new(1);
        sim.set_content(std::sync::Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        ));
        let view = sim.view();
        let ctx = egui::Context::default();
        let mut tools = ToolState::default();
        tools.select(ActiveTool::Road);
        assert!(tools.road.road.is_none());
        let mut ui_state = UiState::default();
        let mut draw = |tools: &mut ToolState| {
            crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| {
                roads_panel(ui, Some(&view), tools, &mut ui_state);
            });
        };
        draw(&mut tools);
        assert!(
            tools.road.road.is_some(),
            "the first road type is preselected"
        );

        // Valid and invalid ghosts both render.
        tools.ghost = Some(Ghost {
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
        });
        draw(&mut tools);
        tools.ghost = Some(Ghost {
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
        });
        draw(&mut tools);
        tools.select(ActiveTool::Bulldozer);
        draw(&mut tools);
        assert_eq!(tools.active, ActiveTool::Bulldozer);
        assert!(tools.ghost.is_none(), "switching tools clears the ghost");
    }

    /// A top bar that keeps its state between frames.
    struct Bar {
        ctx: egui::Context,
        view: SimView,
        state: UiState,
        overlay: OverlayState,
        lighting: LightingSettings,
        rects: TopRects,
        t: f64,
    }

    impl Bar {
        fn new() -> Self {
            let mut bar = Bar {
                ctx: egui::Context::default(),
                view: SimRunner::new(1).view(),
                state: UiState::default(),
                overlay: OverlayState::default(),
                lighting: LightingSettings::default(),
                rects: TopRects::default(),
                t: 0.0,
            };
            bar.frame(vec![]);
            bar.frame(vec![]);
            bar
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<(String, egui::Pos2)> {
            self.t += 0.1;
            let Bar {
                ctx,
                view,
                state,
                overlay,
                lighting,
                rects,
                t,
            } = self;
            crate::testutil::frame_texts(ctx, input(*t, events), |ui| {
                *rects = top_bar(
                    ui,
                    view,
                    &mut CommandOutbox::default(),
                    overlay,
                    lighting,
                    state,
                    &mut FileMenu::default(),
                );
            })
        }

        fn click(&mut self, pos: egui::Pos2) {
            let [down, up] = crate::testutil::click(pos);
            self.frame(down);
            self.frame(up);
            self.frame(vec![]);
        }
    }

    #[test]
    fn the_top_bar_has_no_rcio_bars_and_no_text_overlay_buttons_or_switch() {
        let mut bar = Bar::new();
        let texts: Vec<String> = bar.frame(vec![]).into_iter().map(|(t, _)| t).collect();
        for gone in ["R", "C", "I", "O", "Всегда день"] {
            assert!(!texts.iter().any(|t| t == gone), "{gone:?} in {texts:?}");
        }
        for o in Overlay::ALL {
            assert!(!texts.iter().any(|t| t == o.label()), "{o:?} in {texts:?}");
        }
    }

    #[test]
    fn the_view_button_opens_a_grid_and_a_click_on_an_icon_selects_the_overlay_and_closes_it() {
        let mut bar = Bar::new();
        assert!(bar.rects.overlays.is_empty());
        bar.click(bar.rects.view.unwrap().center());
        assert!(bar.state.view_menu_open);
        assert_eq!(bar.rects.overlays.len(), Overlay::ALL.len());
        bar.frame(vec![]);
        // 5 rows of 4 (the last holds one).
        let ys: std::collections::BTreeSet<i32> = bar
            .rects
            .overlays
            .iter()
            .map(|(_, r)| r.top().round() as i32)
            .collect();
        assert_eq!(ys.len(), 5, "{:?}", bar.rects.overlays);
        let at = bar
            .rects
            .overlays
            .iter()
            .find(|(o, _)| *o == Overlay::Traffic)
            .unwrap()
            .1
            .center();
        bar.click(at);
        assert_eq!(bar.overlay.active, Overlay::Traffic);
        assert!(!bar.state.view_menu_open);
    }

    #[test]
    fn the_settings_menu_holds_the_always_day_switch() {
        let mut bar = Bar::new();
        assert!(!bar.lighting.always_day);
        bar.click(bar.rects.settings.unwrap().center());
        assert!(bar.state.settings_open);
        bar.frame(vec![]);
        bar.click(bar.rects.always_day.unwrap().center());
        assert!(bar.lighting.always_day);
        bar.click(bar.rects.always_day.unwrap().center());
        assert!(!bar.lighting.always_day);
    }

    #[test]
    fn a_press_elsewhere_closes_a_menu() {
        let mut bar = Bar::new();
        bar.click(bar.rects.view.unwrap().center());
        assert!(bar.state.view_menu_open);
        bar.click(egui::pos2(600.0, 500.0));
        assert!(!bar.state.view_menu_open);
    }

    #[test]
    fn the_o_key_opens_the_view_menu_unless_a_modifier_or_a_text_field_has_it() {
        let ctx = egui::Context::default();
        let key = |modifiers| egui::Event::Key {
            key: egui::Key::O,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let mut seen = Vec::new();
        for (t, events) in [
            (0.0, vec![key(egui::Modifiers::NONE)]),
            (0.1, vec![key(egui::Modifiers::CTRL)]),
            (0.2, vec![]),
        ] {
            crate::testutil::frame(&ctx, input(t, events), |ui| {
                seen.push(view_hotkey(ui.ctx()));
            });
        }
        assert_eq!(seen, [true, false, false]);
    }

    #[test]
    fn the_bars_content_fits_a_1280_wide_window() {
        let bar = Bar::new();
        assert!(bar.rects.content_width > 0.0);
        assert!(
            bar.rects.content_width <= 1280.0,
            "{}",
            bar.rects.content_width
        );
    }

    #[test]
    fn zones_panel_activates_the_zoning_tool_and_shows_the_state() {
        let ctx = egui::Context::default();
        let mut tools = ToolState::default();
        let mut state = UiState::default();
        let mut draw = |tools: &mut ToolState| {
            crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| {
                zones_panel(ui, None, tools, &mut state);
            });
        };
        draw(&mut tools);
        assert_eq!(tools.active, ActiveTool::None);
        tools.select(ActiveTool::Zone);
        tools.zone.set_kind(ZoneToolKind::Rectangle);
        draw(&mut tools);
        tools.zone.corner = Some(bevy::math::Vec2::ZERO);
        draw(&mut tools);
        tools.select(ActiveTool::None);
        draw(&mut tools);
        assert!(
            tools.zone.corner.is_none(),
            "leaving the tool cancels the rectangle"
        );
    }

    #[test]
    fn the_top_bar_icons_switch_their_windows() {
        let ctx = egui::Context::default();
        let sim = SimRunner::new(1);
        let view = sim.view();
        let mut state = UiState::default();
        let mut files = FileMenu::default();
        let mut outbox = CommandOutbox::default();
        let mut t = 0.0;
        let mut draw = |events: Vec<egui::Event>, state: &mut UiState, files: &mut FileMenu| {
            t += 0.1;
            let mut rects = TopRects::default();
            crate::testutil::frame(&ctx, input(t, events), |ui| {
                rects = top_bar(
                    ui,
                    &view,
                    &mut outbox,
                    &mut OverlayState::default(),
                    &mut LightingSettings::default(),
                    state,
                    files,
                );
            });
            rects
        };
        draw(vec![], &mut state, &mut files);
        let rects = draw(vec![], &mut state, &mut files);
        assert_eq!(rects.windows.len(), UiWindow::ALL.len());
        for (w, rect) in rects.windows {
            let [down, up] = crate::testutil::click(rect.center());
            draw(down, &mut state, &mut files);
            draw(up, &mut state, &mut files);
            let on = match w {
                UiWindow::Budget => state.budget_open,
                UiWindow::Demand => state.demand_open,
                UiWindow::Metrics => state.metrics_open,
                UiWindow::Services => state.services_open,
                UiWindow::Lines => state.lines_open,
                UiWindow::Policies => state.policies_open,
                UiWindow::Saves => files.open,
                UiWindow::Debug => state.debug_open,
            };
            assert!(on, "{w:?} opens on a click");
        }
        // A second click on each closes it again.
        let rects = draw(vec![], &mut state, &mut files);
        for (w, rect) in rects.windows {
            let [down, up] = crate::testutil::click(rect.center());
            draw(down, &mut state, &mut files);
            draw(up, &mut state, &mut files);
            let on = match w {
                UiWindow::Budget => state.budget_open,
                UiWindow::Demand => state.demand_open,
                UiWindow::Metrics => state.metrics_open,
                UiWindow::Services => state.services_open,
                UiWindow::Lines => state.lines_open,
                UiWindow::Policies => state.policies_open,
                UiWindow::Saves => files.open,
                UiWindow::Debug => state.debug_open,
            };
            assert!(!on, "{w:?} closes on a second click");
        }
        assert_eq!(
            files.request.take(),
            Some(crate::files::FileRequest::Refresh)
        );
    }

    #[test]
    fn the_top_bar_shows_the_population_from_the_city_stats() {
        let ctx = egui::Context::default();
        let sim = SimRunner::new(1);
        let mut view = sim.view();
        std::sync::Arc::make_mut(&mut view.city).population = 12_345;
        let texts = crate::testutil::frame_texts(&ctx, input(0.0, vec![]), |ui| {
            top_bar(
                ui,
                &view,
                &mut CommandOutbox::default(),
                &mut OverlayState::default(),
                &mut LightingSettings::default(),
                &mut UiState::default(),
                &mut FileMenu::default(),
            );
        });
        assert!(texts.iter().any(|(t, _)| t == "☻ 12345"), "{texts:?}");
    }
}
