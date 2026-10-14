//! Road-width bands for the road ghost and the bulldozer highlight (`architecture/system-design.md`
//! §13.5): [`band_geometry`] is the pure part (headless-testable); [`sync_bands`] keeps one mesh
//! entity and rebuilds it only when the ghost or the highlighted run changes. Pipes and power
//! lines stay lines.

use std::sync::Arc;

use bevy::camera::primitives::Aabb;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use cp_net::SegmentId;
use cp_render::VertexColorMaterial;
use cp_render::chunk::MeshData;
use cp_render::road::{BAND_LIFT, band_polyline, to_mesh};
use cp_sim::SimView;

use crate::plugin::{ActiveTool, Ghost, ToolState};
use crate::road::sample_curve;

/// Valid ghost, linear RGBA.
pub const GHOST_OK: [f32; 4] = [0.2, 0.9, 0.3, 0.45];
/// Invalid ghost.
pub const GHOST_BAD: [f32; 4] = [0.95, 0.2, 0.2, 0.5];
/// Bulldozer run.
pub const HIGHLIGHT: [f32; 4] = [1.0, 0.6, 0.1, 0.55];

/// The band geometry of the road ghost (when it has a width) and of the highlighted run.
pub fn band_geometry(view: &SimView, ghost: Option<&Ghost>, run: &[SegmentId]) -> MeshData {
    let terrain = view.terrain.as_deref();
    let mut out = MeshData::default();
    let mut add = |m: MeshData| {
        let base = out.positions.len() as u32;
        out.positions.extend(m.positions);
        out.normals.extend(m.normals);
        out.colors.extend(m.colors);
        out.indices.extend(m.indices.iter().map(|i| i + base));
    };
    if let Some(g) = ghost.filter(|g| g.half_width > 0.0) {
        let colour = if g.valid { GHOST_OK } else { GHOST_BAD };
        for line in &g.polylines {
            add(band_polyline(
                line,
                g.half_width,
                terrain,
                BAND_LIFT,
                colour,
            ));
        }
    }
    for &id in run {
        let (Some(seg), Some(content)) = (view.net.segment(id), view.content.as_deref()) else {
            continue;
        };
        let half = content.road(seg.road).width * 0.5;
        let line = sample_curve(&seg.curve, 4.0);
        add(band_polyline(
            &line,
            half,
            terrain,
            BAND_LIFT + 0.05,
            HIGHLIGHT,
        ));
    }
    out
}

#[derive(Resource, Default)]
pub struct BandState {
    entity: Option<Entity>,
    material: Option<Handle<VertexColorMaterial>>,
    key: Option<BandKey>,
}

#[derive(Clone, PartialEq)]
struct BandKey {
    ghost: Option<Ghost>,
    run: Vec<SegmentId>,
    net_ptr: usize,
}

/// Rebuilds the band entity when (and only when) its inputs changed.
pub fn sync_bands(
    state: Res<ToolState>,
    view: Option<Res<SimView>>,
    mut bands: ResMut<BandState>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VertexColorMaterial>>,
) {
    let Some(view) = view else { return };
    let ghost = state
        .ghost
        .clone()
        .filter(|_| state.active == ActiveTool::Road);
    let run = if state.active == ActiveTool::Bulldozer {
        state.highlight.clone()
    } else {
        Vec::new()
    };
    let key = BandKey {
        ghost,
        run,
        net_ptr: Arc::as_ptr(&view.net) as usize,
    };
    if bands.key.as_ref() == Some(&key) {
        return;
    }
    let data = band_geometry(&view, key.ghost.as_ref(), &key.run);
    bands.key = Some(key);
    if let Some(e) = bands.entity.take() {
        commands.entity(e).despawn();
    }
    if data.indices.is_empty() {
        return;
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &data.positions {
        lo = lo.min(Vec3::from(*p));
        hi = hi.max(Vec3::from(*p));
    }
    let material = bands
        .material
        .get_or_insert_with(|| materials.add(VertexColorMaterial::blended(0.95, 0.02)))
        .clone();
    bands.entity = Some(
        commands
            .spawn((
                Mesh3d(meshes.add(to_mesh(data))),
                MeshMaterial3d(material),
                Transform::default(),
                Aabb::from_min_max(lo, hi),
                NotShadowCaster,
                Name::new("road band"),
            ))
            .id(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_content::ContentDb;
    use cp_core::curve::CubicBezier3;
    use cp_net::{Elevation, NetGraph};
    use cp_sim::SimRunner;

    fn sim() -> (SimRunner, SegmentId, f32) {
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let road = db.road_id("large_6l").unwrap();
        let (def, width) = (db.road(road).clone(), db.road(road).width);
        let mut sim = SimRunner::new(1);
        sim.set_content(Arc::new(db));
        let seg = sim
            .world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(500.0, 0.0, 500.0), Vec3::new(500.0, 0.0, 600.0)),
                Elevation::Ground,
            )
            .unwrap();
        sim.tick_once();
        (sim, seg, width)
    }

    fn x_extent(m: &MeshData) -> f32 {
        m.positions
            .iter()
            .map(|p| (p[0] - 500.0).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn the_highlight_band_is_as_wide_as_the_road_and_orange() {
        let (sim, seg, width) = sim();
        let m = band_geometry(&sim.view(), None, &[seg]);
        assert!(!m.indices.is_empty());
        assert!((x_extent(&m) - width * 0.5).abs() < 1e-3);
        assert!(m.colors.iter().all(|c| *c == HIGHLIGHT));
    }

    #[test]
    fn the_ghost_band_follows_its_half_width_and_validity() {
        let (sim, _, _) = sim();
        let line = vec![Vec3::new(500.0, 0.0, 500.0), Vec3::new(500.0, 0.0, 560.0)];
        let ghost = |valid, half_width| Ghost {
            polylines: vec![line.clone()],
            valid,
            cost: None,
            errors: vec![],
            half_width,
            snap: Default::default(),
            warnings: Vec::new(),
            chevrons: Vec::new(),
        };
        let view = sim.view();
        let ok = band_geometry(&view, Some(&ghost(true, 6.0)), &[]);
        assert!((x_extent(&ok) - 6.0).abs() < 1e-3);
        assert!(ok.colors.iter().all(|c| *c == GHOST_OK));
        let bad = band_geometry(&view, Some(&ghost(false, 14.0)), &[]);
        assert!((x_extent(&bad) - 14.0).abs() < 1e-3);
        assert!(bad.colors.iter().all(|c| *c == GHOST_BAD));
        // A line-only ghost (no width) and no run draw nothing.
        assert!(
            band_geometry(&view, Some(&ghost(true, 0.0)), &[])
                .indices
                .is_empty()
        );
    }
}
