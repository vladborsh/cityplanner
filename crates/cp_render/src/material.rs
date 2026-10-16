//! A lit material that takes its colour from vertex colours and binds no textures.
//!
//! Used for terrain, water and (later) other flat-shaded city geometry. See
//! `vertex_color.wesl` for why it exists.

use bevy::asset::embedded_asset;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

/// Pipeline specialisation key of [`VertexColorMaterial`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VertexColorKey {
    depth_bias: bool,
}

impl From<&VertexColorMaterial> for VertexColorKey {
    fn from(m: &VertexColorMaterial) -> Self {
        Self {
            depth_bias: m.depth_bias,
        }
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(VertexColorKey)]
pub struct VertexColorMaterial {
    /// x = perceptual roughness, y = reflectance; z, w = roughness and reflectance of the
    /// dark (glossy) surfaces such as asphalt, 0 = none (see `vertex_color.wesl`).
    #[uniform(0)]
    pub params: Vec4,
    /// Alpha-blend using the vertex alpha (water) instead of drawing opaque.
    pub blend: bool,
    /// Pull the surface towards the camera in the depth test so a layer lying a few
    /// centimetres above the terrain (overlays) is not cut by it.
    pub depth_bias: bool,
}

impl VertexColorMaterial {
    pub fn opaque(roughness: f32, reflectance: f32) -> Self {
        Self {
            params: Vec4::new(roughness, reflectance, 0.0, 0.0),
            blend: false,
            depth_bias: false,
        }
    }

    /// Dark surfaces of the mesh (vertex colour darker than asphalt grey) get `roughness` and
    /// `reflectance` instead, so a sun glint shows on them and not on kerbs, paint or
    /// pavements (T-178).
    pub fn with_dark_gloss(mut self, roughness: f32, reflectance: f32) -> Self {
        self.params.z = roughness;
        self.params.w = reflectance;
        self
    }

    pub fn with_depth_bias(self, depth_bias: bool) -> Self {
        Self { depth_bias, ..self }
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

    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        if key.bind_group_data.depth_bias
            && let Some(ds) = descriptor.depth_stencil.as_mut()
        {
            // Bevy's depth is reversed (1 = near): positive bias moves towards the camera.
            ds.bias.constant = 8;
            ds.bias.slope_scale = 2.0;
        }
        Ok(())
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
