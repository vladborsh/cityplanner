//! `cp_render` — client rendering. See `architecture/system-design.md` §13.
//!
//! Reads only [`cp_sim::SimView`], never the sim `World`.

pub mod camera;
pub mod chunk;
pub mod material;
pub mod sun;
pub mod terrain;

pub use camera::{CameraSettings, RtsCamera, RtsCameraPlugin};
pub use material::{VertexColorMaterial, VertexColorMaterialPlugin};
pub use terrain::{Sun, TerrainChunk, TerrainRenderPlugin, TerrainStats};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_render";
