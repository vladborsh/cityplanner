//! `cp_render` — client rendering. See `architecture/system-design.md` §13.
//!
//! Reads only [`cp_sim::SimView`], never the sim `World`.

pub mod building;
pub mod building_render;
pub mod camera;
pub mod chunk;
pub mod material;
pub mod overlay;
pub mod road;
pub mod road_render;
pub mod sun;
pub mod terrain;

pub use building_render::{BuildingRenderPlugin, BuildingStats};
pub use camera::{CameraSettings, RtsCamera, RtsCameraPlugin};
pub use material::{VertexColorMaterial, VertexColorMaterialPlugin};
pub use overlay::{Overlay, OverlayPlugin, OverlayState};
pub use road_render::{RoadRenderPlugin, RoadStats};
pub use terrain::{Sun, TerrainChunk, TerrainRenderPlugin, TerrainStats};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_render";
