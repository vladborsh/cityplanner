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
    CHUNK_SIZE, CHUNKS_PER_SIDE, LOD_COUNT, MeshData, build_chunk_mesh, build_water_mesh,
    grid_triangles, select_lod,
};
use crate::sun::sun_state;

/// Adds terrain, sea, rivers, sun and sky. Requires `SimView` to be published as a resource
/// (it may appear later; chunks are spawned when the first terrain shows up).
pub struct TerrainRenderPlugin;

impl Plugin for TerrainRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainStats>()
            .init_resource::<LoadedTerrain>()
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
    meshes: [Handle<Mesh>; LOD_COUNT],
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

struct BuiltChunk {
    lods: [MeshData; LOD_COUNT],
    water: Option<MeshData>,
}

/// Generates every chunk's meshes on all cores.
fn build_all(terrain: &Terrain) -> Vec<BuiltChunk> {
    let total = CHUNKS_PER_SIDE * CHUNKS_PER_SIDE;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut out: Vec<Option<BuiltChunk>> = (0..total).map(|_| None).collect();
    let per = total.div_ceil(threads);
    std::thread::scope(|s| {
        for (t, slice) in out.chunks_mut(per).enumerate() {
            s.spawn(move || {
                for (k, slot) in slice.iter_mut().enumerate() {
                    let i = t * per + k;
                    let (cx, cz) = (i % CHUNKS_PER_SIDE, i / CHUNKS_PER_SIDE);
                    *slot = Some(BuiltChunk {
                        lods: std::array::from_fn(|l| build_chunk_mesh(terrain, cx, cz, l)),
                        water: build_water_mesh(terrain, cx, cz),
                    });
                }
            });
        }
    });
    out.into_iter().map(|c| c.expect("built")).collect()
}

fn sync_terrain(
    view: Option<Res<SimView>>,
    mut loaded: ResMut<LoadedTerrain>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
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

    let ground = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.95,
        reflectance: 0.08,
        ..default()
    });
    let water = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.12,
        reflectance: 0.6,
        ..default()
    });
    let sea_level = terrain.sea_level();

    let built = build_all(terrain);
    commands
        .spawn((
            TerrainRoot,
            Transform::default(),
            Visibility::default(),
            Name::new("terrain"),
        ))
        .with_children(|root| {
            // Sea: one huge plane at sea level; the terrain pokes through it.
            let sea = meshes.add(
                Plane3d::default()
                    .mesh()
                    .size(MAP_SIZE * 4.0, MAP_SIZE * 4.0),
            );
            root.spawn((
                Mesh3d(sea),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgba(0.04, 0.2, 0.33, 0.9),
                    alpha_mode: AlphaMode::Blend,
                    perceptual_roughness: 0.1,
                    reflectance: 0.6,
                    ..default()
                })),
                Transform::from_xyz(MAP_SIZE / 2.0, sea_level, MAP_SIZE / 2.0),
                NotShadowCaster,
                Name::new("sea"),
            ));
            for (i, b) in built.into_iter().enumerate() {
                let (cx, cz) = (i % CHUNKS_PER_SIDE, i / CHUNKS_PER_SIDE);
                let origin = Vec3::new(cx as f32 * CHUNK_SIZE, 0.0, cz as f32 * CHUNK_SIZE);
                let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                for p in &b.lods[0].positions {
                    lo = lo.min(p[1]);
                    hi = hi.max(p[1]);
                }
                // The coarsest skirts hang deepest; include them so culling never clips.
                let min =
                    origin + Vec3::new(0.0, lo - crate::chunk::skirt_depth(LOD_COUNT - 1), 0.0);
                let max = origin + Vec3::new(CHUNK_SIZE, hi, CHUNK_SIZE);
                let handles: [Handle<Mesh>; LOD_COUNT] =
                    b.lods.map(|m| meshes.add(to_bevy_mesh(m)));
                let mut chunk = root.spawn((
                    Mesh3d(handles[0].clone()),
                    MeshMaterial3d(ground.clone()),
                    Transform::from_translation(origin),
                    Aabb::from_min_max(min - origin, max - origin),
                    TerrainChunk {
                        cx,
                        cz,
                        lod: 0,
                        meshes: handles,
                        min,
                        max,
                    },
                ));
                if let Some(w) = b.water {
                    chunk.with_child((
                        Mesh3d(meshes.add(to_bevy_mesh(w))),
                        MeshMaterial3d(water.clone()),
                        NotShadowCaster,
                    ));
                }
            }
        });
}

fn dist_to_box(p: Vec3, min: Vec3, max: Vec3) -> f32 {
    (min - p).max(p - max).max(Vec3::ZERO).length()
}

fn update_lods(
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut chunks: Query<(&mut TerrainChunk, &mut Mesh3d)>,
) {
    let Some(cam) = camera.iter().next() else {
        return;
    };
    let pos = cam.translation();
    for (mut chunk, mut mesh) in &mut chunks {
        let d = dist_to_box(pos, chunk.min, chunk.max);
        let lod = select_lod(d, Some(chunk.lod));
        if lod != chunk.lod {
            chunk.lod = lod;
            mesh.0 = chunk.meshes[lod].clone();
        }
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
    mut sun: Query<(&mut Transform, &mut DirectionalLight), With<Sun>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear: ResMut<ClearColor>,
) {
    let Some(view) = view else { return };
    let s = sun_state(view.game_time);
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
