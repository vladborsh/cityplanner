//! Bevy side of the burning effect (T-143): one mesh entity with the flames and smoke of every
//! burning building near the camera, rebuilt each frame (they animate), alpha-blended and
//! unlit. The burning list (service problems and roof boxes) is cached and refreshed only when
//! the building grid or the service state changes. Reads only `SimView` and the building
//! meshes; time comes from the app's `Time`, never from the sim tick.

use std::sync::Arc;

use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;
use cp_building::BuildingGrid;
use cp_sim::SimView;

use crate::building_render::{BuildingState, BuildingSync};
use crate::fire_fx::{self, Burning};
use crate::problem::Facing;
use crate::problem_render::to_bevy_mesh;

pub struct FireFxPlugin;

impl Plugin for FireFxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FireFxStats>()
            .init_resource::<FireFx>()
            .add_systems(
                Update,
                (refresh_burning, draw_fire).chain().after(BuildingSync),
            );
    }
}

/// Numbers for the debug window.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct FireFxStats {
    /// Burning buildings in the city.
    pub burning: usize,
    /// Triangles drawn this frame.
    pub triangles: usize,
    /// CPU time of the last frame's mesh build, milliseconds.
    pub update_ms: f32,
}

#[derive(Resource, Default)]
struct FireFx {
    grid: Option<Arc<BuildingGrid>>,
    state_revision: Option<u64>,
    pieces: usize,
    burning: Vec<Burning>,
    mesh: Option<Handle<Mesh>>,
    entity: Option<Entity>,
}

fn refresh_burning(
    view: Option<Res<SimView>>,
    buildings: Res<BuildingState>,
    mut fx: ResMut<FireFx>,
    mut stats: ResMut<FireFxStats>,
) {
    let Some(view) = view else { return };
    let pieces = buildings.set.count();
    let revision = view.service_state.revision();
    let stale = fx
        .grid
        .as_ref()
        .is_none_or(|g| !Arc::ptr_eq(g, &view.buildings))
        || fx.state_revision != Some(revision)
        || fx.pieces != pieces;
    if !stale {
        return;
    }
    fx.burning = fire_fx::burning(view.buildings.iter(), &view.service_state, |id| {
        buildings.set.piece(id).map(|p| (p.min, p.max))
    });
    fx.grid = Some(Arc::clone(&view.buildings));
    fx.state_revision = Some(revision);
    fx.pieces = pieces;
    stats.burning = fx.burning.len();
}

fn draw_fire(
    time: Res<Time>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut fx: ResMut<FireFx>,
    mut stats: ResMut<FireFxStats>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let Some(cam) = camera.iter().next() else {
        return;
    };
    let start = bevy::platform::time::Instant::now();
    let (_, rotation, position) = cam.to_scale_rotation_translation();
    let facing = Facing {
        position,
        right: rotation * Vec3::X,
        up: rotation * Vec3::Y,
    };
    let fx = &mut *fx;
    let data = fire_fx::build_mesh(&fx.burning, &facing, time.elapsed_secs());
    stats.triangles = data.triangle_count();
    let mesh = to_bevy_mesh(data);
    match fx.mesh.clone() {
        Some(h) => {
            let _ = meshes.insert(&h, mesh);
        }
        None => {
            let h = meshes.add(mesh);
            let material = materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                cull_mode: None,
                alpha_mode: AlphaMode::Blend,
                ..default()
            });
            fx.entity = Some(
                commands
                    .spawn((
                        Mesh3d(h.clone()),
                        MeshMaterial3d(material),
                        Transform::default(),
                        // Rebuilt in world space each frame; no bounds are kept.
                        NoFrustumCulling,
                        Name::new("fire effects"),
                    ))
                    .id(),
            );
            fx.mesh = Some(h);
        }
    }
    stats.update_ms = start.elapsed().as_secs_f32() * 1000.0;
}
