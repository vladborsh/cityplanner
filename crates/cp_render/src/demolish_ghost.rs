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
/// T-294: stronger (was 0.55): at overview zoom the roofs of the houses are red already.
pub const DEMOLISH_TINT: [f32; 4] = [0.95, 0.05, 0.04, 0.8];
/// The ground outline of a building that would be demolished (T-294) and the faint plate in it.
pub const OUTLINE_COLOR: [f32; 4] = [1.0, 0.06, 0.04, 1.0];
pub const PLATE_COLOR: [f32; 4] = [0.9, 0.04, 0.04, 0.38];
/// Width of the outline, metres: about 2 px at 600 m of camera height.
const OUTLINE_W: f32 = 2.0;
/// Heights of the plate and the outline above the rendered terrain, metres.
const PLATE_LIFT: f32 = 0.35;
const OUTLINE_LIFT: f32 = 0.5;
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

/// The red ground outline and plate of the footprints of `ids` (T-294): readable at overview
/// zoom where the shells are a few pixels. A zoned building's footprint is its lot's cells (the
/// edges no two cells share), a free-standing one its rectangle.
pub fn demolish_outline_mesh(view: &SimView, ids: &[BuildingId]) -> MeshData {
    let mut m = MeshData::default();
    let Some(terrain) = view.terrain.as_deref() else {
        return m;
    };
    for id in ids {
        let Some(b) = view.buildings.get(*id) else {
            continue;
        };
        let quads: Vec<[Vec2; 4]> = match b.footprint {
            cp_building::Footprint::Free { w, d } => {
                vec![cp_building::footprint_quad(b.pos, b.rot, w, d)]
            }
            cp_building::Footprint::Lot(_) => cp_building::held_cells(&view.zones, b)
                .into_iter()
                .filter_map(|c| view.zones.cell_footprint(c).map(|o| o.corners()))
                .collect(),
        };
        for q in &quads {
            plate(&mut m, terrain, q);
        }
        for (a, b) in boundary_edges(&quads) {
            crate::overlay::ribbon_at(
                &mut m,
                terrain,
                a,
                b,
                OUTLINE_W,
                OUTLINE_COLOR,
                OUTLINE_LIFT,
            );
        }
    }
    m
}

/// A translucent plate over one quad, draped [`PLATE_LIFT`] above the rendered terrain.
fn plate(m: &mut MeshData, terrain: &cp_terrain::Terrain, q: &[Vec2; 4]) {
    let base = m.positions.len() as u32;
    for p in q {
        let y = crate::chunk::terrain_mesh_height(terrain, *p) + PLATE_LIFT;
        m.positions.push([p.x, y, p.y]);
        m.normals.push([0.0, 1.0, 0.0]);
        m.colors.push(PLATE_COLOR);
    }
    // Wound to face up whichever way the corners run.
    let area = (q[1] - q[0]).perp_dot(q[3] - q[0]);
    if area > 0.0 {
        m.indices
            .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
    } else {
        m.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// The edges of `quads` that no other quad shares (corners rounded to a centimetre), so the
/// cells of one lot give its outline and not their grid.
fn boundary_edges(quads: &[[Vec2; 4]]) -> Vec<(Vec2, Vec2)> {
    let key = |p: Vec2| ((p.x * 100.0).round() as i64, (p.y * 100.0).round() as i64);
    type Edge = ((i64, i64), (i64, i64), Vec2, Vec2);
    let mut edges: Vec<Edge> = Vec::new();
    for q in quads {
        for i in 0..4 {
            let (a, b) = (q[i], q[(i + 1) % 4]);
            let (ka, kb) = (key(a), key(b));
            let (lo, hi) = if ka <= kb { (ka, kb) } else { (kb, ka) };
            edges.push((lo, hi, a, b));
        }
    }
    let mut out = Vec::new();
    for (i, e) in edges.iter().enumerate() {
        let shared = edges
            .iter()
            .enumerate()
            .any(|(j, o)| j != i && o.0 == e.0 && o.1 == e.1);
        if !shared {
            out.push((e.2, e.3));
        }
    }
    out
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
    mesh.append(demolish_outline_mesh(&view, &layer.ids));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(x: f32, z: f32) -> [Vec2; 4] {
        [
            Vec2::new(x, z),
            Vec2::new(x + 8.0, z),
            Vec2::new(x + 8.0, z + 8.0),
            Vec2::new(x, z + 8.0),
        ]
    }

    /// T-294: the outline of a lot is the edges its cells do not share, not their grid.
    #[test]
    fn the_outline_of_a_lot_is_its_boundary_and_not_the_grid_of_its_cells() {
        // One cell: four edges. Two side by side: six (the shared edge is inside). 2 x 2: eight.
        assert_eq!(boundary_edges(&[quad(0.0, 0.0)]).len(), 4);
        assert_eq!(boundary_edges(&[quad(0.0, 0.0), quad(8.0, 0.0)]).len(), 6);
        let four = [
            quad(0.0, 0.0),
            quad(8.0, 0.0),
            quad(0.0, 8.0),
            quad(8.0, 8.0),
        ];
        assert_eq!(boundary_edges(&four).len(), 8);
    }

    #[test]
    fn the_tint_is_strong_and_the_outline_is_opaque_red() {
        // Stronger than the 0.55 of before; red dominates; the outline is opaque.
        const {
            assert!(DEMOLISH_TINT[3] >= 0.75);
            assert!(DEMOLISH_TINT[0] > 0.9 && DEMOLISH_TINT[1] < 0.1 && DEMOLISH_TINT[2] < 0.1);
            assert!(OUTLINE_COLOR[3] == 1.0);
            assert!(OUTLINE_COLOR[0] > 0.9 && OUTLINE_COLOR[1] < 0.15);
            assert!(OUTLINE_W >= 1.5);
        }
    }
}
