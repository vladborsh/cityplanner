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
/// Frames between attempts after a blank capture.
const RETRY_FRAMES: u32 = 20;

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
    attempts: u32,
    blanks: u32,
    result: Option<Result<(), (u8, String)>>,
}

impl Capture {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            state: State::Idle,
            armed_at: None,
            attempts: 0,
            blanks: 0,
            result: None,
        }
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
            commands
                .spawn(Screenshot::primary_window())
                .observe(on_captured);
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
    fn empty_image_data_counts_as_blank() {
        let mut img = rgba(4, 4, [1, 2, 3, 4]);
        img.data = None;
        assert!(image_is_blank(&img));
    }
}
