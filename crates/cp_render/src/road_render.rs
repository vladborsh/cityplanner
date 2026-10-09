//! Bevy side of road rendering: keeps one mesh entity per 256 m chunk of road, rebuilding
//! only the chunks whose pieces changed (see [`crate::road`]). Reads only `SimView`.

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
use crate::road::{RoadSet, Update};

/// Beyond this distance a chunk shows its marking-free mesh, metres.
pub const FAR_DISTANCE: f32 = 600.0;

pub struct RoadRenderPlugin;

impl Plugin for RoadRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoadStats>()
            .init_resource::<RoadState>()
            .add_systems(Update, (sync_roads, update_road_lods).chain());
    }
}

/// Numbers for the debug window and benchmarks.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct RoadStats {
    pub pieces: usize,
    pub chunks: usize,
    /// Triangles of all `near` meshes (what close-up views draw).
    pub triangles: usize,
    /// Pieces rebuilt by the last update, and how long the whole update took.
    pub last_rebuilt: usize,
    pub last_update_ms: f32,
}

#[derive(Component)]
pub struct RoadChunk {
    near: Handle<Mesh>,
    far: Handle<Mesh>,
    far_active: bool,
    min: Vec3,
    max: Vec3,
}

#[derive(Resource, Default)]
struct RoadState {
    set: RoadSet,
    last_net: Option<Arc<NetGraph>>,
    last_version: u64,
    chunks: BTreeMap<(i32, i32), Entity>,
    material: Option<Handle<VertexColorMaterial>>,
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

fn sync_roads(
    view: Option<Res<SimView>>,
    mut state: ResMut<RoadState>,
    mut stats: ResMut<RoadStats>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
    mut chunks: Query<&mut RoadChunk>,
) {
    let Some(view) = view else { return };
    let Some(terrain) = view.terrain.as_ref() else {
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
    let up: Update = state.set.update(&view.net, &view.junctions, terrain);
    state.last_net = Some(Arc::clone(&view.net));
    state.last_version = view.junctions.version();
    let material = state
        .material
        .get_or_insert_with(|| materials.add(VertexColorMaterial::opaque(0.9, 0.04)))
        .clone();

    for chunk in &up.dirty_chunks {
        let near = state.set.chunk_mesh(*chunk, false);
        let far = state.set.chunk_mesh(*chunk, true);
        let bounds = state.set.chunk_bounds(*chunk);
        let (Some(near), Some(far), Some((min, max))) = (near, far, bounds) else {
            if let Some(e) = state.chunks.remove(chunk) {
                commands.entity(e).despawn();
            }
            continue;
        };
        let aabb = Aabb::from_min_max(min, max);
        if let Some(e) = state.chunks.get(chunk).copied() {
            if let Ok(mut c) = chunks.get_mut(e) {
                let _ = meshes.insert(&c.near, to_bevy_mesh(near));
                let _ = meshes.insert(&c.far, to_bevy_mesh(far));
                c.min = min;
                c.max = max;
                commands.entity(e).insert(aabb);
                continue;
            }
            // Not spawned yet (same frame): replace it.
            commands.entity(e).despawn();
        }
        let (h_near, h_far) = (
            meshes.add(to_bevy_mesh(near)),
            meshes.add(to_bevy_mesh(far)),
        );
        let e = commands
            .spawn((
                Mesh3d(h_near.clone()),
                MeshMaterial3d(material.clone()),
                Transform::default(),
                aabb,
                NotShadowCaster,
                RoadChunk {
                    near: h_near,
                    far: h_far,
                    far_active: false,
                    min,
                    max,
                },
                Name::new("road chunk"),
            ))
            .id();
        state.chunks.insert(*chunk, e);
    }
    *stats = RoadStats {
        pieces: state.set.piece_count(),
        chunks: state.chunks.len(),
        triangles: state.set.triangle_count(false),
        last_rebuilt: up.rebuilt,
        last_update_ms: start.elapsed().as_secs_f32() * 1000.0,
    };
}

fn update_road_lods(
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut chunks: Query<(&mut RoadChunk, &mut Mesh3d)>,
) {
    let Some(cam) = camera.iter().next() else {
        return;
    };
    let pos = cam.translation();
    for (mut c, mut mesh) in &mut chunks {
        let d = (c.min - pos).max(pos - c.max).max(Vec3::ZERO).length();
        let far = if c.far_active {
            d > FAR_DISTANCE * 0.9
        } else {
            d > FAR_DISTANCE * 1.1
        };
        if far != c.far_active {
            c.far_active = far;
            mesh.0 = if far { c.far.clone() } else { c.near.clone() };
        }
    }
}
