//! `cp_render` — client rendering. See `architecture/system-design.md` §13.
//!
//! Reads only [`cp_sim::SimView`], never the sim `World`.

pub mod agent;
pub mod agent_render;
pub mod building;
pub mod building_render;
pub mod camera;
pub mod cargo_render;
pub mod chunk;
pub mod city_events;
pub mod demolish_ghost;
pub mod dust;
pub mod event_fx;
pub mod fire_fx;
pub mod fire_render;
pub mod furniture_render;
pub mod grading;
pub mod material;
pub mod mood;
pub mod overlay;
pub mod parked;
pub mod portal_render;
pub mod problem;
pub mod problem_render;
pub mod road;
mod road_render;
pub mod sun;
pub mod terrain;
pub mod thought;
pub mod thought_render;
pub mod train_render;
pub mod ui_capture;
pub mod underground;
pub mod utility_render;
pub mod warmup;
pub mod zone_ghost;

pub use agent_render::{AgentRenderPlugin, AgentRenderStats, TickFraction};
pub use building_render::{BuildingRenderPlugin, BuildingStats};
pub use camera::{CameraSettings, RtsCamera, RtsCameraPlugin};
pub use cargo_render::CargoRenderPlugin;
pub use chunk::terrain_mesh_height;
pub use demolish_ghost::{DemolishGhostPlugin, DemolishLayer};
pub use event_fx::{CityChange, DustDemo, EventFxPlugin, MoodDemo};
pub use fire_render::{FireFxPlugin, FireFxStats};
pub use furniture_render::{FurnitureRenderPlugin, FurnitureStats};
pub use grading::{GradingPlugin, GradingSettings};
pub use material::{VertexColorMaterial, VertexColorMaterialPlugin};
pub use overlay::{
    AreaFamily, AreasMode, CoverageCount, CoverageGhost, CoverageGhostSummary, CoverageSite,
    DistrictsMode, EmitGhost, EmitSite, LINE_STEP, LineGhost, Overlay, OverlayPlugin, OverlayState,
    ZoningMode, coverage_color, happiness_color, land_value_color, park_color, parking_color,
    resource_color, ribbon_at, traffic_color, zone_color,
};
pub use overlay::{
    BuildingStatus, NetworkStats, UtilityLayer, UtilityReport, network_color, utility_report,
};
pub use portal_render::PortalRenderPlugin;
pub use problem_render::{ProblemIconPlugin, ProblemIconStats};
pub use road_render::{RoadRenderPlugin, RoadStats};
pub use sun::LightingSettings;
pub use terrain::{Sun, TerrainChunk, TerrainRenderPlugin, TerrainStats};
pub use thought_render::{SelectedThought, ShownThoughts, ThoughtDemo, ThoughtPlugin};
pub use train_render::TrainRenderPlugin;
pub use ui_capture::UiCapture;
pub use underground::{UndergroundPlugin, UndergroundView};
pub use utility_render::{PipeGhost, PipeLayers, PowerGhost, UtilityRenderPlugin, ghost_graph};
pub use warmup::PipelineWarmUpPlugin;
pub use zone_ghost::{ZoneGhostPlugin, ZoneStripLayer};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_render";
