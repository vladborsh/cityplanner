//! The game's threads at the user-interactive quality of service on macOS (T-281): with other
//! work on the machine (a build) the scheduler gives them the cores first, so the frames stay
//! on time. Default-QoS work beside the game cost it 120 -> 76 fps (16 busy processes); the
//! same work below the game's class leaves it at 117. Other platforms: nothing.
//!
//! Every thread of the game: the main thread ([`set_current_thread`] at start), Bevy's task
//! pools ([`task_pool_options`]: compute, async compute and IO, as each thread starts) and the
//! pipelined render thread (an exclusive system of the render app, which runs on it).

use bevy::app::{TaskPoolOptions, TaskPoolPlugin};
use bevy::prelude::*;

/// Gives the calling thread the user-interactive class, once per thread.
pub fn set_current_thread() {
    #[cfg(target_os = "macos")]
    {
        use std::cell::Cell;
        thread_local!(static DONE: Cell<bool> = const { Cell::new(false) });
        if DONE.with(|d| d.replace(true)) {
            return;
        }
        // <pthread/qos.h>: QOS_CLASS_USER_INTERACTIVE = 0x21.
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        // SAFETY: the call only changes the scheduling class of the calling thread.
        let _ = unsafe { pthread_set_qos_class_self_np(0x21, 0) };
    }
}

/// Bevy's task pools with [`set_current_thread`] on every thread they start.
pub fn task_pool_plugin() -> TaskPoolPlugin {
    let mut options = TaskPoolOptions::default();
    for pool in [
        &mut options.compute,
        &mut options.async_compute,
        &mut options.io,
    ] {
        pool.on_thread_spawn = Some(std::sync::Arc::new(set_current_thread));
    }
    TaskPoolPlugin {
        task_pool_options: options,
    }
}

/// The render thread: an exclusive system runs on the thread that runs the render schedule.
pub fn add_render_thread(app: &mut App) {
    if let Some(render) = app.get_sub_app_mut(bevy::render::RenderApp) {
        render.add_systems(bevy::render::Render, |_: &mut World| set_current_thread());
    }
}
