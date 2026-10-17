//! Bevy side of the street furniture (T-233): lamp posts, glowing lamp heads, light pools and
//! utility poles, one entity per mesh kind per 256 m road chunk (see
//! [`crate::road::furniture`]). Reads only `SimView`. By night the heads turn warm and bright
//! and the pools fade in; by day (and with the always-day light) the heads are plain grey
//! fixtures and the pools are invisible.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use cp_net::NetGraph;
use cp_sim::SimView;

use crate::chunk::MeshData;
use crate::material::VertexColorMaterial;
use crate::road::furniture::FurnitureSet;
use crate::sun::{LightingSettings, lit_state};

/// Chunks farther than this are not drawn, metres.
pub const DRAW_DISTANCE: f32 = 450.0;

pub struct FurnitureRenderPlugin;

impl Plugin for FurnitureRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FurnitureStats>()
            .init_resource::<FurnitureState>()
            .add_systems(
                Update,
                (sync_furniture, furniture_night, furniture_cull).chain(),
            );
    }
}

/// Numbers for the debug window and benchmarks.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct FurnitureStats {
    pub lamps: usize,
    pub poles: usize,
    pub chunks: usize,
    pub last_update_ms: f32,
}

#[derive(Component)]
pub struct FurnitureChunk {
    meshes: [Handle<Mesh>; 3],
    min: Vec3,
    max: Vec3,
}

#[derive(Resource, Default)]
struct FurnitureState {
    set: FurnitureSet,
    last_net: Option<Arc<NetGraph>>,
    last_version: u64,
    chunks: BTreeMap<(i32, i32), [Entity; 3]>,
    posts: Option<Handle<VertexColorMaterial>>,
    heads: Option<Handle<StandardMaterial>>,
    pools: Option<Handle<StandardMaterial>>,
    /// The night factor the materials were last set for.
    night: f32,
}

fn to_bevy_mesh(m: MeshData) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, m.colors)
    .with_inserted_indices(Indices::U32(m.indices))
}

/// How dark it is, `0` in daylight to `1` at night (smooth over twilight), from the main
/// light's strength.
pub fn night_factor(illuminance: f32) -> f32 {
    let t = ((illuminance - 300.0) / (3000.0 - 300.0)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

fn head_color(night: f32) -> Color {
    let (day, lit) = ([0.55, 0.56, 0.58], [1.0, 0.86, 0.5]);
    let c = |k: usize| day[k] + (lit[k] - day[k]) * night;
    Color::linear_rgb(c(0), c(1), c(2))
}

#[allow(clippy::too_many_arguments)]
fn sync_furniture(
    view: Option<Res<SimView>>,
    mut state: ResMut<FurnitureState>,
    mut stats: ResMut<FurnitureStats>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut vc: ResMut<Assets<VertexColorMaterial>>,
    mut std_mats: ResMut<Assets<StandardMaterial>>,
    mut chunks: Query<&mut FurnitureChunk>,
) {
    let Some(view) = view else { return };
    let (Some(terrain), Some(db)) = (view.terrain.as_ref(), view.content.as_ref()) else {
        return;
    };
    let same_net = state
        .last_net
        .as_ref()
        .is_some_and(|n| Arc::ptr_eq(n, &view.net));
    if same_net && state.last_version == view.junctions.version() {
        return;
    }
    let start = bevy::platform::time::Instant::now();
    let up = state.set.update(&view.net, &view.junctions, terrain, db);
    state.last_net = Some(Arc::clone(&view.net));
    state.last_version = view.junctions.version();
    let posts = state
        .posts
        .get_or_insert_with(|| vc.add(VertexColorMaterial::opaque(0.85, 0.05)))
        .clone();
    let heads = state
        .heads
        .get_or_insert_with(|| {
            std_mats.add(StandardMaterial {
                base_color: head_color(0.0),
                unlit: true,
                ..default()
            })
        })
        .clone();
    let pools = state
        .pools
        .get_or_insert_with(|| {
            std_mats.add(StandardMaterial {
                base_color: Color::linear_rgba(1.0, 1.0, 1.0, 0.0),
                unlit: true,
                alpha_mode: AlphaMode::Add,
                depth_bias: 4.0,
                ..default()
            })
        })
        .clone();

    for chunk in &up.dirty_chunks {
        let built: Vec<_> = (0..3).map(|k| state.set.chunk_mesh(*chunk, k)).collect();
        let bounds = state.set.chunk_bounds(*chunk);
        let (Some(Some(a)), Some(Some(b)), Some(Some(c)), Some((min, max))) = (
            built.first().cloned(),
            built.get(1).cloned(),
            built.get(2).cloned(),
            bounds,
        ) else {
            if let Some(es) = state.chunks.remove(chunk) {
                for e in es {
                    commands.entity(e).despawn();
                }
            }
            continue;
        };
        let aabb = Aabb::from_min_max(min, max);
        if let Some(es) = state.chunks.get(chunk).copied() {
            let mut ok = true;
            for (e, m) in es.iter().zip([a.clone(), b.clone(), c.clone()]) {
                match chunks.get_mut(*e) {
                    Ok(mut fc) => {
                        let slot = es.iter().position(|x| x == e).unwrap_or(0);
                        let _ = meshes.insert(&fc.meshes[slot], to_bevy_mesh(m));
                        fc.min = min;
                        fc.max = max;
                        commands.entity(*e).insert(aabb);
                    }
                    Err(_) => ok = false,
                }
            }
            if ok {
                continue;
            }
            for e in es {
                commands.entity(e).despawn();
            }
        }
        let handles = [a, b, c].map(|m| meshes.add(to_bevy_mesh(m)));
        let spawn = |commands: &mut Commands, slot: usize, name: &'static str| {
            let mut e = commands.spawn((
                Mesh3d(handles[slot].clone()),
                Transform::default(),
                aabb,
                NotShadowCaster,
                FurnitureChunk {
                    meshes: handles.clone(),
                    min,
                    max,
                },
                Name::new(name),
            ));
            match slot {
                0 => e.insert(MeshMaterial3d(posts.clone())),
                1 => e.insert(MeshMaterial3d(heads.clone())),
                _ => e.insert(MeshMaterial3d(pools.clone())),
            };
            e.id()
        };
        let es = [
            spawn(&mut commands, 0, "street posts"),
            spawn(&mut commands, 1, "lamp heads"),
            spawn(&mut commands, 2, "lamp pools"),
        ];
        state.chunks.insert(*chunk, es);
    }
    let (lamps, poles) = state.set.counts();
    debug!(
        "furniture: {lamps} lamps, {poles} poles, {} chunks, {up_ms:.1} ms",
        state.chunks.len(),
        up_ms = start.elapsed().as_secs_f32() * 1000.0,
    );
    *stats = FurnitureStats {
        lamps,
        poles,
        chunks: state.chunks.len(),
        last_update_ms: start.elapsed().as_secs_f32() * 1000.0,
    };
}

/// Heads glow and pools light up with the night.
fn furniture_night(
    view: Option<Res<SimView>>,
    lighting: Option<Res<LightingSettings>>,
    mut state: ResMut<FurnitureState>,
    mut std_mats: ResMut<Assets<StandardMaterial>>,
) {
    let Some(view) = view else { return };
    let settings = lighting.map(|l| *l).unwrap_or_default();
    let night = night_factor(lit_state(view.game_time, settings).illuminance);
    if (night - state.night).abs() < 0.01 {
        return;
    }
    state.night = night;
    if let Some(mut h) = state.heads.as_ref().and_then(|h| std_mats.get_mut(h)) {
        h.base_color = head_color(night);
    }
    if let Some(mut p) = state.pools.as_ref().and_then(|h| std_mats.get_mut(h)) {
        p.base_color = Color::linear_rgba(1.0, 1.0, 1.0, night);
    }
}

fn furniture_cull(
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut chunks: Query<(&FurnitureChunk, &mut Visibility)>,
) {
    let Some(cam) = camera.iter().next() else {
        return;
    };
    let pos = cam.translation();
    for (c, mut vis) in &mut chunks {
        let d = (c.min - pos).max(pos - c.max).max(Vec3::ZERO).length();
        let want = if d > DRAW_DISTANCE {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if *vis != want {
            *vis = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn night_factor_is_zero_in_daylight_one_in_the_dark_and_smooth_between() {
        assert_eq!(night_factor(10_000.0), 0.0);
        assert_eq!(night_factor(250.0), 1.0);
        let mid = night_factor(1500.0);
        assert!(mid > 0.2 && mid < 0.8, "{mid}");
        assert!(night_factor(800.0) > night_factor(2000.0));
    }
}
