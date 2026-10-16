//! Bevy glue for the tools: cursor ray → terrain point, hotkeys, clicks, ghost drawing.
//!
//! All decisions live in the pure modules; this file only moves data between Bevy and them.
//! Keys: PgUp / PgDn raise / lower the road, Esc cancels the drawing (then deselects the tool),
//! Enter finishes a free-mode road or a pipe / power-line polyline. The left mouse button clicks (ignored while the pointer is
//! over the UI, see [`UiCapture`]).

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use cp_core::Cost;
use cp_net::{
    NetCommand, RoadBuildError, RoadPath, SegmentId, SnapReport, SnapReportKind, one_way_report,
};
use cp_render::camera::math::ray_terrain_hit;
use cp_render::{RtsCamera, UiCapture};
use cp_sim::{SimCommand, SimView};

use crate::CommandOutbox;
use crate::band::{BandState, sync_bands};
use crate::building::{BuildingTool, PlacementGhost, pick_building, preview as place_preview};
use crate::bulldozer::{
    PICK_RADIUS, Span, demolition, pick_segment, remove_run_commands, span_commands,
    spans_outside_owned,
};
use crate::road::{
    MarkerKind, RoadMarker, RoadTool, TOO_CLOSE, chevron_lines, preview_report, quantized,
    road_is_one_way, road_markers, sample_curve,
};
use crate::upgrade::{upgrade_commands, upgrade_preview};
use crate::utility::{
    LINK_PICK_RADIUS, UtilityTool, pick_link, plan_polylines, preview as utility_preview,
};
use crate::zone::ZoneTool;
use cp_building::{BuildingCommand, BuildingId};
use cp_content::UtilityKind;
use cp_utility::{UtilityCommand, UtilityLinkId, UtilityNet};
use cp_zone::{ZoneCellRef, ZoneCommand};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActiveTool {
    #[default]
    None,
    Road,
    Bulldozer,
    /// Changing the road type of the run under the cursor to the one chosen in the Roads panel.
    Upgrade,
    /// Zoning: fill block, brushes, rectangle, eraser.
    Zone,
    /// The district brush (Districts panel).
    District,
    /// Placing a service building (type chosen in the Services tab, T rotates).
    Place,
    /// Drawing pipes or power lines (Water / Power tabs).
    Utility,
    /// Removing the pipe or power-line piece under the cursor.
    UtilityRemove,
    /// Drawing a bus line stop by stop (Transport category, T-157).
    Line,
}

/// The road ghost: what the cursor would build.
#[derive(Clone, Debug, PartialEq)]
pub struct Ghost {
    /// Centre lines to draw (the planned segments, or the raw path when invalid).
    pub polylines: Vec<Vec<Vec3>>,
    pub valid: bool,
    /// Cost of the plan; `None` when invalid.
    pub cost: Option<Cost>,
    /// One player-facing line per validation error.
    pub errors: Vec<String>,
    /// Half the road width, metres: drawn as a band. `0.0` draws the polylines as lines.
    pub half_width: f32,
    /// What snapped the cursor point, with its guide line (road ghost only).
    pub snap: SnapReport,
    /// One player-facing line per one-way warning (§6.6); shown in yellow, never blocks.
    pub warnings: Vec<String>,
    /// Direction chevrons of a one-way road, three segments each (road ghost only).
    pub chevrons: Vec<[Vec3; 2]>,
}

#[derive(Resource, Default, Debug)]
pub struct ToolState {
    pub active: ActiveTool,
    pub road: RoadTool,
    pub ghost: Option<Ghost>,
    /// Terrain point under the cursor (XZ).
    pub hover: Option<Vec2>,
    /// `Alt` is held: the road tool neither joins nor steps the length (§6.7).
    pub alt: bool,
    /// Control points of the roads around the cursor (and the 8 m ticks of the one it is next
    /// to) while the road tool is active (§6.7).
    pub markers: Vec<RoadMarker>,
    /// Segments the bulldozer would remove: the whole road run under the cursor (T-070).
    pub highlight: Vec<SegmentId>,
    /// The highlighted run leaves the bought tiles: drawn red, no command on click (§5.1).
    pub highlight_outside: bool,
    /// The exact pieces the bulldozer would take (§6.7); `highlight` lists their segments.
    pub spans: Vec<Span>,
    /// `Shift` is held: the bulldozer takes the whole road run.
    pub shift: bool,
    /// What the upgrade tool would do to the run under the cursor (T-081).
    pub upgrade_preview: Option<crate::upgrade::UpgradePreview>,
    pub zone: ZoneTool,
    pub district: crate::district::DistrictTool,
    /// Cells the district brush would change at the cursor (the highlight).
    pub district_preview: Vec<cp_district::Cell>,
    pub building: BuildingTool,
    pub utility: UtilityTool,
    /// Which network the remove tool picks from.
    pub remove_net: Option<UtilityNet>,
    /// The pipe / power-line ghost (separate from the road ghost).
    pub utility_ghost: Option<Ghost>,
    /// Link the remove tool would delete.
    pub link_highlight: Option<UtilityLinkId>,
    /// What the placement tool would put under the cursor.
    pub place_ghost: Option<PlacementGhost>,
    /// Building the bulldozer would remove (preferred over a segment).
    pub building_highlight: Option<BuildingId>,
    /// Cells the zoning tool would paint at the cursor (the highlight).
    pub zone_preview: Vec<ZoneCellRef>,
    /// The bus-line tool (T-157), its ghost and the last refusal to show in the verdict.
    pub line: crate::transit::LineTool,
    pub line_ghost: Option<crate::transit::LineGhostInfo>,
    pub line_notice: Option<String>,
    /// The stop the bulldozer would remove.
    pub stop_highlight: Option<cp_transit::StopId>,
    /// The speed Space resumes after a pause (never `Pause`; `X1` until one was running).
    pub resume_speed: cp_core::SimSpeed,
    /// Inputs the current ghost was computed from; unchanged inputs skip the revalidation.
    ghost_key: Option<GhostKey>,
    utility_key: Option<UtilityGhostKey>,
    line_key: Option<LineGhostKey>,
}

#[derive(Clone, PartialEq, Debug)]
struct LineGhostKey {
    hover: (i32, i32),
    tool: crate::transit::LineTool,
    transit_ptr: usize,
}

#[derive(Clone, PartialEq, Debug)]
struct UtilityGhostKey {
    hover: (i32, i32),
    tool: UtilityTool,
    graph_ptr: usize,
}

#[derive(Clone, PartialEq, Debug)]
struct GhostKey {
    hover: (i32, i32),
    alt: bool,
    road: RoadTool,
    net_ptr: usize,
}

impl ToolState {
    pub fn select(&mut self, tool: ActiveTool) {
        self.active = tool;
        self.road.cancel();
        self.ghost = None;
        self.highlight.clear();
        self.highlight_outside = false;
        self.spans.clear();
        self.upgrade_preview = None;
        self.ghost_key = None;
        self.zone.cancel();
        self.zone_preview.clear();
        self.district.cancel();
        self.district_preview.clear();
        self.place_ghost = None;
        self.building_highlight = None;
        self.utility.cancel();
        self.utility_ghost = None;
        self.utility_key = None;
        self.link_highlight = None;
        self.line.cancel();
        self.line_ghost = None;
        self.line_notice = None;
        self.line_key = None;
        self.stop_highlight = None;
        self.building.moving = None;
    }

    /// Picks the service building `id` up to carry it elsewhere (T-177): the placement tool
    /// with its type, its rules and its ghost, ending in `Move`.
    pub fn begin_move(&mut self, view: &SimView, id: BuildingId) -> bool {
        let mut tool = BuildingTool::default();
        if !tool.start_move(view, id) {
            return false;
        }
        self.select(ActiveTool::Place);
        self.building = tool;
        true
    }
}

/// Hands the line tool's ghost to the renderer.
fn sync_line_ghost(state: Res<ToolState>, mut out: ResMut<cp_render::LineGhost>) {
    let want = match (&state.line_ghost, state.active) {
        (Some(g), ActiveTool::Line) => cp_render::LineGhost {
            route: g.route.clone(),
            color: g.color,
            stops: g.stops.clone(),
            hover: g.hover,
        },
        _ => cp_render::LineGhost::default(),
    };
    if *out != want {
        *out = want;
    }
}

/// Tells the renderer whether the district brush is active (the district overlay shows then).
fn sync_districts_mode(state: Res<ToolState>, mut mode: ResMut<cp_render::DistrictsMode>) {
    let on = state.active == ActiveTool::District;
    if mode.0 != on {
        mode.0 = on;
    }
}

/// Tells the renderer whether the zone tool is active (the zone grid shows more then).
fn sync_zoning_mode(state: Res<ToolState>, mut mode: ResMut<cp_render::ZoningMode>) {
    let zoning = state.active == ActiveTool::Zone;
    if mode.0 != zoning {
        mode.0 = zoning;
    }
}

/// Tells the renderer which pipe layers to draw (T-103): the network while a water tool or the
/// water overlay is active, the pipe ghost, and the piece the remove tool would delete.
fn sync_pipe_layers(
    state: Res<ToolState>,
    overlay: Res<cp_render::OverlayState>,
    view: Option<Res<SimView>>,
    mut layers: ResMut<cp_render::PipeLayers>,
) {
    let content = view.as_deref().and_then(|v| v.content.as_deref());
    let visible = crate::utility::pipes_visible(&state, overlay.active, content);
    let ghost = state
        .utility_ghost
        .as_ref()
        .filter(|_| state.active == ActiveTool::Utility && state.utility.kind == UtilityKind::Water)
        .map(|g| cp_render::PipeGhost {
            curves: g
                .polylines
                .iter()
                .flat_map(|l| l.windows(2))
                .map(|w| cp_core::curve::CubicBezier3::line(w[0], w[1]))
                .collect(),
            valid: g.valid,
        });
    let highlight = view
        .as_deref()
        .zip(state.link_highlight)
        .filter(|_| {
            matches!(
                state.active,
                ActiveTool::UtilityRemove | ActiveTool::Bulldozer
            )
        })
        .and_then(|(v, id)| v.utilities.link(id))
        .filter(|l| l.net == UtilityNet::Pipe)
        .map(|l| l.curve);
    let power_ghost = state
        .utility_ghost
        .as_ref()
        .filter(|_| state.active == ActiveTool::Utility && state.utility.kind == UtilityKind::Power)
        .map(|g| cp_render::PowerGhost {
            segments: g
                .polylines
                .iter()
                .flat_map(|l| l.windows(2))
                .map(|w| [w[0], w[1]])
                .collect(),
            valid: g.valid,
        });
    let power_highlight = view
        .as_deref()
        .zip(state.link_highlight)
        .filter(|_| {
            matches!(
                state.active,
                ActiveTool::UtilityRemove | ActiveTool::Bulldozer
            )
        })
        .and_then(|(v, id)| v.utilities.link(id))
        .filter(|l| l.net == UtilityNet::Power)
        .map(|l| [l.curve.point(0.0), l.curve.point(1.0)]);
    let want = cp_render::PipeLayers {
        visible,
        ghost,
        highlight,
        power_ghost,
        power_highlight,
    };
    if *layers != want {
        *layers = want;
    }
}

pub struct ToolsPlugin;

impl Plugin for ToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ToolState>()
            .init_resource::<crate::zone_lines::ZoneLines>()
            .init_resource::<cp_render::ZoningMode>()
            .add_systems(Update, sync_zoning_mode)
            .init_resource::<cp_render::DistrictsMode>()
            .add_systems(Update, sync_districts_mode)
            .init_resource::<cp_render::PipeLayers>()
            .add_systems(Update, sync_pipe_layers.after(tool_hover))
            .init_resource::<cp_render::LineGhost>()
            .add_systems(Update, sync_line_ghost.after(tool_hover))
            .init_resource::<UiCapture>()
            .init_resource::<CommandOutbox>()
            .init_resource::<crate::Picked>()
            .init_resource::<crate::Inspected>()
            .add_systems(
                Update,
                (tool_pick, draw_selection).chain().after(tool_hover),
            )
            .add_systems(
                Update,
                (
                    tool_hotkeys,
                    tool_hover,
                    tool_click,
                    draw_ghost,
                    crate::zone_lines::sync_zone_lines,
                )
                    .chain(),
            )
            .init_resource::<BandState>()
            .add_systems(Update, sync_bands.after(tool_hover));
    }
}

/// Exported for the headless input tests (`tests/click_through.rs`).
#[doc(hidden)]
pub fn tool_hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    capture: Res<UiCapture>,
    view: Option<Res<SimView>>,
    mut state: ResMut<ToolState>,
    mut outbox: ResMut<CommandOutbox>,
) {
    // A text field has the keyboard (the save name): its keys are not hotkeys.
    if capture.keyboard_captured {
        return;
    }
    if keys.just_pressed(KeyCode::Space)
        && let Some(v) = view.as_deref()
    {
        let next = if v.speed == cp_core::SimSpeed::Pause {
            state.resume_speed
        } else {
            state.resume_speed = v.speed;
            cp_core::SimSpeed::Pause
        };
        outbox.push(SimCommand::SetSpeed { speed: next });
    }
    if keys.just_pressed(KeyCode::KeyB) && state.active != ActiveTool::Bulldozer {
        state.select(ActiveTool::Bulldozer);
    }
    if state.active == ActiveTool::Road {
        if keys.just_pressed(KeyCode::PageUp) {
            state.road.raise();
        }
        if keys.just_pressed(KeyCode::PageDown) {
            state.road.lower();
        }
    }
    if state.active == ActiveTool::Utility && keys.just_pressed(KeyCode::Enter) {
        let before = state.utility.clone();
        if let Some(path) = state.utility.finish() {
            state.utility_key = None;
            if let Some(cmd) =
                crate::utility::chain_commit(view.as_deref(), &mut state.utility, before, path)
            {
                outbox.push(cmd);
            }
        }
    }
    if state.active == ActiveTool::Line && keys.just_pressed(KeyCode::Backspace) {
        state.line.undo();
        state.line_key = None;
        state.line_notice = None;
    }
    if state.active == ActiveTool::Place && keys.just_pressed(KeyCode::KeyT) {
        let align = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let hover = state.hover;
        state.building.turn(view.as_deref(), hover, align);
    }
    if keys.just_pressed(KeyCode::Escape) {
        if state.active == ActiveTool::Zone && state.zone.corner.is_some() {
            state.zone.cancel();
        } else if state.active == ActiveTool::Line && !state.line.stops.is_empty() {
            state.line.cancel();
            state.line_key = None;
            state.line_notice = None;
        } else if state.active == ActiveTool::Utility && !state.utility.points.is_empty() {
            state.utility.cancel();
            state.utility_key = None;
        } else if state.road.points.is_empty() {
            state.select(ActiveTool::None);
        } else {
            state.road.cancel();
        }
    }
    if state.active == ActiveTool::Road && keys.just_pressed(KeyCode::Enter) {
        let before = state.road.clone();
        if let Some(req) = state.road.finish_free() {
            state.ghost_key = None;
            let alt = state.alt;
            if let Some(req) =
                crate::road::chain_commit(view.as_deref(), &mut state.road, before, req, alt)
            {
                outbox.push(build_command(req));
            }
        }
    }
}

fn build_command(req: cp_net::RoadPathRequest) -> SimCommand {
    SimCommand::Net(NetCommand::BuildRoad {
        road: req.road,
        path: req.path,
        elevation: req.elevation,
    })
}

/// Terrain point under the cursor.
fn cursor_ground(
    view: &SimView,
    window: &Window,
    camera: &Camera,
    transform: &GlobalTransform,
) -> Option<Vec2> {
    let cursor = window.cursor_position()?;
    let ray = camera.viewport_to_world(transform, cursor).ok()?;
    let terrain = view.terrain.as_deref()?;
    let hit = ray_terrain_hit(terrain, ray.origin, *ray.direction, 20_000.0)?;
    Some(Vec2::new(hit.x, hit.z))
}

fn tool_hover(
    view: Option<Res<SimView>>,
    capture: Res<UiCapture>,
    window: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mut state: ResMut<ToolState>,
) {
    state.alt = keys
        .as_deref()
        .is_some_and(|k| k.pressed(KeyCode::AltLeft) || k.pressed(KeyCode::AltRight));
    state.shift = keys
        .as_deref()
        .is_some_and(|k| k.pressed(KeyCode::ShiftLeft) || k.pressed(KeyCode::ShiftRight));
    let (Some(view), Ok(window), Ok((camera, transform))) =
        (view, window.single(), camera.single())
    else {
        return;
    };
    state.hover = if capture.pointer_over_ui {
        None
    } else {
        cursor_ground(&view, window, camera, transform)
    };
    let hover = state.hover;
    match (state.active, hover) {
        (ActiveTool::Bulldozer, Some(p)) => {
            state.building_highlight = pick_building(&view, p);
            state.stop_highlight = if state.building_highlight.is_some() {
                None
            } else {
                pick_stop(&view, p)
            };
            state.spans = if state.building_highlight.is_some() || state.stop_highlight.is_some() {
                Vec::new()
            } else {
                demolition(&view.net, p, PICK_RADIUS, state.shift)
            };
            // A pipe or power line only when no road is under the cursor.
            state.link_highlight = if state.building_highlight.is_some()
                || state.stop_highlight.is_some()
                || !state.spans.is_empty()
            {
                None
            } else {
                [UtilityNet::Power, UtilityNet::Pipe]
                    .into_iter()
                    .find_map(|net| pick_link(&view.utilities, net, p, LINK_PICK_RADIUS))
            };
            state.highlight = state.spans.iter().map(|s| s.0).collect();
            state.highlight_outside = spans_outside_owned(&view.net, &view.tiles, &state.spans);
        }
        (ActiveTool::Upgrade, Some(p)) => {
            let preview = match (state.road.road, view.content.as_deref()) {
                (Some(target), Some(content)) => {
                    pick_segment(&view.net, p, PICK_RADIUS).map(|seg| {
                        upgrade_preview(&view.net, &view.tiles, &view.zones, content, seg, target)
                    })
                }
                _ => None,
            };
            state.highlight = preview.as_ref().map(|u| u.run.clone()).unwrap_or_default();
            state.highlight_outside = preview
                .as_ref()
                .is_some_and(|u| u.errors.iter().any(|e| e == crate::upgrade::OUTSIDE_LAND));
            state.upgrade_preview = preview;
        }
        (ActiveTool::Line, Some(p)) => {
            let key = LineGhostKey {
                hover: (p.x.round() as i32, p.y.round() as i32),
                tool: state.line.clone(),
                transit_ptr: std::sync::Arc::as_ptr(&view.transit) as usize,
            };
            if state.line_key.as_ref() != Some(&key) {
                state.line_ghost = crate::transit::line_ghost(&view, &state.line, p);
                state.line_key = Some(key);
            }
        }
        (ActiveTool::Place, Some(p)) => {
            state.place_ghost = place_preview(&view, &state.building, p);
        }
        (ActiveTool::Zone, Some(p)) => {
            state.zone_preview = state.zone.preview(&view.zones, p);
        }
        (ActiveTool::District, Some(p)) => {
            state.district_preview = state.district.preview(p);
        }
        (ActiveTool::Utility, Some(p)) => {
            let key = UtilityGhostKey {
                hover: (p.x.round() as i32, p.y.round() as i32),
                tool: state.utility.clone(),
                graph_ptr: std::sync::Arc::as_ptr(&view.utilities) as usize,
            };
            if state.utility_key.as_ref() != Some(&key) {
                state.utility_ghost = compute_utility_ghost(&view, &state.utility, p);
                state.utility_key = Some(key);
            }
        }
        (ActiveTool::UtilityRemove, Some(p)) => {
            state.link_highlight = state
                .remove_net
                .and_then(|net| pick_link(&view.utilities, net, p, LINK_PICK_RADIUS));
        }
        (ActiveTool::Road, Some(p)) => {
            let key = GhostKey {
                hover: (p.x.round() as i32, p.y.round() as i32),
                alt: state.alt,
                road: state.road.clone(),
                net_ptr: std::sync::Arc::as_ptr(&view.net) as usize,
            };
            if state.ghost_key.as_ref() != Some(&key) {
                state.ghost = compute_ghost_alt(&view, &state.road, p, state.alt);
                state.markers = road_markers(&view, p);
                state.ghost_key = Some(key);
            }
        }
        _ => {
            state.highlight.clear();
            state.highlight_outside = false;
            state.spans.clear();
            state.upgrade_preview = None;
            state.building_highlight = None;
            state.stop_highlight = None;
            if state.active != ActiveTool::Line {
                state.line_ghost = None;
                state.line_key = None;
            }
            state.place_ghost = None;
            state.zone_preview.clear();
            state.district_preview.clear();
            state.link_highlight = None;
            if state.active != ActiveTool::Utility {
                state.utility_ghost = None;
                state.utility_key = None;
            }
            if state.active != ActiveTool::Road {
                state.ghost = None;
                state.markers.clear();
            }
        }
    }
}

/// The stop within reach of `p` (the bulldozer's target; stops come before roads).
fn pick_stop(view: &SimView, p: Vec2) -> Option<cp_transit::StopId> {
    view.transit
        .net
        .stops
        .iter()
        .map(|(id, st)| (st.pos.distance(p), id))
        .filter(|(d, _)| *d <= crate::transit::STOP_PICK_RADIUS)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
}

/// The ghost for the road tool with the cursor at `hover`, or `None` when nothing is drawn
/// yet (or there is no road type / terrain / content).
pub fn compute_ghost(view: &SimView, tool: &RoadTool, hover: Vec2) -> Option<Ghost> {
    compute_ghost_alt(view, tool, hover, false)
}

/// [`compute_ghost`] with the `Alt` state: the path is quantized to the 8 m step and the joins
/// of §6.7 first (what `BuildRoad` will be sent), unless `alt`.
pub fn compute_ghost_alt(view: &SimView, tool: &RoadTool, hover: Vec2, alt: bool) -> Option<Ghost> {
    let path = tool.path_with_hover(hover)?;
    let (req, step) = quantized(view, tool.request(path)?, alt);
    let path = req.path.clone();
    let half_width = view
        .content
        .as_deref()
        .map_or(0.0, |c| c.road(req.road).width * 0.5);
    let (result, mut report) = preview_report(view, &req)?;
    // The step's own snap (a control-point perpendicular, a joined tick) is what the player
    // should see, unless the validator found something more specific for the last vertex.
    if matches!(step.kind, SnapReportKind::ControlLine(_))
        || (report.kind == SnapReportKind::None && step.kind != SnapReportKind::None)
    {
        report = step;
    }
    Some(match result {
        Ok(plan)
            if !cp_net::clearance_report(&view.net, view.content.as_deref()?, &plan).is_empty() =>
        {
            Ghost {
                polylines: plan
                    .segments
                    .iter()
                    .map(|s| sample_curve(&s.curve, 3.0))
                    .collect(),
                valid: false,
                cost: None,
                errors: vec![TOO_CLOSE.to_string()],
                half_width,
                snap: report,
                warnings: Vec::new(),
                chevrons: Vec::new(),
            }
        }
        Ok(plan) => {
            let content = view.content.as_deref()?;
            let one_way = road_is_one_way(content.road(plan.road));
            Ghost {
                polylines: plan
                    .segments
                    .iter()
                    .map(|s| sample_curve(&s.curve, 3.0))
                    .collect(),
                valid: true,
                cost: Some(plan.cost),
                errors: Vec::new(),
                half_width,
                snap: report,
                warnings: one_way_report(&view.net, content, &plan)
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                chevrons: if one_way {
                    plan.segments
                        .iter()
                        .flat_map(|s| chevron_lines(&s.curve, s.length))
                        .collect()
                } else {
                    Vec::new()
                },
            }
        }
        Err(errors) => Ghost {
            polylines: vec![raw_polyline(view, &path)],
            valid: false,
            cost: None,
            errors: errors.iter().map(RoadBuildError::to_string).collect(),
            half_width,
            snap: report,
            warnings: Vec::new(),
            chevrons: Vec::new(),
        },
    })
}

/// The ghost for the pipe / power-line tool with the cursor at `hover`: green pieces of the
/// plan when `BuildLink` would accept the path, the raw red path with every problem otherwise.
pub fn compute_utility_ghost(view: &SimView, tool: &UtilityTool, hover: Vec2) -> Option<Ghost> {
    let path = tool.path_with_hover(hover)?;
    Some(match utility_preview(view, tool.kind, &path)? {
        Ok(plan) => Ghost {
            polylines: plan_polylines(&view.utilities, &plan),
            valid: true,
            cost: Some(plan.cost),
            errors: Vec::new(),
            half_width: 0.0,
            snap: SnapReport::default(),
            warnings: Vec::new(),
            chevrons: Vec::new(),
        },
        Err(problems) => Ghost {
            polylines: vec![
                path.iter()
                    .map(|p| {
                        let y = view.terrain.as_deref().map_or(0.0, |t| t.height(*p));
                        Vec3::new(p.x, y + 0.5, p.y)
                    })
                    .collect(),
            ],
            valid: false,
            cost: None,
            errors: problems.iter().map(ToString::to_string).collect(),
            half_width: 0.0,
            snap: SnapReport::default(),
            warnings: Vec::new(),
            chevrons: Vec::new(),
        },
    })
}

/// The drawn path itself at terrain height, for the red ghost.
fn raw_polyline(view: &SimView, path: &RoadPath) -> Vec<Vec3> {
    let lift = |p: Vec2| {
        let y = view.terrain.as_deref().map_or(0.0, |t| t.height(p));
        Vec3::new(p.x, y + 0.5, p.y)
    };
    match path {
        RoadPath::Straight { from, to } => vec![lift(*from), lift(*to)],
        RoadPath::Curve { from, control, to } => (0..=24)
            .map(|i| {
                let t = i as f32 / 24.0;
                let u = 1.0 - t;
                lift(*from * (u * u) + *control * (2.0 * u * t) + *to * (t * t))
            })
            .collect(),
        RoadPath::Free(pts) => pts.iter().map(|p| lift(*p)).collect(),
    }
}

/// Exported for the headless input tests (`tests/click_through.rs`).
#[doc(hidden)]
pub fn tool_click(
    buttons: Res<ButtonInput<MouseButton>>,
    capture: Res<UiCapture>,
    view: Option<Res<SimView>>,
    mut state: ResMut<ToolState>,
    mut outbox: ResMut<CommandOutbox>,
) {
    // A right click puts a picked-up building back where it stood.
    if buttons.just_pressed(MouseButton::Right)
        && !capture.pointer_blocked()
        && state.active == ActiveTool::Place
        && state.building.moving.is_some()
    {
        state.select(ActiveTool::None);
        return;
    }
    if buttons.just_released(MouseButton::Left) {
        state.zone.end_stroke();
        state.district.end_stroke();
    }
    if state.active == ActiveTool::District
        && !capture.pointer_blocked()
        && let (Some(view), Some(p)) = (view.as_deref(), state.hover)
        && (buttons.just_pressed(MouseButton::Left) || buttons.pressed(MouseButton::Left))
    {
        if let Some(cmd) = state.district.drag(&view.district_map, p) {
            outbox.push(SimCommand::District(cmd));
        }
        return;
    }
    if state.active == ActiveTool::Zone
        && !capture.pointer_blocked()
        && let (Some(view), Some(p)) = (view.as_deref(), state.hover)
    {
        let painted = if buttons.just_pressed(MouseButton::Left) {
            state.zone.click(&view.zones, p)
        } else if buttons.pressed(MouseButton::Left) {
            state.zone.drag(&view.zones, p)
        } else {
            None
        };
        if let Some((cells, zone)) = painted {
            outbox.push(SimCommand::Zone(ZoneCommand::PaintZones { cells, zone }));
        }
        return;
    }
    if !buttons.just_pressed(MouseButton::Left) || capture.pointer_blocked() {
        return;
    }
    let Some(p) = state.hover else {
        return;
    };
    match state.active {
        ActiveTool::Road => {
            let before = state.road.clone();
            if let Some(req) = state.road.click(p) {
                let alt = state.alt;
                if let Some(req) =
                    crate::road::chain_commit(view.as_deref(), &mut state.road, before, req, alt)
                {
                    outbox.push(build_command(req));
                }
            }
            state.ghost_key = None;
        }
        ActiveTool::Bulldozer => {
            if let Some(building) = state.building_highlight {
                outbox.push(SimCommand::Building(BuildingCommand::Demolish { building }));
                state.building_highlight = None;
            } else if let Some(stop) = state.stop_highlight.take() {
                outbox.push(SimCommand::Transit(
                    cp_transit::TransitCommand::RemoveStop { stop },
                ));
            } else if let Some(link) = state.link_highlight.take() {
                outbox.push(SimCommand::Utility(UtilityCommand::RemoveLink { link }));
            } else if !state.highlight.is_empty() && !state.highlight_outside {
                let commands = match view.as_deref() {
                    Some(v) if !state.spans.is_empty() => span_commands(&v.net, &state.spans),
                    _ => remove_run_commands(&state.highlight),
                };
                for command in commands {
                    outbox.push(command);
                }
                state.highlight.clear();
                state.spans.clear();
            }
        }
        ActiveTool::Upgrade => {
            if let (Some(preview), Some(target)) = (state.upgrade_preview.take(), state.road.road) {
                for command in upgrade_commands(&preview, target) {
                    outbox.push(command);
                }
                state.highlight.clear();
            }
        }
        ActiveTool::Place => {
            if let Some(def) = state.building.def
                && let Some(g) = state.place_ghost.as_ref().filter(|g| g.valid)
            {
                // A free-standing ghost carries its own pose (the cursor rounded to 1 m).
                let (pos, rot) = match &g.free {
                    Some(f) => (f.pos, f.rot),
                    None => (p, state.building.rot()),
                };
                if let Some(building) = state.building.moving {
                    outbox.push(SimCommand::Building(BuildingCommand::Move {
                        building,
                        pos,
                        rot,
                    }));
                    state.select(ActiveTool::None);
                } else {
                    outbox.push(SimCommand::Building(BuildingCommand::Place {
                        def,
                        pos,
                        rot,
                    }));
                }
            }
        }
        ActiveTool::Utility => {
            let before = state.utility.clone();
            if let Some(path) = state.utility.click(p)
                && let Some(cmd) =
                    crate::utility::chain_commit(view.as_deref(), &mut state.utility, before, path)
            {
                outbox.push(cmd);
            }
            state.utility_key = None;
        }
        ActiveTool::Line => {
            if let Some(view) = view.as_deref() {
                state.line_notice = None;
                match state.line.click(view, p) {
                    crate::transit::Click::Create(cmd) => outbox.push(cmd),
                    crate::transit::Click::Refused(why) => state.line_notice = Some(why),
                    crate::transit::Click::Added | crate::transit::Click::Nothing => {}
                }
            }
            state.line_key = None;
        }
        ActiveTool::UtilityRemove => {
            if let Some(link) = state.link_highlight.take() {
                outbox.push(SimCommand::Utility(UtilityCommand::RemoveLink { link }));
            }
        }
        ActiveTool::None | ActiveTool::Zone | ActiveTool::District => {}
    }
}

fn draw_ghost(state: Res<ToolState>, view: Option<Res<SimView>>, mut gizmos: Gizmos) {
    if let (Some(view), Some(stop)) = (view.as_deref(), state.stop_highlight)
        && let Some(st) = view.transit.net.stops.get(stop)
    {
        let y = view.terrain.as_deref().map_or(0.0, |t| t.height(st.pos)) + 1.0;
        gizmos.circle(
            Isometry3d::new(
                Vec3::new(st.pos.x, y, st.pos.y),
                Quat::from_rotation_arc(Vec3::Z, Vec3::Y),
            ),
            3.5,
            Color::srgb(0.95, 0.2, 0.2),
        );
    }
    if state.active == ActiveTool::District
        && let Some(view) = view.as_deref()
    {
        let color = match (state.district.kind, state.district.selected) {
            (crate::district::DistrictToolKind::Erase, _) | (_, None) => {
                Color::srgb(0.95, 0.95, 0.95)
            }
            (_, Some(id)) => view.districts.get(id).map_or(Color::WHITE, |d| {
                let c =
                    cp_district::PALETTE_RGB[usize::from(d.color) % cp_district::PALETTE_RGB.len()];
                Color::srgb_u8(c[0], c[1], c[2])
            }),
        };
        let height = |p: Vec2| view.terrain.as_deref().map_or(0.0, |t| t.height(p)) + 0.8;
        for &cell in &state.district_preview {
            let c = cp_district::DistrictMap::cell_center(cell);
            let h = cp_district::CELL_M * 0.5;
            let k = [
                Vec2::new(c.x - h, c.y - h),
                Vec2::new(c.x + h, c.y - h),
                Vec2::new(c.x + h, c.y + h),
                Vec2::new(c.x - h, c.y + h),
            ]
            .map(|q| Vec3::new(q.x, height(q), q.y));
            gizmos.linestrip([k[0], k[1], k[2], k[3], k[0]], color);
        }
    }
    if let Some(ghost) = &state.ghost
        && state.active == ActiveTool::Road
    {
        let color = if ghost.valid {
            Color::srgb(0.9, 1.0, 0.9)
        } else {
            Color::srgb(0.95, 0.2, 0.2)
        };
        for [a, b] in &ghost.chevrons {
            gizmos.line(*a, *b, color);
        }
    }
    if let Some(ghost) = &state.ghost
        && state.active == ActiveTool::Road
        && let Some((a, b)) = ghost.snap.guide
    {
        let y = |p: Vec2| {
            view.as_deref()
                .and_then(|v| v.terrain.as_deref())
                .map_or(0.0, |t| t.height(p))
                + 0.5
        };
        gizmos.line(
            Vec3::new(a.x, y(a), a.y),
            Vec3::new(b.x, y(b), b.y),
            Color::srgba(0.4, 0.8, 1.0, 0.8),
        );
    }
    if state.active == ActiveTool::Road
        && let Some(view) = view.as_deref()
    {
        let y = |p: Vec2| view.terrain.as_deref().map_or(0.0, |t| t.height(p)) + 0.7;
        let at = |p: Vec2, dx: f32, dz: f32| Vec3::new(p.x + dx, y(p), p.y + dz);
        let lit = state
            .ghost
            .as_ref()
            .filter(|g| matches!(g.snap.kind, SnapReportKind::ControlLine(_)))
            .and_then(|g| g.snap.guide)
            .map(|(a, b)| (a + b) * 0.5);
        for m in &state.markers {
            match m.kind {
                MarkerKind::Control => {
                    let highlighted = lit.is_some_and(|c| c.distance(m.pos) < 1.0);
                    let (r, color) = if highlighted {
                        (3.5, Color::srgb(1.0, 0.85, 0.2))
                    } else {
                        (2.2, Color::srgba(0.4, 0.8, 1.0, 0.9))
                    };
                    gizmos.linestrip(
                        [
                            at(m.pos, r, 0.0),
                            at(m.pos, 0.0, r),
                            at(m.pos, -r, 0.0),
                            at(m.pos, 0.0, -r),
                            at(m.pos, r, 0.0),
                        ],
                        color,
                    );
                }
                MarkerKind::Tick => {
                    let color = Color::srgba(0.4, 0.8, 1.0, 0.6);
                    gizmos.line(at(m.pos, -0.8, 0.0), at(m.pos, 0.8, 0.0), color);
                    gizmos.line(at(m.pos, 0.0, -0.8), at(m.pos, 0.0, 0.8), color);
                }
            }
        }
    }
    if let (Some(view), Some(g)) = (view.as_deref(), &state.place_ghost)
        && state.active == ActiveTool::Place
    {
        let color = if g.valid {
            Color::srgb(0.2, 0.9, 0.3)
        } else {
            Color::srgb(0.95, 0.2, 0.2)
        };
        draw_cells(view, &g.cells, color, &mut gizmos);
        if let Some(f) = &g.free {
            draw_free(view, f, color, &mut gizmos);
        }
    }
    if let (Some(view), Some(id)) = (view.as_deref(), state.building_highlight)
        && let Some(b) = view.buildings.get(id)
    {
        let cells: Vec<ZoneCellRef> = b.lot().map(|l| l.cells().collect()).unwrap_or_default();
        draw_cells(view, &cells, Color::srgb(1.0, 0.6, 0.1), &mut gizmos);
    }
}

/// The rectangle of a free-standing ghost, the connection circle (40 m) around it and a line
/// to the network it would connect to.
fn draw_free(view: &SimView, f: &crate::FreeGhost, color: Color, gizmos: &mut Gizmos) {
    let height = |p: Vec2| view.terrain.as_deref().map_or(0.0, |t| t.height(p)) + 0.6;
    let k = f.polygon.map(|q| Vec3::new(q.x, height(q), q.y));
    gizmos.linestrip([k[0], k[1], k[2], k[3], k[0]], color);
    gizmos.line(k[0], k[2], color);
    gizmos.line(k[1], k[3], color);
    let ring = Color::srgba(0.4, 0.8, 1.0, 0.7);
    const SEGMENTS: usize = 48;
    gizmos.linestrip(
        (0..=SEGMENTS).map(|i| {
            let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let p = f.pos + Vec2::new(a.cos(), a.sin()) * cp_utility::LINE_REACH_M;
            Vec3::new(p.x, height(p), p.y)
        }),
        ring,
    );
    if let Some(h) = &f.hint
        && let Some(t) = h.target
    {
        let (a, b) = (f.pos, t);
        let lit = Color::srgb(1.0, 0.85, 0.2);
        gizmos.line(
            Vec3::new(a.x, height(a), a.y),
            Vec3::new(b.x, height(b), b.y),
            lit,
        );
        gizmos.circle(
            Isometry3d::new(
                Vec3::new(b.x, height(b), b.y),
                Quat::from_rotation_arc(Vec3::Z, Vec3::Y),
            ),
            3.0,
            lit,
        );
    }
}

/// Outlines (crossed) the footprints of `cells`.
fn draw_cells(view: &SimView, cells: &[ZoneCellRef], color: Color, gizmos: &mut Gizmos) {
    let height = |p: Vec2| view.terrain.as_deref().map_or(0.0, |t| t.height(p)) + 0.6;
    for c in cells {
        let Some(o) = view.zones.cell_footprint(*c) else {
            continue;
        };
        let k = o.corners().map(|q| Vec3::new(q.x, height(q), q.y));
        gizmos.linestrip([k[0], k[1], k[2], k[3], k[0]], color);
        gizmos.line(k[0], k[2], color);
        gizmos.line(k[1], k[3], color);
    }
}

/// With no tool active, a click selects what is under the cursor ([`crate::pick`]), a click on
/// nothing (or Esc) deselects. Choosing a tool deselects.
#[allow(clippy::too_many_arguments)]
fn tool_pick(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    capture: Res<UiCapture>,
    state: Res<ToolState>,
    view: Option<Res<SimView>>,
    fraction: Option<Res<cp_render::TickFraction>>,
    camera: Query<&GlobalTransform, With<RtsCamera>>,
    overlay: Res<cp_render::OverlayState>,
    mut picked: ResMut<crate::Picked>,
) {
    if state.active != ActiveTool::None {
        if picked.0.is_some() {
            picked.0 = None;
        }
        return;
    }
    if keys.just_pressed(KeyCode::Escape) && picked.0.is_some() {
        picked.0 = None;
    }
    if !buttons.just_pressed(MouseButton::Left) || capture.pointer_blocked() {
        return;
    }
    let (Some(view), Some(p), Ok(cam)) = (view, state.hover, camera.single()) else {
        return;
    };
    let ground = view.terrain.as_deref().map_or(0.0, |t| t.height(p));
    let height = cam.translation().y - ground;
    let alpha = fraction.map_or(1.0, |f| f.0);
    let lines = overlay.active == cp_render::Overlay::Transit;
    picked.0 = crate::pick_with(&view, alpha, p, height, lines);
}

/// Outlines the selection: a building's lot, a citizen's home and workplace (or the pedestrian
/// walking), a vehicle with the rest of its route, a segment along its length.
fn draw_selection(
    picked: Res<crate::Picked>,
    inspected: Res<crate::Inspected>,
    view: Option<Res<SimView>>,
    fraction: Option<Res<cp_render::TickFraction>>,
    mut gizmos: Gizmos,
) {
    use cp_render::agent::{pedestrian_positions, vehicle_positions};
    use cp_sim::{Inspection, Selection};
    let (Some(view), Some(sel), Some(info)) = (view.as_deref(), picked.0, inspected.0.as_ref())
    else {
        return;
    };
    let alpha = fraction.map_or(1.0, |f| f.0);
    let ground = |p: Vec2| view.terrain.as_deref().map_or(0.0, |t| t.height(p));
    let cyan = Color::srgb(0.2, 0.85, 1.0);
    let ring = |gizmos: &mut Gizmos, p: Vec2, r: f32| {
        gizmos.circle(
            Isometry3d::new(
                Vec3::new(p.x, ground(p) + 1.0, p.y),
                Quat::from_rotation_arc(Vec3::Z, Vec3::Y),
            ),
            r,
            cyan,
        );
    };
    let lot = |id: cp_building::BuildingId| -> Vec<ZoneCellRef> {
        view.buildings
            .get(id)
            .map(|b| cp_building::held_cells(&view.zones, b))
            .unwrap_or_default()
    };
    match info {
        Inspection::Building(b) => draw_cells(view, &b.footprint, cyan, &mut gizmos),
        Inspection::Citizen(c) => {
            if let Some((h, _)) = &c.home {
                draw_cells(view, &lot(*h), Color::srgb(0.3, 0.9, 0.4), &mut gizmos);
            }
            if let Some((w, _, _)) = &c.work {
                draw_cells(view, &lot(*w), Color::srgb(1.0, 0.6, 0.1), &mut gizmos);
            }
            if let Selection::Pedestrian(key) = sel
                && let Some((_, pos)) = pedestrian_positions(
                    &view.agents.prev.pedestrians,
                    &view.agents.cur.pedestrians,
                    alpha,
                )
                .into_iter()
                .find(|(k, _)| *k == key)
            {
                ring(&mut gizmos, pos, 2.0);
            }
        }
        Inspection::Vehicle(v) => {
            for lane in &v.route {
                gizmos.linestrip(lane.iter().map(|p| *p + Vec3::Y * 0.9), cyan);
            }
            if let Some((_, pos)) =
                vehicle_positions(&view.agents.prev.vehicles, &view.agents.cur.vehicles, alpha)
                    .into_iter()
                    .find(|(k, _)| *k == v.key)
            {
                ring(&mut gizmos, Vec2::new(pos.x, pos.z), 4.0);
            }
            if let Some(t) = &v.trip {
                draw_cells(view, &lot(t.to), Color::srgb(1.0, 0.6, 0.1), &mut gizmos);
            }
        }
        Inspection::Segment(s) => {
            gizmos.linestrip(
                s.line.iter().map(|p| *p + Vec3::Y * 0.9),
                Color::srgb(1.0, 0.85, 0.2),
            );
        }
        Inspection::Line(l) => {
            if let Some(route) = view.transit.routes.get(&l.id) {
                gizmos.linestrip(
                    route
                        .points
                        .iter()
                        .map(|p| Vec3::new(p.x, ground(*p) + 1.4, p.y)),
                    cyan,
                );
            }
        }
    }
}
