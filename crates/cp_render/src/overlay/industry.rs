//! The M5 overlays (T-207): "Resources" tints every 64 m cell with the resource that is richest
//! there (alpha by richness; spent ore and oil grey), and "Industry areas" tints the painted
//! 32 m cells of the industry areas with the colour of their resource and draws their outline.
//! Both are draped like the district overlay and rebuilt only when their data, the camera's
//! 500 m cell or the line width step change. The areas show automatically while the area brush
//! is active ([`AreasMode`]).

use bevy::camera::primitives::Aabb;
use bevy::light::NotShadowCaster;
use bevy::math::Vec2;
use bevy::prelude::*;
use cp_content::{ParkKind, ResourceKind};
use cp_district::{AreaId, AreaKind, AreaMap, Areas, CELL_M, SIDE};
use cp_sim::SimView;
use cp_terrain::{RES_CELL, RES_GRID, ResourceDeposits, ResourceMap, Terrain};

use super::tiles::border_width_step;
use super::{Overlay, OverlayLayers, OverlayState, drape_height, push, ribbon_with, to_bevy_mesh};
use crate::chunk::MeshData;

/// Alpha of the area tint.
pub const FILL_ALPHA: f32 = 0.38;
/// Alpha of the area outline.
pub const LINE_ALPHA: f32 = 0.9;
/// Height of the tints above the terrain, metres.
pub const FILL_LIFT: f32 = 0.35;
/// Height of the outline above the terrain, metres.
pub const LINE_LIFT: f32 = 0.55;
/// Longest ribbon piece of the outline, metres.
pub const LINE_PIECE: f32 = 4.0;
/// Richness below which a cell is left clear in the Resources overlay.
pub const MIN_RICHNESS: f32 = 0.08;
/// Colour of spent ore and oil.
pub const SPENT: [f32; 3] = [0.5, 0.5, 0.5];

/// Which areas an overlay draws (T-226): the industry ones or the parks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AreaFamily {
    Industry,
    Park,
}

impl AreaFamily {
    /// Whether an area of `kind` belongs to the family.
    pub fn has(self, kind: AreaKind) -> bool {
        matches!(
            (self, kind),
            (AreaFamily::Industry, AreaKind::Industry(_)) | (AreaFamily::Park, AreaKind::Park(_))
        )
    }
}

/// The areas the active area brush paints (written by `cp_tools`): their overlay shows then too.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AreasMode(pub Option<AreaFamily>);

/// The colour of a park kind: lawn green, deep forest, zoo orange, fairground violet.
pub fn park_color(kind: ParkKind) -> [f32; 3] {
    match kind {
        ParkKind::CityPark => [0.36, 0.72, 0.36],
        ParkKind::Nature => [0.10, 0.45, 0.38],
        ParkKind::Zoo => [0.86, 0.55, 0.20],
        ParkKind::Amusement => [0.66, 0.36, 0.82],
    }
}

/// The colour of a resource: gold fields, green forest, steel-blue ore, violet oil.
pub fn resource_color(kind: ResourceKind) -> [f32; 3] {
    match kind {
        ResourceKind::Farming => [0.92, 0.78, 0.22],
        ResourceKind::Forestry => [0.16, 0.58, 0.26],
        ResourceKind::Ore => [0.42, 0.58, 0.86],
        ResourceKind::Oil => [0.52, 0.2, 0.62],
    }
}

/// The resource an area works, if it is an industry area.
fn area_resource(areas: &Areas, id: AreaId) -> Option<ResourceKind> {
    match areas.get(id)?.kind {
        AreaKind::Industry(r) => Some(r),
        AreaKind::Park(_) => None,
    }
}

fn area_color(areas: &Areas, id: AreaId, alpha: f32) -> [f32; 4] {
    let [r, g, b] = match areas.get(id).map(|a| a.kind) {
        Some(AreaKind::Park(k)) => park_color(k),
        _ => area_resource(areas, id).map_or([0.8, 0.8, 0.8], resource_color),
    };
    [r, g, b, alpha]
}

/// Whether area `id` is of the family.
fn in_family(areas: &Areas, id: AreaId, family: AreaFamily) -> bool {
    areas.get(id).is_some_and(|a| family.has(a.kind))
}

/// The colour of resource cell `(cx, cz)` of the Resources overlay, if it has a resource: the
/// richest one, its alpha following the richness; ore or oil with nothing left is grey.
pub fn resource_cell_color(
    map: &ResourceMap,
    deposits: &ResourceDeposits,
    cx: usize,
    cz: usize,
) -> Option<[f32; 4]> {
    let (kind, richness) = ResourceKind::ALL
        .into_iter()
        .map(|k| (k, map.richness(k, cx, cz)))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    if richness < MIN_RICHNESS {
        return None;
    }
    let spent =
        kind.is_finite() && !deposits.is_unfilled() && deposits.remaining(kind, cx, cz) == 0;
    let [r, g, b] = if spent { SPENT } else { resource_color(kind) };
    Some([r, g, b, 0.25 + 0.45 * richness.min(1.0)])
}

/// The Resources tint: one quad per resource cell that has something.
pub fn resources_mesh(
    map: &ResourceMap,
    deposits: &ResourceDeposits,
    terrain: &Terrain,
    cam: Option<Vec3>,
) -> MeshData {
    let mut m = MeshData::default();
    for cz in 0..RES_GRID {
        for cx in 0..RES_GRID {
            let Some(color) = resource_cell_color(map, deposits, cx, cz) else {
                continue;
            };
            let (x0, z0) = (cx as f32 * RES_CELL, cz as f32 * RES_CELL);
            let ids = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(i, j)| {
                let p = Vec2::new(x0 + i * RES_CELL, z0 + j * RES_CELL);
                push(
                    &mut m,
                    [p.x, drape_height(terrain, cam, p) + FILL_LIFT, p.y],
                    color,
                )
            });
            m.indices
                .extend_from_slice(&[ids[0], ids[2], ids[1], ids[0], ids[3], ids[2]]);
        }
    }
    m
}

/// The area tint: one quad per painted cell, in the colour of the area's resource.
pub fn area_fill_mesh(
    map: &AreaMap,
    areas: &Areas,
    terrain: &Terrain,
    cam: Option<Vec3>,
    family: AreaFamily,
) -> MeshData {
    let mut m = MeshData::default();
    for r in 0..SIDE as u16 {
        for c in 0..SIDE as u16 {
            let Some(id) = map.get((c, r)).filter(|&id| in_family(areas, id, family)) else {
                continue;
            };
            let color = area_color(areas, id, FILL_ALPHA);
            let (x0, z0) = (f32::from(c) * CELL_M, f32::from(r) * CELL_M);
            let ids = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(i, j)| {
                let p = Vec2::new(x0 + i * CELL_M, z0 + j * CELL_M);
                push(
                    &mut m,
                    [p.x, drape_height(terrain, cam, p) + FILL_LIFT, p.y],
                    color,
                )
            });
            m.indices
                .extend_from_slice(&[ids[0], ids[2], ids[1], ids[0], ids[3], ids[2]]);
        }
    }
    m
}

/// A square marker on every gate of a park (T-226), in the colour of its kind.
pub fn gate_markers_mesh(view: &SimView, terrain: &Terrain, cam: Option<Vec3>) -> MeshData {
    let mut m = MeshData::default();
    let Some(db) = view.content.as_deref() else {
        return m;
    };
    const HALF: f32 = 7.0;
    for (_, b) in view.buildings.iter() {
        let Some(spec) = db.building(b.def).park.as_ref() else {
            continue;
        };
        let (cp_content::ParkRole::Gate, Some(kind)) = (spec.role, spec.park) else {
            continue;
        };
        let [r, g, bl] = park_color(kind);
        let color = [(r + 1.0) * 0.5, (g + 1.0) * 0.5, (bl + 1.0) * 0.5, 1.0];
        let ids = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(i, j)| {
            let p = b.pos + Vec2::new(i * HALF, j * HALF);
            push(
                &mut m,
                [p.x, drape_height(terrain, cam, p) + LINE_LIFT + 0.2, p.y],
                color,
            )
        });
        m.indices
            .extend_from_slice(&[ids[0], ids[2], ids[1], ids[0], ids[3], ids[2]]);
    }
    m
}

/// The sides of painted cells that border a cell of another area (or none).
pub fn area_contour_edges(map: &AreaMap) -> Vec<(AreaId, Vec2, Vec2)> {
    let mut out = Vec::new();
    let at = |c: i32, r: i32| -> Option<AreaId> {
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

/// The outline of the areas: a ribbon of width `w` along every contour side.
pub fn area_contour_mesh(
    map: &AreaMap,
    areas: &Areas,
    terrain: &Terrain,
    cam: Option<Vec3>,
    w: f32,
    family: AreaFamily,
) -> MeshData {
    let mut m = MeshData::default();
    let height = |q: Vec2| drape_height(terrain, cam, q) + LINE_LIFT;
    for (id, a, b) in area_contour_edges(map) {
        if !in_family(areas, id, family) {
            continue;
        }
        let d = (b - a).normalize_or_zero();
        ribbon_with(
            &mut m,
            a - d * (w * 0.5),
            b + d * (w * 0.5),
            w,
            area_color(areas, id, LINE_ALPHA),
            LINE_PIECE,
            &height,
        );
    }
    m
}

/// Marks the Resources overlay entity.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceLayer;

/// Marks the area fill and outline entities.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AreaLayer;

type ResKey = (bool, usize, usize, (i32, i32, i32), usize);
type AreaKey = (
    Option<AreaFamily>,
    usize,
    usize,
    (i32, i32, i32),
    i32,
    usize,
);

#[derive(Resource, Default)]
pub struct IndustryLayers {
    resources: Vec<Entity>,
    resources_key: Option<ResKey>,
    areas: Vec<Entity>,
    areas_key: Option<AreaKey>,
}

fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    material: &Handle<crate::material::VertexColorMaterial>,
    data: MeshData,
    name: &'static str,
    resources: bool,
) -> Option<Entity> {
    if data.indices.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &data.positions {
        lo = lo.min(Vec3::from(*p));
        hi = hi.max(Vec3::from(*p));
    }
    let mut e = commands.spawn((
        Mesh3d(meshes.add(to_bevy_mesh(data))),
        MeshMaterial3d(material.clone()),
        Transform::default(),
        Aabb::from_min_max(lo, hi),
        NotShadowCaster,
        Visibility::Visible,
        Name::new(name),
    ));
    if resources {
        e.insert(ResourceLayer);
    } else {
        e.insert(AreaLayer);
    }
    Some(e.id())
}

fn bucket(cam: Option<Vec3>) -> (i32, i32, i32) {
    cam.map_or((0, 0, 0), |c| {
        (
            (c.x / 500.0).floor() as i32,
            (c.y / 500.0).floor() as i32,
            (c.z / 500.0).floor() as i32,
        )
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sync_industry_layers(
    view: Option<Res<SimView>>,
    camera: Query<&Transform, With<Camera3d>>,
    overlay: Res<OverlayLayers>,
    state: Res<OverlayState>,
    mode: Res<AreasMode>,
    mut layers: ResMut<IndustryLayers>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let (Some(view), Some(material)) = (view, overlay.material.as_ref()) else {
        return;
    };
    let Some(terrain) = view.terrain.as_ref() else {
        return;
    };
    let cam = camera.single().ok().map(|t| t.translation);
    let terrain_ptr = std::sync::Arc::as_ptr(terrain).cast::<()>() as usize;

    // Resources.
    let visible = state.active == Overlay::Resources;
    let key = (
        visible,
        std::sync::Arc::as_ptr(&view.deposits) as usize,
        0,
        bucket(cam),
        terrain_ptr,
    );
    if layers.resources_key.as_ref() != Some(&key) {
        layers.resources_key = Some(key);
        for e in layers.resources.drain(..) {
            commands.entity(e).despawn();
        }
        if visible {
            let data = resources_mesh(terrain.resources(), &view.deposits, terrain, cam);
            layers.resources = spawn(
                &mut commands,
                &mut meshes,
                material,
                data,
                "resources",
                true,
            )
            .into_iter()
            .collect();
        }
    }

    // Areas: the family of the overlay on, else of the brush.
    let shown = match state.active {
        Overlay::IndustryAreas => Some(AreaFamily::Industry),
        Overlay::Parks => Some(AreaFamily::Park),
        _ => mode.0,
    };
    let step = cam.map_or(0, |c| border_width_step(c.y));
    // The gates of the parks move the markers: the buildings are part of the key.
    let key = (
        shown,
        std::sync::Arc::as_ptr(&view.area_map) as usize,
        std::sync::Arc::as_ptr(&view.areas) as usize,
        bucket(cam),
        step,
        terrain_ptr ^ std::sync::Arc::as_ptr(&view.buildings) as usize,
    );
    if layers.areas_key.as_ref() == Some(&key) {
        return;
    }
    layers.areas_key = Some(key);
    for e in layers.areas.drain(..) {
        commands.entity(e).despawn();
    }
    let Some(family) = shown else {
        return;
    };
    let w = (step as f32 * 0.5).exp2();
    let (map, areas) = (&view.area_map, &view.areas);
    let mut meshes_data = vec![
        area_fill_mesh(map, areas, terrain, cam, family),
        area_contour_mesh(map, areas, terrain, cam, w, family),
    ];
    if family == AreaFamily::Park {
        meshes_data.push(gate_markers_mesh(&view, terrain, cam));
    }
    layers.areas = meshes_data
        .into_iter()
        .filter_map(|d| {
            spawn(
                &mut commands,
                &mut meshes,
                material,
                d,
                "industry_areas",
                false,
            )
        })
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_content::ResourceKind as R;

    fn map_with(px: [u8; 4]) -> ResourceMap {
        ResourceMap::from_pixels(vec![px; RES_GRID * RES_GRID]).unwrap()
    }

    /// The tint of a cell follows its richest resource; spent ore is grey; poor cells are clear.
    #[test]
    fn a_cell_takes_the_colour_of_its_richest_resource_and_spent_ore_goes_grey() {
        let deposits = ResourceDeposits::from_map(&map_with([0, 0, 255, 0]), 100.0);
        for (px, kind) in [
            ([255, 0, 0, 0], R::Farming),
            ([0, 255, 0, 0], R::Forestry),
            ([0, 0, 255, 0], R::Ore),
            ([0, 0, 0, 255], R::Oil),
        ] {
            let map = map_with(px);
            let d = ResourceDeposits::from_map(&map, 100.0);
            let c = resource_cell_color(&map, &d, 3, 3).unwrap();
            assert_eq!(c[..3], resource_color(kind), "{kind:?}");
            assert!(c[3] > 0.6);
        }
        // Mixed: ore beats forest where it is richer.
        let c = resource_cell_color(&map_with([0, 100, 200, 0]), &deposits, 0, 0).unwrap();
        assert_eq!(c[..3], resource_color(R::Ore));
        // Spent: the map says ore, the deposit is empty.
        let map = map_with([0, 0, 255, 0]);
        let mut d = ResourceDeposits::from_map(&map, 10.0);
        d.take(R::Ore, 5, 5, 10);
        assert_eq!(resource_cell_color(&map, &d, 5, 5).unwrap()[..3], SPENT);
        assert_eq!(
            resource_cell_color(&map, &d, 6, 5).unwrap()[..3],
            resource_color(R::Ore)
        );
        // Nothing (or almost nothing) there: clear.
        assert!(resource_cell_color(&map_with([10, 0, 0, 0]), &d, 0, 0).is_none());
    }

    #[test]
    fn the_four_resource_colours_differ_and_the_area_tint_uses_its_resource() {
        let mut seen: Vec<[f32; 3]> = ResourceKind::ALL
            .iter()
            .map(|k| resource_color(*k))
            .collect();
        seen.dedup();
        assert_eq!(seen.len(), 4);
        let mut areas = Areas::default();
        areas.list.push(Some(cp_district::Area {
            name: "Wood".into(),
            kind: AreaKind::Industry(R::Forestry),
            level: 1,
            xp: 0,
        }));
        let mut map = AreaMap::default();
        map.set((4, 4), Some(AreaId(1)));
        map.set((5, 4), Some(AreaId(1)));
        assert_eq!(area_contour_edges(&map).len(), 6);
        let t = Terrain::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands"),
        )
        .unwrap();
        let m = area_fill_mesh(&map, &areas, &t, None, AreaFamily::Industry);
        assert_eq!(m.indices.len(), 2 * 6);
        let [r, g, b, a] = m.colors[0];
        assert_eq!([r, g, b], resource_color(R::Forestry));
        assert_eq!(a, FILL_ALPHA);
    }

    /// T-226: the four park kinds have their own colours, and each family draws only its areas.
    #[test]
    fn park_kinds_have_colours_and_a_family_draws_only_its_own_areas() {
        let mut seen: Vec<[f32; 3]> = ParkKind::ALL.iter().map(|k| park_color(*k)).collect();
        seen.dedup();
        assert_eq!(seen.len(), 4);
        let mut areas = Areas::default();
        for kind in [AreaKind::Park(ParkKind::Zoo), AreaKind::Industry(R::Ore)] {
            areas.list.push(Some(cp_district::Area {
                name: "A".into(),
                kind,
                level: 1,
                xp: 0,
            }));
        }
        let mut map = AreaMap::default();
        map.set((4, 4), Some(AreaId(1)));
        map.set((8, 4), Some(AreaId(2)));
        let t = Terrain::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands"),
        )
        .unwrap();
        let parks = area_fill_mesh(&map, &areas, &t, None, AreaFamily::Park);
        assert_eq!(parks.indices.len(), 6, "one quad: the zoo cell");
        assert_eq!(parks.colors[0][..3], park_color(ParkKind::Zoo));
        let industry = area_fill_mesh(&map, &areas, &t, None, AreaFamily::Industry);
        assert_eq!(industry.colors[0][..3], resource_color(R::Ore));
        let outline = area_contour_mesh(&map, &areas, &t, None, 1.0, AreaFamily::Park);
        let all = area_contour_mesh(&map, &areas, &t, None, 1.0, AreaFamily::Industry);
        assert!(!outline.indices.is_empty() && !all.indices.is_empty());
        assert_ne!(outline.indices.len(), 0);
    }
}
