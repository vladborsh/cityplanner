//! Bevy side of building rendering: one mesh entity per 256 m chunk, three detail levels per
//! chunk (windows / floor bands / plain box), rebuilt only for chunks whose buildings
//! changed (see [`crate::building`]). Reads only `SimView`.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use cp_building::BuildingGrid;
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_sim::SimView;

use crate::building::{BuildingSet, Rotor, RotorKind, rotor_mesh, select_level};
use crate::chunk::MeshData;
use crate::material::VertexColorMaterial;

pub struct BuildingRenderPlugin;

impl Plugin for BuildingRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildingStats>()
            .init_resource::<BuildingState>()
            .add_systems(
                Update,
                (sync_buildings, update_building_lods, spin_rotors)
                    .chain()
                    .in_set(BuildingSync),
            );
    }
}

/// Numbers for the debug window and benchmarks.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct BuildingStats {
    pub buildings: usize,
    pub chunks: usize,
    pub constructing: usize,
    /// Triangles of all `near` meshes.
    pub near_triangles: usize,
    /// Triangles currently selected for drawing (by each chunk's detail level).
    pub drawn_triangles: usize,
    pub last_rebuilt: usize,
    pub last_update_ms: f32,
}

#[derive(Component)]
pub struct BuildingChunk {
    meshes: [Handle<Mesh>; 3],
    level: usize,
    min: Vec3,
    max: Vec3,
    tris: [usize; 3],
}

/// The systems that bring the building meshes up to date; the problem icons run after them.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BuildingSync;

#[derive(Resource, Default)]
pub(crate) struct BuildingState {
    pub(crate) set: BuildingSet,
    last: Option<Arc<BuildingGrid>>,
    /// Tick bucket at which construction looks were last refreshed.
    last_bucket: u64,
    chunks: BTreeMap<crate::building::ChunkKey, Entity>,
    material: Option<Handle<VertexColorMaterial>>,
    /// Turning parts (turbine rotors, ferris wheels, carousels): the entity and the rotor it
    /// was built for.
    rotors: BTreeMap<cp_building::BuildingId, (Entity, Rotor)>,
}

/// A construction stage lasts a few dozen ticks (T-235): re-check about as often.
const REFRESH_TICKS: u64 = TICKS_PER_DAY / 128;

/// A turning rotor (view only: the angle comes from the frame clock, never from the sim).
#[derive(Component)]
struct RotorSpin {
    /// Turn at this angle plus the clock, radians.
    phase: f32,
    /// Rotation that takes +Z to the rotor axis.
    facing: Quat,
    /// Radians per second.
    speed: f32,
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

fn sync_buildings(
    view: Option<Res<SimView>>,
    mut state: ResMut<BuildingState>,
    mut stats: ResMut<BuildingStats>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
    mut chunks: Query<&mut BuildingChunk>,
) {
    let Some(view) = view else { return };
    let (Some(terrain), Some(db)) = (view.terrain.as_ref(), view.content.as_ref()) else {
        return;
    };
    let bucket = view.tick.0 / REFRESH_TICKS;
    let changed = state
        .last
        .as_ref()
        .is_none_or(|g| !Arc::ptr_eq(g, &view.buildings));
    let ticking = state.set.constructing() > 0 && bucket != state.last_bucket;
    if !changed && !ticking {
        return;
    }
    let start = bevy::platform::time::Instant::now();
    let up = state
        .set
        .update(&view.buildings, &view.net, db, terrain, Tick(view.tick.0));
    state.last = Some(Arc::clone(&view.buildings));
    state.last_bucket = bucket;
    let material = state
        .material
        .get_or_insert_with(|| materials.add(VertexColorMaterial::opaque(0.85, 0.05)))
        .clone();

    if up.rebuilt > 0 || up.removed > 0 || state.rotors.is_empty() {
        let want: BTreeMap<_, _> = state.set.rotors().map(|(id, r)| (id, *r)).collect();
        let stale: Vec<_> = state
            .rotors
            .iter()
            .filter(|(id, (_, r))| want.get(id) != Some(r))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            if let Some((e, _)) = state.rotors.remove(&id) {
                commands.entity(e).despawn();
            }
        }
        for (id, r) in want {
            if state.rotors.contains_key(&id) {
                continue;
            }
            let facing = Quat::from_rotation_arc(Vec3::Z, r.axis);
            let e = commands
                .spawn((
                    Mesh3d(meshes.add(to_bevy_mesh(rotor_mesh(&r, 0.0)))),
                    MeshMaterial3d(material.clone()),
                    Transform::from_translation(r.hub).with_rotation(facing),
                    RotorSpin {
                        phase: id.index as f32 * 0.7,
                        facing,
                        speed: r.speed(),
                    },
                    Name::new(match r.kind {
                        RotorKind::Turbine => "wind rotor",
                        RotorKind::FerrisWheel => "ferris wheel",
                        RotorKind::Carousel => "carousel",
                    }),
                ))
                .id();
            state.rotors.insert(id, (e, r));
        }
    }

    for chunk in &up.dirty_chunks {
        let built: Vec<_> = (0..3).map(|l| state.set.chunk_mesh(*chunk, l)).collect();
        let bounds = state.set.chunk_bounds(*chunk);
        let (Some(Some(l0)), Some(Some(l1)), Some(Some(l2)), Some((min, max))) = (
            built.first().cloned(),
            built.get(1).cloned(),
            built.get(2).cloned(),
            bounds,
        ) else {
            if let Some(e) = state.chunks.remove(chunk) {
                commands.entity(e).despawn();
            }
            continue;
        };
        let tris = [
            l0.triangle_count(),
            l1.triangle_count(),
            l2.triangle_count(),
        ];
        let aabb = Aabb::from_min_max(min, max);
        if let Some(e) = state.chunks.get(chunk).copied() {
            if let Ok(mut c) = chunks.get_mut(e) {
                for (h, m) in c
                    .meshes
                    .clone()
                    .iter()
                    .zip([l0.clone(), l1.clone(), l2.clone()])
                {
                    let _ = meshes.insert(h, to_bevy_mesh(m));
                }
                c.min = min;
                c.max = max;
                c.tris = tris;
                commands.entity(e).insert(aabb);
                continue;
            }
            commands.entity(e).despawn();
        }
        let handles = [l0, l1, l2].map(|m| meshes.add(to_bevy_mesh(m)));
        let e = commands
            .spawn((
                Mesh3d(handles[0].clone()),
                MeshMaterial3d(material.clone()),
                Transform::default(),
                aabb,
                BuildingChunk {
                    meshes: handles,
                    level: 0,
                    min,
                    max,
                    tris,
                },
                Name::new("building chunk"),
            ))
            .id();
        state.chunks.insert(*chunk, e);
    }
    *stats = BuildingStats {
        buildings: state.set.count(),
        chunks: state.chunks.len(),
        constructing: state.set.constructing(),
        near_triangles: state.set.triangle_count(0),
        drawn_triangles: stats.drawn_triangles,
        last_rebuilt: up.rebuilt,
        last_update_ms: start.elapsed().as_secs_f32() * 1000.0,
    };
}

fn update_building_lods(
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut chunks: Query<(&mut BuildingChunk, &mut Mesh3d, &ViewVisibility)>,
    mut stats: ResMut<BuildingStats>,
) {
    let Some(cam) = camera.iter().next() else {
        return;
    };
    let pos = cam.translation();
    let mut drawn = 0;
    for (mut c, mut mesh, vis) in &mut chunks {
        let d = (c.min - pos).max(pos - c.max).max(Vec3::ZERO).length();
        let level = select_level(d, Some(c.level));
        if level != c.level {
            c.level = level;
            mesh.0 = c.meshes[level].clone();
        }
        if vis.get() {
            drawn += c.tris[c.level];
        }
    }
    stats.drawn_triangles = drawn;
}

fn spin_rotors(time: Res<Time>, mut rotors: Query<(&RotorSpin, &mut Transform)>) {
    let t = time.elapsed_secs();
    for (spin, mut tf) in &mut rotors {
        tf.rotation = spin.facing * Quat::from_rotation_z(spin.phase + t * spin.speed);
    }
}
