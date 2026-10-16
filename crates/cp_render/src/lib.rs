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
pub mod utility_render;

pub use agent_render::{AgentRenderPlugin, AgentRenderStats, TickFraction};
pub use building_render::{BuildingRenderPlugin, BuildingStats};
pub use camera::{CameraSettings, RtsCamera, RtsCameraPlugin};
pub use chunk::terrain_mesh_height;
pub use fire_render::{FireFxPlugin, FireFxStats};
pub use material::{VertexColorMaterial, VertexColorMaterialPlugin};
pub use overlay::{
    DistrictsMode, LINE_STEP, Overlay, OverlayPlugin, OverlayState, ZoningMode, ribbon_at,
};
pub use problem_render::{ProblemIconPlugin, ProblemIconStats};
pub use road_render::{RoadRenderPlugin, RoadStats};
pub use sun::LightingSettings;
pub use terrain::{Sun, TerrainChunk, TerrainRenderPlugin, TerrainStats};
pub use ui_capture::UiCapture;
pub use utility_render::{PipeGhost, PipeLayers, PowerGhost, UtilityRenderPlugin, ghost_graph};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_render";
