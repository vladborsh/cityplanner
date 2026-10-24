//! Bevy side of building rendering: one mesh entity per 256 m chunk, three detail levels per
//! chunk (windows / floor bands / plain box), rebuilt only for chunks whose buildings
//! changed (see [`crate::building`]). Reads only `SimView`. By night the lit windows glow
//! (T-245): the night factor and the lateness of the night go to the one building material.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use cp_building::BuildingGrid;
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_sim::SimView;

use crate::building::lights::lateness;
use crate::building::{BuildingSet, Rotor, RotorKind, rotor_mesh, select_level};
use crate::chunk::MeshData;
use crate::furniture_render::night_factor;
use crate::material::VertexColorMaterial;
use crate::sun::{LightingSettings, lit_state};
use crate::utility_render::PipeLayers;

pub struct BuildingRenderPlugin;

impl Plugin for BuildingRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildingStats>()
            .init_resource::<BuildingState>()
            .add_systems(
                Update,
                (
                    sync_buildings,
                    building_night,
                    building_shadows,
                    update_building_lods,
                    spin_rotors,
                )
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
#[cfg_attr(test, derive(Default))]
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

/// Light of a fully lit window at night, on top of the lit surface (an unlit lamp head is 1).
pub const WINDOW_GLOW: f32 = 0.9;

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
    let up = state.set.update(
        &view.buildings,
        &view.net,
        db,
        terrain,
        Some(&view.junctions),
        Tick(view.tick.0),
    );
    state.last = Some(Arc::clone(&view.buildings));
    state.last_bucket = bucket;
    let material = state
        .material
        .get_or_insert_with(|| {
            materials.add(VertexColorMaterial::opaque(0.85, 0.05).with_window_glow(WINDOW_GLOW))
        })
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

/// The windows glow with the night (the street lamps' night factor) and go dark one by one as
/// it gets late.
fn building_night(
    view: Option<Res<SimView>>,
    lighting: Option<Res<LightingSettings>>,
    state: Res<BuildingState>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
) {
    let Some(view) = view else { return };
    let Some(handle) = state.material.as_ref() else {
        return;
    };
    let settings = lighting.map(|l| *l).unwrap_or_default();
    let night = night_factor(lit_state(view.game_time, settings).illuminance);
    let late = lateness(view.game_time);
    let Some(m) = materials.get(handle) else {
        return;
    };
    // Small steps are skipped, but the ends (full day, full night) are always reached.
    let close = |old: f32, new: f32, step: f32| {
        old == new || ((old - new).abs() < step && new > 0.0 && new < 1.0)
    };
    if close(m.glow.x, night, 0.005) && close(m.glow.y, late, 0.004) {
        return;
    }
    if let Some(mut m) = materials.get_mut(handle) {
        m.glow.x = night;
        m.glow.y = late;
    }
}

/// Alpha of the buildings while the water layer is shown (BUG-109): pipes lie under them and
/// the player must see where to carry on.
pub const WATER_LAYER_BUILDING_ALPHA: f32 = 0.35;

/// The parts of a building that cast a shadow: the chunks and the turning parts.
type BuildingParts = Or<(With<BuildingChunk>, With<RotorSpin>)>;

/// In the water layer the buildings (chunks and the turning parts) cast no shadow: the see-through
/// shell would leave a dark ghost on the ground. The alpha itself is set by one owner,
/// `underground::fade_surfaces`, with the Underground view.
fn building_shadows(
    layers: Option<Res<PipeLayers>>,
    mut commands: Commands,
    parts: Query<(Entity, Has<NotShadowCaster>), BuildingParts>,
) {
    let on = layers.is_some_and(|l| l.visible);
    for (e, has) in &parts {
        if on && !has {
            commands.entity(e).insert(NotShadowCaster);
        } else if !on && has {
            commands.entity(e).remove::<NotShadowCaster>();
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk() -> BuildingChunk {
        BuildingChunk::default()
    }

    /// BUG-109: in the water layer the buildings and their rotors cast no shadow; outside it
    /// they do.
    #[test]
    fn buildings_cast_no_shadow_only_in_the_water_layer() {
        let mut app = App::new();
        app.init_resource::<PipeLayers>()
            .add_systems(Update, building_shadows);
        let chunk = app.world_mut().spawn(chunk()).id();
        let rotor = app
            .world_mut()
            .spawn(RotorSpin {
                phase: 0.0,
                facing: Quat::IDENTITY,
                speed: 1.0,
            })
            .id();
        let shadowless =
            |app: &App| [chunk, rotor].map(|e| app.world().entity(e).contains::<NotShadowCaster>());
        app.update();
        assert_eq!(shadowless(&app), [false, false]);
        app.world_mut().resource_mut::<PipeLayers>().visible = true;
        app.update();
        assert_eq!(shadowless(&app), [true, true]);
        app.world_mut().resource_mut::<PipeLayers>().visible = false;
        app.update();
        assert_eq!(shadowless(&app), [false, false]);
    }
}
