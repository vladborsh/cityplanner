//! Map overlays (`architecture/system-design.md` §13.4): traffic load on roads, zone cells,
//! and land value / pollution heat maps draped over the terrain.
//!
//! Each overlay is its own vertex-coloured, alpha-blended mesh layer built from [`SimView`]
//! data; only the active layer is visible. Switching is a visibility flip applied the same
//! frame, so it never rebuilds geometry. Layers rebuild when their data (an `Arc` in the view)
//! changes. This uses the plain [`VertexColorMaterial`] instead of a uniform-buffer
//! material: a per-id value array would hit WebGL2's uniform block size limit on big
//! networks, and per-vertex colour needs no extra bindings.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use cp_content::ZoneType;
use cp_net::NetGraph;
use cp_sim::{ScalarGrid, SimView, ZoneOverlay};
use cp_terrain::{MAP_SIZE, Terrain};

use crate::chunk::{MeshData, terrain_mesh_height};
use crate::material::VertexColorMaterial;

mod utility;
pub use utility::{
    BuildingStatus, UtilityLayer, building_status, coverage_counts, status_color,
    utility_overlay_mesh,
};

/// Which overlay is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Overlay {
    #[default]
    Off,
    Traffic,
    Zones,
    LandValue,
    Pollution,
    /// Pipes (underground), their reach and the water / sewage state of buildings.
    Water,
    /// Power lines, their reach and the power state of buildings.
    Power,
}

impl Overlay {
    pub const ALL: [Overlay; 7] = [
        Overlay::Off,
        Overlay::Traffic,
        Overlay::Zones,
        Overlay::LandValue,
        Overlay::Pollution,
        Overlay::Water,
        Overlay::Power,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Overlay::Off => "No overlay",
            Overlay::Traffic => "Traffic",
            Overlay::Zones => "Zones",
            Overlay::LandValue => "Land value",
            Overlay::Pollution => "Pollution",
            Overlay::Water => "Water",
            Overlay::Power => "Power",
        }
    }
}

/// The active overlay. The UI writes it; the render systems read it.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayState {
    pub active: Overlay,
}

/// Height of the overlay layers above the surface they cover, metres.
const LIFT: f32 = 0.3;
/// Heat-map resolution when the sim has no grid yet.
const PLACEHOLDER_SIDE: usize = 32;
/// Colour of roads with no load data, and of every road before traffic exists.
pub const NO_DATA: [f32; 4] = [0.42, 0.46, 0.52, 0.55];

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// Green → yellow → red for traffic load `0..=1` (clamped; NaN reads as 0).
pub fn traffic_color(load: f32) -> [f32; 4] {
    ramp(
        load,
        &[
            [0.10, 0.65, 0.20, 0.8],
            [0.90, 0.80, 0.10, 0.8],
            [0.85, 0.12, 0.10, 0.8],
        ],
    )
}

/// Land value: cold blue (cheap) → white-green → warm gold (expensive).
pub fn land_value_color(v: f32) -> [f32; 4] {
    ramp(
        v,
        &[
            [0.15, 0.25, 0.70, 0.45],
            [0.30, 0.75, 0.55, 0.45],
            [0.95, 0.80, 0.20, 0.45],
        ],
    )
}

/// Pollution: transparent-looking green → brown → dark purple.
pub fn pollution_color(v: f32) -> [f32; 4] {
    ramp(
        v,
        &[
            [0.30, 0.70, 0.30, 0.25],
            [0.65, 0.50, 0.15, 0.5],
            [0.40, 0.08, 0.35, 0.7],
        ],
    )
}

fn ramp(v: f32, stops: &[[f32; 4]; 3]) -> [f32; 4] {
    let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
    if v < 0.5 {
        lerp4(stops[0], stops[1], v * 2.0)
    } else {
        lerp4(stops[1], stops[2], (v - 0.5) * 2.0)
    }
}

/// Zone colours follow the usual city-builder palette: green residential, blue commercial,
/// yellow industrial, violet office; the high-density variant is stronger.
pub fn zone_color(zone: ZoneType) -> Option<[f32; 4]> {
    Some(match zone {
        ZoneType::None => return None,
        ZoneType::ResLow => [0.35, 0.80, 0.35, 0.65],
        ZoneType::ResHigh => [0.10, 0.60, 0.15, 0.8],
        ZoneType::ComLow => [0.40, 0.60, 0.95, 0.65],
        ZoneType::ComHigh => [0.15, 0.30, 0.85, 0.8],
        ZoneType::Industrial => [0.90, 0.80, 0.20, 0.7],
        ZoneType::Office => [0.70, 0.40, 0.85, 0.7],
    })
}

/// Colour of zone cells that are valid but not painted yet: a faint outline-like tint so the
/// player sees where zoning is possible.
pub const UNPAINTED: [f32; 4] = [0.85, 0.85, 0.85, 0.18];

fn push(m: &mut MeshData, p: [f32; 3], c: [f32; 4]) -> u32 {
    let i = m.positions.len() as u32;
    m.positions.push(p);
    m.normals.push([0.0, 1.0, 0.0]);
    m.colors.push(c);
    i
}

/// Two triangles for the quad `v`, wound as given.
fn quad(m: &mut MeshData, v: [u32; 4]) {
    m.indices
        .extend_from_slice(&[v[0], v[1], v[2], v[0], v[2], v[3]]);
}

/// A quad that faces up whatever the corner order of `corners` (convex, in XZ).
fn flat(m: &mut MeshData, corners: [[f32; 3]; 4], c: [f32; 4]) {
    let area: f32 = (0..4)
        .map(|i| {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            a[0] * b[2] - b[0] * a[2]
        })
        .sum();
    let ids = corners.map(|p| push(m, p, c));
    if area > 0.0 {
        quad(m, [ids[3], ids[2], ids[1], ids[0]]);
    } else {
        quad(m, ids);
    }
}

/// Colour of invalid zone cells (blocked by a road, another block or a building): muted grey.
pub const INVALID: [f32; 4] = [0.5, 0.5, 0.5, 0.25];
/// The zone layer is rebuilt at most this often while it is on screen.
const ZONE_REBUILD_SECS: f32 = 0.25;
/// Largest vertex spacing of a zone cell's tessellation, metres: a cell is 8 m wide, the
/// terrain mesh turns every 10 m, so the drape has to be finer than the terrain.
pub const ZONE_STEP: f32 = 2.0;

/// A cell whose terrain deviates from its corners' bilinear surface by at most this (m) is
/// drawn as one quad.
const FLAT_TOLERANCE: f32 = 0.03;

/// The zone layer: every cell is a grid of quads at most [`ZONE_STEP`] wide, each vertex
/// [`LIFT`] above the *rendered* terrain ([`terrain_mesh_height`]), coloured by the painted
/// type; invalid cells are drawn in muted grey ([`INVALID`]) so the edge of the grid is solid.
pub fn zone_mesh(zones: &ZoneOverlay, terrain: &Terrain) -> MeshData {
    use bevy::math::Vec2;
    let mut m = MeshData::default();
    for cell in &zones.cells {
        let color = if cell.valid {
            zone_color(cell.zone).unwrap_or(UNPAINTED)
        } else {
            INVALID
        };
        let c = cell.corners.map(|[x, z]| Vec2::new(x, z));
        let longest = (0..4)
            .map(|i| c[i].distance(c[(i + 1) % 4]))
            .fold(0.0f32, f32::max);
        let at = |u: f32, v: f32| c[0].lerp(c[1], u).lerp(c[3].lerp(c[2], u), v);
        // Where the rendered terrain is flat or planar across the cell one quad already lies on
        // it, so only creased cells pay for the fine grid: compare the terrain with the bilinear
        // of the corner heights on a 5x5 probe.
        let corner_h = c.map(|p| terrain_mesh_height(terrain, p));
        let probe_err = (0..25)
            .map(|k| {
                let (u, v) = ((k % 5) as f32 / 4.0, (k / 5) as f32 / 4.0);
                let bil = (corner_h[0] * (1.0 - u) + corner_h[1] * u) * (1.0 - v)
                    + (corner_h[3] * (1.0 - u) + corner_h[2] * u) * v;
                (terrain_mesh_height(terrain, at(u, v)) - bil).abs()
            })
            .fold(0.0f32, f32::max);
        let n = if probe_err <= FLAT_TOLERANCE {
            1
        } else {
            ((longest / ZONE_STEP).ceil() as usize).max(1)
        };
        // Which way the corner order runs decides the winding that faces up.
        let area: f32 = (0..4)
            .map(|i| c[i].x * c[(i + 1) % 4].y - c[(i + 1) % 4].x * c[i].y)
            .sum();
        let base = m.positions.len() as u32;
        for j in 0..=n {
            let v = j as f32 / n as f32;
            for i in 0..=n {
                let u = i as f32 / n as f32;
                let p = at(u, v);
                push(
                    &mut m,
                    [p.x, terrain_mesh_height(terrain, p) + LIFT, p.y],
                    color,
                );
            }
        }
        let at = |i: usize, j: usize| base + (j * (n + 1) + i) as u32;
        for j in 0..n {
            for i in 0..n {
                let q = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
                if area > 0.0 {
                    quad(&mut m, [q[3], q[2], q[1], q[0]]);
                } else {
                    quad(&mut m, q);
                }
            }
        }
    }
    m
}

/// The traffic layer: a ribbon along every segment, coloured by `load` of that segment.
pub fn road_mesh(
    net: &NetGraph,
    load: &std::collections::BTreeMap<cp_net::SegmentId, f32>,
) -> MeshData {
    let mut m = MeshData::default();
    for (id, seg) in net.segments() {
        let color = load.get(&id).map_or(NO_DATA, |l| traffic_color(*l));
        let half = seg
            .lanes
            .iter()
            .filter_map(|&l| net.lane(l))
            .map(|l| l.offset.abs() + l.width * 0.5)
            .fold(0.0f32, f32::max)
            .max(2.0);
        let steps = ((seg.length / 8.0).ceil() as usize).clamp(2, 256);
        let mut prev: Option<[u32; 2]> = None;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let p = seg.curve.point(t);
            let tg = seg.curve.tangent(t);
            let side = bevy::math::Vec2::new(tg.z, -tg.x)
                .try_normalize()
                .unwrap_or(bevy::math::Vec2::X)
                * half;
            let y = p.y + LIFT;
            let l = push(&mut m, [p.x + side.x, y, p.z + side.y], color);
            let r = push(&mut m, [p.x - side.x, y, p.z - side.y], color);
            if let Some([pl, pr]) = prev {
                // Wind the span to face up whichever way the road runs.
                let (a, b, c, d) = (pl, l, r, pr);
                let (pa, pb, pc) = (
                    m.positions[a as usize],
                    m.positions[b as usize],
                    m.positions[c as usize],
                );
                let cross = (pb[0] - pa[0]) * (pc[2] - pa[2]) - (pb[2] - pa[2]) * (pc[0] - pa[0]);
                if cross > 0.0 {
                    quad(&mut m, [d, c, b, a]);
                } else {
                    quad(&mut m, [a, b, c, d]);
                }
            }
            prev = Some([l, r]);
        }
    }
    m
}

/// A heat map: one quad per grid cell over the whole map, draped on the terrain. `None`
/// (domain not landed yet) draws a uniform "zero" tint so the overlay is still visible.
pub fn heat_mesh(
    grid: Option<&ScalarGrid>,
    color: fn(f32) -> [f32; 4],
    terrain: &Terrain,
) -> MeshData {
    let placeholder;
    let grid = match grid {
        Some(g) => g,
        None => {
            placeholder = ScalarGrid::new(
                PLACEHOLDER_SIDE,
                vec![0.0; PLACEHOLDER_SIDE * PLACEHOLDER_SIDE],
            );
            &placeholder
        }
    };
    let n = grid.side;
    let step = MAP_SIZE / n as f32;
    let node = |ix: usize, iz: usize| {
        let (x, z) = (ix as f32 * step, iz as f32 * step);
        [x, terrain.height(bevy::math::Vec2::new(x, z)) + LIFT, z]
    };
    let mut m = MeshData::default();
    for iz in 0..n {
        for ix in 0..n {
            let c = color(grid.values[iz * n + ix]);
            flat(
                &mut m,
                [
                    node(ix, iz),
                    node(ix + 1, iz),
                    node(ix + 1, iz + 1),
                    node(ix, iz + 1),
                ],
                c,
            );
        }
    }
    m
}

// ---- Bevy side ----

pub struct OverlayPlugin;

impl Plugin for OverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OverlayState>()
            .init_resource::<OverlayLayers>()
            .add_systems(
                Update,
                (sync_overlay_meshes, apply_overlay_visibility).chain(),
            );
    }
}

/// Marks a layer entity.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayLayer(pub Overlay);

#[derive(Default)]
struct LayerSource {
    /// Pointer of the `Arc` the mesh was last built from.
    key: usize,
    entity: Option<Entity>,
    /// `Time<Real>` seconds of the last build (for throttled layers).
    built_at: f32,
}

#[derive(Resource, Default)]
struct OverlayLayers {
    traffic: LayerSource,
    zones: LayerSource,
    land: LayerSource,
    pollution: LayerSource,
    water: LayerSource,
    power: LayerSource,
    material: Option<Handle<VertexColorMaterial>>,
    terrain_key: usize,
}

fn to_bevy_mesh(m: MeshData) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, m.colors)
    .with_inserted_indices(Indices::U32(m.indices))
}

fn ptr<T: ?Sized>(a: &Arc<T>) -> usize {
    Arc::as_ptr(a).cast::<()>() as usize
}

/// Keys the heat-map layers on their optional grid (`0` while absent).
fn opt_ptr(a: &Option<Arc<ScalarGrid>>) -> usize {
    a.as_ref().map_or(0, ptr)
}

#[allow(clippy::too_many_arguments)]
fn upsert(
    src: &mut LayerSource,
    key: usize,
    which: Overlay,
    mesh: impl FnOnce() -> MeshData,
    material: &Handle<VertexColorMaterial>,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    force: bool,
) {
    if src.entity.is_some() && src.key == key && !force {
        return;
    }
    if let Some(e) = src.entity.take() {
        commands.entity(e).despawn();
    }
    src.key = key;
    let data = mesh();
    if data.indices.is_empty() {
        return;
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &data.positions {
        lo = lo.min(Vec3::from(*p));
        hi = hi.max(Vec3::from(*p));
    }
    src.entity = Some(
        commands
            .spawn((
                Mesh3d(meshes.add(to_bevy_mesh(data))),
                MeshMaterial3d(material.clone()),
                Transform::default(),
                Aabb::from_min_max(lo, hi),
                NotShadowCaster,
                Visibility::Hidden,
                OverlayLayer(which),
                Name::new(format!("overlay {}", which.label())),
            ))
            .id(),
    );
}

fn sync_overlay_meshes(
    view: Option<Res<SimView>>,
    state: Res<OverlayState>,
    time: Res<Time<Real>>,
    mut layers: ResMut<OverlayLayers>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
) {
    let Some(view) = view else { return };
    let Some(terrain) = view.terrain.as_ref() else {
        return;
    };
    let material = layers
        .material
        .get_or_insert_with(|| {
            materials.add(VertexColorMaterial::blended(0.95, 0.02).with_depth_bias(true))
        })
        .clone();
    // A different terrain invalidates every draped layer.
    let tkey = ptr(terrain);
    let terrain_changed = layers.terrain_key != tkey;
    layers.terrain_key = tkey;
    let o = &view.overlays;
    let l = &mut *layers;
    // Traffic depends on the network as well as the load values.
    let traffic_key = ptr(&view.net) ^ ptr(&o.road_load).rotate_left(17);
    upsert(
        &mut l.traffic,
        traffic_key,
        Overlay::Traffic,
        || road_mesh(&view.net, &o.road_load),
        &material,
        &mut commands,
        &mut meshes,
        false,
    );
    // The zone snapshot changes whenever a building goes up, and the fine mesh is the costly
    // one: build it only while it is shown, and at most every `ZONE_REBUILD_SECS` (a stale
    // layer is replaced on the next frame that allows it; switching it on builds at once).
    let now = time.elapsed_secs();
    let zones_key = ptr(&o.zones);
    let zones_due = l.zones.entity.is_none()
        || terrain_changed
        || (l.zones.key != zones_key && now - l.zones.built_at >= ZONE_REBUILD_SECS);
    if state.active == Overlay::Zones && zones_due {
        l.zones.built_at = now;
        upsert(
            &mut l.zones,
            zones_key,
            Overlay::Zones,
            || zone_mesh(&o.zones, terrain),
            &material,
            &mut commands,
            &mut meshes,
            true,
        );
    }
    upsert(
        &mut l.land,
        opt_ptr(&o.land_value),
        Overlay::LandValue,
        || heat_mesh(o.land_value.as_deref(), land_value_color, terrain),
        &material,
        &mut commands,
        &mut meshes,
        terrain_changed,
    );
    upsert(
        &mut l.pollution,
        opt_ptr(&o.pollution),
        Overlay::Pollution,
        || heat_mesh(o.pollution.as_deref(), pollution_color, terrain),
        &material,
        &mut commands,
        &mut meshes,
        terrain_changed,
    );
    if let Some(db) = view.content.as_deref() {
        // The utility layers depend on the network, the buildings (flags) and the zone grid
        // (lot footprints).
        let key = ptr(&view.utilities)
            ^ ptr(&view.buildings).rotate_left(13)
            ^ ptr(&view.zones).rotate_left(29);
        for (src, layer, which) in [
            (&mut l.water, UtilityLayer::Water, Overlay::Water),
            (&mut l.power, UtilityLayer::Power, Overlay::Power),
        ] {
            upsert(
                src,
                key,
                which,
                || {
                    utility_overlay_mesh(
                        layer,
                        &view.utilities,
                        &view.buildings,
                        &view.zones,
                        db,
                        terrain,
                    )
                },
                &material,
                &mut commands,
                &mut meshes,
                terrain_changed,
            );
        }
    }
}

/// Shows exactly the active layer. Runs every frame after the sync (cheap: a handful of
/// entities), so a switch and a rebuild in the same frame never show a stale layer.
fn apply_overlay_visibility(
    state: Res<OverlayState>,
    mut layers: Query<(&OverlayLayer, &mut Visibility)>,
) {
    for (layer, mut vis) in &mut layers {
        let want = if layer.0 == state.active {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
    }
}

#[cfg(test)]
mod tests;
