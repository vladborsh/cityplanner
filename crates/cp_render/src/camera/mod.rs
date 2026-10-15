//! RTS orbit camera. See `architecture/system-design.md` §13.2.
//!
//! Controls: pan with WASD, screen edge or middle-button drag; rotate with Q/E or
//! right-button drag (horizontal = yaw, vertical = tilt); R raises / F lowers the tilt
//! angle (towards top-down / towards the horizon); mouse wheel zooms towards the
//! point under the cursor. Input moves a *target* rig; the shown rig eases to it, and
//! panning/rotation keep their momentum (inertia). All limits live in [`math`].
//! Reads only [`SimView`] (terrain, owned tiles).
//!
//! Manual checklist (T-012, extended by T-073): WASD / edge / middle-drag pan; Q/E and
//! right-drag rotate; wheel zooms to the cursor; right-drag up/down tilts; **hold R: the
//! view tilts up towards top-down and stops at 89°; hold F: it tilts down and stops at
//! 15°; release: it eases to rest**; limits hold at the map bounds and terrain.

pub mod math;

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use cp_sim::SimView;

use crate::ui_capture::{UiCapture, track_window_focus};

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
    /// Tilt speed of R (raise) / F (lower), rad/s.
    pub key_tilt_speed: f32,
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
            key_tilt_speed: 1.0,
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
            .init_resource::<UiCapture>()
            .add_systems(PreUpdate, track_window_focus)
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
    capture: Res<UiCapture>,
    mut drag_from_map: Local<(bool, bool)>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut cams: Query<(&Camera, &GlobalTransform, &mut Transform, &mut RtsCamera)>,
) {
    let dt = time.delta_secs().min(0.1);
    let terrain = view.as_ref().and_then(|v| v.terrain.as_deref());
    let bounds = owned_bounds(&view.as_ref().map(|v| v.tiles).unwrap_or_default());
    let window = window.single().ok();
    let cursor = window.and_then(|w| w.cursor_position());
    let win_size = window.map_or(Vec2::new(1600.0, 900.0), |w| w.size());
    // The UI owns the pointer and the keyboard when it says so. A drag belongs to the map
    // only if it started there (sweeping over a panel mid-drag must not stall it).
    let over_ui = capture.pointer_over_ui;
    let typing = capture.keyboard_captured;
    let (drag_mid, drag_right) = &mut *drag_from_map;
    for (button, from_map) in [
        (MouseButton::Middle, &mut *drag_mid),
        (MouseButton::Right, &mut *drag_right),
    ] {
        if buttons.just_pressed(button) {
            *from_map = !over_ui;
        } else if !buttons.pressed(button) {
            *from_map = false;
        }
    }
    let (middle_drag, right_drag) = (
        *drag_mid && buttons.pressed(MouseButton::Middle),
        *drag_right && buttons.pressed(MouseButton::Right),
    );

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
            if !typing && keys.pressed(key) {
                dir += v;
            }
        }
        if settings.edge_pan
            && !over_ui
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
        let panning_by_drag = middle_drag;
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
        let key = |k: KeyCode| !typing && keys.pressed(k);
        let tilt_dir = key(KeyCode::KeyR) as i32 - key(KeyCode::KeyF) as i32;
        rts.target.pitch = math::key_tilt(rts.target.pitch, tilt_dir, settings.key_tilt_speed, dt);
        let key_dir = key(KeyCode::KeyQ) as i32 - key(KeyCode::KeyE) as i32;
        if right_drag {
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
            _ if over_ui => 0.0,
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

#[cfg(test)]
mod tests {
    use bevy::input::ButtonState;
    use bevy::input::InputPlugin;
    use bevy::input::mouse::{MouseButtonInput, MouseWheel};

    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, InputPlugin, RtsCameraPlugin))
            .add_message::<bevy::window::WindowFocused>()
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(50),
            ));
        let rig = Rig::from_pose(Vec3::new(500.0, 300.0, 500.0), Vec3::new(500.0, 0.0, 400.0));
        app.world_mut().spawn((
            Camera::default(),
            Transform::default(),
            GlobalTransform::default(),
            RtsCamera::new(rig),
        ));
        app.update();
        app
    }

    fn target(app: &mut App) -> Rig {
        app.world_mut()
            .query::<&RtsCamera>()
            .single(app.world())
            .unwrap()
            .target
    }

    fn wheel(app: &mut App) {
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 3.0,
            phase: bevy::input::touch::TouchPhase::Moved,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    #[test]
    fn the_wheel_zooms_only_when_the_pointer_is_not_over_the_ui() {
        let mut app = app();
        let before = target(&mut app);
        app.world_mut().resource_mut::<UiCapture>().pointer_over_ui = true;
        wheel(&mut app);
        let over_ui = target(&mut app);
        assert_eq!(over_ui.distance, before.distance, "zoomed over a panel");
        assert_eq!(over_ui.focus, before.focus);

        app.world_mut().resource_mut::<UiCapture>().pointer_over_ui = false;
        wheel(&mut app);
        assert!(
            target(&mut app).distance < before.distance,
            "no zoom on the map"
        );
    }

    #[test]
    fn a_text_field_stops_the_camera_keys() {
        let mut app = app();
        let before = target(&mut app);
        app.world_mut()
            .resource_mut::<UiCapture>()
            .keyboard_captured = true;
        app.world_mut()
            .write_message(bevy::input::keyboard::KeyboardInput {
                key_code: KeyCode::KeyQ,
                logical_key: bevy::input::keyboard::Key::Character("q".into()),
                state: ButtonState::Pressed,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            });
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(target(&mut app).yaw, before.yaw, "rotated while typing");
    }

    #[test]
    fn a_drag_that_starts_over_the_ui_does_not_pan() {
        let mut app = app();
        let before = target(&mut app);
        app.world_mut().resource_mut::<UiCapture>().pointer_over_ui = true;
        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Middle,
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        app.world_mut().resource_mut::<UiCapture>().pointer_over_ui = false;
        app.world_mut()
            .write_message(bevy::input::mouse::MouseMotion {
                delta: Vec2::new(80.0, 0.0),
            });
        app.update();
        assert_eq!(target(&mut app).focus, before.focus);
    }
}
