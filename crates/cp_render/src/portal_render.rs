//! The portals of road tunnels (T-173): where a tunnel of a road comes up to the surface, a
//! frame (two pillars and a lintel) with a dark opening, standing across the road. The road in
//! the tunnel itself is seen only in the Underground view (`underground.rs`); the ground view
//! leaves tunnel pieces out, so the portal is what shows where a street goes under.
//!
//! The mesh is a pure function of the network ([`portal_mesh`]); the entity is rebuilt when the
//! network snapshot changes.

use std::sync::Arc;

use bevy::prelude::*;
use cp_content::{ContentDb, RoadClass};
use cp_core::curve::to_xz;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimView;

use crate::chunk::MeshData;
use crate::material::VertexColorMaterial;
use crate::utility_render::spawn_mesh;

const FRAME: [f32; 4] = [0.62, 0.60, 0.56, 1.0];
const OPENING: [f32; 4] = [0.05, 0.05, 0.07, 1.0];
/// Height of the opening, metres, and the thickness of the frame.
const OPENING_H: f32 = 5.0;
const PILLAR: f32 = 1.0;
const DEPTH: f32 = 1.6;

/// A box with half sizes `h` along the axes `ax` (a rotated cuboid).
fn obox(m: &mut MeshData, c: Vec3, ax: [Vec3; 3], h: Vec3, color: [f32; 4]) {
    for (k, sign) in [
        (0, 1.0f32),
        (0, -1.0),
        (1, 1.0),
        (1, -1.0),
        (2, 1.0),
        (2, -1.0),
    ] {
        let (n, u, v) = (ax[k] * sign, ax[(k + 1) % 3], ax[(k + 2) % 3]);
        let (hn, hu, hv) = (h[k], h[(k + 1) % 3], h[(k + 2) % 3]);
        let base = m.positions.len() as u32;
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            m.positions
                .push((c + n * hn + u * hu * a + v * hv * b).to_array());
            m.normals.push(n.to_array());
            m.colors.push(color);
        }
        // `u × v` may point either way round `n`: wind to face outwards.
        if u.cross(v).dot(n) > 0.0 {
            m.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        } else {
            m.indices
                .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        }
    }
}

/// The portals of every road tunnel in `net`: one at each end that stands on the surface.
pub fn portal_mesh(net: &NetGraph, content: &ContentDb) -> MeshData {
    let mut m = MeshData::default();
    for (_, s) in net.segments() {
        if s.elevation != Elevation::Tunnel || content.road(s.road).class == RoadClass::Rail {
            continue;
        }
        let half = content.road(s.road).width * 0.5;
        for (node, at_start) in [(s.start, true), (s.end, false)] {
            let Some(n) = net.node(node).filter(|n| n.elevation == Elevation::Ground) else {
                continue;
            };
            // Into the tunnel, level.
            let t = if at_start {
                s.curve.tangent(0.0)
            } else {
                -s.curve.tangent(1.0)
            };
            let Some(fwd) = to_xz(t).try_normalize() else {
                continue;
            };
            let fwd = Vec3::new(fwd.x, 0.0, fwd.y);
            let right = Vec3::new(-fwd.z, 0.0, fwd.x);
            let ax = [right, Vec3::Y, fwd];
            let p = n.pos;
            // The pillars either side, the lintel over the opening, the dark opening behind.
            for side in [-1.0f32, 1.0] {
                obox(
                    &mut m,
                    p + right * side * (half + PILLAR * 0.5) + Vec3::Y * (OPENING_H * 0.5 + 0.5),
                    ax,
                    Vec3::new(PILLAR * 0.5, OPENING_H * 0.5 + 0.5, DEPTH * 0.5),
                    FRAME,
                );
            }
            obox(
                &mut m,
                p + Vec3::Y * (OPENING_H + 0.5),
                ax,
                Vec3::new(half + PILLAR, 0.5, DEPTH * 0.5),
                FRAME,
            );
            obox(
                &mut m,
                p + fwd * (DEPTH * 0.5) + Vec3::Y * (OPENING_H * 0.5),
                ax,
                Vec3::new(half, OPENING_H * 0.5, 0.1),
                OPENING,
            );
        }
    }
    m
}

#[derive(Resource, Default)]
struct PortalRender {
    net: Option<Arc<NetGraph>>,
    entity: Option<Entity>,
    material: Option<Handle<VertexColorMaterial>>,
}

fn sync_portals(
    view: Option<Res<SimView>>,
    mut st: ResMut<PortalRender>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
) {
    let (Some(view), true) = (view, true) else {
        return;
    };
    let Some(content) = view.content.as_deref() else {
        return;
    };
    if st.net.as_ref().is_some_and(|n| Arc::ptr_eq(n, &view.net)) {
        return;
    }
    st.net = Some(Arc::clone(&view.net));
    if let Some(e) = st.entity.take() {
        commands.entity(e).despawn();
    }
    let material = st
        .material
        .get_or_insert_with(|| materials.add(VertexColorMaterial::opaque(0.8, 0.05)))
        .clone();
    st.entity = spawn_mesh(
        &mut commands,
        &mut meshes,
        material,
        portal_mesh(&view.net, content),
        "road tunnel portals",
    );
}

pub struct PortalRenderPlugin;

impl Plugin for PortalRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PortalRender>()
            .add_systems(Update, sync_portals);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::Vec3 as V;
    use cp_core::curve::CubicBezier3;

    #[test]
    fn a_road_tunnel_has_a_portal_at_each_surface_end_and_a_track_none() {
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let mut net = NetGraph::new();
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        // A tunnel in three pieces: down, deep, up; the nodes at x = 0 and 240 are portals.
        net.add_segment(
            &def,
            road,
            CubicBezier3::line(V::new(0.0, 0.0, 0.0), V::new(80.0, -9.0, 0.0)),
            Elevation::Tunnel,
        )
        .unwrap();
        net.add_segment(
            &def,
            road,
            CubicBezier3::line(V::new(80.0, -9.0, 0.0), V::new(160.0, -12.0, 0.0)),
            Elevation::Tunnel,
        )
        .unwrap();
        net.add_segment(
            &def,
            road,
            CubicBezier3::line(V::new(160.0, -12.0, 0.0), V::new(240.0, 0.0, 0.0)),
            Elevation::Tunnel,
        )
        .unwrap();
        let both = portal_mesh(&net, &db);
        // 4 boxes of 24 vertices at each of the two portals.
        assert_eq!(both.positions.len(), 2 * 4 * 24);
        let one = both;
        assert!(
            one.positions
                .iter()
                .all(|p| p.iter().all(|v| v.is_finite()))
        );
        // A metro track under the ground has none.
        let mut metro = NetGraph::new();
        let track = db.road_id("metro_track").unwrap();
        metro
            .add_segment(
                db.road(track),
                track,
                CubicBezier3::line(V::new(0.0, -12.0, 0.0), V::new(90.0, -12.0, 0.0)),
                Elevation::Tunnel,
            )
            .unwrap();
        assert!(portal_mesh(&metro, &db).positions.is_empty());
    }
}
