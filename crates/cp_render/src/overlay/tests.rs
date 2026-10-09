use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_terrain::Terrain;
use cp_zone::{ZoneCellRef, ZoneGrid};

use super::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn terrain() -> Arc<Terrain> {
    Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap())
}

/// Two crossing 90 m streets near the map centre, zone blocks derived, every other cell of
/// the first block painted.
fn city() -> SimRunner {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain());
    sim.set_content(Arc::clone(&content));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        for (a, b) in [
            ((5000.0, 5000.0), (5090.0, 5000.0)),
            ((5045.0, 4950.0), (5045.0, 5050.0)),
        ] {
            g.add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1)),
                Elevation::Ground,
            )
            .unwrap();
        }
    }
    sim.tick_once();
    let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
    let (id, n) = grid
        .blocks()
        .map(|(i, b)| (i, b.cell_count()))
        .next()
        .unwrap();
    for cell in (0..n).step_by(2) {
        grid.set_zone(
            ZoneCellRef {
                block: id,
                cell: cell as u16,
            },
            ZoneType::ResLow,
        );
    }
    sim
}

/// Every triangle's normal points up.
fn faces_up(m: &MeshData) -> bool {
    m.indices.chunks(3).all(|t| {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(m.positions[i as usize]));
        (b - a).cross(c - a).y > 0.0
    })
}

#[test]
fn ramps_hit_their_end_stops_clamp_and_ignore_nan() {
    for f in [traffic_color, land_value_color, pollution_color] {
        assert_eq!(f(-3.0), f(0.0));
        assert_eq!(f(7.0), f(1.0));
        assert_eq!(f(f32::NAN), f(0.0));
        assert_ne!(f(0.0), f(1.0));
        // Continuous: neighbouring inputs give neighbouring colours.
        let (a, b) = (f(0.499), f(0.501));
        assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 0.02));
    }
    // Traffic goes green -> red.
    let (low, high) = (traffic_color(0.0), traffic_color(1.0));
    assert!(low[1] > low[0] && high[0] > high[1]);
}

#[test]
fn every_painted_zone_type_has_its_own_colour_and_none_has_none() {
    use ZoneType::*;
    let painted = [ResLow, ResHigh, ComLow, ComHigh, Industrial, Office];
    let colours: Vec<_> = painted.iter().map(|z| zone_color(*z).unwrap()).collect();
    for (i, a) in colours.iter().enumerate() {
        assert!(colours[i + 1..].iter().all(|b| a != b), "{painted:?}");
    }
    assert_eq!(zone_color(None), Option::None);
}

#[test]
fn overlay_state_defaults_to_off_and_lists_every_overlay_once() {
    assert_eq!(OverlayState::default().active, Overlay::Off);
    let mut all = Overlay::ALL.to_vec();
    all.sort_by_key(|o| o.label());
    all.dedup();
    assert_eq!(all.len(), 5);
}

#[test]
fn zone_layer_covers_exactly_the_valid_cells() {
    let sim = city();
    let view = sim.view();
    let t = view.terrain.as_ref().unwrap();
    let grid = sim.world().resource::<ZoneGrid>();
    let total: usize = grid.blocks().map(|(_, b)| b.cell_count()).sum();
    assert!(
        total > grid.valid_cells(),
        "the crossing invalidates some cells"
    );

    let m = zone_mesh(&view.overlays.zones, t);
    assert_eq!(view.overlays.zones.cells.len(), total);
    assert_eq!(m.positions.len(), grid.valid_cells() * 4);
    assert_eq!(m.triangle_count(), grid.valid_cells() * 2);
    assert!(faces_up(&m));

    // Painted cells get their zone colour, unpainted valid ones the faint tint.
    let painted = zone_color(ZoneType::ResLow).unwrap();
    let n_painted = m.colors.iter().filter(|c| **c == painted).count() / 4;
    let n_valid_painted = view
        .overlays
        .zones
        .cells
        .iter()
        .filter(|c| c.valid && c.zone == ZoneType::ResLow)
        .count();
    assert!(n_valid_painted > 0);
    assert_eq!(n_painted, n_valid_painted);
    assert!(m.colors.contains(&UNPAINTED));
    // The layer lies on the ground.
    assert!(
        m.positions
            .iter()
            .all(|p| { (p[1] - t.height(bevy_math::Vec2::new(p[0], p[2])) - LIFT).abs() < 1e-3 })
    );
}

#[test]
fn the_zone_snapshot_is_shared_until_zoning_changes() {
    let mut sim = city();
    let (a, b) = (sim.view(), sim.view());
    assert!(Arc::ptr_eq(&a.overlays.zones, &b.overlays.zones));
    sim.tick_once();
    assert!(Arc::ptr_eq(&a.overlays.zones, &sim.view().overlays.zones));
    {
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let (id, _) = grid
            .blocks()
            .map(|(i, b)| (i, b.cell_count()))
            .next()
            .unwrap();
        grid.set_zone(ZoneCellRef { block: id, cell: 1 }, ZoneType::Office);
    }
    assert!(!Arc::ptr_eq(&a.overlays.zones, &sim.view().overlays.zones));
}

#[test]
fn traffic_layer_has_a_ribbon_per_segment_coloured_by_load() {
    let sim = city();
    let view = sim.view();
    let ids: Vec<_> = view.net.segments().map(|(id, _)| id).collect();
    assert!(ids.len() >= 2);

    // No data yet: everything is the neutral colour.
    let none = road_mesh(&view.net, &BTreeMap::new());
    assert!(!none.positions.is_empty() && faces_up(&none));
    assert!(none.colors.iter().all(|c| *c == NO_DATA));

    // One jammed segment, the rest empty.
    let load: BTreeMap<_, _> = ids
        .iter()
        .map(|id| (*id, 0.0))
        .chain([(ids[0], 1.0)])
        .collect();
    let m = road_mesh(&view.net, &load);
    assert_eq!(m.positions.len(), none.positions.len());
    let red = traffic_color(1.0);
    let green = traffic_color(0.0);
    assert!(m.colors.contains(&red) && m.colors.contains(&green));
    assert!(m.colors.iter().all(|c| *c == red || *c == green));
}

#[test]
fn heat_layer_drapes_a_grid_and_a_missing_grid_draws_the_placeholder() {
    let t = terrain();
    let g = ScalarGrid::new(4, (0..16).map(|i| i as f32 / 15.0).collect());
    let m = heat_mesh(Some(&g), land_value_color, &t);
    assert_eq!(m.triangle_count(), 4 * 4 * 2);
    assert!(faces_up(&m));
    assert_eq!(m.colors[0], land_value_color(0.0));
    assert_eq!(*m.colors.last().unwrap(), land_value_color(1.0));
    // Spans the whole map.
    let max_x = m.positions.iter().map(|p| p[0]).fold(0.0, f32::max);
    assert!((max_x - MAP_SIZE).abs() < 1.0);

    let p = heat_mesh(None, pollution_color, &t);
    assert_eq!(p.triangle_count(), PLACEHOLDER_SIDE * PLACEHOLDER_SIDE * 2);
    assert!(p.colors.iter().all(|c| *c == pollution_color(0.0)));
}

/// An app with the visibility system and one hidden entity per layer.
fn layer_app() -> (App, Vec<(Overlay, Entity)>) {
    let mut app = App::new();
    app.init_resource::<OverlayState>()
        .add_systems(Update, apply_overlay_visibility);
    let layers = [
        Overlay::Traffic,
        Overlay::Zones,
        Overlay::LandValue,
        Overlay::Pollution,
    ]
    .map(|o| {
        (
            o,
            app.world_mut()
                .spawn((OverlayLayer(o), Visibility::Hidden))
                .id(),
        )
    })
    .to_vec();
    (app, layers)
}

fn visible(app: &App, layers: &[(Overlay, Entity)]) -> Vec<Overlay> {
    layers
        .iter()
        .filter(|(_, e)| *app.world().get::<Visibility>(*e).unwrap() == Visibility::Visible)
        .map(|(o, _)| *o)
        .collect()
}

#[test]
fn switching_overlays_flips_exactly_one_layer_within_one_update() {
    let (mut app, layers) = layer_app();
    app.update();
    assert!(visible(&app, &layers).is_empty(), "off shows nothing");
    for o in Overlay::ALL {
        app.world_mut().resource_mut::<OverlayState>().active = o;
        app.update(); // one frame
        let want: Vec<_> = if o == Overlay::Off { vec![] } else { vec![o] };
        assert_eq!(visible(&app, &layers), want, "{o:?}");
    }
}
