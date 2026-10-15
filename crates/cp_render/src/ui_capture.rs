//! Which input the UI owns this frame. Lives in `cp_render` (the lowest crate that reads
//! input: the camera) so `cp_tools` and `cp_ui` both reach it without a new dependency
//! edge; `cp_ui` writes it, the camera and the tools read it.

use bevy::prelude::*;
use bevy::window::WindowFocused;

/// Frames after the window gains focus in which clicks are not the game's: the click that
/// only activates the window (this frame and the next, as the focus and the button messages
/// can arrive one frame apart).
const FOCUS_GRACE_FRAMES: u8 = 2;

/// Set by the UI at the start of every frame (`cp_ui`), read by the camera and the tools.
#[derive(Resource, Default, Debug)]
pub struct UiCapture {
    /// The pointer is over a panel or window, or egui is using it (a drag, a slider).
    pub pointer_over_ui: bool,
    /// egui wants the keyboard (a text field has focus): no camera or tool hotkeys.
    pub keyboard_captured: bool,
    /// Frames left in which a click is only the window's activation (see [`track_window_focus`]).
    pub focus_grace: u8,
}

impl UiCapture {
    /// Mouse clicks must not reach the map: over the UI, or the window was just activated.
    pub fn pointer_blocked(&self) -> bool {
        self.pointer_over_ui || self.focus_grace > 0
    }
}

/// Starts the grace period when the window gains focus and counts it down otherwise.
pub fn track_window_focus(mut focus: MessageReader<WindowFocused>, mut capture: ResMut<UiCapture>) {
    let gained = focus.read().filter(|e| e.focused).count() > 0;
    capture.focus_grace = if gained {
        FOCUS_GRACE_FRAMES
    } else {
        capture.focus_grace.saturating_sub(1)
    };
}
