//! RTS orbit camera. See `architecture/system-design.md` §13.2.
//!
//! Controls: pan with WASD, screen edge or middle-button drag; rotate with Q/E or
//! right-button drag (horizontal = yaw, vertical = tilt); mouse wheel zooms towards the
//! point under the cursor. Input moves a *target* rig; the shown rig eases to it, and
//! panning/rotation keep their momentum (inertia). All limits live in [`math`].
//! Reads only [`SimView`] (terrain, owned tiles).

pub mod math;

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use cp_sim::SimView;

pub use math::Rig;
use math::{owned_bounds, ray_terrain_hit, zoom_to_cursor};

/// Sensitivities and smoothing. Insert your own before the plugin to override.
#[derive(Resource, Clone, Debug)]
pub struct CameraSettings {
    /// Pan speed in orbit distances per second (so panning scales with zoom).
    pub pan_speed: f32,
    /// Pan with the mouse at the screen edge.
    pub edge_pan: bool,
    /// Edge band width in pixels.
    pub edge_margin: f32,
    /// Rotation speed of Q/E, rad/s.
    pub key_rotate_speed: f32,
    /// Right-drag yaw/tilt, rad per pixel.
    pub drag_rotate_sens: f32,
    /// Middle-drag pan multiplier (1 = the ground follows the cursor).
    pub drag_pan_sens: f32,
    /// Distance factor per wheel notch (< 1; smaller = faster zoom).
    pub zoom_step: f32,
    /// How fast velocities follow key input, 1/s.
    pub acceleration: f32,
    /// Momentum decay once input stops, 1/s (higher = stops sooner).
    pub inertia_damping: f32,
    /// How fast the shown camera eases to the target, 1/s.
    pub smoothing: f32,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            pan_speed: 0.9,
            edge_pan: true,
            edge_margin: 6.0,
            key_rotate_speed: 1.4,
            drag_rotate_sens: 0.005,
            drag_pan_sens: 1.0,
            zoom_step: 0.85,
            acceleration: 10.0,
            inertia_damping: 6.0,
            smoothing: 14.0,
        }
    }
}

/// Camera state on the camera entity.
#[derive(Component, Clone, Debug)]
pub struct RtsCamera {
    /// Where input wants the camera.
    pub target: Rig,
    /// What is shown (eases towards `target`).
    pub current: Rig,
    pan_vel: Vec2,
    yaw_vel: f32,
}

impl RtsCamera {
    pub fn new(rig: Rig) -> Self {
        Self {
            target: rig,
            current: rig,
            pan_vel: Vec2::ZERO,
            yaw_vel: 0.0,
        }
    }

    /// Camera that sees `look` from `pos` (before limits; they apply on the first frame).
    pub fn from_pose(pos: Vec3, look: Vec3) -> Self {
        Self::new(Rig::from_pose(pos, look))
    }
}

pub struct RtsCameraPlugin;

impl Plugin for RtsCameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraSettings>()
            .add_systems(Update, rts_camera);
    }
}

fn ease(rate: f32, dt: f32) -> f32 {
    1.0 - (-rate * dt).exp()
}

/// Ground-plane directions (forward, right) for a yaw.
fn ground_axes(yaw: f32) -> (Vec2, Vec2) {
    (
        Vec2::new(-yaw.sin(), -yaw.cos()),
        Vec2::new(yaw.cos(), -yaw.sin()),
    )
}

#[allow(clippy::too_many_arguments)]
fn rts_camera(
    time: Res<Time<Real>>,
    settings: Res<CameraSettings>,
    view: Option<Res<SimView>>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut cams: Query<(&Camera, &GlobalTransform, &mut Transform, &mut RtsCamera)>,
) {
    let dt = time.delta_secs().min(0.1);
    let terrain = view.as_ref().and_then(|v| v.terrain.as_deref());
    let bounds = owned_bounds(&view.as_ref().map(|v| v.tiles).unwrap_or_default());
    let window = window.single().ok();
    let cursor = window.and_then(|w| w.cursor_position());
    let win_size = window.map_or(Vec2::new(1600.0, 900.0), |w| w.size());

    for (camera, global, mut tf, mut rts) in &mut cams {
        let rts = &mut *rts;
        let (forward, right) = ground_axes(rts.target.yaw);

        // --- pan ---
        let mut dir = Vec2::ZERO;
        for (key, v) in [
            (KeyCode::KeyW, forward),
            (KeyCode::KeyS, -forward),
            (KeyCode::KeyD, right),
            (KeyCode::KeyA, -right),
        ] {
            if keys.pressed(key) {
                dir += v;
            }
        }
        if settings.edge_pan
            && let Some(c) = cursor
            && window.is_some_and(|w| w.focused)
        {
            let m = settings.edge_margin;
            if c.x <= m {
                dir -= right;
            } else if c.x >= win_size.x - m {
                dir += right;
            }
            if c.y <= m {
                dir += forward;
            } else if c.y >= win_size.y - m {
                dir -= forward;
            }
        }
        let panning_by_drag = buttons.pressed(MouseButton::Middle);
        if panning_by_drag {
            // The ground follows the cursor; vertical pixels are foreshortened by tilt.
            let fov_y = camera_fov(camera).unwrap_or(std::f32::consts::FRAC_PI_4);
            let per_px = 2.0 * rts.target.distance * (fov_y * 0.5).tan() / win_size.y
                * settings.drag_pan_sens;
            let d = motion.delta;
            let step = -right * d.x * per_px + forward * d.y * per_px / rts.target.pitch.sin();
            rts.target.focus += step;
            if dt > 0.0 {
                rts.pan_vel = rts.pan_vel.lerp(step / dt, ease(30.0, dt));
            }
        } else if dir != Vec2::ZERO {
            let desired = dir.normalize() * rts.target.distance * settings.pan_speed;
            rts.pan_vel = rts.pan_vel.lerp(desired, ease(settings.acceleration, dt));
        } else {
            rts.pan_vel *= (-settings.inertia_damping * dt).exp();
        }
        if !panning_by_drag {
            rts.target.focus += rts.pan_vel * dt;
        }

        // --- rotate / tilt ---
        let key_dir = keys.pressed(KeyCode::KeyQ) as i32 - keys.pressed(KeyCode::KeyE) as i32;
        if buttons.pressed(MouseButton::Right) {
            let d = motion.delta;
            rts.target.yaw -= d.x * settings.drag_rotate_sens;
            rts.target.pitch -= d.y * settings.drag_rotate_sens;
            rts.yaw_vel = 0.0;
        } else if key_dir != 0 {
            let desired = key_dir as f32 * settings.key_rotate_speed;
            rts.yaw_vel += (desired - rts.yaw_vel) * ease(settings.acceleration, dt);
            rts.target.yaw += rts.yaw_vel * dt;
        } else {
            rts.yaw_vel *= (-settings.inertia_damping * dt).exp();
            rts.target.yaw += rts.yaw_vel * dt;
        }

        // --- zoom to cursor ---
        let notches = match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y,
            MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
        };
        if notches != 0.0 {
            let factor = settings.zoom_step.powf(notches);
            let focus3 = rts.target.focus_point(terrain);
            let anchor = cursor
                .and_then(|c| camera.viewport_to_world(global, c).ok())
                .and_then(|ray| ray_terrain_hit(terrain?, ray.origin, *ray.direction, 60_000.0))
                .unwrap_or(focus3);
            let (focus, distance) = zoom_to_cursor(focus3, rts.target.distance, anchor, factor);
            rts.target.focus = Vec2::new(focus.x, focus.z);
            rts.target.distance = distance;
        }

        rts.target.constrain(terrain, &bounds);

        // --- ease the shown rig to the target, then place the camera ---
        let k = ease(settings.smoothing, dt);
        let (t, c) = (rts.target, &mut rts.current);
        c.focus = c.focus.lerp(t.focus, k);
        c.yaw += (t.yaw - c.yaw) * k;
        c.pitch += (t.pitch - c.pitch) * k;
        c.distance += (t.distance - c.distance) * k;
        c.constrain(terrain, &bounds);
        let pos = c.camera_position(terrain);
        *tf = Transform::from_translation(pos).looking_at(c.focus_point(terrain), Vec3::Y);
    }
}

fn camera_fov(camera: &Camera) -> Option<f32> {
    // The clip-from-view matrix gives the vertical FOV for a perspective projection.
    let m = camera.clip_from_view();
    let f = m.y_axis.y;
    (f.is_finite() && f > 0.0).then(|| 2.0 * (1.0 / f).atan())
}
