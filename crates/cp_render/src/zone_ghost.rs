//! The zone strip of the road tool's ghost (T-185): a soft translucent fill over the cells the
//! road will get, styled like the pipes' coverage band. `cp_tools` writes [`ZoneStripLayer`]
//! every frame; the mesh is rebuilt when it changes. The outline, the ticks and the block
//! guides are gizmos drawn by the tool.

use bevy::prelude::*;
use cp_sim::SimView;

use crate::chunk::{MeshData, terrain_mesh_height};
use crate::material::VertexColorMaterial;
use crate::utility_render::{BAND_LIFT, spawn_mesh};

/// Fill of the cells a road would zone.
const FILL: [f32; 4] = [0.35, 0.65, 0.95, 0.22];

/// The cells to fill: corners in the XZ plane.
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub struct ZoneStripLayer {
    pub cells: Vec<[Vec2; 4]>,
}

#[derive(Resource, Default)]
struct ZoneStripRender {
    shown: Vec<[Vec2; 4]>,
    entity: Option<Entity>,
    material: Option<Handle<VertexColorMaterial>>,
}

/// The fill mesh of `cells`, draped on the rendered terrain.
pub fn strip_mesh(cells: &[[Vec2; 4]], terrain: &cp_terrain::Terrain) -> MeshData {
    let mut m = MeshData::default();
    for c in cells {
        let base = m.positions.len() as u32;
        for p in c {
            let y = terrain_mesh_height(terrain, *p) + BAND_LIFT + 0.05;
            m.positions.push([p.x, y, p.y]);
            m.normals.push([0.0, 1.0, 0.0]);
            m.colors.push(FILL);
        }
        // Both windings: the corner order depends on the side of the road.
        m.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        m.indices
            .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
    }
    m
}

fn sync_zone_strip(
    view: Option<Res<SimView>>,
    layer: Res<ZoneStripLayer>,
    mut state: ResMut<ZoneStripRender>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
) {
    if state.shown == layer.cells {
        return;
    }
    let Some(terrain) = view.as_deref().and_then(|v| v.terrain.as_deref()) else {
        return;
    };
    if let Some(e) = state.entity.take() {
        commands.entity(e).despawn();
    }
    let material = state
        .material
        .get_or_insert_with(|| {
            materials.add(VertexColorMaterial::blended(0.9, 0.02).with_depth_bias(true))
        })
        .clone();
    state.entity = spawn_mesh(
        &mut commands,
        &mut meshes,
        material,
        strip_mesh(&layer.cells, terrain),
        "zone strip ghost",
    );
    state.shown = layer.cells.clone();
}

pub struct ZoneGhostPlugin;

impl Plugin for ZoneGhostPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneStripLayer>()
            .init_resource::<ZoneStripRender>()
            .add_systems(Update, sync_zone_strip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_is_one_quad_on_the_ground() {
        use cp_terrain::{GRID, MapMeta};
        let meta = MapMeta {
            name: "t".into(),
            sea_level: 0.0,
            start_tile: (2, 2),
            external_connections: vec![],
            entry_road: None,
            entry_rail: None,
        };
        let t =
            cp_terrain::Terrain::from_samples(meta, &vec![500; GRID * GRID], &vec![0; GRID * GRID])
                .unwrap();
        let q = [
            Vec2::new(0.0, 0.0),
            Vec2::new(8.0, 0.0),
            Vec2::new(8.0, 8.0),
            Vec2::new(0.0, 8.0),
        ];
        let m = strip_mesh(&[q, q], &t);
        assert_eq!(m.positions.len(), 8);
        assert_eq!(m.indices.len(), 2 * 12);
    }
}
