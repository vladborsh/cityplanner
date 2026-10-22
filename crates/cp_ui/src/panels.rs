//! Top bar and the content of the tool panels (the palette that opens them is in
//! [`crate::palette`]).

use bevy::prelude::Resource;
use bevy_egui::egui;
use cp_building::Demand;
use cp_core::SimSpeed;
use cp_locale::{tr, trf, trn};
use cp_render::{Overlay, OverlayState};
use cp_sim::SimView;
use cp_tools::{
    ActiveTool, BulldozeLayer, CommandOutbox, PALETTE, ToolState, ZoneToolKind, format_money,
    snap_label, zone_name,
};

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
    /// The language of the UI (T-284); [`cp_locale::set_lang`] follows it.
    pub lang: cp_locale::Lang,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            ui_scale: 1.0,
            camera: cp_render::CameraSettings::default(),
            advisor: true,
            advisor_problems_only: false,
            audio: AudioPrefs::default(),
            lang: cp_locale::Lang::En,
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
    /// A menu was opened this frame and has not taken up its tool yet (T-289).
    pub entered: Option<Category>,
    /// The last build tool used in each menu this session (T-289; not saved).
    pub last_tool: Vec<(Category, crate::menus::Remembered)>,
    /// A tool was active at the end of the last frame: the Esc that cleared it must not also
    /// reset the view (BUG-059).
    pub tool_was_active: bool,
    pub debug_open: bool,
    /// The F8 colour grading panel (T-273; not saved).
    pub grading_open: bool,
    /// The budget window is shown (button in the top bar).
    pub budget_open: bool,
    pub demand_open: bool,
    pub metrics_open: bool,
    /// The Jobs & education window (T-307).
    pub jobs_open: bool,
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
    /// The settings menu of the overlays panel is open.
    pub settings_open: bool,
    /// The overlays panel at the top right is folded to its strip (T-289; not saved).
    pub overlays_collapsed: bool,
    /// Slider drafts of the budget window.
    pub budget: crate::budget::BudgetDraft,
    /// What an automatically opened overlay belongs to (BUG-083).
    pub auto_overlay: AutoOverlay,
}

/// The category and tool an overlay that a tool opened by itself is tied to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoOverlay {
    /// The `OverlayState::auto_opens` this binding was made for.
    seen: u32,
    to: Option<(Option<Category>, ToolKey)>,
}

/// Which tool, and which of its things, is picked: switching between two service buildings or
/// two networks is a switch of tool for the overlay.
#[derive(Clone, Debug, Default, PartialEq)]
struct ToolKey {
    active: ActiveTool,
    building: Option<cp_content::BuildingDefId>,
    utility: Option<cp_content::UtilityKind>,
    remove: BulldozeLayer,
}

impl ToolKey {
    fn of(tools: &ToolState) -> Self {
        Self {
            active: tools.active,
            building: tools.building.def,
            utility: Some(tools.utility.kind),
            remove: tools.bulldoze_layer,
        }
    }
}

/// BUG-083: an overlay a tool or a category opened by itself belongs to that tool. When the
/// player goes on to another category, or picks another tool or another thing of the tool, it is
/// closed — unless the new tool opened its own layout in the same frame, which is a fresh
/// automatic open and is tied to the new tool. Leaving the tool for none keeps it (the Esc chain:
/// tool, layout, menu). An overlay the player picked (View menu, services panel, advisor) is
/// never touched. Called once per frame, after the panels.
pub(crate) fn release_auto_overlay(
    state: &mut UiState,
    tools: &ToolState,
    overlay: &mut OverlayState,
) {
    let auto = &mut state.auto_overlay;
    if !overlay.auto || overlay.active == Overlay::Off {
        auto.to = None;
        auto.seen = overlay.auto_opens;
        return;
    }
    let now = (state.category, ToolKey::of(tools));
    let fresh = overlay.auto_opens != auto.seen;
    let Some(bound) = auto.to.as_ref().filter(|_| !fresh) else {
        auto.seen = overlay.auto_opens;
        auto.to = Some(now);
        return;
    };
    let category_moved = now.0.is_some() && now.0 != bound.0;
    let tool_moved = now.1 != bound.1 && now.1.active != ActiveTool::None;
    if category_moved || tool_moved {
        overlay.close();
        auto.to = None;
    }
}

/// The windows the top bar switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiWindow {
    Budget,
    Demand,
    Metrics,
    Jobs,
    Services,
    Lines,
    Policies,
    Saves,
    Debug,
}

impl UiState {
    /// Opens `w` (the advisor's "Show"); the save menu is not a `UiState` window.
    pub fn open_window(&mut self, w: UiWindow) {
        match w {
            UiWindow::Budget => self.budget_open = true,
            UiWindow::Demand => self.demand_open = true,
            UiWindow::Metrics => self.metrics_open = true,
            UiWindow::Jobs => self.jobs_open = true,
            UiWindow::Services => self.services_open = true,
            UiWindow::Lines => self.lines_open = true,
            UiWindow::Policies => self.policies_open = true,
            UiWindow::Debug => self.debug_open = true,
            UiWindow::Saves => {}
        }
    }
}

impl UiWindow {
    pub const ALL: [UiWindow; 9] = [
        UiWindow::Budget,
        UiWindow::Demand,
        UiWindow::Metrics,
        UiWindow::Jobs,
        UiWindow::Services,
        UiWindow::Lines,
        UiWindow::Policies,
        UiWindow::Saves,
        UiWindow::Debug,
    ];

    /// The window named `name` (lower case: `budget`, `jobs`, ...), for the `--open` flag.
    pub fn parse(name: &str) -> Option<UiWindow> {
        Some(match name {
            "budget" => UiWindow::Budget,
            "demand" => UiWindow::Demand,
            "metrics" => UiWindow::Metrics,
            "jobs" => UiWindow::Jobs,
            "services" => UiWindow::Services,
            "lines" => UiWindow::Lines,
            "policies" => UiWindow::Policies,
            "saves" => UiWindow::Saves,
            "debug" => UiWindow::Debug,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            UiWindow::Budget => "Budget",
            UiWindow::Demand => "Demand",
            UiWindow::Metrics => "Metrics",
            UiWindow::Jobs => "Jobs & education",
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
            UiWindow::Jobs => Icon::Jobs,
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

/// The short name of day `d` of the week (0 is Monday).
fn weekday(d: usize) -> String {
    match d {
        0 => tr("Mon"),
        1 => tr("Tue"),
        2 => tr("Wed"),
        3 => tr("Thu"),
        4 => tr("Fri"),
        5 => tr("Sat"),
        _ => tr("Sun"),
    }
}

/// "Mon 12:30, day 4" from the game time.
pub fn format_time(view: &SimView) -> String {
    let t = view.game_time;
    let minutes = u64::from(t.tick_of_day) * 24 * 60 / cp_core::time::TICKS_PER_DAY;
    trf!(
        "{weekday} {time}  ·  day {day}",
        weekday = weekday(t.day_of_week() as usize),
        time = format!("{:02}:{:02}", minutes / 60, minutes % 60),
        day = t.day + 1
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
    /// Width of what the bar draws.
    pub content_width: f32,
}

/// Whether the O key was pressed (folds or opens the overlays panel): not with a modifier, not while a text
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

/// Top bar: the clock and the speed icons in the middle (T-289: the balance, the population and
/// the gauges are in the bottom panel, the window buttons and the overlays in the panel at the
/// top right). A speed click pushes `SetSpeed` into `outbox`.
pub fn top_bar(root: &mut egui::Ui, view: &SimView, outbox: &mut CommandOutbox) -> TopRects {
    let mut rects = TopRects::default();
    let bar = egui::Panel::top("top_bar")
        .exact_size(Sizes::TOP_BAR_H)
        .show(root, |ui| {
            let full = ui.max_rect();
            let mid = egui::Rect::from_center_size(
                full.center(),
                egui::vec2(Sizes::TOP_CENTER_W, full.height()),
            );
            let mut centre = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(mid)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
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
            rects.content_width = Sizes::TOP_CENTER_W;
        });
    rects.bar = Some(bar.response.rect);
    rects
}

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

/// The line under a service ghost's verdict (T-254): what its road coverage would reach.
pub fn coverage_text(c: cp_render::CoverageCount) -> String {
    trf!(
        plural "Covers {n} buildings, {newly} newly · capacity {capacity}",
        n = c.reached,
        newly = c.newly,
        capacity = c.capacity
    )
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
            // The 1-cell cursor before the first click (T-286).
            Some(g) if g.cursor_piece && g.valid => {
                ui.weak(tr("Click to start a road. Esc cancels."));
            }
            Some(g) if g.valid => {
                let snap = snap_label(g.snap.kind).map_or(String::new(), |l| format!("  ⌖ {l}"));
                // The 8-cell block guide's verdict (T-185).
                let block = g
                    .block
                    .as_ref()
                    .map_or(String::new(), |b| format!("  ▦ {}", b.note));
                let length = trf!("{len} m", len = format!("{:.0}", plan_length(g)));
                ok(
                    ui,
                    &tr("can build"),
                    g.cost,
                    format!(" · {length}{snap}{block}"),
                );
                // One-way direction warnings (§6.6): yellow, they never block the build.
                for w in &g.warnings {
                    ui.colored_label(egui::Color32::YELLOW, format!("⚠ {w}"));
                }
            }
            Some(g) => bad(ui, &g.errors, &tr("cannot build here")),
            None => {
                ui.weak(tr("Click to start a road. Esc cancels."));
            }
        },
        (Category::Roads, ActiveTool::Flip) => match &tools.flip_preview {
            Some(f) if !f.errors.is_empty() => bad(ui, &f.errors, &tr("cannot flip")),
            Some(f) => {
                let n = f.run.len();
                ok(
                    ui,
                    &tr("can flip"),
                    None,
                    format!(" · {}", trn("{n} segments", n as i64)),
                );
            }
            None => {
                ui.weak(tr(
                    "Click a one-way road to turn its whole run round. Esc leaves the tool.",
                ));
            }
        },
        (Category::Roads, ActiveTool::Junctions) => {
            ui.weak(tr(
                "Click a traffic light to switch it, a sign to step it: main, yield, none. Esc leaves the tool.",
            ));
        }
        (Category::Roads, ActiveTool::Upgrade) => match &tools.upgrade_preview {
            Some(u) if !u.errors.is_empty() => bad(ui, &u.errors, &tr("cannot upgrade")),
            Some(u) if u.is_noop() => {
                ui.weak(tr("This run already has the chosen road type."));
            }
            Some(u) => {
                let n = u.changes.len();
                ok(
                    ui,
                    &tr("can upgrade"),
                    Some(u.cost),
                    format!(" · {}", trn("{n} segments", n as i64)),
                );
                if let Some(w) = u.warning() {
                    ui.colored_label(egui::Color32::YELLOW, format!("⚠ {w}"));
                }
            }
            None => {
                ui.weak(tr("Pick a road type, then click a road to change its whole run. Esc leaves the tool."));
            }
        },
        (
            Category::Buildings
            | Category::Power
            | Category::Water
            | Category::Transit
            | Category::Industry
            | Category::Parks,
            ActiveTool::Place,
        ) => match &tools.place_ghost {
            Some(g) => {
                if let Some((have, need)) = g.richness {
                    let color = if have >= need {
                        egui::Color32::LIGHT_GREEN
                    } else {
                        egui::Color32::from_rgb(230, 90, 80)
                    };
                    ui.colored_label(
                        color,
                        trf!(
                            "Lot richness {have}% (needs {need}%)",
                            have = have,
                            need = need
                        ),
                    );
                }
                if g.valid {
                    ok(ui, &tr("can place"), g.cost, String::new());
                    if let Some(c) = tools.coverage {
                        ui.colored_label(egui::Color32::from_rgb(110, 190, 255), coverage_text(c));
                        ui.weak(tr(
                            "Blue: newly covered · white: already covered · green to yellow: coverage now",
                        ));
                    }
                    if let Some(text) = g.demolish_text() {
                        ui.colored_label(egui::Color32::from_rgb(230, 90, 80), text);
                    }
                } else {
                    bad(
                        ui,
                        &g.error.iter().cloned().collect::<Vec<_>>(),
                        &tr("cannot place here"),
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
                        ui.weak(tr(
                            "T turns 15°, Shift+T along the nearest road, Esc leaves the tool.",
                        ));
                    }
                    None => {
                        ui.weak(tr("T rotates, Esc leaves the tool."));
                    }
                }
            }
            None => {
                ui.weak(tr(
                    "Hover a spot beside a road. T rotates, Esc leaves the tool.",
                ));
            }
        },
        (Category::Buildings | Category::Power | Category::Water, ActiveTool::Utility) => {
            match &tools.utility_ghost {
                Some(g) if g.cursor_piece && g.valid => {
                    ui.weak(tr("Click to start. Alt switches the snaps off. Buildings within reach of the network are connected (the band shows the reach). Esc cancels."));
                }
                Some(g) if g.valid => {
                    let snap =
                        snap_label(g.snap.kind).map_or(String::new(), |l| format!("  ⌖ {l}"));
                    ok(ui, &tr("can build"), g.cost, snap)
                }
                Some(g) => bad(ui, &g.errors, &tr("cannot build here")),
                None => {
                    ui.weak(tr("Click to start. Alt switches the snaps off. Buildings within reach of the network are connected (the band shows the reach). Esc cancels."));
                }
            }
        }
        (Category::Transit, ActiveTool::Line) => {
            if let Some(notice) = &tools.line_notice {
                bad(ui, std::slice::from_ref(notice), &tr("cannot place a stop"));
            }
            match &tools.line_ghost {
                Some(g) if g.error.is_some() && g.hover.is_some_and(|h| !h.1) => {
                    bad(
                        ui,
                        &g.error.iter().cloned().collect::<Vec<_>>(),
                        &tr("cannot stop here"),
                    );
                }
                Some(g) if g.closes => ok(
                    ui,
                    &tr("click to close the line"),
                    Some(g.cost),
                    format!(" · {}", cp_tools::route_summary(g)),
                ),
                Some(g) if g.route.len() > 1 => {
                    if let Some(e) = &g.error {
                        ui.colored_label(egui::Color32::YELLOW, format!("⚠ {e}"));
                    }
                    ui.weak(trf!(
                        plural "{n} stops · {route}. Click the first stop to close the line.",
                        n = g.stops.len(),
                        route = cp_tools::route_summary(g)
                    ));
                }
                _ => {
                    ui.weak(tr(
                        "Click a road to put the first stop. Backspace undoes, Esc cancels.",
                    ));
                }
            }
        }
        (Category::Transit, _) => {
            ui.weak(tr(
                "Pick the bus line tool or the depot; the Transit view shows the lines.",
            ));
        }
        (Category::Zones, ActiveTool::Zone) => {
            let n = tools.zone_preview.len();
            let what = match tools.zone.kind {
                ZoneToolKind::Rectangle if tools.zone.corner.is_some() => {
                    tr("click the opposite corner")
                }
                ZoneToolKind::Rectangle => tr("click the first corner"),
                ZoneToolKind::FillBlock => tr("click a block to fill it"),
                _ => tr("hold and drag to paint"),
            };
            ui.weak(trf!(
                plural "{what} — {n} cells under the cursor. Esc cancels.",
                n = n,
                what = what
            ));
        }
        (Category::Districts, ActiveTool::District) if tools.district.selected.is_none() => {
            ui.weak(tr(
                "Choose a district in the list (or press New district), then paint.",
            ));
        }
        (Category::Districts, ActiveTool::District) => {
            let n = tools.district_preview.len();
            ui.weak(trf!(
                plural "hold and drag to paint — {n} cells under the cursor. Esc leaves the brush.",
                n = n
            ));
        }
        (Category::Districts, _) => {
            ui.weak(tr(
                "Pick a brush or create a district; the overlay shows the districts.",
            ));
        }
        (Category::Industry, ActiveTool::Area) if tools.area.selected.is_none() => {
            ui.weak(tr(
                "Choose an area in the list (or press New area), then paint.",
            ));
        }
        (Category::Industry, ActiveTool::Area) => {
            let n = tools.area_preview.len();
            ui.weak(trf!(
                plural "hold and drag to paint — {n} cells under the cursor. Esc leaves the brush.",
                n = n
            ));
        }
        (Category::Industry, _) => {
            ui.weak(tr(
                "Pick a brush or create an area; the overlays show the resources and the areas.",
            ));
        }
        (Category::Parks, ActiveTool::Area) if tools.area.selected.is_none() => {
            ui.weak(tr(
                "Choose a park area in the list (or press New area), then paint.",
            ));
        }
        (Category::Parks, ActiveTool::Area) => {
            let n = tools.area_preview.len();
            ui.weak(trf!(
                plural "hold and drag to paint — {n} cells under the cursor. Esc leaves the brush.",
                n = n
            ));
        }
        (Category::Parks, _) => {
            ui.weak(tr(
                "Pick a brush or a park kind; the overlay shows the park areas and their gates.",
            ));
        }
        (Category::Zones, _) => {
            ui.weak(tr(
                "Pick a zoning tool or a zone type to show the zone grid.",
            ));
        }
        // The bulldozer in one utility layer (T-286).
        (_, ActiveTool::Bulldozer) if tools.bulldoze_layer == BulldozeLayer::Water => {
            ui.weak(tr("Removes only pipes and water buildings. Hover one and click; B or Esc goes back to the pipe tool."));
        }
        (_, ActiveTool::Bulldozer) if tools.bulldoze_layer == BulldozeLayer::Power => {
            ui.weak(tr("Removes only power lines and power buildings. Hover one and click; B or Esc goes back to the line tool."));
        }
        (_, ActiveTool::Bulldozer) if tools.highlight_outside => {
            bad(ui, &[tr("outside your land")], &tr("cannot remove"));
        }
        (_, ActiveTool::Bulldozer) => {
            ui.weak(tr("Hover a building, bus stop or road segment, click to remove it. Esc leaves the tool."));
        }
        (Category::Buildings, _) => {
            ui.weak(tr(
                "Pick a building; the overlay shows which buildings lack supply.",
            ));
        }
        (Category::Power | Category::Water, _) => {
            ui.weak(tr("Pick a building or a network tool; the overlay shows the network and which buildings lack supply."));
        }
        _ => {
            ui.weak(tr("Pick a tool."));
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
    crate::buildings::buildings_panel(ui, view, tools, overlay, crate::buildings::Scope::Buildings);
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
    section(ui, &tr("Tool"), |ui| {
        ui.horizontal_wrapped(|ui| {
            for kind in ZoneToolKind::ALL {
                let selected = tools.active == ActiveTool::Zone && tools.zone.kind == kind;
                if tool_button(ui, zone_kind_icon(kind), &tr(kind.label()), selected).clicked() {
                    if tools.active != ActiveTool::Zone {
                        tools.select(ActiveTool::Zone);
                    }
                    tools.zone.set_kind(kind);
                }
            }
            if tool_tile_enabled(
                ui,
                TileIcon::Icon(Icon::Remove),
                &tr("Off"),
                false,
                tools.active == ActiveTool::Zone,
            )
            .clicked()
            {
                tools.select(ActiveTool::None);
            }
        });
    });
    section(ui, &tr("Zone"), |ui| {
        // A zone type that waits for a milestone is hidden; once open it comes as its own row,
        // one row per milestone that opened zones (T-283).
        let mut rows: std::collections::BTreeMap<usize, Vec<cp_content::ZoneType>> =
            Default::default();
        for zone in PALETTE {
            let locked = view.is_some_and(|v| crate::windows::zone_lock(v, zone).is_some());
            if locked {
                continue;
            }
            let row = view
                .and_then(|v| v.content.as_deref())
                .and_then(|db| db.zone_unlock_milestone(zone))
                .unwrap_or(0);
            rows.entry(row).or_default().push(zone);
        }
        for zones in rows.values() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
                for &zone in zones {
                    let tile = tool_tile_enabled(
                        ui,
                        TileIcon::Icon(crate::icons::zone_icon(zone)),
                        &tr(zone_name(zone)),
                        tools.zone.zone == zone,
                        true,
                    );
                    let tile = match view {
                        Some(view) => tile.on_hover_text(crate::windows::zone_tooltip(view, zone)),
                        None => tile,
                    };
                    if tile.clicked() {
                        tools.zone.zone = zone;
                        if tools.active != ActiveTool::Zone {
                            tools.select(ActiveTool::Zone);
                        }
                    }
                }
            });
        }
    });
    if ui.button(tr("Demand by zone ▸")).clicked() {
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
            rects = top_bar(ui, view, &mut CommandOutbox::default());
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
                top_bar(ui, view, outbox);
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
            ..Default::default()
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
            ..Default::default()
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
        rects: TopRects,
        t: f64,
    }

    impl Bar {
        fn new() -> Self {
            let mut bar = Bar {
                ctx: egui::Context::default(),
                view: SimRunner::new(1).view(),
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
                rects,
                t,
            } = self;
            crate::testutil::frame_texts(ctx, input(*t, events), |ui| {
                *rects = top_bar(ui, view, &mut CommandOutbox::default());
            })
        }
    }

    #[test]
    fn the_top_bar_has_only_the_clock_and_speeds() {
        let mut bar = Bar::new();
        let texts: Vec<String> = bar.frame(vec![]).into_iter().map(|(t, _)| t).collect();
        for gone in ["R", "C", "I", "O", "Always day"] {
            assert!(!texts.iter().any(|t| t == gone), "{gone:?} in {texts:?}");
        }
        for o in Overlay::ALL {
            assert!(!texts.iter().any(|t| t == o.label()), "{o:?} in {texts:?}");
        }
    }

    #[test]
    fn the_o_key_folds_the_overlays_panel_unless_a_modifier_or_a_text_field_has_it() {
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
        // T-284: in Russian too.
        for lang in cp_locale::Lang::ALL {
            let _lang = crate::testutil::in_lang(lang);
            let bar = Bar::new();
            assert!(bar.rects.content_width > 0.0);
            assert!(
                bar.rects.content_width <= 1280.0,
                "{lang:?}: {}",
                bar.rects.content_width
            );
        }
    }

    /// T-284: the language switches while the game runs: the next frame of the bar is in it.
    #[test]
    fn switching_the_language_changes_the_bars_text_at_once() {
        let mut bar = Bar::new();
        let texts = |bar: &mut Bar| -> Vec<String> {
            bar.frame(vec![]).into_iter().map(|(t, _)| t).collect()
        };
        let en = texts(&mut bar);
        let _lang = crate::testutil::in_lang(cp_locale::Lang::Ru);
        let ru = texts(&mut bar);
        assert_ne!(en, ru, "the bar did not change");
        let cyrillic = |t: &String| t.chars().any(|c| ('\u{400}'..='\u{4FF}').contains(&c));
        assert!(!en.iter().any(cyrillic), "{en:?}");
        assert!(ru.iter().any(cyrillic), "{ru:?}");
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

    /// T-283: a locked zone type is hidden; once its milestone is reached it comes as a row of
    /// its own under the first one.
    #[test]
    fn locked_zones_are_hidden_and_an_unlocked_group_is_a_separate_row() {
        let db = std::sync::Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        );
        // (tiles drawn, distinct tile rows) of the zones panel at milestone `reached`.
        let drawn = |reached: u8| {
            let mut sim = SimRunner::new(1);
            sim.set_content(db.clone());
            sim.world_mut()
                .insert_resource(cp_sim::Progress::at(reached));
            let view = sim.view();
            let ctx = egui::Context::default();
            let mut tools = ToolState::default();
            let mut state = UiState::default();
            crate::tile::take_rects(&ctx);
            crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| {
                zones_panel(ui, Some(&view), &mut tools, &mut state);
            });
            let rects = crate::tile::take_rects(&ctx).0;
            let mut tops: Vec<i32> = rects.iter().map(|(t, _)| t.top() as i32).collect();
            tops.sort_unstable();
            tops.dedup();
            (rects.len(), tops.len())
        };
        let (hamlet, hamlet_rows) = drawn(0);
        let (busy, busy_rows) = drawn(5);
        assert_eq!(busy, hamlet + 3, "ResHigh, ComHigh and Office appear");
        assert_eq!(busy_rows, hamlet_rows + 1, "as a row of their own");
    }
}
