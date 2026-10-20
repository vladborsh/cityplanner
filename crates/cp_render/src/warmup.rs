//! Compiles at start-up the render pipelines a city needs only later (T-281b). Bevy builds a
//! pipeline on the render thread the first frame something draws with it, synchronously on
//! macOS, and with a cold shader cache (the first run after an update) a PBR pipeline takes
//! ~140 ms: the first problem icon or fire froze the game that long. Two invisible meshes with
//! the icons' and the fire's materials live for the first [`WARM_FRAMES`] frames, so those
//! pipelines compile in the start-up freeze instead.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;

use crate::chunk::MeshData;

/// Frames the warm-up meshes are drawn for (a pipeline compiles the frame it is first queued).
pub const WARM_FRAMES: u32 = 8;

#[derive(Resource, Default)]
struct WarmUp {
    entities: Vec<Entity>,
    frames: u32,
}

fn spawn_warm_up(
    mut warm: ResMut<WarmUp>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    // One triangle of zero area: queued and drawn, no pixel.
    let mesh = meshes.add(crate::problem_render::to_bevy_mesh(MeshData {
        positions: vec![[0.0; 3]; 3],
        normals: vec![[0.0, 1.0, 0.0]; 3],
        colors: vec![[1.0; 4]; 3],
        indices: vec![0, 1, 2],
    }));
    // The materials of `problem_render` (opaque) and `fire_render` (blended).
    for alpha_mode in [AlphaMode::Opaque, AlphaMode::Blend] {
        let material = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            unlit: true,
            cull_mode: None,
            alpha_mode,
            ..default()
        });
        warm.entities.push(
            commands
                .spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material),
                    Transform::default(),
                    NoFrustumCulling,
                    Name::new("pipeline warm-up"),
                ))
                .id(),
        );
    }
}

fn end_warm_up(mut warm: ResMut<WarmUp>, mut commands: Commands) {
    if warm.entities.is_empty() {
        return;
    }
    warm.frames += 1;
    if warm.frames >= WARM_FRAMES {
        for e in warm.entities.drain(..) {
            commands.entity(e).despawn();
        }
    }
}

pub struct PipelineWarmUpPlugin;

impl Plugin for PipelineWarmUpPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WarmUp>()
            .add_systems(Startup, spawn_warm_up)
            .add_systems(Update, end_warm_up);
    }
}
