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

pub mod area;
pub mod band;
pub mod building;
pub mod bulldozer;
mod district;
pub mod flip;
mod outbox;
pub mod pick;
pub mod plugin;
pub mod road;
pub mod transit;
pub mod upgrade;
pub mod utility;
pub mod zone;
pub mod zone_lines;
pub mod zone_preview;

pub use area::AreaTool;
pub use building::{
    ALIGN_REACH_M, BuildingTool, ConnectionHint, FREE_ROT_STEP, FreeGhost, PlacementGhost,
    connection_hint, error_text, pick_building,
};
pub use bulldozer::{
    Span, demolition, pick_segment, span_commands, spans_length, spans_outside_owned,
};
pub use cp_render::UiCapture;
pub use district::{DistrictTool, DistrictToolKind};
pub use flip::{FlipPreview, NOT_ONE_WAY, flip_commands, flip_preview};
pub use outbox::CommandOutbox;
pub use pick::{Inspected, Picked, pick, pick_line, pick_with};
pub use plugin::{
    ActiveTool, Ghost, GhostDemo, ToolState, ToolsPlugin, compute_ghost, compute_ghost_alt,
    compute_utility_ghost, compute_utility_ghost_alt, tool_click, tool_hotkeys,
};
pub use road::{
    CHEVRON_SPACING, ELEVATION_STEP, MAX_ELEVATION_LEVEL, MarkerKind, ROAD_TUNNEL_DEPTH,
    RoadMarker, RoadMode, RoadTool, TOO_CLOSE, chevron_lines, format_money, preview,
    preview_report, quantized, road_is_one_way, road_markers, snap_label, too_close,
};
pub use transit::{
    Click, LineGhostInfo, LineTool, ROAD_PICK_RADIUS, STOP_PICK_RADIUS, Target, line_ghost,
    route_summary,
};
pub use upgrade::{UpgradePreview, upgrade_commands, upgrade_preview};
pub use utility::{
    LINK_PICK_RADIUS, UtilityMode, UtilityTool, pick_link, snap_cursor, snapped_cursor,
};
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
