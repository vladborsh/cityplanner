//! The buildings a placement would demolish (T-252b): each drawn again as a translucent red
//! shell just outside its own walls and roof, so the whole building reads red, not only a
//! ground outline. `cp_tools` writes [`DemolishLayer`] every frame; the mesh is rebuilt when the
//! ids or the buildings change.

use std::sync::Arc;

use bevy::prelude::*;
use cp_building::{BuildingGrid, BuildingId};
use cp_core::Tick;
use cp_sim::SimView;

use crate::building::build_building;
use crate::chunk::MeshData;
use crate::material::VertexColorMaterial;
use crate::utility_render::spawn_mesh;

/// Tint of a building that would be demolished.
pub const DEMOLISH_TINT: [f32; 4] = [0.95, 0.12, 0.08, 0.55];
/// Tint of a building a pipe ghost would connect (T-286).
pub const CONNECT_TINT: [f32; 4] = [0.15, 0.75, 1.0, 0.5];

/// How far the shell stands off the building's surfaces, metres.
const SHELL_OFFSET: f32 = 0.15;

/// The buildings to tint, in id order.
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub struct DemolishLayer {
    /// Red: what the click would demolish.
    pub ids: Vec<BuildingId>,
    /// Blue: what a pipe ghost would connect (T-286).
    pub connect: Vec<BuildingId>,
}

#[derive(Resource, Default)]
struct DemolishRender {
    shown: DemolishLayer,
    buildings: Option<Arc<BuildingGrid>>,
    entity: Option<Entity>,
    material: Option<Handle<VertexColorMaterial>>,
}

/// The red shells of `ids`: each building's near mesh, pushed out along its normals and tinted.
pub fn demolish_mesh(view: &SimView, ids: &[BuildingId]) -> MeshData {
    let mut m = MeshData::default();
    push_shells(&mut m, view, ids, DEMOLISH_TINT);
    m
}

/// Appends the shells of `ids` tinted `tint` to `m`.
fn push_shells(m: &mut MeshData, view: &SimView, ids: &[BuildingId], tint: [f32; 4]) {
    let (Some(terrain), Some(db)) = (view.terrain.as_deref(), view.content.as_deref()) else {
        return;
    };
    for id in ids {
        let Some(b) = view.buildings.get(*id) else {
            continue;
        };
        let piece = build_building(
            *id,
            b,
            db.building(b.def),
            &view.net,
            terrain,
            db,
            Tick(view.tick.0),
        );
        let near = piece.near;
        let base = m.positions.len() as u32;
        for (p, n) in near.positions.iter().zip(&near.normals) {
            let (p, n) = (Vec3::from(*p), Vec3::from(*n));
            m.positions.push((p + n * SHELL_OFFSET).to_array());
            m.normals.push(n.to_array());
            m.colors.push(tint);
        }
        m.indices.extend(near.indices.iter().map(|i| base + i));
    }
}

fn sync_demolish(
    view: Option<Res<SimView>>,
    layer: Res<DemolishLayer>,
    mut state: ResMut<DemolishRender>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
) {
    let Some(view) = view else { return };
    let same_grid = state
        .buildings
        .as_ref()
        .is_some_and(|g| Arc::ptr_eq(g, &view.buildings));
    let empty = layer.ids.is_empty() && layer.connect.is_empty();
    if state.shown == *layer && (same_grid || empty) {
        return;
    }
    if let Some(e) = state.entity.take() {
        commands.entity(e).despawn();
    }
    let material = state
        .material
        .get_or_insert_with(|| {
            materials.add(VertexColorMaterial::blended(0.9, 0.02).with_depth_bias(true))
        })
        .clone();
    let mut mesh = demolish_mesh(&view, &layer.ids);
    push_shells(&mut mesh, &view, &layer.connect, CONNECT_TINT);
    state.entity = spawn_mesh(&mut commands, &mut meshes, material, mesh, "demolish ghost");
    state.shown = layer.clone();
    state.buildings = Some(Arc::clone(&view.buildings));
}

pub struct DemolishGhostPlugin;

impl Plugin for DemolishGhostPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DemolishLayer>()
            .init_resource::<DemolishRender>()
            .add_systems(Update, sync_demolish);
    }
}
