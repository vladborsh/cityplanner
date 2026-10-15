//! Which tiles the player owns (`architecture/system-design.md` §5, §13.4): a light line along
//! the outer contour of the open tiles, and a translucent dark veil over the tiles that are
//! not open. Both are draped on the terrain the camera sees ([`drape_height`]) and rebuilt
//! only when the ownership, the camera's 500 m cell or the line width step changes.

use bevy::camera::primitives::Aabb;
use bevy::light::NotShadowCaster;
use bevy::math::Vec2;
use bevy::prelude::*;
use cp_sim::SimView;
use cp_terrain::{TILE_SIZE, TILES_PER_SIDE, Terrain, TileOwnership};

use super::{OverlayLayers, drape_height, push, ribbon_with, to_bevy_mesh};
use crate::chunk::MeshData;

/// Longest ribbon piece of the border, metres.
pub const BORDER_STEP: f32 = 2.0;
/// Height of the border above the terrain, metres.
pub const BORDER_LIFT: f32 = 0.5;
/// Border width at the nearest zoom; it grows with the camera height to stay a few pixels.
pub const BORDER_WIDTH: f32 = 3.5;
/// Height of the veil above the terrain, metres.
pub const SHADE_LIFT: f32 = 0.3;
/// Vertex spacing of the veil, metres (per tile: `ceil(TILE_SIZE / SHADE_STEP)` cells a side).
pub const SHADE_STEP: f32 = 40.0;
pub const BORDER_COLOR: [f32; 4] = [0.96, 0.95, 0.80, 0.95];
pub const SHADE_COLOR: [f32; 4] = [0.0, 0.0, 0.04, 0.25];

/// The outer contour of the open tiles as oriented segments `(from, to)`: every side of an open
/// tile that borders a tile which is not open. Sides on the map edge border no tile and are not
/// part of it; sides between two open tiles are not either.
pub fn border_edges(tiles: &TileOwnership) -> Vec<(Vec2, Vec2)> {
    let s = TILE_SIZE;
    let open = |tx: i32, tz: i32| tx >= 0 && tz >= 0 && tiles.is_open(tx as usize, tz as usize);
    let inside = |tx: i32, tz: i32| {
        (0..TILES_PER_SIDE as i32).contains(&tx) && (0..TILES_PER_SIDE as i32).contains(&tz)
    };
    let mut out = Vec::new();
    for tz in 0..TILES_PER_SIDE as i32 {
        for tx in 0..TILES_PER_SIDE as i32 {
            if !open(tx, tz) {
                continue;
            }
            let (x0, z0) = (tx as f32 * s, tz as f32 * s);
            let (x1, z1) = (x0 + s, z0 + s);
            // (neighbour, side from, side to)
            for ((nx, nz), a, b) in [
                ((tx, tz - 1), Vec2::new(x0, z0), Vec2::new(x1, z0)),
                ((tx + 1, tz), Vec2::new(x1, z0), Vec2::new(x1, z1)),
                ((tx, tz + 1), Vec2::new(x1, z1), Vec2::new(x0, z1)),
                ((tx - 1, tz), Vec2::new(x0, z1), Vec2::new(x0, z0)),
            ] {
                if inside(nx, nz) && !open(nx, nz) {
                    out.push((a, b));
                }
            }
        }
    }
    out
}

/// The border line: a ribbon of width `w` along every contour edge, cut every [`BORDER_STEP`],
/// each vertex [`BORDER_LIFT`] above the terrain the camera at `cam` sees there. Each ribbon
/// reaches `w / 2` past its ends so the corners close.
pub fn border_mesh(
    tiles: &TileOwnership,
    terrain: &Terrain,
    cam: Option<Vec3>,
    w: f32,
) -> MeshData {
    let mut m = MeshData::default();
    let height = |q: Vec2| drape_height(terrain, cam, q) + BORDER_LIFT;
    for (a, b) in border_edges(tiles) {
        let d = (b - a).normalize_or_zero();
        ribbon_with(
            &mut m,
            a - d * (w * 0.5),
            b + d * (w * 0.5),
            w,
            BORDER_COLOR,
            BORDER_STEP,
            &height,
        );
    }
    m
}

/// The veil over every tile that is not open: a grid of at most [`SHADE_STEP`] cells, each vertex
/// [`SHADE_LIFT`] above the highest terrain around it (itself and the four neighbours half a
/// step away), so the coarse grid does not sink into creases.
pub fn shade_mesh(tiles: &TileOwnership, terrain: &Terrain, cam: Option<Vec3>) -> MeshData {
    let n = (TILE_SIZE / SHADE_STEP).ceil() as usize;
    let step = TILE_SIZE / n as f32;
    let mut m = MeshData::default();
    for tz in 0..TILES_PER_SIDE {
        for tx in 0..TILES_PER_SIDE {
            if tiles.is_open(tx, tz) {
                continue;
            }
            let (ox, oz) = (tx as f32 * TILE_SIZE, tz as f32 * TILE_SIZE);
            let base = m.positions.len() as u32;
            for j in 0..=n {
                for i in 0..=n {
                    let p = Vec2::new(ox + i as f32 * step, oz + j as f32 * step);
                    let h = [Vec2::ZERO, Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y]
                        .iter()
                        .map(|o| drape_height(terrain, cam, p + *o * (step * 0.5)))
                        .fold(f32::MIN, f32::max);
                    push(&mut m, [p.x, h + SHADE_LIFT, p.y], SHADE_COLOR);
                }
            }
            let at = |i: usize, j: usize| base + (j * (n + 1) + i) as u32;
            for j in 0..n {
                for i in 0..n {
                    let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1));
                    m.indices.extend_from_slice(&[a, c, b, b, c, d]);
                }
            }
        }
    }
    m
}

/// Half-octave step of the border width at camera height `y`: [`BORDER_WIDTH`] up to 1.75 km,
/// then growing (0.2 % of the height, at most 60 m) so the line stays a few pixels wide.
pub fn border_width_step(y: f32) -> i32 {
    ((y * 0.002).clamp(BORDER_WIDTH, 60.0).log2() * 2.0).round() as i32
}

/// Ownership, camera cell, line width step and terrain pointer a build used.
type TileKey = (TileOwnership, (i32, i32, i32), i32, usize);

/// The two tile layers' entities and what they were built from.
#[derive(Resource, Default)]
pub struct TileLayers {
    border: Option<Entity>,
    shade: Option<Entity>,
    key: Option<TileKey>,
}

/// Marks the tile border and veil entities.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileLayer {
    Border,
    Shade,
}

fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    material: &Handle<crate::material::VertexColorMaterial>,
    data: MeshData,
    which: TileLayer,
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
                which,
                Name::new(format!("tiles {which:?}")),
            ))
            .id(),
    )
}

pub(super) fn sync_tile_layers(
    view: Option<Res<SimView>>,
    camera: Query<&Transform, With<Camera3d>>,
    overlay: Res<OverlayLayers>,
    mut layers: ResMut<TileLayers>,
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
    let bucket = cam.map_or((0, 0, 0), |c| {
        (
            (c.x / 500.0).floor() as i32,
            (c.y / 500.0).floor() as i32,
            (c.z / 500.0).floor() as i32,
        )
    });
    // Line width: BORDER_WIDTH near the ground, growing with the camera height.
    let step = cam.map_or(0, |c| border_width_step(c.y));
    let key = (
        view.tiles,
        bucket,
        step,
        std::sync::Arc::as_ptr(terrain).cast::<()>() as usize,
    );
    if layers.key.as_ref() == Some(&key) {
        return;
    }
    layers.key = Some(key);
    for e in [layers.border.take(), layers.shade.take()]
        .into_iter()
        .flatten()
    {
        commands.entity(e).despawn();
    }
    let w = (step as f32 * 0.5).exp2();
    layers.border = spawn(
        &mut commands,
        &mut meshes,
        material,
        border_mesh(&view.tiles, terrain, cam, w),
        TileLayer::Border,
    );
    layers.shade = spawn(
        &mut commands,
        &mut meshes,
        material,
        shade_mesh(&view.tiles, terrain, cam),
        TileLayer::Shade,
    );
}
