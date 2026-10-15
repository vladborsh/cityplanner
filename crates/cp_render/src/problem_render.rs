//! Bevy side of the problem icons (T-125): one mesh entity holding every icon in range as
//! camera-facing quads, rebuilt each frame (they bob), drawn unlit in a single call. The
//! marked buildings (flags and roof height) are cached and refreshed only when the building
//! grid changes. Reads only `SimView` and the building meshes; render time comes from the
//! app's `Time`, never from the sim tick.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use cp_building::BuildingGrid;
use cp_sim::SimView;

use crate::building_render::{BuildingState, BuildingSync};
use crate::problem::{self, Facing, Marked};

pub struct ProblemIconPlugin;

impl Plugin for ProblemIconPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ProblemIconStats>()
            .init_resource::<ProblemIcons>()
            .add_systems(
                Update,
                (refresh_marked, draw_problem_icons)
                    .chain()
                    .after(BuildingSync),
            );
    }
}

/// Numbers for the debug window and benchmarks.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct ProblemIconStats {
    /// Buildings with a problem anywhere in the city.
    pub marked: usize,
    /// Icons drawn this frame (in range).
    pub shown: usize,
    /// CPU time of the last frame's collect and mesh build, milliseconds.
    pub update_ms: f32,
}

#[derive(Resource, Default)]
struct ProblemIcons {
    last: Option<Arc<BuildingGrid>>,
    /// Pieces known when `marked` was built; roofs appear a frame after the grid changes.
    pieces: usize,
    marked: Vec<Marked>,
    lod: problem::Lod,
    mesh: Option<Handle<Mesh>>,
    entity: Option<Entity>,
}

fn to_bevy_mesh(m: crate::chunk::MeshData) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, m.colors)
    .with_inserted_indices(Indices::U32(m.indices))
}

fn refresh_marked(
    view: Option<Res<SimView>>,
    buildings: Res<BuildingState>,
    mut icons: ResMut<ProblemIcons>,
    mut stats: ResMut<ProblemIconStats>,
) {
    let Some(view) = view else { return };
    let pieces = buildings.set.count();
    let stale = icons
        .last
        .as_ref()
        .is_none_or(|g| !Arc::ptr_eq(g, &view.buildings))
        || icons.pieces != pieces;
    if !stale {
        return;
    }
    icons.marked = problem::mark(view.buildings.iter(), |id| {
        buildings.set.piece(id).map(|p| {
            let c = p.center();
            Vec3::new(c.x, p.max.y, c.z)
        })
    });
    icons.lod.clear();
    icons.last = Some(Arc::clone(&view.buildings));
    icons.pieces = pieces;
    stats.marked = icons.marked.len();
}

fn draw_problem_icons(
    time: Res<Time>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut icons: ResMut<ProblemIcons>,
    mut stats: ResMut<ProblemIconStats>,
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
    let icons = &mut *icons;
    let shown = problem::collect(&icons.marked, &mut icons.lod, position, time.elapsed_secs());
    let mesh = to_bevy_mesh(problem::build_mesh(&shown, &facing));
    match icons.mesh.clone() {
        Some(h) => {
            let _ = meshes.insert(&h, mesh);
        }
        None => {
            let h = meshes.add(mesh);
            let material = materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                cull_mode: None,
                ..default()
            });
            let e = commands
                .spawn((
                    Mesh3d(h.clone()),
                    MeshMaterial3d(material),
                    Transform::default(),
                    // The mesh is rebuilt in world space each frame; its bounds are not kept.
                    NoFrustumCulling,
                    Name::new("problem icons"),
                ))
                .id();
            icons.mesh = Some(h);
            icons.entity = Some(e);
        }
    }
    stats.shown = shown.len();
    stats.update_ms = start.elapsed().as_secs_f32() * 1000.0;
}
