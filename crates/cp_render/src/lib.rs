//! `cp_render` — client rendering. See `architecture/system-design.md` §13.
//!
//! Reads only [`cp_sim::SimView`], never the sim `World`.

pub mod agent;
pub mod agent_render;
pub mod building;
pub mod building_render;
pub mod camera;
pub mod chunk;
pub mod fire_fx;
pub mod fire_render;
pub mod material;
pub mod overlay;
pub mod problem;
pub mod problem_render;
pub mod road;
pub mod road_render;
pub mod sun;
pub mod terrain;
pub mod ui_capture;
pub mod underground;
pub mod utility_render;
pub mod zone_ghost;

pub use agent_render::{AgentRenderPlugin, AgentRenderStats, TickFraction};
pub use building_render::{BuildingRenderPlugin, BuildingStats};
pub use camera::{CameraSettings, RtsCamera, RtsCameraPlugin};
pub use chunk::terrain_mesh_height;
pub use fire_render::{FireFxPlugin, FireFxStats};
pub use material::{VertexColorMaterial, VertexColorMaterialPlugin};
pub use overlay::{
    BuildingStatus, NetworkStats, UtilityLayer, UtilityReport, network_color, utility_report,
};
pub use overlay::{
    DistrictsMode, LINE_STEP, LineGhost, Overlay, OverlayPlugin, OverlayState, ZoningMode,
    ribbon_at,
};
pub use problem_render::{ProblemIconPlugin, ProblemIconStats};
pub use road_render::{RoadRenderPlugin, RoadStats};
pub use sun::LightingSettings;
pub use terrain::{Sun, TerrainChunk, TerrainRenderPlugin, TerrainStats};
pub use ui_capture::UiCapture;
pub use underground::{UndergroundPlugin, UndergroundView};
pub use utility_render::{PipeGhost, PipeLayers, PowerGhost, UtilityRenderPlugin, ghost_graph};
pub use zone_ghost::{ZoneGhostPlugin, ZoneStripLayer};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_render";
