//! The land overlay (T-152): a tint over every tile of the map by what it is to the player —
//! owned, buyable, locked behind a milestone, or out of reach. The prices are drawn by the UI.

use bevy::camera::primitives::Aabb;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use cp_sim::SimView;
use cp_sim::tiles::{TileState, tile_state};
use cp_terrain::{TILE_SIZE, TILES_PER_SIDE, Terrain};

use super::tiles::{SHADE_LIFT, SHADE_STEP};
use super::{Overlay, OverlayLayers, OverlayState, drape_height, push, to_bevy_mesh};
use crate::chunk::MeshData;

pub const OWNED_COLOR: [f32; 4] = [0.20, 0.70, 0.30, 0.22];
pub const BUYABLE_COLOR: [f32; 4] = [0.95, 0.78, 0.20, 0.38];
pub const LOCKED_COLOR: [f32; 4] = [0.45, 0.48, 0.55, 0.38];
pub const FAR_COLOR: [f32; 4] = [0.10, 0.10, 0.15, 0.30];

/// The tint of a tile state.
pub fn tile_color(state: TileState) -> [f32; 4] {
    match state {
        TileState::Owned => OWNED_COLOR,
        TileState::Buyable(_) => BUYABLE_COLOR,
        TileState::NeedsMilestone => LOCKED_COLOR,
        TileState::Far => FAR_COLOR,
    }
}

/// One draped grid per tile, coloured by `color(tx, tz)`.
pub fn land_mesh(
    terrain: &Terrain,
    cam: Option<Vec3>,
    color: impl Fn(usize, usize) -> [f32; 4],
) -> MeshData {
    let n = (TILE_SIZE / SHADE_STEP).ceil() as usize;
    let step = TILE_SIZE / n as f32;
    let mut m = MeshData::default();
    for tz in 0..TILES_PER_SIDE {
        for tx in 0..TILES_PER_SIDE {
            let c = color(tx, tz);
            let (ox, oz) = (tx as f32 * TILE_SIZE, tz as f32 * TILE_SIZE);
            let base = m.positions.len() as u32;
            for j in 0..=n {
                for i in 0..=n {
                    let p = Vec2::new(ox + i as f32 * step, oz + j as f32 * step);
                    let h = [Vec2::ZERO, Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y]
                        .iter()
                        .map(|o| drape_height(terrain, cam, p + *o * (step * 0.5)))
                        .fold(f32::MIN, f32::max);
                    // Above the veil, so the tint is not hidden by it.
                    push(&mut m, [p.x, h + SHADE_LIFT + 0.1, p.y], c);
                }
            }
            let at = |i: usize, j: usize| base + (j * (n + 1) + i) as u32;
            for j in 0..n {
                for i in 0..n {
                    let (a, b, c2, d) = (at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1));
                    m.indices.extend_from_slice(&[a, c2, b, b, c2, d]);
                }
            }
        }
    }
    m
}

/// Visible, the tile states (prices zeroed), camera cell and terrain pointer a build used.
type LandKey = (
    bool,
    [TileState; TILES_PER_SIDE * TILES_PER_SIDE],
    (i32, i32, i32),
    usize,
);

/// The layer's entity and what it was built from.
#[derive(Resource, Default)]
pub struct LandLayer {
    entity: Option<Entity>,
    key: Option<LandKey>,
}

#[derive(Component)]
pub struct LandTint;

pub(super) fn sync_land_layer(
    view: Option<Res<SimView>>,
    camera: Query<&Transform, With<Camera3d>>,
    overlay: Res<OverlayLayers>,
    state: Res<OverlayState>,
    mut layer: ResMut<LandLayer>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let (Some(view), Some(material)) = (view, overlay.material.as_ref()) else {
        return;
    };
    let Some(terrain) = view.terrain.as_ref() else {
        return;
    };
    let visible = state.active == Overlay::Land;
    let cam = camera.single().ok().map(|t| t.translation);
    let bucket = cam.map_or((0, 0, 0), |c| {
        (
            (c.x / 500.0).floor() as i32,
            (c.y / 500.0).floor() as i32,
            (c.z / 500.0).floor() as i32,
        )
    });
    let states: [TileState; TILES_PER_SIDE * TILES_PER_SIDE] = std::array::from_fn(|i| {
        let (tx, tz) = ((i % TILES_PER_SIDE) as u8, (i / TILES_PER_SIDE) as u8);
        // Only the colour matters for the key: a price change does not rebuild.
        match tile_state(&view, (tx, tz)) {
            TileState::Buyable(_) => TileState::Buyable(cp_core::Cost::ZERO),
            s => s,
        }
    });
    let key = (
        visible,
        states,
        bucket,
        std::sync::Arc::as_ptr(terrain).cast::<()>() as usize,
    );
    if layer.key.as_ref() == Some(&key) {
        return;
    }
    layer.key = Some(key);
    if let Some(e) = layer.entity.take() {
        commands.entity(e).despawn();
    }
    if !visible {
        return;
    }
    let data = land_mesh(terrain, cam, |tx, tz| {
        tile_color(states[tz * TILES_PER_SIDE + tx])
    });
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &data.positions {
        lo = lo.min(Vec3::from(*p));
        hi = hi.max(Vec3::from(*p));
    }
    layer.entity = Some(
        commands
            .spawn((
                Mesh3d(meshes.add(to_bevy_mesh(data))),
                MeshMaterial3d(material.clone()),
                Transform::default(),
                Aabb::from_min_max(lo, hi),
                NotShadowCaster,
                Visibility::Visible,
                LandTint,
                Name::new("land tint"),
            ))
            .id(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tile_gets_a_grid_in_its_colour() {
        let t = Terrain::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands"),
        )
        .unwrap();
        let m = land_mesh(
            &t,
            None,
            |tx, _| if tx == 0 { OWNED_COLOR } else { FAR_COLOR },
        );
        let n = (TILE_SIZE / SHADE_STEP).ceil() as usize;
        assert_eq!(m.indices.len(), TILES_PER_SIDE * TILES_PER_SIDE * n * n * 6);
        assert_eq!(m.colors[0], OWNED_COLOR);
    }
}
