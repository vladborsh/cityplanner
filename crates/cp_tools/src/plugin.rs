//! Bevy glue for the tools: cursor ray → terrain point, hotkeys, clicks, ghost drawing.
//!
//! All decisions live in the pure modules; this file only moves data between Bevy and them.
//! Keys: PgUp / PgDn raise / lower the road, Esc cancels the drawing (then deselects the tool),
//! Enter finishes a free-mode road. The left mouse button clicks (ignored while the pointer is
//! over the UI, see [`UiCapture`]).

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use cp_core::Cost;
use cp_net::{NetCommand, RoadBuildError, RoadPath, SegmentId};
use cp_render::RtsCamera;
use cp_render::camera::math::ray_terrain_hit;
use cp_sim::{SimCommand, SimView};

use crate::CommandOutbox;
use crate::bulldozer::{PICK_RADIUS, centre_line, pick_segment};
use crate::road::{RoadTool, preview, sample_curve};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActiveTool {
    #[default]
    None,
    Road,
    Bulldozer,
}

/// Set by the UI each frame: the pointer is over a panel/window, so clicks are not the tool's.
#[derive(Resource, Default, Debug)]
pub struct UiCapture {
    pub pointer_over_ui: bool,
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
}

#[derive(Resource, Default, Debug)]
pub struct ToolState {
    pub active: ActiveTool,
    pub road: RoadTool,
    pub ghost: Option<Ghost>,
    /// Terrain point under the cursor (XZ).
    pub hover: Option<Vec2>,
    /// Segment the bulldozer would remove.
    pub highlight: Option<SegmentId>,
    /// Inputs the current ghost was computed from; unchanged inputs skip the revalidation.
    ghost_key: Option<GhostKey>,
}

#[derive(Clone, PartialEq, Debug)]
struct GhostKey {
    hover: (i32, i32),
    road: RoadTool,
    net_ptr: usize,
}

impl ToolState {
    pub fn select(&mut self, tool: ActiveTool) {
        self.active = tool;
        self.road.cancel();
        self.ghost = None;
        self.highlight = None;
        self.ghost_key = None;
    }
}

pub struct ToolsPlugin;

impl Plugin for ToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ToolState>()
            .init_resource::<UiCapture>()
            .init_resource::<CommandOutbox>()
            .add_systems(
                Update,
                (tool_hotkeys, tool_hover, tool_click, draw_ghost).chain(),
            );
    }
}

fn tool_hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<ToolState>,
    mut outbox: ResMut<CommandOutbox>,
) {
    if state.active == ActiveTool::Road {
        if keys.just_pressed(KeyCode::PageUp) {
            state.road.raise();
        }
        if keys.just_pressed(KeyCode::PageDown) {
            state.road.lower();
        }
    }
    if keys.just_pressed(KeyCode::Escape) {
        if state.road.points.is_empty() {
            state.select(ActiveTool::None);
        } else {
            state.road.cancel();
        }
    }
    if state.active == ActiveTool::Road
        && keys.just_pressed(KeyCode::Enter)
        && let Some(req) = state.road.finish_free()
    {
        state.ghost_key = None;
        outbox.push(build_command(req));
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
    mut state: ResMut<ToolState>,
) {
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
            state.highlight = pick_segment(&view.net, p, PICK_RADIUS);
        }
        (ActiveTool::Road, Some(p)) => {
            let key = GhostKey {
                hover: (p.x.round() as i32, p.y.round() as i32),
                road: state.road.clone(),
                net_ptr: std::sync::Arc::as_ptr(&view.net) as usize,
            };
            if state.ghost_key.as_ref() != Some(&key) {
                state.ghost = compute_ghost(&view, &state.road, p);
                state.ghost_key = Some(key);
            }
        }
        _ => {
            state.highlight = None;
            if state.active != ActiveTool::Road {
                state.ghost = None;
            }
        }
    }
}

/// The ghost for the road tool with the cursor at `hover`, or `None` when nothing is drawn
/// yet (or there is no road type / terrain / content).
pub fn compute_ghost(view: &SimView, tool: &RoadTool, hover: Vec2) -> Option<Ghost> {
    let path = tool.path_with_hover(hover)?;
    let req = tool.request(path.clone())?;
    Some(match preview(view, &req)? {
        Ok(plan) => Ghost {
            polylines: plan
                .segments
                .iter()
                .map(|s| sample_curve(&s.curve, 3.0))
                .collect(),
            valid: true,
            cost: Some(plan.cost),
            errors: Vec::new(),
        },
        Err(errors) => Ghost {
            polylines: vec![raw_polyline(view, &path)],
            valid: false,
            cost: None,
            errors: errors.iter().map(RoadBuildError::to_string).collect(),
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

fn tool_click(
    buttons: Res<ButtonInput<MouseButton>>,
    capture: Res<UiCapture>,
    mut state: ResMut<ToolState>,
    mut outbox: ResMut<CommandOutbox>,
) {
    if !buttons.just_pressed(MouseButton::Left) || capture.pointer_over_ui {
        return;
    }
    let Some(p) = state.hover else {
        return;
    };
    match state.active {
        ActiveTool::Road => {
            if let Some(req) = state.road.click(p) {
                outbox.push(build_command(req));
            }
            state.ghost_key = None;
        }
        ActiveTool::Bulldozer => {
            if let Some(seg) = state.highlight {
                outbox.push(SimCommand::Net(NetCommand::RemoveSegment { seg }));
                state.highlight = None;
            }
        }
        ActiveTool::None => {}
    }
}

fn draw_ghost(state: Res<ToolState>, view: Option<Res<SimView>>, mut gizmos: Gizmos) {
    if let Some(ghost) = &state.ghost
        && state.active == ActiveTool::Road
    {
        let color = if ghost.valid {
            Color::srgb(0.2, 0.9, 0.3)
        } else {
            Color::srgb(0.95, 0.2, 0.2)
        };
        for line in &ghost.polylines {
            gizmos.linestrip(line.iter().map(|p| *p + Vec3::Y * 0.4), color);
        }
    }
    if let (Some(seg), Some(view)) = (state.highlight, view) {
        let line = centre_line(&view.net, seg, 3.0);
        gizmos.linestrip(
            line.iter().map(|p| *p + Vec3::Y * 0.6),
            Color::srgb(1.0, 0.6, 0.1),
        );
    }
}
