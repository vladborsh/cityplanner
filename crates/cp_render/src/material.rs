//! A lit material that takes its colour from vertex colours and binds no textures.
//!
//! Used for terrain, water and (later) other flat-shaded city geometry. See
//! `vertex_color.wesl` for why it exists.

use bevy::asset::{RenderAssetUsages, embedded_asset, uuid_handle};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, TextureDimension, TextureFormat};
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
    /// dimmed); z = procedural surface detail, see [`Self::with_detail`]; w = how much the
    /// shadow of a passing cloud darkens the surface (T-272, written by [`sync_cloud_shadows`]).
    #[uniform(1)]
    pub fade: Vec4,
    /// Lit windows (T-245): x = night factor (0 day .. 1 night), y = lateness of the night
    /// (`building::lights::lateness`), z = glow intensity, w = 1 when the vertex alpha holds
    /// window glow codes (`building::lights::glow_alpha`, `pattern_alpha`; the buildings), 0 otherwise.
    #[uniform(2)]
    pub glow: Vec4,
    /// The cloud shadows' noise (T-281b): the one shared [`CLOUD_NOISE`] texture.
    #[texture(3)]
    #[sampler(4)]
    pub cloud: Handle<Image>,
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
            // Cloud shadows on from the start (T-272): a material made with them needs no update
            // before its first frame; [`sync_cloud_shadows`] turns them off with the setting.
            fade: Vec4::new(1.0, 0.0, 0.0, CLOUD_SHADOW),
            glow: Vec4::ZERO,
            cloud: CLOUD_NOISE,
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

    /// Procedural surface detail in the shader (T-238): [`DETAIL_ROADS`] (asphalt grain,
    /// patches and cracks, sidewalk tiles) or [`DETAIL_GROUND`] (grass patches).
    pub fn with_detail(mut self, kind: f32) -> Self {
        self.fade.z = kind;
        self
    }

    /// The vertex alpha holds window glow codes, lit with `intensity` (see [`Self::glow`]).
    pub fn with_window_glow(mut self, intensity: f32) -> Self {
        self.glow = Vec4::new(0.0, 0.0, intensity, 1.0);
        self
    }

    pub fn with_sort_bias(self, sort_bias: f32) -> Self {
        Self { sort_bias, ..self }
    }

    /// An overlay layer (`depth_bias`) never gets a cloud shadow.
    pub fn with_depth_bias(mut self, depth_bias: bool) -> Self {
        self.depth_bias = depth_bias;
        if depth_bias {
            self.fade.w = 0.0;
        }
        self
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

/// [`VertexColorMaterial::with_detail`] of the road meshes.
pub const DETAIL_ROADS: f32 = 1.0;
/// [`VertexColorMaterial::with_detail`] of the terrain.
pub const DETAIL_GROUND: f32 = 2.0;

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

/// How much the middle of a cloud's shadow darkens a lit surface (T-272: "not too dark").
pub const CLOUD_SHADOW: f32 = 0.26;

/// Sets the cloud shadow uniform of every scene material from the lighting settings:
/// [`CLOUD_SHADOW`] when on, 0 when off. Overlay layers (depth-biased) never get one.
pub fn sync_cloud_shadows(
    lighting: Option<Res<crate::LightingSettings>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
) {
    let on = lighting.is_some_and(|l| l.cloud_shadows);
    let stale: Vec<_> = materials
        .iter()
        .filter(|(_, m)| {
            let want = if on && !m.depth_bias {
                CLOUD_SHADOW
            } else {
                0.0
            };
            m.fade.w != want
        })
        .map(|(id, _)| id)
        .collect();
    for id in stale {
        if let Some(mut m) = materials.get_mut(id) {
            m.fade.w = if on && !m.depth_bias {
                CLOUD_SHADOW
            } else {
                0.0
            };
        }
    }
}

/// The cloud shadows' value noise (T-281b), made once at start-up by [`cloud_noise_image`]: one
/// texture sample per octave in `vertex_color.wesl` instead of four hashes.
pub const CLOUD_NOISE: Handle<Image> = uuid_handle!("5d0c6a3e-8f2b-4c71-9e4a-2b7f1c9d3e60");

/// Noise cells across the tileable [`CLOUD_NOISE`] texture (`CLOUD_PERIOD` in the shader).
pub const CLOUD_PERIOD: u32 = 64;
/// Texels per noise cell: the smooth interpolation is baked in, the sampler's linear filter
/// only bridges an eighth of a cell.
const CLOUD_TEXELS: u32 = 8;

/// Value noise that repeats every [`CLOUD_PERIOD`] cells, smoothstep-interpolated, in an R8
/// texture with a repeating, linearly filtered sampler.
pub fn cloud_noise_image() -> Image {
    fn hash(x: u32, y: u32) -> f32 {
        let mut v = (x % CLOUD_PERIOD).wrapping_mul(747_796_405)
            ^ (y % CLOUD_PERIOD).wrapping_mul(2_891_336_453);
        v = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
        v ^= v >> 16;
        v = v.wrapping_mul(0x85eb_ca6b);
        v ^= v >> 13;
        (v & 0xffff) as f32 / 65_535.0
    }
    let side = CLOUD_PERIOD * CLOUD_TEXELS;
    let mut data = Vec::with_capacity((side * side) as usize);
    for ty in 0..side {
        for tx in 0..side {
            let (ix, iy) = (tx / CLOUD_TEXELS, ty / CLOUD_TEXELS);
            let f = |t: u32| {
                let f = (t % CLOUD_TEXELS) as f32 / CLOUD_TEXELS as f32;
                f * f * (3.0 - 2.0 * f)
            };
            let (ux, uy) = (f(tx), f(ty));
            let top = hash(ix, iy) * (1.0 - ux) + hash(ix + 1, iy) * ux;
            let bottom = hash(ix, iy + 1) * (1.0 - ux) + hash(ix + 1, iy + 1) * ux;
            let n = top * (1.0 - uy) + bottom * uy;
            data.push((n * 255.0).round() as u8);
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

fn insert_cloud_noise(images: Option<ResMut<Assets<Image>>>) {
    if let Some(mut images) = images {
        let _ = images.insert(&CLOUD_NOISE, cloud_noise_image());
    }
}

pub struct VertexColorMaterialPlugin;

impl Plugin for VertexColorMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "vertex_color.wesl");
        app.add_plugins(MaterialPlugin::<VertexColorMaterial>::default())
            .init_resource::<OverlayState>()
            .add_systems(PreStartup, insert_cloud_noise)
            .add_systems(Update, (sync_overlay_dim, sync_cloud_shadows));
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

    /// T-272: the cloud shadows are on by default, off with the setting, and never on an
    /// overlay layer.
    #[test]
    fn cloud_shadows_follow_the_setting_and_spare_overlay_layers() {
        let mut app = App::new();
        app.init_resource::<crate::LightingSettings>()
            .init_resource::<Assets<VertexColorMaterial>>()
            .add_systems(Update, sync_cloud_shadows);
        let (scene, layer) = {
            let mut mats = app
                .world_mut()
                .resource_mut::<Assets<VertexColorMaterial>>();
            (
                mats.add(VertexColorMaterial::opaque(0.9, 0.05)),
                mats.add(VertexColorMaterial::blended(0.9, 0.05).with_depth_bias(true)),
            )
        };
        let cloud = |app: &App, h: &Handle<VertexColorMaterial>| {
            app.world()
                .resource::<Assets<VertexColorMaterial>>()
                .get(h)
                .unwrap()
                .fade
                .w
        };
        app.update();
        assert_eq!(cloud(&app, &scene), CLOUD_SHADOW);
        assert_eq!(cloud(&app, &layer), 0.0);
        app.world_mut()
            .resource_mut::<crate::LightingSettings>()
            .cloud_shadows = false;
        app.update();
        assert_eq!(cloud(&app, &scene), 0.0);
    }
}
