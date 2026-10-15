//! The zone grid's outlines in zoning mode, as draped line meshes (`system-design.md` §13.4).
//!
//! Each line is a thin ribbon cut every ≤ [`LINE_STEP`] m, every vertex on the *rendered*
//! terrain ([`cp_render::terrain_mesh_height`]), so it follows ridges instead of cutting
//! straight across them like a gizmo. Two meshes are kept: the grid around the cursor
//! (rebuilt only when the zone grid, the cursor's 100 m bucket, the camera's 250 m bucket or
//! the line width step changes) and the cells the tool would paint (rebuilt when that preview
//! changes). Nothing farther than [`DRAW_DISTANCE`] from the camera is drawn.

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use cp_render::chunk::MeshData;
use cp_render::terrain_mesh_height;
use cp_sim::SimView;
use cp_terrain::Terrain;
use cp_zone::ZoneCellRef;

use crate::plugin::{ActiveTool, ToolState};
use crate::zone::zone_color;

/// Longest ribbon piece, metres.
pub const LINE_STEP: f32 = cp_render::LINE_STEP;
/// Lines farther than this from the camera are not built.
pub const DRAW_DISTANCE: f32 = 1500.0;
/// Cells outlined around the cursor's bucket centre, metres (covers 400 m around the cursor).
const RADIUS: f32 = 500.0;
/// Height of the lines above the rendered terrain, metres.
const LIFT: f32 = 0.5;

/// Appends a ribbon of width `w` along the straight XZ segment `a → b`, draped on the terrain.
pub fn ribbon(m: &mut MeshData, terrain: &Terrain, a: Vec2, b: Vec2, w: f32, color: [f32; 4]) {
    cp_render::ribbon_at(m, terrain, a, b, w, color, LIFT);
}

/// The outline of one cell (its four edges), plus both diagonals when `crossed`.
pub fn cell_lines(
    m: &mut MeshData,
    terrain: &Terrain,
    k: [Vec2; 4],
    w: f32,
    color: [f32; 4],
    crossed: bool,
) {
    for i in 0..4 {
        ribbon(m, terrain, k[i], k[(i + 1) % 4], w, color);
    }
    if crossed {
        ribbon(m, terrain, k[0], k[2], w, color);
        ribbon(m, terrain, k[1], k[3], w, color);
    }
}

fn to_mesh(m: MeshData) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, m.colors)
    .with_inserted_indices(Indices::U32(m.indices))
}

#[derive(Default)]
struct Layer {
    entity: Option<Entity>,
    key: Option<LayerKey>,
}

#[derive(Clone, PartialEq)]
enum LayerKey {
    Grid {
        revision: u64,
        bucket: (i32, i32),
        camera: (i32, i32, i32),
        width_step: i32,
    },
    Hot {
        cells: Vec<ZoneCellRef>,
        paint: [u32; 3],
        width_step: i32,
        camera: (i32, i32, i32),
    },
}

#[derive(Resource, Default)]
pub struct ZoneLines {
    grid: Layer,
    hot: Layer,
    material: Option<Handle<StandardMaterial>>,
}

fn clear(layer: &mut Layer, commands: &mut Commands) {
    if let Some(e) = layer.entity.take() {
        commands.entity(e).despawn();
    }
    layer.key = None;
}

fn install(
    layer: &mut Layer,
    key: LayerKey,
    data: MeshData,
    material: &Handle<StandardMaterial>,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
) {
    if let Some(e) = layer.entity.take() {
        commands.entity(e).despawn();
    }
    layer.key = Some(key);
    if data.indices.is_empty() {
        return;
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &data.positions {
        lo = lo.min(Vec3::from(*p));
        hi = hi.max(Vec3::from(*p));
    }
    layer.entity = Some(
        commands
            .spawn((
                Mesh3d(meshes.add(to_mesh(data))),
                MeshMaterial3d(material.clone()),
                Transform::default(),
                Aabb::from_min_max(lo, hi),
                NotShadowCaster,
                Name::new("zone lines"),
            ))
            .id(),
    );
}

/// Quantised log2 of the line width wanted at `dist` metres (≈ 1.5 px at 1080p): rebuild only
/// when this step changes.
fn width_step(dist: f32) -> i32 {
    let w = (dist * 0.003).clamp(0.25, 4.0);
    (w.log2() * 2.0).round() as i32
}

fn step_width(step: i32) -> f32 {
    (step as f32 * 0.5).exp2()
}

fn bucket(v: f32, size: f32) -> i32 {
    (v / size).floor() as i32
}

#[allow(clippy::too_many_arguments)]
pub fn sync_zone_lines(
    state: Res<ToolState>,
    view: Option<Res<SimView>>,
    camera: Query<&Transform, With<Camera3d>>,
    mut lines: ResMut<ZoneLines>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let lines = &mut *lines;
    let active = state.active == ActiveTool::Zone;
    let (Some(view), Ok(cam), true) = (view.as_deref(), camera.single(), active) else {
        clear(&mut lines.grid, &mut commands);
        clear(&mut lines.hot, &mut commands);
        return;
    };
    let (Some(terrain), Some(centre)) = (view.terrain.as_deref(), state.hover) else {
        clear(&mut lines.grid, &mut commands);
        clear(&mut lines.hot, &mut commands);
        return;
    };
    let material = lines
        .material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                cull_mode: None,
                ..default()
            })
        })
        .clone();
    let cam_pos = cam.translation;
    let cam_bucket = (
        bucket(cam_pos.x, 250.0),
        bucket(cam_pos.y, 250.0),
        bucket(cam_pos.z, 250.0),
    );
    let step = width_step(cam_pos.distance(Vec3::new(
        centre.x,
        terrain_mesh_height(terrain, centre),
        centre.y,
    )));
    let w = step_width(step);
    let near = |k: &[Vec2; 4]| {
        let c = (k[0] + k[2]) * 0.5;
        cam_pos.distance(Vec3::new(c.x, terrain_mesh_height(terrain, c), c.y)) <= DRAW_DISTANCE
    };

    // The grid: every valid cell around the cursor's bucket, grey when unpainted.
    let bucket_xy = (bucket(centre.x, 100.0), bucket(centre.y, 100.0));
    let key = LayerKey::Grid {
        revision: view.zones.revision(),
        bucket: bucket_xy,
        camera: cam_bucket,
        width_step: step,
    };
    if lines.grid.key.as_ref() != Some(&key) {
        let c = Vec2::new(
            (bucket_xy.0 as f32 + 0.5) * 100.0,
            (bucket_xy.1 as f32 + 0.5) * 100.0,
        );
        let mut m = MeshData::default();
        for cell in view.zones.cells_within(c, RADIUS) {
            if !view.zones.is_valid(cell) {
                continue;
            }
            let Some(o) = view.zones.cell_footprint(cell) else {
                continue;
            };
            let k = o.corners();
            if !near(&k) {
                continue;
            }
            let zone = view
                .zones
                .zone_of(cell)
                .unwrap_or(cp_content::ZoneType::None);
            let rgb = zone_color(zone);
            let painted = zone != cp_content::ZoneType::None;
            cell_lines(
                &mut m,
                terrain,
                k,
                w,
                [rgb[0], rgb[1], rgb[2], 0.9],
                painted,
            );
        }
        install(
            &mut lines.grid,
            key,
            m,
            &material,
            &mut commands,
            &mut meshes,
        );
    }

    // The cells the tool would paint, brightened with the tool's colour.
    let paint = zone_color(state.zone.paint_type());
    let key = LayerKey::Hot {
        cells: state.zone_preview.clone(),
        paint: paint.map(f32::to_bits),
        width_step: step,
        camera: cam_bucket,
    };
    if lines.hot.key.as_ref() != Some(&key) {
        let mut m = MeshData::default();
        for &cell in &state.zone_preview {
            if !view.zones.is_valid(cell) {
                continue;
            }
            let Some(o) = view.zones.cell_footprint(cell) else {
                continue;
            };
            let k = o.corners();
            if near(&k) {
                cell_lines(
                    &mut m,
                    terrain,
                    k,
                    w * 1.5,
                    [paint[0], paint[1], paint[2], 1.0],
                    true,
                );
            }
        }
        install(
            &mut lines.hot,
            key,
            m,
            &material,
            &mut commands,
            &mut meshes,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_terrain::{GRID, MapMeta};

    fn ridge() -> Terrain {
        let meta = MapMeta {
            name: "t".into(),
            sea_level: 0.0,
            start_tile: (2, 2),
            external_connections: vec![],
        };
        let h: Vec<u16> = (0..GRID * GRID)
            .map(|k| 2000 + ((k % GRID) % 2) as u16 * 400)
            .collect();
        Terrain::from_samples(meta, &h, &vec![0; GRID * GRID]).unwrap()
    }

    #[test]
    fn ribbons_follow_the_rendered_terrain_and_face_up() {
        let t = ridge();
        let mut m = MeshData::default();
        for (a, b) in [
            (Vec2::new(5000.0, 5000.0), Vec2::new(5040.0, 5003.0)),
            (Vec2::new(5040.0, 5003.0), Vec2::new(5000.0, 5000.0)),
        ] {
            ribbon(&mut m, &t, a, b, 0.5, [1.0; 4]);
        }
        assert!(m.positions.len() >= 2 * 2 * 11);
        for p in &m.positions {
            let h = terrain_mesh_height(&t, Vec2::new(p[0], p[2]));
            assert!((p[1] - h - LIFT).abs() < 1e-4);
        }
        // Cut at most every LINE_STEP: consecutive pairs are no farther apart.
        for w in m.positions.chunks(2).collect::<Vec<_>>().windows(2) {
            let (a, b) = (
                Vec2::new(w[0][0][0], w[0][0][2]),
                Vec2::new(w[1][0][0], w[1][0][2]),
            );
            assert!(a.distance(b) <= LINE_STEP + 0.51);
        }
        for t in m.indices.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(m.positions[i as usize]));
            assert!((b - a).cross(c - a).y > 0.0, "triangle faces down");
        }
    }

    #[test]
    fn width_grows_with_distance_within_limits() {
        assert!(step_width(width_step(10.0)) <= step_width(width_step(1000.0)));
        assert!(step_width(width_step(1.0e6)) <= 4.5);
        assert!(step_width(width_step(0.0)) >= 0.2);
    }
}
