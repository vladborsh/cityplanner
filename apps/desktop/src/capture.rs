//! Reliable `--screenshot`: never saves, or reports success for, a blank frame.
//!
//! On a loaded machine the first captures after startup were sometimes all black (the swap
//! chain had not presented real content yet) while the process still exited 0. [`Capture`]
//! checks every captured image, ignores uniform ones, asks again, and gives up with a
//! non-zero exit code and a message if no real frame arrives within [`TIMEOUT`].
//!
//! Usage: add [`CapturePlugin`], insert a [`Capture`] for the output path, and call
//! [`Capture::arm`] once the scene is ready. The plugin then drives the capture and exits
//! the app (0 on success, [`EXIT_BLANK`] on failure).

use std::path::PathBuf;
use std::time::Duration;

use bevy::image::TextureFormatPixelInfo;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

/// Exit code when no non-blank frame could be captured in time.
pub const EXIT_BLANK: u8 = 3;
/// Exit code when the file could not be written.
pub const EXIT_WRITE: u8 = 4;
/// How long to keep trying once armed.
pub const TIMEOUT: Duration = Duration::from_secs(20);
/// Exit code of the watchdog (same as a blank result, 3): the whole screenshot job did not finish in time, e.g. because
/// the window stopped producing frames (so no per-frame check could fire).
pub const EXIT_HUNG: i32 = EXIT_BLANK as i32;
/// Wall-clock limit for the whole job, from creating the [`Capture`] (startup, map load,
/// warm-up and capture), on top of any benchmark time.
pub const JOB_DEADLINE: Duration = Duration::from_secs(90);
/// Frames between attempts after a blank capture.
const RETRY_FRAMES: u32 = 20;

/// Whether the run may open an audio device: not a `--hidden` one (T-189).
pub fn audio_wanted(hidden: bool) -> bool {
    !hidden
}

/// `--hidden`: create the window invisible (agents and scripts must not pop windows up on
/// the user's screen). Rendering, screenshots and benchmarks work the same.
pub fn hidden_requested(args: &[String]) -> bool {
    args.iter().any(|a| a == "--hidden")
}

/// Size of the off-screen render target used by `--hidden`.
pub const HIDDEN_SIZE: (u32, u32) = (1600, 900);

/// `WindowPlugin` for a windowed run, or for `--hidden`: **no window at all**.
///
/// macOS renders nothing for a window that is invisible, unfocused or off-screen (every
/// capture was blank; tried `visible: false` and an off-screen borderless window at the
/// bottom of the stack). So `--hidden` creates no window: cameras render into an [`Image`]
/// ([`offscreen_target`]) which is captured directly. The egui UI needs a window and is
/// left out of hidden runs.
pub fn window_plugin(window: Window, hidden: bool) -> WindowPlugin {
    if hidden {
        WindowPlugin {
            primary_window: None,
            exit_condition: bevy::window::ExitCondition::DontExit,
            close_when_requested: false,
            ..default()
        }
    } else {
        WindowPlugin {
            primary_window: Some(window),
            ..default()
        }
    }
}

/// The off-screen colour target of a hidden run.
pub fn offscreen_target(images: &mut Assets<Image>) -> Handle<Image> {
    use bevy::render::render_resource::TextureFormat;
    images.add(Image::new_target_texture(
        HIDDEN_SIZE.0,
        HIDDEN_SIZE.1,
        TextureFormat::Rgba8UnormSrgb,
        None,
    ))
}

/// `true` if every pixel is identical (a black, white or otherwise flat frame). Real frames
/// always contain terrain, roads or UI and so are never uniform.
pub fn is_uniform(data: &[u8], bytes_per_pixel: usize) -> bool {
    let bpp = bytes_per_pixel.max(1);
    let Some(first) = data.get(..bpp) else {
        return true; // no data at all
    };
    data.chunks_exact(bpp).all(|px| px == first)
}

/// [`is_uniform`] for a captured [`Image`].
pub fn image_is_blank(image: &Image) -> bool {
    let Some(data) = image.data.as_ref() else {
        return true;
    };
    is_uniform(
        data,
        image.texture_descriptor.format.pixel_size().unwrap_or(4),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    /// Armed: request a capture on the next frame.
    Armed,
    /// A capture is in flight.
    Pending,
    /// Blank result: wait this many frames, then ask again.
    Backoff(u32),
}

/// One screenshot job.
#[derive(Resource)]
pub struct Capture {
    path: PathBuf,
    state: State,
    /// Real time at which the job was armed.
    armed_at: Option<Duration>,
    /// Off-screen target to capture instead of the primary window (`--hidden`).
    target: Option<Handle<Image>>,
    attempts: u32,
    blanks: u32,
    result: Option<Result<(), (u8, String)>>,
}

/// Runs `on_expire` on a separate thread if `deadline` passes. The thread is detached; when
/// the process finishes first it simply dies with it.
pub fn spawn_watchdog(deadline: Duration, on_expire: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        std::thread::sleep(deadline);
        on_expire();
    });
}

impl Capture {
    /// A screenshot job with a process-level watchdog: if the job has not ended after
    /// [`JOB_DEADLINE`] (plus `extra`, e.g. the benchmark length) the process exits with
    /// [`EXIT_HUNG`] and a message. This works even when no frames are produced any more.
    pub fn new(path: PathBuf, extra: Duration) -> Self {
        let shown = path.clone();
        // `CP_CAPTURE_DEADLINE_SECS` shortens the limit (used to prove the watchdog).
        let base = std::env::var("CP_CAPTURE_DEADLINE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .map_or(JOB_DEADLINE, Duration::from_secs);
        let limit = base + extra;
        spawn_watchdog(limit, move || {
            eprintln!(
                "screenshot failed: job still running after {} s (window not producing frames?); not writing {shown:?}",
                limit.as_secs()
            );
            std::process::exit(EXIT_HUNG);
        });
        Self::without_watchdog(path)
    }

    fn without_watchdog(path: PathBuf) -> Self {
        Self {
            path,
            state: State::Idle,
            armed_at: None,
            target: None,
            attempts: 0,
            blanks: 0,
            result: None,
        }
    }

    /// Capture this image instead of the primary window.
    pub fn with_target(mut self, target: Handle<Image>) -> Self {
        self.target = Some(target);
        self
    }

    /// Starts capturing (idempotent).
    pub fn arm(&mut self) {
        if self.state == State::Idle {
            self.state = State::Armed;
        }
    }

    pub fn is_active(&self) -> bool {
        self.state != State::Idle
    }
}

pub struct CapturePlugin;

impl Plugin for CapturePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Last, drive);
    }
}

fn on_captured(captured: On<ScreenshotCaptured>, mut cap: ResMut<Capture>) {
    if image_is_blank(&captured.image) {
        cap.blanks += 1;
        cap.state = State::Backoff(RETRY_FRAMES);
        return;
    }
    let saved = captured
        .image
        .clone()
        .try_into_dynamic()
        .map_err(|e| e.to_string())
        .and_then(|img| img.save(&cap.path).map_err(|e| e.to_string()));
    cap.result = Some(match saved {
        Ok(()) => Ok(()),
        Err(e) => Err((EXIT_WRITE, format!("cannot write {:?}: {e}", cap.path))),
    });
}

fn drive(
    mut commands: Commands,
    cap: Option<ResMut<Capture>>,
    time: Res<Time<Real>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(mut cap) = cap else { return };
    if let Some(result) = cap.result.take() {
        match result {
            Ok(()) => {
                info!(
                    "screenshot saved to {:?} (attempt {}, {} blank frame(s) skipped)",
                    cap.path, cap.attempts, cap.blanks
                );
                exit.write(AppExit::Success);
            }
            Err((code, msg)) => {
                eprintln!("screenshot failed: {msg}");
                exit.write(AppExit::from_code(code));
            }
        }
        cap.state = State::Idle;
        return;
    }
    if cap.state == State::Idle {
        return;
    }
    let armed_at = *cap.armed_at.get_or_insert(time.elapsed());
    if time.elapsed().saturating_sub(armed_at) > TIMEOUT {
        eprintln!(
            "screenshot failed: no non-blank frame in {} s ({} attempts, {} blank); not writing {:?}",
            TIMEOUT.as_secs(),
            cap.attempts,
            cap.blanks,
            cap.path
        );
        exit.write(AppExit::from_code(EXIT_BLANK));
        cap.state = State::Idle;
        return;
    }
    match cap.state {
        State::Armed => {
            cap.attempts += 1;
            cap.state = State::Pending;
            let shot = match &cap.target {
                Some(image) => Screenshot::image(image.clone()),
                None => Screenshot::primary_window(),
            };
            commands.spawn(shot).observe(on_captured);
        }
        State::Backoff(0) => cap.state = State::Armed,
        State::Backoff(n) => cap.state = State::Backoff(n - 1),
        State::Idle | State::Pending => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    fn rgba(w: u32, h: u32, px: [u8; 4]) -> Image {
        Image::new_fill(
            Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &px,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        )
    }

    #[test]
    fn all_black_and_flat_frames_are_blank() {
        assert!(image_is_blank(&rgba(64, 36, [0, 0, 0, 255])));
        assert!(image_is_blank(&rgba(64, 36, [0, 0, 0, 0])));
        assert!(image_is_blank(&rgba(8, 8, [120, 130, 140, 255])));
        assert!(is_uniform(&[], 4));
    }

    #[test]
    fn a_single_different_pixel_makes_it_a_real_frame() {
        let mut img = rgba(64, 36, [0, 0, 0, 255]);
        let n = img.data.as_ref().unwrap().len();
        img.data.as_mut().unwrap()[n - 4] = 9;
        assert!(!image_is_blank(&img));
    }

    #[test]
    fn pixel_size_is_respected() {
        // Two 3-byte pixels that differ: not uniform; equal: uniform.
        assert!(!is_uniform(&[1, 2, 3, 1, 2, 4], 3));
        assert!(is_uniform(&[1, 2, 3, 1, 2, 3], 3));
        // Same bytes read as 1-byte pixels would differ: the stride matters.
        assert!(!is_uniform(&[1, 2, 3, 1, 2, 3], 1));
    }

    #[test]
    fn hidden_flag_means_no_window() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(hidden_requested(&args(&["cp", "--hidden", "--bench", "3"])));
        // A hidden run opens no audio device; a normal one does (T-189).
        assert!(!audio_wanted(true) && audio_wanted(false));
        assert!(!hidden_requested(&args(&["cp", "--bench", "3"])));
        let w = Window::default();
        assert!(window_plugin(w.clone(), true).primary_window.is_none());
        assert!(window_plugin(w, false).primary_window.is_some());
    }

    #[test]
    fn watchdog_fires_without_any_frames() {
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_watchdog(Duration::from_millis(30), move || {
            tx.send(()).unwrap();
        });
        // Nothing else is running (no app, no frames): it still fires.
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn empty_image_data_counts_as_blank() {
        let mut img = rgba(4, 4, [1, 2, 3, 4]);
        img.data = None;
        assert!(image_is_blank(&img));
    }
}
