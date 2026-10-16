//! A lit material that takes its colour from vertex colours and binds no textures.
//!
//! Used for terrain, water and (later) other flat-shaded city geometry. See
//! `vertex_color.wesl` for why it exists.

use bevy::asset::embedded_asset;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

use crate::overlay::{Overlay, OverlayState};

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
    /// x = factor on the alpha of every fragment (the Underground view fades the ground, T-164);
    /// y = how far the surface is desaturated and darkened (0 = not, 1 = fully) while a data
    /// overlay is active (T-183b, written by [`sync_overlay_dim`]; overlay layers are never
    /// dimmed); z, w unused.
    #[uniform(1)]
    pub fade: Vec4,
    /// Alpha-blend using the vertex alpha (water) instead of drawing opaque.
    pub blend: bool,
    /// Pull the surface towards the camera in the depth test so a layer lying a few
    /// centimetres above the terrain (overlays) is not cut by it.
    pub depth_bias: bool,
    /// Added to the distance an alpha-blended draw is sorted by: layers with a larger value
    /// are drawn later, over the others (the tunnels over the see-through ground).
    pub sort_bias: f32,
}

impl VertexColorMaterial {
    pub fn opaque(roughness: f32, reflectance: f32) -> Self {
        Self {
            params: Vec4::new(roughness, reflectance, 0.0, 0.0),
            fade: Vec4::new(1.0, 0.0, 0.0, 0.0),
            blend: false,
            depth_bias: false,
            sort_bias: 0.0,
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

    /// Whether the surface is drawn see-through: alpha-blended with `alpha` on every fragment
    /// (`1.0` and `blend` off give the surface back as it was).
    pub fn set_see_through(&mut self, on: bool, alpha: f32) {
        self.blend = on;
        self.fade.x = if on { alpha } else { 1.0 };
    }

    pub fn with_sort_bias(self, sort_bias: f32) -> Self {
        Self { sort_bias, ..self }
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

    fn depth_bias(&self) -> f32 {
        self.sort_bias
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

/// Dim amount of the city while a data overlay is active.
pub const OVERLAY_DIM: f32 = 1.0;

/// Sets the dim uniform of every scene material from the active overlay: [`OVERLAY_DIM`] with
/// any overlay, 0 with none. Overlay layers (depth-biased) keep their colours.
pub fn sync_overlay_dim(
    state: Res<OverlayState>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
) {
    let target = if state.active == Overlay::Off {
        0.0
    } else {
        OVERLAY_DIM
    };
    let stale: Vec<_> = materials
        .iter()
        .filter(|(_, m)| !m.depth_bias && m.fade.y != target)
        .map(|(id, _)| id)
        .collect();
    for id in stale {
        if let Some(mut m) = materials.get_mut(id) {
            m.fade.y = target;
        }
    }
}

pub struct VertexColorMaterialPlugin;

impl Plugin for VertexColorMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "vertex_color.wesl");
        app.add_plugins(MaterialPlugin::<VertexColorMaterial>::default())
            .init_resource::<OverlayState>()
            .add_systems(Update, sync_overlay_dim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dim_uniform_follows_the_overlay_state_and_spares_overlay_layers() {
        let mut app = App::new();
        app.init_resource::<OverlayState>()
            .init_resource::<Assets<VertexColorMaterial>>()
            .add_systems(Update, sync_overlay_dim);
        let (scene, layer) = {
            let mut mats = app
                .world_mut()
                .resource_mut::<Assets<VertexColorMaterial>>();
            (
                mats.add(VertexColorMaterial::opaque(0.9, 0.05)),
                mats.add(VertexColorMaterial::blended(0.9, 0.05).with_depth_bias(true)),
            )
        };
        let dim = |app: &App, h: &Handle<VertexColorMaterial>| {
            app.world()
                .resource::<Assets<VertexColorMaterial>>()
                .get(h)
                .unwrap()
                .fade
                .y
        };
        app.update();
        assert_eq!(dim(&app, &scene), 0.0);
        app.world_mut().resource_mut::<OverlayState>().active = Overlay::Power;
        app.update();
        assert_eq!(dim(&app, &scene), OVERLAY_DIM);
        assert_eq!(dim(&app, &layer), 0.0);
        app.world_mut().resource_mut::<OverlayState>().active = Overlay::Off;
        app.update();
        assert_eq!(dim(&app, &scene), 0.0);
    }
}
