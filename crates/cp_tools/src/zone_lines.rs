//! The cells the zone tool would paint, outlined in the tool's colour as draped line meshes
//! (`system-design.md` §13.4: zoning mode adds the highlight under the cursor).
//!
//! Each line is a thin ribbon cut every ≤ [`LINE_STEP`] m, every vertex on the *rendered*
//! terrain ([`cp_render::terrain_mesh_height`]), so it follows ridges instead of cutting
//! straight across them like a gizmo. The mesh is rebuilt when the preview, the paint, the
//! line width step or the camera's 250 m bucket changes. Nothing farther than
//! [`DRAW_DISTANCE`] from the camera is drawn.
//!
//! The outlines of the whole grid are the zone layer's (`cp_render`, per chunk). BUG-098: a
//! second, grey grid drawn here within 500 m of the cursor sorted above some chunks' white
//! outlines and below others', so part of the grid turned grey along a chunk edge.

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
/// Height of the lines above the rendered terrain, metres.
const LIFT: f32 = 0.5;
/// Added to the sort distance of the highlight so it is drawn after the zone layer's chunks
/// (blended layers are sorted back to front, and those all stand at the origin).
const SORT_BIAS: f32 = 1.0e6;

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
struct LayerKey {
    cells: Vec<ZoneCellRef>,
    paint: [u32; 3],
    width_step: i32,
    camera: (i32, i32, i32),
}

#[derive(Resource, Default)]
pub struct ZoneLines {
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
        clear(&mut lines.hot, &mut commands);
        return;
    };
    let (Some(terrain), Some(centre)) = (view.terrain.as_deref(), state.hover) else {
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
                depth_bias: SORT_BIAS,
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

    // The cells the tool would paint, brightened with the tool's colour.
    let paint = zone_color(state.zone.paint_type());
    let key = LayerKey {
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
            entry_road: None,
            entry_rail: None,
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

    /// BUG-098: in zoning mode the tool outlines only the cells it would paint; the grid itself
    /// is the zone layer's. A grey grid of its own within 500 m of the cursor sorted above some
    /// zone chunks and below others, and turned part of the grid grey along a chunk edge.
    #[test]
    fn zoning_mode_outlines_only_the_cells_the_tool_would_paint() {
        use std::path::Path;
        use std::sync::Arc;

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let db = Arc::new(cp_content::ContentDb::load_dir(root.join("assets/content")).unwrap());
        let mut sim = cp_sim::SimRunner::new(1);
        sim.set_terrain(Arc::new(
            Terrain::load(&root.join("assets/maps/flatlands")).unwrap(),
        ));
        sim.set_content(Arc::clone(&db));
        sim.world_mut()
            .resource_mut::<cp_core::treasury::Treasury>()
            .balance = cp_core::Cost(1_000_000_000);
        let (a, b) = (Vec2::new(5000.0, 5100.0), Vec2::new(5240.0, 5100.0));
        sim.push_command(
            cp_sim::SimCommand::Net(cp_net::NetCommand::BuildRoad {
                road: db.road_id("basic_2l").unwrap(),
                path: cp_net::RoadPath::Straight { from: a, to: b },
                elevation: cp_net::ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.tick_once();
        let view = sim.view();
        let near_cursor = view.zones.cells_within(Vec2::new(5120.0, 5100.0), 120.0);
        assert!(near_cursor.len() > 20, "{} cells", near_cursor.len());

        let mut app = App::new();
        app.insert_resource(view.clone())
            .init_resource::<ToolState>()
            .init_resource::<ZoneLines>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .add_systems(Update, sync_zone_lines);
        app.world_mut().spawn((
            Camera3d::default(),
            Transform::from_xyz(5120.0, 200.0, 5300.0)
                .looking_at(Vec3::new(5120.0, 0.0, 5100.0), Vec3::Y),
        ));
        {
            let mut state = app.world_mut().resource_mut::<ToolState>();
            state.select(ActiveTool::Zone);
            state.hover = Some(Vec2::new(5120.0, 5100.0));
        }
        let lines = |app: &mut App| {
            let mut q = app.world_mut().query::<(&Name, &Mesh3d)>();
            let w = app.world();
            q.iter(w)
                .filter(|(n, _)| n.as_str() == "zone lines")
                .map(|(_, m)| m.0.clone())
                .collect::<Vec<_>>()
        };

        // Over the grid with nothing to paint: no lines of the tool's own.
        app.update();
        assert!(
            lines(&mut app).is_empty(),
            "the tool drew a grid of its own"
        );

        // Three cells to paint: only they are outlined, in front of the zone layer.
        let hot = near_cursor[..3].to_vec();
        app.world_mut().resource_mut::<ToolState>().zone_preview = hot.clone();
        app.update();
        let meshes = lines(&mut app);
        assert_eq!(meshes.len(), 1);
        let mesh = app
            .world()
            .resource::<Assets<Mesh>>()
            .get(&meshes[0])
            .unwrap();
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(pos)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("no positions");
        };
        let feet: Vec<_> = hot
            .iter()
            .map(|&c| view.zones.cell_footprint(c).unwrap())
            .collect();
        for p in pos {
            let q = Vec2::new(p[0], p[2]);
            assert!(
                feet.iter().any(|o| {
                    let k = o.corners();
                    let c = (k[0] + k[2]) * 0.5;
                    q.distance(c) <= cp_zone::CELL_SIZE
                }),
                "a line at {q} is off the three cells"
            );
        }
        let lines_res = app.world().resource::<ZoneLines>();
        let mat = lines_res.material.as_ref().unwrap();
        let mats = app.world().resource::<Assets<StandardMaterial>>();
        assert_eq!(mats.get(mat).unwrap().depth_bias, SORT_BIAS);
    }
}
