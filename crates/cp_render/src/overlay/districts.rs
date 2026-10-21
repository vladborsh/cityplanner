//! The district overlay (T-133): the painted 32 m cells are tinted with their district's colour
//! (alpha 0.25) as a soft-edged field and the contour of each district is a smooth line
//! (BUG-112, [`super::blob`]). Shown with the "Districts" entry of
//! the View menu and automatically while the district brush is active. Draped like the tile
//! border and rebuilt only when the map, the records, the camera's 500 m cell or the line width
//! step changes.

use bevy::camera::primitives::Aabb;
use bevy::light::NotShadowCaster;
use bevy::math::Vec2;
use bevy::prelude::*;
use cp_district::{CELL_M, DistrictMap, Districts, PALETTE_RGB, SIDE};
use cp_sim::SimView;
use cp_terrain::Terrain;

use super::tiles::border_width_step;
use super::{Overlay, OverlayLayers, OverlayState, drape_height, to_bevy_mesh};
use crate::chunk::MeshData;

/// Alpha of the cell tint.
pub const FILL_ALPHA: f32 = 0.25;
/// Alpha of the contour line.
pub const LINE_ALPHA: f32 = 0.9;
/// Height of the tint above the terrain, metres.
pub const FILL_LIFT: f32 = 0.35;
/// Height of the contour above the terrain, metres.
pub const LINE_LIFT: f32 = 0.55;
/// Longest ribbon piece of the contour, metres.
pub const LINE_PIECE: f32 = 4.0;

/// Whether the district brush is active (written by `cp_tools`): the overlay shows then too.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DistrictsMode(pub bool);

fn color_of(districts: &Districts, id: cp_district::DistrictId, alpha: f32) -> [f32; 4] {
    let idx = districts.get(id).map_or(0, |d| usize::from(d.color)) % PALETTE_RGB.len();
    let [r, g, b] = PALETTE_RGB[idx];
    [
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        alpha,
    ]
}

/// The ids painted anywhere on the map, ascending.
fn ids_of(map: &DistrictMap) -> Vec<cp_district::DistrictId> {
    let mut ids: Vec<_> = (0..SIDE as u16)
        .flat_map(|r| (0..SIDE as u16).filter_map(move |c| map.get((c, r))))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn cell_is(map: &DistrictMap, id: cp_district::DistrictId) -> impl Fn(i32, i32) -> bool + '_ {
    move |c, r| c >= 0 && r >= 0 && map.get((c as u16, r as u16)) == Some(id)
}

/// The tint: a soft-edged fill per district (BUG-112), each corner on the terrain the camera
/// at `cam` sees.
pub fn fill_mesh(
    map: &DistrictMap,
    districts: &Districts,
    terrain: &Terrain,
    cam: Option<Vec3>,
) -> MeshData {
    let mut m = MeshData::default();
    let height = |q: Vec2| drape_height(terrain, cam, q) + FILL_LIFT;
    for id in ids_of(map) {
        let [r, g, b, _] = color_of(districts, id, FILL_ALPHA);
        super::blob::soft_fill(
            &mut m,
            &cell_is(map, id),
            SIDE,
            CELL_M,
            [r, g, b],
            FILL_ALPHA,
            &height,
        );
    }
    m
}

/// The sides of painted cells that border a cell of another district (or none), as oriented
/// segments with the district's id.
pub fn contour_edges(map: &DistrictMap) -> Vec<(cp_district::DistrictId, Vec2, Vec2)> {
    let mut out = Vec::new();
    let at = |c: i32, r: i32| -> Option<cp_district::DistrictId> {
        if c < 0 || r < 0 {
            return None;
        }
        map.get((c as u16, r as u16))
    };
    for r in 0..SIDE as i32 {
        for c in 0..SIDE as i32 {
            let Some(id) = at(c, r) else { continue };
            let (x0, z0) = (c as f32 * CELL_M, r as f32 * CELL_M);
            let (x1, z1) = (x0 + CELL_M, z0 + CELL_M);
            for ((nc, nr), a, b) in [
                ((c, r - 1), Vec2::new(x0, z0), Vec2::new(x1, z0)),
                ((c + 1, r), Vec2::new(x1, z0), Vec2::new(x1, z1)),
                ((c, r + 1), Vec2::new(x1, z1), Vec2::new(x0, z1)),
                ((c - 1, r), Vec2::new(x0, z1), Vec2::new(x0, z0)),
            ] {
                if at(nc, nr) != Some(id) {
                    out.push((id, a, b));
                }
            }
        }
    }
    out
}

/// The contour lines: a ribbon of width `w` along the smooth loop of every district
/// (BUG-112: the cell sides linked into loops and rounded, see [`super::blob`]).
pub fn contour_mesh(
    map: &DistrictMap,
    districts: &Districts,
    terrain: &Terrain,
    cam: Option<Vec3>,
    w: f32,
) -> MeshData {
    let mut m = MeshData::default();
    let height = |q: Vec2| drape_height(terrain, cam, q) + LINE_LIFT;
    for id in ids_of(map) {
        let loops = super::blob::smooth_loops(&cell_is(map, id), SIDE, CELL_M);
        super::blob::contour_ribbons(
            &mut m,
            &loops,
            w,
            color_of(districts, id, LINE_ALPHA),
            LINE_PIECE,
            &height,
        );
    }
    m
}

/// What a build used: visibility, the data pointers, camera cell, width step, terrain.
type Key = (bool, usize, usize, (i32, i32, i32), i32, usize);

#[derive(Resource, Default)]
pub struct DistrictLayers {
    entities: Vec<Entity>,
    key: Option<Key>,
}

/// Marks the district fill and contour entities.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct DistrictLayer;

fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    material: &Handle<crate::material::VertexColorMaterial>,
    data: MeshData,
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
                DistrictLayer,
                Name::new("districts"),
            ))
            .id(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sync_district_layers(
    view: Option<Res<SimView>>,
    camera: Query<&Transform, With<Camera3d>>,
    overlay: Res<OverlayLayers>,
    state: Res<OverlayState>,
    mode: Res<DistrictsMode>,
    mut layers: ResMut<DistrictLayers>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let (Some(view), Some(material)) = (view, overlay.material.as_ref()) else {
        return;
    };
    let Some(terrain) = view.terrain.as_ref() else {
        return;
    };
    let visible = state.active == Overlay::Districts || mode.0;
    let cam = camera.single().ok().map(|t| t.translation);
    let bucket = cam.map_or((0, 0, 0), |c| {
        (
            (c.x / 500.0).floor() as i32,
            (c.y / 500.0).floor() as i32,
            (c.z / 500.0).floor() as i32,
        )
    });
    let step = cam.map_or(0, |c| border_width_step(c.y));
    let key = (
        visible,
        std::sync::Arc::as_ptr(&view.district_map) as usize,
        std::sync::Arc::as_ptr(&view.districts) as usize,
        bucket,
        step,
        std::sync::Arc::as_ptr(terrain).cast::<()>() as usize,
    );
    if layers.key.as_ref() == Some(&key) {
        return;
    }
    layers.key = Some(key);
    for e in layers.entities.drain(..) {
        commands.entity(e).despawn();
    }
    if !visible {
        return;
    }
    let w = (step as f32 * 0.5).exp2();
    let (map, ds) = (&view.district_map, &view.districts);
    layers.entities = [
        fill_mesh(map, ds, terrain, cam),
        contour_mesh(map, ds, terrain, cam, w),
    ]
    .into_iter()
    .filter_map(|d| spawn(&mut commands, &mut meshes, material, d))
    .collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_district::DistrictId;

    #[test]
    fn a_two_cell_district_has_six_contour_sides_and_two_quads() {
        let mut map = DistrictMap::default();
        let d = DistrictId(1);
        map.set((4, 4), Some(d));
        map.set((5, 4), Some(d));
        assert_eq!(contour_edges(&map).len(), 6);
        // A neighbouring district's shared side counts for both.
        map.set((6, 4), Some(DistrictId(2)));
        assert_eq!(contour_edges(&map).len(), 6 + 3 + 1);
        let ds = Districts::default();
        let t = Terrain::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands"),
        )
        .unwrap();
        // BUG-112: the soft fill is, per district, a quad per cell in or beside it: 4 x 3 round
        // the two-cell district and 3 x 3 round the one-cell one. A corner's alpha is the tint
        // times the share of its four cells in the district: a strip one cell thick peaks at a
        // half, the ring's outer corners are clear.
        let m = fill_mesh(&map, &ds, &t, None);
        assert_eq!(m.indices.len(), (4 * 3 + 3 * 3) * 6);
        let alphas: Vec<f32> = m.colors.iter().map(|c| c[3]).collect();
        let max = alphas.iter().copied().fold(0.0f32, f32::max);
        assert!((max - FILL_ALPHA * 0.5).abs() < 1e-5, "{max}");
        assert!(alphas.contains(&0.0));
    }
}
