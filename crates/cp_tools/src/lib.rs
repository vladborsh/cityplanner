//! `cp_tools` — client tools (`architecture/system-design.md` §13.5).
//!
//! A tool is state + a preview "ghost" + cost + validation errors, all computed from the same
//! pure functions the commands use (`cp_net::validate_road_path`), plus a click that pushes a
//! `SimCommand` into the [`CommandOutbox`]. Tools see only [`cp_sim::SimView`] and the outbox —
//! never the sim world.
//!
//! * [`road`] — the road tool: straight, curve (3 clicks) and free modes, road type,
//!   elevation, ghost.
//! * [`utility`] — the pipe and power-line tools (ghost from `plan_link_path`) and link picking.
//! * [`bulldozer`] — picks the segment under the cursor.
//! * [`plugin`] — the Bevy glue: cursor ray → terrain point, hotkeys, clicks, ghost drawing.

pub mod band;
pub mod building;
pub mod bulldozer;
mod outbox;
pub mod plugin;
pub mod road;
pub mod utility;
pub mod zone;

pub use building::{BuildingTool, PlacementGhost, pick_building};
pub use bulldozer::pick_segment;
pub use outbox::CommandOutbox;
pub use plugin::{ActiveTool, Ghost, ToolState, ToolsPlugin, UiCapture, compute_utility_ghost};
pub use road::{ELEVATION_STEP, MAX_ELEVATION_LEVEL, RoadMode, RoadTool, format_money, preview};
pub use utility::{LINK_PICK_RADIUS, UtilityMode, UtilityTool, pick_link};
pub use zone::{PALETTE, ZoneTool, ZoneToolKind, zone_color, zone_name};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_tools";

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_tools");
    }
}
