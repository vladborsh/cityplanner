//! Bevy side of terrain rendering: spawns chunk entities from `SimView`'s terrain,
//! switches LODs by camera distance, and drives sun, ambient light and sky from `GameTime`.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::light::{CascadeShadowConfigBuilder, GlobalAmbientLight, NotShadowCaster};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use cp_sim::SimView;
use cp_terrain::{MAP_SIZE, Terrain};

use crate::chunk::{
    CHUNK_CELLS, CHUNK_SIZE, CHUNKS_PER_SIDE, LOD_COUNT, MeshData, build_chunk_mesh,
    build_water_mesh, grid_triangles, select_lod,
};
use crate::material::{VertexColorMaterial, VertexColorMaterialPlugin};
use crate::sun::{LightingSettings, lit_state};

/// Adds terrain, sea, rivers, sun and sky. Requires `SimView` to be published as a resource
/// (it may appear later; chunks are spawned when the first terrain shows up).
pub struct TerrainRenderPlugin;

impl Plugin for TerrainRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(VertexColorMaterialPlugin)
            .init_resource::<TerrainStats>()
            .init_resource::<LoadedTerrain>()
            .init_resource::<LightingSettings>()
            .add_systems(Startup, spawn_sun)
            .add_systems(Update, (sync_terrain, update_lods, apply_sun).chain())
            .add_systems(Last, collect_stats);
    }
}

/// One terrain chunk entity.
#[derive(Component)]
pub struct TerrainChunk {
    pub cx: usize,
    pub cz: usize,
    pub lod: usize,
    /// Built LOD meshes (cache); the coarsest exists from the start.
    meshes: [Option<Handle<Mesh>>; LOD_COUNT],
    /// Axis-aligned bounds in world space.
    min: Vec3,
    max: Vec3,
}

/// The main light.
#[derive(Component)]
pub struct Sun;

/// Per-frame render statistics for terrain.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct TerrainStats {
    pub chunks: usize,
    pub visible_chunks: usize,
    /// Visible chunks per LOD.
    pub visible_by_lod: [usize; LOD_COUNT],
    /// Triangles submitted for terrain in the main view (grid + skirts, excluding water).
    pub terrain_triangles: usize,
}

#[derive(Resource, Default)]
struct LoadedTerrain(Option<Arc<Terrain>>);

#[derive(Component)]
struct TerrainRoot;

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

/// Materials shared by all chunks of one terrain.
#[derive(Clone)]
struct TerrainMaterials {
    ground: Handle<VertexColorMaterial>,
    water: Handle<VertexColorMaterial>,
}

/// Wall-clock budget per frame for generating LOD meshes on demand. Render-side only; the
/// sim never sees it. Spreads work over frames, which also suits the single-threaded
/// browser build.
const LOD_BUILD_BUDGET: std::time::Duration = std::time::Duration::from_millis(6);

/// Spawns the terrain root and sea; returns the root and the shared materials.
fn begin_terrain(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<VertexColorMaterial>,
    sea_level: f32,
) -> (Entity, TerrainMaterials) {
    let mats = TerrainMaterials {
        ground: materials.add(VertexColorMaterial::opaque(0.95, 0.08)),
        water: materials.add(VertexColorMaterial::blended(0.12, 0.6)),
    };
    // Sea: one huge quad at sea level (vertex-coloured like everything else).
    let half = MAP_SIZE * 2.0;
    let sea = meshes.add(to_bevy_mesh(MeshData {
        positions: vec![
            [-half, 0.0, -half],
            [half, 0.0, -half],
            [-half, 0.0, half],
            [half, 0.0, half],
        ],
        normals: vec![[0.0, 1.0, 0.0]; 4],
        colors: vec![[0.04, 0.2, 0.33, 0.9]; 4],
        indices: vec![0, 2, 1, 1, 2, 3],
    }));
    let sea_material = materials.add(VertexColorMaterial::blended(0.1, 0.6));
    let root = commands
        .spawn((
            TerrainRoot,
            Transform::default(),
            Visibility::default(),
            Name::new("terrain"),
        ))
        .with_children(|root| {
            // Sea: one huge plane at sea level; the terrain pokes through it.
            root.spawn((
                Mesh3d(sea),
                MeshMaterial3d(sea_material),
                Transform::from_xyz(MAP_SIZE / 2.0, sea_level, MAP_SIZE / 2.0),
                NotShadowCaster,
                Name::new("sea"),
            ));
        })
        .id();
    (root, mats)
}

/// Height range of a chunk's nodes (all of them, whatever the LOD).
fn height_range(terrain: &Terrain, cx: usize, cz: usize) -> (f32, f32) {
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for j in 0..=CHUNK_CELLS {
        for i in 0..=CHUNK_CELLS {
            let h = terrain.node_height(cx * CHUNK_CELLS + i, cz * CHUNK_CELLS + j);
            lo = lo.min(h);
            hi = hi.max(h);
        }
    }
    (lo, hi)
}

/// Spawns chunk `i` showing the coarsest LOD (cheap); finer LODs are built on demand.
fn spawn_chunk(
    root: &mut ChildSpawnerCommands,
    terrain: &Terrain,
    meshes: &mut Assets<Mesh>,
    mats: &TerrainMaterials,
    i: usize,
) {
    let (cx, cz) = (i % CHUNKS_PER_SIDE, i / CHUNKS_PER_SIDE);
    let origin = Vec3::new(cx as f32 * CHUNK_SIZE, 0.0, cz as f32 * CHUNK_SIZE);
    let (lo, hi) = height_range(terrain, cx, cz);
    // The coarsest skirts hang deepest; include them so culling never clips.
    let min = origin + Vec3::new(0.0, lo - crate::chunk::skirt_depth(LOD_COUNT - 1), 0.0);
    let max = origin + Vec3::new(CHUNK_SIZE, hi, CHUNK_SIZE);
    let coarse = meshes.add(to_bevy_mesh(build_chunk_mesh(
        terrain,
        cx,
        cz,
        LOD_COUNT - 1,
    )));
    let mut handles: [Option<Handle<Mesh>>; LOD_COUNT] = Default::default();
    handles[LOD_COUNT - 1] = Some(coarse.clone());
    let mut chunk = root.spawn((
        Mesh3d(coarse),
        MeshMaterial3d(mats.ground.clone()),
        Transform::from_translation(origin),
        Aabb::from_min_max(min - origin, max - origin),
        TerrainChunk {
            cx,
            cz,
            lod: LOD_COUNT - 1,
            meshes: handles,
            min,
            max,
        },
    ));
    if let Some(w) = build_water_mesh(terrain, cx, cz) {
        chunk.with_child((
            Mesh3d(meshes.add(to_bevy_mesh(w))),
            MeshMaterial3d(mats.water.clone()),
            NotShadowCaster,
        ));
    }
}

fn sync_terrain(
    view: Option<Res<SimView>>,
    mut loaded: ResMut<LoadedTerrain>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
    old: Query<Entity, With<TerrainRoot>>,
) {
    let Some(terrain) = view.as_ref().and_then(|v| v.terrain.as_ref()) else {
        return;
    };
    if loaded.0.as_ref().is_some_and(|t| Arc::ptr_eq(t, terrain)) {
        return;
    }
    loaded.0 = Some(Arc::clone(terrain));
    for e in &old {
        commands.entity(e).despawn();
    }
    let (root, mats) = begin_terrain(
        &mut commands,
        &mut meshes,
        &mut materials,
        terrain.sea_level(),
    );
    commands.entity(root).with_children(|root| {
        for i in 0..CHUNKS_PER_SIDE * CHUNKS_PER_SIDE {
            spawn_chunk(root, terrain, &mut meshes, &mats, i);
        }
    });
}

fn dist_to_box(p: Vec3, min: Vec3, max: Vec3) -> f32 {
    (min - p).max(p - max).max(Vec3::ZERO).length()
}

/// Switches chunks to the LOD their distance calls for. A chunk keeps showing its current
/// mesh until the wanted one exists; missing meshes are generated nearest-first within
/// [`LOD_BUILD_BUDGET`] per frame.
fn update_lods(
    camera: Query<&GlobalTransform, With<Camera3d>>,
    loaded: Res<LoadedTerrain>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut chunks: Query<(Entity, &mut TerrainChunk, &mut Mesh3d)>,
) {
    let (Some(cam), Some(terrain)) = (camera.iter().next(), loaded.0.as_ref()) else {
        return;
    };
    let pos = cam.translation();
    let mut missing: Vec<(f32, Entity, usize)> = Vec::new();
    for (entity, mut chunk, mut mesh) in &mut chunks {
        let d = dist_to_box(pos, chunk.min, chunk.max);
        let want = select_lod(d, Some(chunk.lod));
        if want == chunk.lod {
            continue;
        }
        if let Some(h) = chunk.meshes[want].clone() {
            chunk.lod = want;
            mesh.0 = h;
        } else {
            missing.push((d, entity, want));
        }
    }
    missing.sort_by(|a, b| a.0.total_cmp(&b.0));
    let start = bevy::platform::time::Instant::now();
    for (_, entity, want) in missing {
        if start.elapsed() >= LOD_BUILD_BUDGET {
            break;
        }
        let Ok((_, mut chunk, mut mesh)) = chunks.get_mut(entity) else {
            continue;
        };
        let handle = meshes.add(to_bevy_mesh(build_chunk_mesh(
            terrain, chunk.cx, chunk.cz, want,
        )));
        chunk.meshes[want] = Some(handle.clone());
        chunk.lod = want;
        mesh.0 = handle;
    }
}

fn spawn_sun(mut commands: Commands) {
    commands.spawn((
        Sun,
        DirectionalLight {
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            num_cascades: 4,
            minimum_distance: 1.0,
            maximum_distance: 3000.0,
            first_cascade_far_bound: 80.0,
            overlap_proportion: 0.2,
        }
        .build(),
        Transform::default(),
    ));
}

fn apply_sun(
    view: Option<Res<SimView>>,
    lighting: Res<LightingSettings>,
    mut sun: Query<(&mut Transform, &mut DirectionalLight), With<Sun>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear: ResMut<ClearColor>,
) {
    let Some(view) = view else { return };
    let s = lit_state(view.game_time, *lighting);
    let rgb = |c: [f32; 3]| Color::linear_rgb(c[0], c[1], c[2]);
    for (mut tf, mut light) in &mut sun {
        let dir = Vec3::from(s.light_dir);
        *tf = Transform::IDENTITY.looking_to(dir, Vec3::Y);
        light.illuminance = s.illuminance;
        light.color = rgb(s.light_color);
    }
    ambient.brightness = s.ambient_brightness;
    ambient.color = rgb(s.ambient_color);
    clear.0 = rgb(s.sky_color);
}

fn collect_stats(
    chunks: Query<(&TerrainChunk, &ViewVisibility, &Mesh3d)>,
    mut stats: ResMut<TerrainStats>,
) {
    let mut s = TerrainStats::default();
    for (c, vis, _) in &chunks {
        s.chunks += 1;
        if vis.get() {
            s.visible_chunks += 1;
            s.visible_by_lod[c.lod] += 1;
            let n = crate::chunk::verts_per_side(c.lod);
            s.terrain_triangles += grid_triangles(c.lod) + 4 * (n - 1) * 2;
        }
    }
    *stats = s;
}
