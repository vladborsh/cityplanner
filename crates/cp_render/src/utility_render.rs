//! Power lines and pylons in the world (`architecture/system-design.md` §13.3): a pylon at every
//! node of the power network and two sagging wires along every link. Pipes are underground and
//! are drawn only by the water overlay ([`crate::overlay`]).
//!
//! The geometry is a pure function of the [`UtilityGraph`] ([`power_mesh`]); the Bevy side
//! keeps one mesh entity and rebuilds it when the graph snapshot (an `Arc` in the view) changes.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use cp_sim::SimView;
use cp_utility::{UtilityGraph, UtilityNet};

use crate::chunk::MeshData;
use crate::material::VertexColorMaterial;

/// Height of a pylon, metres.
pub const PYLON_HEIGHT: f32 = 16.0;
/// Half length of the crossarm, metres.
pub const ARM_HALF: f32 = 3.0;
/// Wire sag at mid-span as a fraction of the span length.
const SAG: f32 = 0.015;
const WIRE_STEPS: usize = 8;
const PYLON_COLOR: [f32; 4] = [0.52, 0.54, 0.58, 1.0];
const WIRE_COLOR: [f32; 4] = [0.12, 0.12, 0.14, 1.0];

/// A square prism of side `t` from `a` to `b` (side faces only), flat-shaded.
fn prism(m: &mut MeshData, a: Vec3, b: Vec3, t: f32, color: [f32; 4]) {
    let Some(d) = (b - a).try_normalize() else {
        return;
    };
    let up = if d.y.abs() > 0.9 { Vec3::X } else { Vec3::Y };
    let s = d.cross(up).normalize() * (t * 0.5);
    let v = s.cross(d).normalize() * (t * 0.5);
    let ring = [s + v, -s + v, -s - v, s - v];
    for i in 0..4 {
        let (p, q) = (ring[i], ring[(i + 1) % 4]);
        let n = (p + q).normalize_or_zero().to_array();
        let base = m.positions.len() as u32;
        for c in [a + p, a + q, b + q, b + p] {
            m.positions.push(c.to_array());
            m.normals.push(n);
            m.colors.push(color);
        }
        // Both windings: the prism is thin and seen from any side.
        m.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        m.indices
            .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
    }
}

/// The crossarm direction of a node: horizontal, perpendicular to its first link.
fn arm_dir(graph: &UtilityGraph, node: &cp_utility::UtilityNode) -> Vec3 {
    let along = node
        .links
        .first()
        .and_then(|l| graph.link(*l))
        .map(|l| {
            let t = if graph.node(l.a).is_some_and(|n| n.pos == node.pos) {
                0.0
            } else {
                1.0
            };
            l.curve.tangent(t)
        })
        .unwrap_or(Vec3::X);
    Vec3::new(along.z, 0.0, -along.x)
        .try_normalize()
        .unwrap_or(Vec3::Z)
}

/// Wire attachment points (left, right) at the top of the pylon on `node`.
fn tips(graph: &UtilityGraph, node: &cp_utility::UtilityNode) -> [Vec3; 2] {
    let top = node.pos + Vec3::Y * PYLON_HEIGHT;
    let arm = arm_dir(graph, node) * ARM_HALF;
    [top + arm, top - arm]
}

/// Pylons and wires of the whole power network.
pub fn power_mesh(graph: &UtilityGraph) -> MeshData {
    let mut m = MeshData::default();
    for (_, n) in graph.nodes().filter(|(_, n)| n.net == UtilityNet::Power) {
        let top = n.pos + Vec3::Y * PYLON_HEIGHT;
        prism(&mut m, n.pos, top, 1.0, PYLON_COLOR);
        let [l, r] = tips(graph, n);
        prism(&mut m, l, r, 0.5, PYLON_COLOR);
    }
    for (_, link) in graph.links().filter(|(_, l)| l.net == UtilityNet::Power) {
        let (Some(a), Some(b)) = (graph.node(link.a), graph.node(link.b)) else {
            continue;
        };
        let (ta, mut tb) = (tips(graph, a), tips(graph, b));
        // Pair the tips so the two wires do not cross.
        if ta[0].distance(tb[0]) + ta[1].distance(tb[1])
            > ta[0].distance(tb[1]) + ta[1].distance(tb[0])
        {
            tb.swap(0, 1);
        }
        for (p, q) in ta.into_iter().zip(tb) {
            let sag = SAG * p.distance(q);
            let at = |i: usize| {
                let t = i as f32 / WIRE_STEPS as f32;
                p.lerp(q, t) - Vec3::Y * (sag * 4.0 * t * (1.0 - t))
            };
            for i in 0..WIRE_STEPS {
                prism(&mut m, at(i), at(i + 1), 0.15, WIRE_COLOR);
            }
        }
    }
    m
}

pub struct UtilityRenderPlugin;

impl Plugin for UtilityRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PowerRender>()
            .add_systems(Update, sync_power);
    }
}

#[derive(Resource, Default)]
struct PowerRender {
    last: Option<Arc<UtilityGraph>>,
    entity: Option<Entity>,
    material: Option<Handle<VertexColorMaterial>>,
}

fn sync_power(
    view: Option<Res<SimView>>,
    mut state: ResMut<PowerRender>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
) {
    let Some(view) = view else { return };
    if state
        .last
        .as_ref()
        .is_some_and(|g| Arc::ptr_eq(g, &view.utilities))
    {
        return;
    }
    state.last = Some(Arc::clone(&view.utilities));
    if let Some(e) = state.entity.take() {
        commands.entity(e).despawn();
    }
    let data = power_mesh(&view.utilities);
    if data.indices.is_empty() {
        return;
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &data.positions {
        lo = lo.min(Vec3::from(*p));
        hi = hi.max(Vec3::from(*p));
    }
    let material = state
        .material
        .get_or_insert_with(|| materials.add(VertexColorMaterial::opaque(0.8, 0.1)))
        .clone();
    let mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, data.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, data.colors)
    .with_inserted_indices(Indices::U32(data.indices));
    state.entity = Some(
        commands
            .spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material),
                Transform::default(),
                Aabb::from_min_max(lo, hi),
                NotShadowCaster,
                Name::new("power lines"),
            ))
            .id(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec3;
    use cp_core::curve::CubicBezier3;

    fn line_graph() -> UtilityGraph {
        let mut g = UtilityGraph::new();
        let a = g.add_node(UtilityNet::Power, Vec3::new(100.0, 5.0, 100.0));
        let b = g.add_node(UtilityNet::Power, Vec3::new(180.0, 5.0, 100.0));
        g.add_link(
            a,
            b,
            CubicBezier3::line(Vec3::new(100.0, 5.0, 100.0), Vec3::new(180.0, 5.0, 100.0)),
        )
        .unwrap();
        // A pipe next to it is not drawn in the world.
        let p = g.add_node(UtilityNet::Pipe, Vec3::new(100.0, 5.0, 300.0));
        let q = g.add_node(UtilityNet::Pipe, Vec3::new(180.0, 5.0, 300.0));
        g.add_link(
            p,
            q,
            CubicBezier3::line(Vec3::new(100.0, 5.0, 300.0), Vec3::new(180.0, 5.0, 300.0)),
        )
        .unwrap();
        g
    }

    #[test]
    fn power_mesh_has_pylons_and_sagging_wires_and_no_pipes() {
        let m = power_mesh(&line_graph());
        assert!(m.triangle_count() > 0);
        assert!(m.positions.iter().all(|p| p[2] < 200.0), "no pipe geometry");
        // Pylons rise to PYLON_HEIGHT above the node; wires hang below the tips mid-span.
        let top = m.positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        assert!((top - (5.0 + PYLON_HEIGHT)).abs() < 1.0, "{top}");
        let mid_low = m
            .positions
            .iter()
            .filter(|p| (p[0] - 140.0).abs() < 6.0)
            .map(|p| p[1])
            .fold(f32::MAX, f32::min);
        assert!(mid_low < 5.0 + PYLON_HEIGHT - 0.5, "wire sags: {mid_low}");
        assert!(mid_low > 5.0 + 3.0, "but stays well above the ground");
        assert_eq!(m.positions.len(), m.normals.len());
        assert_eq!(m.positions.len(), m.colors.len());
    }

    #[test]
    fn an_empty_network_draws_nothing() {
        assert!(power_mesh(&UtilityGraph::new()).indices.is_empty());
    }
}
