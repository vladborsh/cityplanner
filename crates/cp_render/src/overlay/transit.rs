//! The Transit view (T-157): every line as a ribbon of its colour along its route, the stops as
//! squares, and the line tool's ghost. The waiting counts of the stops are drawn by the UI.

use bevy::camera::primitives::Aabb;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use cp_sim::SimView;
use cp_terrain::Terrain;
use cp_transit::{LINE_PALETTE_RGB, TransitView};

use super::{Overlay, OverlayLayers, OverlayState, flat, to_bevy_mesh};
use crate::chunk::{MeshData, terrain_mesh_height};

/// Half the width of a line's ribbon, metres.
pub const RIBBON_HALF_WIDTH: f32 = 1.4;
/// Height of the ribbons above the ground; each line is a little above the one before so that
/// lines sharing a street do not flicker.
pub const RIBBON_LIFT: f32 = 0.45;
const RIBBON_LAYER_LIFT: f32 = 0.04;
/// Half side of a stop's square, metres.
pub const STOP_HALF: f32 = 2.2;

/// The colour of palette entry `index`, linear-ish RGBA for the vertex-colour material.
pub fn line_color(index: u8, alpha: f32) -> [f32; 4] {
    let [r, g, b] = LINE_PALETTE_RGB[usize::from(index) % LINE_PALETTE_RGB.len()];
    [
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        alpha,
    ]
}

/// What the line tool shows, written by `cp_tools`.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct LineGhost {
    /// The planned route as a polyline (empty while there is nothing to route).
    pub route: Vec<Vec2>,
    /// Palette index of the line being drawn.
    pub color: u8,
    /// The stops placed so far.
    pub stops: Vec<Vec2>,
    /// The stop under the cursor and whether the rules let it stand there.
    pub hover: Option<(Vec2, bool)>,
}

/// A ribbon along `points`, `half` either side, draped on the terrain.
pub fn ribbon(
    m: &mut MeshData,
    points: &[Vec2],
    half: f32,
    lift: f32,
    color: [f32; 4],
    terrain: &Terrain,
) {
    let mut prev: Option<[[f32; 3]; 2]> = None;
    let mut last_p: Option<Vec2> = None;
    for (i, &p) in points.iter().enumerate() {
        // Skip points that sit on top of the previous one (a junction hop joins two lanes).
        if last_p.is_some_and(|l| l.distance_squared(p) < 0.01) {
            continue;
        }
        let a = points[i.saturating_sub(1)];
        let b = points[(i + 1).min(points.len() - 1)];
        let dir = (b - a).normalize_or_zero();
        let side = Vec2::new(dir.y, -dir.x) * half;
        let at = |q: Vec2| [q.x, terrain_mesh_height(terrain, q) + lift, q.y];
        let ring = [at(p + side), at(p - side)];
        if let Some(pr) = prev {
            let base = m.positions.len() as u32;
            for q in [pr[0], ring[0], ring[1], pr[1]] {
                m.positions.push(q);
                m.normals.push([0.0, 1.0, 0.0]);
                m.colors.push(color);
            }
            // Wound to face up whichever way the ribbon runs.
            let up = {
                let v = |k: u32| Vec3::from(m.positions[(base + k) as usize]);
                (v(1) - v(0)).cross(v(2) - v(0)).y >= 0.0
            };
            if up {
                m.indices
                    .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            } else {
                m.indices
                    .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
            }
        }
        prev = Some(ring);
        last_p = Some(p);
    }
}

/// A square of half side `half` centred on `c`.
pub fn square(m: &mut MeshData, c: Vec2, half: f32, lift: f32, color: [f32; 4], terrain: &Terrain) {
    let h = terrain_mesh_height(terrain, c) + lift;
    let at = |dx: f32, dz: f32| [c.x + dx, h, c.y + dz];
    flat(
        m,
        [
            at(-half, -half),
            at(half, -half),
            at(half, half),
            at(-half, half),
        ],
        color,
    );
}

/// A stop: a dark square with a white one on top (the line colour rides in the ribbons).
pub fn stop_marker(m: &mut MeshData, c: Vec2, tint: [f32; 4], terrain: &Terrain) {
    square(
        m,
        c,
        STOP_HALF,
        RIBBON_LIFT + 0.10,
        [0.08, 0.09, 0.12, 0.95],
        terrain,
    );
    square(m, c, STOP_HALF * 0.68, RIBBON_LIFT + 0.14, tint, terrain);
}

/// The ribbons and the stops of a snapshot.
pub fn transit_mesh(t: &TransitView, terrain: &Terrain) -> MeshData {
    let mut m = MeshData::default();
    for (n, (id, route)) in t.routes.iter().enumerate() {
        let Some(line) = t.net.lines.get(*id) else {
            continue;
        };
        let alpha = if route.ok { 0.92 } else { 0.45 };
        ribbon(
            &mut m,
            &route.points,
            RIBBON_HALF_WIDTH,
            RIBBON_LIFT + RIBBON_LAYER_LIFT * (n % 8) as f32,
            line_color(line.color, alpha),
            terrain,
        );
    }
    for (_, stop) in t.net.stops.iter() {
        // The tint of a stop is the colour of the first line that calls there; white if none.
        let tint = stop
            .lines
            .first()
            .and_then(|l| t.net.lines.get(*l))
            .map_or([0.92, 0.94, 0.98, 1.0], |l| line_color(l.color, 1.0));
        stop_marker(&mut m, stop.pos, tint, terrain);
    }
    m
}

/// The ghost of the line tool: the route in the line's colour, the placed stops and the one
/// under the cursor (green when it may stand, red when not).
pub fn ghost_mesh(g: &LineGhost, terrain: &Terrain) -> MeshData {
    let mut m = MeshData::default();
    ribbon(
        &mut m,
        &g.route,
        RIBBON_HALF_WIDTH,
        RIBBON_LIFT + 0.3,
        line_color(g.color, 0.75),
        terrain,
    );
    for &s in &g.stops {
        stop_marker(&mut m, s, line_color(g.color, 1.0), terrain);
    }
    if let Some((p, ok)) = g.hover {
        let tint = if ok {
            [0.35, 0.95, 0.45, 1.0]
        } else {
            [0.95, 0.22, 0.22, 1.0]
        };
        stop_marker(&mut m, p, tint, terrain);
    }
    m
}

#[derive(Component)]
pub struct TransitRibbons;

#[derive(Component)]
pub struct TransitGhost;

/// The layers' entities and what they were built from.
#[derive(Resource, Default)]
pub struct TransitLayer {
    lines: Option<Entity>,
    lines_key: Option<(bool, usize, u64, usize)>,
    ghost: Option<Entity>,
    ghost_key: Option<(bool, LineGhost, usize)>,
}

fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    material: &Handle<crate::material::VertexColorMaterial>,
    data: MeshData,
    name: &'static str,
    marker: impl Component,
) -> Option<Entity> {
    if data.indices.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &data.positions {
        lo = lo.min(Vec3::from(*p));
        hi = hi.max(Vec3::from(*p));
    }
    Some(
        commands
            .spawn((
                Mesh3d(meshes.add(to_bevy_mesh(data))),
                MeshMaterial3d(material.clone()),
                Transform::default(),
                Aabb::from_min_max(lo, hi),
                NotShadowCaster,
                Visibility::Visible,
                marker,
                Name::new(name),
            ))
            .id(),
    )
}

pub(super) fn sync_transit_layer(
    view: Option<Res<SimView>>,
    overlay: Res<OverlayLayers>,
    state: Res<OverlayState>,
    ghost: Res<LineGhost>,
    mut layer: ResMut<TransitLayer>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let (Some(view), Some(material)) = (view, overlay.material.as_ref()) else {
        return;
    };
    let Some(terrain) = view.terrain.as_ref() else {
        return;
    };
    let visible = state.active == Overlay::Transit;
    let tptr = std::sync::Arc::as_ptr(terrain).cast::<()>() as usize;
    let key = (visible, tptr, view.transit.plans_key, ptr_of(&view));
    if layer.lines_key != Some(key) {
        layer.lines_key = Some(key);
        if let Some(e) = layer.lines.take() {
            commands.entity(e).despawn();
        }
        if visible {
            layer.lines = spawn(
                &mut commands,
                &mut meshes,
                material,
                transit_mesh(&view.transit, terrain),
                "transit lines",
                TransitRibbons,
            );
        }
    }
    let gkey = (visible, ghost.clone(), tptr);
    if layer.ghost_key.as_ref() != Some(&gkey) {
        layer.ghost_key = Some(gkey);
        if let Some(e) = layer.ghost.take() {
            commands.entity(e).despawn();
        }
        if visible {
            layer.ghost = spawn(
                &mut commands,
                &mut meshes,
                material,
                ghost_mesh(&ghost, terrain),
                "line ghost",
                TransitGhost,
            );
        }
    }
}

/// Identity of the stops and lines (they are small: hashed by value).
fn ptr_of(view: &SimView) -> usize {
    let t = &view.transit.net;
    let mut h: usize = t.stops.len() * 31 + t.lines.len();
    for (id, s) in t.stops.iter() {
        h = h
            .wrapping_mul(1_000_003)
            .wrapping_add(id.index as usize ^ (s.lines.len() << 8));
    }
    for (id, l) in t.lines.iter() {
        h = h
            .wrapping_mul(1_000_003)
            .wrapping_add(id.index as usize ^ (usize::from(l.color) << 16) ^ (l.stops.len() << 24));
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terrain() -> Terrain {
        Terrain::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands"),
        )
        .unwrap()
    }

    #[test]
    fn a_ribbon_has_a_quad_per_span_and_faces_up_whichever_way_it_runs() {
        let t = terrain();
        for pts in [
            vec![
                Vec2::new(500.0, 500.0),
                Vec2::new(500.0, 520.0),
                Vec2::new(500.0, 540.0),
            ],
            vec![
                Vec2::new(500.0, 540.0),
                Vec2::new(500.0, 520.0),
                Vec2::new(500.0, 500.0),
            ],
        ] {
            let mut m = MeshData::default();
            ribbon(&mut m, &pts, 1.4, 0.4, [1.0; 4], &t);
            assert_eq!(m.indices.len(), 2 * 6);
            for tri in m.indices.chunks(3) {
                let [a, b, c] =
                    [tri[0], tri[1], tri[2]].map(|i| Vec3::from(m.positions[i as usize]));
                assert!((b - a).cross(c - a).y > 0.0);
            }
        }
    }

    #[test]
    fn the_palette_gives_distinct_colours() {
        let all: Vec<_> = (0..cp_transit::PALETTE_LEN)
            .map(|i| line_color(i, 1.0).map(f32::to_bits))
            .collect();
        for (i, a) in all.iter().enumerate() {
            assert!(all[i + 1..].iter().all(|b| a != b));
        }
    }
}
