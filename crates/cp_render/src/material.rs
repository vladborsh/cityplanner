//! A lit material that takes its colour from vertex colours and binds no textures.
//!
//! Used for terrain, water and (later) other flat-shaded city geometry. See
//! `vertex_color.wesl` for why it exists.

use bevy::asset::embedded_asset;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct VertexColorMaterial {
    /// x = perceptual roughness, y = reflectance.
    #[uniform(0)]
    pub params: Vec4,
    /// Alpha-blend using the vertex alpha (water) instead of drawing opaque.
    pub blend: bool,
}

impl VertexColorMaterial {
    pub fn opaque(roughness: f32, reflectance: f32) -> Self {
        Self {
            params: Vec4::new(roughness, reflectance, 0.0, 0.0),
            blend: false,
        }
    }

    pub fn blended(roughness: f32, reflectance: f32) -> Self {
        Self {
            blend: true,
            ..Self::opaque(roughness, reflectance)
        }
    }
}

impl Material for VertexColorMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://cp_render/vertex_color.wesl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        if self.blend {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        }
    }

    // One draw per chunk is the budget on WebGL2; skip the extra depth/normal prepass.
    fn enable_prepass() -> bool {
        false
    }
}

pub struct VertexColorMaterialPlugin;

impl Plugin for VertexColorMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "vertex_color.wesl");
        app.add_plugins(MaterialPlugin::<VertexColorMaterial>::default());
    }
}
