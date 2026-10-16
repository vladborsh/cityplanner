use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_sim::{SimRunner, ZoneCellView};
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
    assert_eq!(all.len(), 16);
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
    // Every cell is drawn (invalid ones muted grey); flat ones as a single quad.
    assert!(m.triangle_count() >= total * 2);
    assert!(faces_up(&m));
    let invalid = total - grid.valid_cells();
    let n_grey = m.colors.iter().filter(|c| **c == INVALID).count();
    assert!(n_grey > 0, "{n_grey} grey vertices");
    assert!(n_grey >= invalid * 4);

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
    // The layer lies on the rendered terrain.
    assert!(m.positions.iter().all(|p| {
        let h = terrain_mesh_height(t, bevy_math::Vec2::new(p[0], p[2]));
        (p[1] - h - LIFT).abs() < 1e-3
    }));
}

/// A steep, bumpy synthetic slope: every vertex of every cell lies at least 0.25 m above the
/// rendered terrain, and the 2 m chord between vertices never dips under it by more than the lift.
#[test]
fn zone_cells_on_a_slope_stay_above_the_terrain_mesh() {
    use cp_terrain::{GRID, MapMeta};
    let meta = MapMeta {
        name: "t".into(),
        sea_level: 0.0,
        start_tile: (2, 2),
        external_connections: vec![],
        entry_road: None,
    };
    let h: Vec<u16> = (0..GRID * GRID)
        .map(|k| {
            let (x, z) = (k % GRID, k / GRID);
            (2000 + x * 40 + (x * 7 + z * 13) % 90) as u16
        })
        .collect();
    let t = Terrain::from_samples(meta, &h, &vec![0; GRID * GRID]).unwrap();
    // Cells straddling node lines and the cell diagonals, rotated against the 10 m grid.
    let zones = ZoneOverlay {
        cells: (0..40)
            .map(|k| {
                let o = bevy_math::Vec2::new(5003.0 + k as f32 * 7.3, 5004.0 + k as f32 * 3.1);
                let (u, v) = (
                    bevy_math::Vec2::new(0.8, 0.6) * 8.0,
                    bevy_math::Vec2::new(-0.6, 0.8) * 8.0,
                );
                let c = [o, o + u, o + u + v, o + v];
                ZoneCellView {
                    corners: c.map(|p| [p.x, p.y]),
                    zone: ZoneType::ResLow,
                    valid: true,
                }
            })
            .collect(),
    };
    let m = zone_mesh(&zones, &t);
    assert!(!m.positions.is_empty());
    // Bumpy ground needs the fine grid (flat cells stay a single quad, see the city test).
    assert!(m.triangle_count() > zones.cells.len() * 2 * 4);
    for p in &m.positions {
        let h = terrain_mesh_height(&t, bevy_math::Vec2::new(p[0], p[2]));
        assert!(p[1] >= h + 0.25, "{p:?} vs {h}");
    }
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

/// An app with the visibility system, one hidden entity per layer and the zone outlines.
fn layer_app() -> (App, Vec<(Overlay, Entity)>, Entity) {
    let mut app = App::new();
    app.init_resource::<OverlayState>()
        .add_systems(Update, apply_overlay_visibility);
    let layers = [
        Overlay::Traffic,
        Overlay::Zones,
        Overlay::LandValue,
        Overlay::Pollution,
        Overlay::Water,
        Overlay::Power,
        Overlay::Garbage,
        Overlay::Health,
        Overlay::Deathcare,
        Overlay::Fire,
        Overlay::Police,
        Overlay::Education,
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
    let outlines = app
        .world_mut()
        .spawn((
            OverlayLayer(Overlay::Zones),
            ZoneOutlines,
            Visibility::Hidden,
        ))
        .id();
    (app, layers, outlines)
}

fn visible(app: &App, layers: &[(Overlay, Entity)]) -> Vec<Overlay> {
    layers
        .iter()
        .filter(|(_, e)| *app.world().get::<Visibility>(*e).unwrap() == Visibility::Visible)
        .map(|(o, _)| *o)
        .collect()
}

#[test]
fn switching_overlays_shows_that_layer_and_the_zone_grid_stays_on() {
    let (mut app, layers, outlines) = layer_app();
    for o in Overlay::ALL {
        app.world_mut().resource_mut::<OverlayState>().active = o;
        app.update(); // one frame
        // The district and land overlays are their own layers, not one of these.
        let mut want: Vec<_> = if matches!(
            o,
            Overlay::Off | Overlay::Districts | Overlay::Land | Overlay::Transit
        ) {
            vec![]
        } else {
            vec![o]
        };
        // BUG-026: the zone fill is on with every overlay that does not recolour the ground.
        if zone_fill_visible(o) && !want.contains(&Overlay::Zones) {
            want.push(Overlay::Zones);
        }
        let mut got = visible(&app, &layers);
        got.sort_by_key(|o| Overlay::ALL.iter().position(|a| a == o));
        want.sort_by_key(|o| Overlay::ALL.iter().position(|a| a == o));
        assert_eq!(got, want, "{o:?}");
        assert_eq!(
            *app.world().get::<Visibility>(outlines).unwrap(),
            Visibility::Visible,
            "outlines with {o:?}"
        );
    }
    for (o, on) in [
        (Overlay::Off, true),
        (Overlay::Zones, true),
        (Overlay::Traffic, true),
        (Overlay::LandValue, false),
        (Overlay::Pollution, false),
        (Overlay::Water, false),
        (Overlay::Power, false),
    ] {
        assert_eq!(zone_fill_visible(o), on, "{o:?}");
    }
}

// ---- water / power overlays ----

mod utility_overlays {
    use super::*;
    use crate::overlay::utility::{LINE, PIPE, POWER_BAND, WATER_BAND};
    use cp_building::{AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingState, Lot};
    use cp_content::UtilityKind;
    use cp_core::{Id, Tick};
    use cp_sim::SimCommand;
    use cp_utility::UtilityCommand;

    fn put(sim: &mut SimRunner, def: &str, x: f32, z: f32, lot: Lot) {
        let db = sim.view().content.unwrap();
        sim.world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(Building {
                def: db.building_id(def).unwrap(),
                footprint: cp_building::Footprint::Lot(lot),
                level: 1,
                state: BuildingState::Active,
                ready_at: Tick(0),
                pos: bevy_math::Vec2::new(x, z),
                rot: 0.0,
                access: Some(AccessPoint {
                    segment: Id::new(0, 0),
                    t: 0.0,
                }),
                flags: BuildingFlags::default(),
                level_progress: 0.0,
            });
    }

    fn build(sim: &mut SimRunner, kind: UtilityKind, path: &[(f32, f32)]) {
        let path = path
            .iter()
            .map(|p| bevy_math::Vec2::new(p.0, p.1))
            .collect();
        sim.push_command(
            SimCommand::Utility(UtilityCommand::BuildLink { kind, path }),
            sim.tick(),
        );
        sim.apply_due_commands();
    }

    /// A wind turbine (300 power) and 205 houses (1.5 each) beside a power line: five do not fit.
    /// The first two houses stand on real zone cells, so the overlay has quads for them.
    fn district() -> SimRunner {
        let mut sim = city();
        let block = sim
            .world()
            .resource::<ZoneGrid>()
            .blocks()
            .map(|(id, _)| id)
            .next()
            .unwrap();
        let lot = |block, col| Lot {
            block,
            col,
            w: 1,
            d: 1,
        };
        put(
            &mut sim,
            "wind_turbine",
            5010.0,
            5030.0,
            lot(Id::new(99, 0), 0),
        );
        for i in 0..205 {
            let real = if i < 2 { block } else { Id::new(99, 0) };
            put(
                &mut sim,
                "house_low_1",
                5000.0 + (i % 40) as f32 * 2.0,
                5035.0 + (i / 40) as f32 * 2.0,
                lot(real, i as u16),
            );
        }
        build(
            &mut sim,
            UtilityKind::Power,
            &[(5000.0, 5020.0), (5090.0, 5020.0)],
        );
        build(
            &mut sim,
            UtilityKind::Water,
            &[(5000.0, 5060.0), (5090.0, 5060.0)],
        );
        for _ in 0..70 {
            sim.tick_once();
        }
        sim
    }

    #[test]
    fn a_powered_district_shows_supplied_sources_and_the_deficit() {
        let sim = district();
        let view = sim.view();
        let db = view.content.as_deref().unwrap();
        let status = |layer| -> Vec<_> {
            view.buildings
                .iter()
                .map(|(_, b)| building_status(layer, db, b))
                .collect()
        };
        let power = status(UtilityLayer::Power);
        let count = |s| power.iter().filter(|x| **x == Some(s)).count();
        assert_eq!(count(BuildingStatus::Source), 1);
        assert_eq!(count(BuildingStatus::Supplied), 200);
        assert_eq!(count(BuildingStatus::NoPower), 5, "the deficit is visible");

        // The deficit cuts the highest ids first: every unpowered building has a higher id
        // than every supplied house.
        let ids = |want| -> Vec<_> {
            view.buildings
                .iter()
                .filter(|(_, b)| building_status(UtilityLayer::Power, db, b) == Some(want))
                .map(|(id, _)| id)
                .collect()
        };
        let (cut, fed) = (ids(BuildingStatus::NoPower), ids(BuildingStatus::Supplied));
        assert!(cut.iter().min() > fed.iter().max(), "{cut:?} vs {fed:?}");

        // Water: no pump anywhere, so every house lacks water and nobody is a source.
        let water = status(UtilityLayer::Water);
        assert!(
            water
                .iter()
                .filter_map(|s| *s)
                .all(|s| s == BuildingStatus::NoWater)
        );
        assert_eq!(water.iter().flatten().count(), 205);
    }

    #[test]
    fn the_layers_hold_the_network_the_reach_band_and_the_state_colours() {
        let sim = district();
        let view = sim.view();
        let (db, t) = (
            view.content.as_deref().unwrap(),
            view.terrain.as_ref().unwrap(),
        );
        let mesh = |layer| {
            utility_overlay_mesh(layer, &view.utilities, &view.buildings, &view.zones, db, t)
        };

        let power = mesh(UtilityLayer::Power);
        assert!(faces_up(&power));
        assert!(power.colors.contains(&LINE) && power.colors.contains(&POWER_BAND));
        assert!(!power.colors.contains(&PIPE), "no pipes on the power layer");
        // The deficit shows red, the supplied house green (houses 0 and 1 have real cells).
        assert!(
            power
                .colors
                .contains(&status_color(UtilityLayer::Power, BuildingStatus::Supplied))
        );
        // The band reaches `line_reach_m` either side of the line.
        let line_reach = sim.view().content.unwrap().balance().networks.line_reach_m;
        let z_max = power
            .positions
            .iter()
            .map(|p| p[2])
            .fold(f32::MIN, f32::max);
        assert!((z_max - (5020.0 + line_reach)).abs() < 1.0, "{z_max}");

        // The water layer is the pipe, not the power line; pipes are drawn here only.
        let water = mesh(UtilityLayer::Water);
        assert!(faces_up(&water));
        assert!(water.colors.contains(&PIPE) && water.colors.contains(&WATER_BAND));
        assert!(!water.colors.contains(&LINE));
        assert!(
            water
                .colors
                .contains(&status_color(UtilityLayer::Water, BuildingStatus::NoWater)),
            "houses without a pump are marked"
        );
    }

    /// BUG-036: a free-standing building (a wind turbine) has no lot cells, yet the Power layer
    /// marks its rectangle in its state colour.
    #[test]
    fn a_free_standing_building_is_marked_by_its_rectangle() {
        let sim = district();
        let view = sim.view();
        let t = view.terrain.as_ref().unwrap();
        let mut b = view.buildings.iter().next().unwrap().1.clone();
        b.footprint = cp_building::Footprint::Free { w: 2, d: 2 };
        let c = status_color(UtilityLayer::Power, BuildingStatus::NoPower);
        let mut m = crate::chunk::MeshData::default();
        super::utility::lot_quads(&mut m, &b, &view.zones, t, c);
        assert_eq!(m.positions.len(), 4, "one quad");
        assert!(m.colors.iter().all(|x| *x == c));
    }

    #[test]
    fn coverage_counts_split_buildings_by_reach_of_their_network() {
        let mut sim = district();
        // One more house far from every line.
        put(
            &mut sim,
            "house_low_1",
            5400.0,
            5400.0,
            Lot {
                block: Id::new(99, 0),
                col: 0,
                w: 1,
                d: 1,
            },
        );
        let view = sim.view();
        let db = view.content.as_deref().unwrap();
        let (yes, no) = coverage_counts(UtilityLayer::Power, &view.utilities, &view.buildings, db);
        // 205 houses + the far one are concerned (the turbine is a source: also concerned).
        assert_eq!((yes, no), (206, 1));
        let (yes, no) = coverage_counts(UtilityLayer::Water, &view.utilities, &view.buildings, db);
        // Pipe at z = 5060 reaches the houses (z 5035..5043) within 40 m; the turbine is not a
        // water source or consumer.
        assert_eq!((yes, no), (205, 1));
    }

    #[test]
    fn layers_rebuild_only_when_the_network_or_buildings_change() {
        let mut sim = district();
        let (a, b) = (sim.view(), sim.view());
        assert!(Arc::ptr_eq(&a.utilities, &b.utilities));
        assert!(Arc::ptr_eq(&a.buildings, &b.buildings));
        build(
            &mut sim,
            UtilityKind::Power,
            &[(5000.0, 5100.0), (5050.0, 5100.0)],
        );
        assert!(!Arc::ptr_eq(&a.utilities, &sim.view().utilities));
    }
}

/// BUG-026: a street built with the real command has zone cells, and the zone mesh is not
/// empty and lies above the rendered terrain.
#[test]
fn a_built_street_has_a_zone_mesh_above_the_terrain() {
    use cp_net::{ElevationMode, NetCommand, RoadPath};
    use cp_sim::SimCommand;
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = terrain();
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::clone(&t));
    sim.set_content(Arc::clone(&content));
    let tick = sim.tick();
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: content.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: bevy_math::Vec2::new(4600.0, 4700.0),
                to: bevy_math::Vec2::new(4800.0, 4700.0),
            },
            elevation: ElevationMode::Ground,
        }),
        tick,
    );
    sim.tick_once();
    sim.tick_once();
    let view = sim.view();
    let cells = view.overlays.zones.cells.len();
    eprintln!(
        "cells={cells} grid_cells={}",
        view.zones
            .cells_within(bevy_math::Vec2::new(4700.0, 4700.0), 500.0)
            .len()
    );
    assert!(cells > 0, "no zone cells after BuildRoad");
    let m = zone_mesh(&view.overlays.zones, &t);
    assert!(!m.indices.is_empty());
    for p in &m.positions {
        let h = terrain_mesh_height(&t, bevy_math::Vec2::new(p[0], p[2]));
        assert!(p[1] >= h + 0.25, "{p:?} vs {h}");
    }
}

fn built_street() -> (SimRunner, Arc<Terrain>) {
    use cp_net::{ElevationMode, NetCommand, RoadPath};
    use cp_sim::SimCommand;
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = terrain();
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::clone(&t));
    sim.set_content(Arc::clone(&content));
    let tick = sim.tick();
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: content.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: bevy_math::Vec2::new(4600.0, 4700.0),
                to: bevy_math::Vec2::new(4800.0, 4700.0),
            },
            elevation: ElevationMode::Ground,
        }),
        tick,
    );
    sim.tick_once();
    sim.tick_once();
    (sim, t)
}

#[test]
fn outside_zoning_mode_only_painted_valid_cells_are_filled_and_outlines_cover_valid_cells() {
    let (_, t) = built_street();
    let mut zones = built_street().0.view().overlays.zones.as_ref().clone();
    zones.cells[0].zone = ZoneType::ResLow;
    zones.cells[1].zone = ZoneType::ResLow;
    zones.cells[1].valid = false;
    let fill = zone_mesh_with(&zones, &t, false, None);
    let n = fill.triangle_count();
    assert!(n >= 2, "the painted valid cell is filled");
    // Only cell 0 (1 quad on flat ground at least): unpainted and invalid cells are skipped.
    assert!(n < zones.cells.len(), "{n}");
    let full = zone_mesh_with(&zones, &t, true, None);
    assert!(full.triangle_count() >= zones.cells.len() * 2);
    assert!(
        fill.colors
            .iter()
            .all(|c| *c == zone_color(ZoneType::ResLow).unwrap())
    );

    let cam = Vec3::new(4700.0, 200.0, 4750.0);
    let o = zone_outline_mesh(&zones, &t, cam, 0, false);
    assert!(!o.indices.is_empty());
    assert!(faces_up(&o));
    for p in &o.positions {
        let h = terrain_mesh_height(&t, bevy_math::Vec2::new(p[0], p[2]));
        assert!(p[1] >= h + OUTLINE_LIFT - 1e-3, "{p:?} vs {h}");
    }
    // A camera farther than the LOD0 range draws no outlines.
    let far = zone_outline_mesh(
        &zones,
        &t,
        Vec3::new(4700.0, 200.0, 4700.0 + 1600.0),
        0,
        false,
    );
    assert!(far.indices.is_empty());
    // The invalid cell has none: one fewer cell outlined.
    let mut all_valid = zones.clone();
    all_valid.cells[1].valid = true;
    assert!(zone_outline_mesh(&all_valid, &t, cam, 0, false).triangle_count() > o.triangle_count());
}

/// A coarse terrain LOD may pass above the fine mesh: seen from far away the fill clears it.
#[test]
fn the_zone_fill_clears_the_coarse_terrain_lods_seen_from_afar() {
    use cp_terrain::{GRID, MapMeta};
    let meta = MapMeta {
        name: "t".into(),
        sea_level: 0.0,
        start_tile: (2, 2),
        external_connections: vec![],
        entry_road: None,
    };
    let h: Vec<u16> = (0..GRID * GRID)
        .map(|k| {
            let (x, z) = (k % GRID, k / GRID);
            (2000 + ((x * 7 + z * 13) % 40) * 30) as u16
        })
        .collect();
    let t = Terrain::from_samples(meta, &h, &vec![0; GRID * GRID]).unwrap();
    let zones = ZoneOverlay {
        cells: (0..30)
            .map(|k| {
                let o = bevy_math::Vec2::new(5003.0 + k as f32 * 9.7, 5004.0 + k as f32 * 3.1);
                ZoneCellView {
                    corners: [
                        o,
                        o + bevy_math::Vec2::X * 8.0,
                        o + bevy_math::Vec2::splat(8.0),
                        o + bevy_math::Vec2::Y * 8.0,
                    ]
                    .map(|p| [p.x, p.y]),
                    zone: ZoneType::ResLow,
                    valid: true,
                }
            })
            .collect(),
    };
    let cam = Vec3::new(5100.0, 3000.0, 5100.0 + 4000.0);
    let m = zone_mesh_with(&zones, &t, false, Some(cam));
    assert!(!m.positions.is_empty());
    for p in &m.positions {
        let q = bevy_math::Vec2::new(p[0], p[2]);
        for l in 0..crate::chunk::LOD_COUNT {
            let h = crate::road::lod_height(&t, l, q);
            assert!(p[1] >= h + 0.25, "{p:?} below LOD{l} {h}");
        }
    }
}

// ---- owned tile border (BUG-028) ----

mod tile_border {
    use super::*;
    use crate::overlay::tiles::{BORDER_LIFT, SHADE_LIFT, border_edges, border_mesh, shade_mesh};
    use bevy_math::Vec2;
    use cp_terrain::{MAP_SIZE, TILE_SIZE, TileOwnership};

    fn length(edges: &[(Vec2, Vec2)]) -> f32 {
        edges.iter().map(|(a, b)| a.distance(*b)).sum()
    }

    #[test]
    fn the_start_set_has_a_closed_outer_contour_of_24_km_and_no_inner_edges() {
        let tiles = TileOwnership::m1_around((2, 2));
        let edges = border_edges(&tiles);
        assert!(
            (length(&edges) - 4.0 * 3.0 * TILE_SIZE).abs() < 1.0,
            "{}",
            length(&edges)
        );
        for (a, b) in &edges {
            // One side open, the other not: the contour is never between two open tiles.
            let mid = (*a + *b) * 0.5;
            let n = (*b - *a).perp().normalize() * 1.0;
            let (p, q) = (mid + n, mid - n);
            assert_ne!(
                tiles.contains_point(p),
                tiles.contains_point(q),
                "{a} -> {b}"
            );
        }
        // Closed: every corner is shared by exactly two edges.
        let mut ends: Vec<(i32, i32)> = edges
            .iter()
            .flat_map(|(a, b)| [*a, *b])
            .map(|p| (p.x.round() as i32, p.y.round() as i32))
            .collect();
        ends.sort();
        for c in ends.chunk_by(|a, b| a == b) {
            assert_eq!(c.len(), 2, "{c:?}");
        }
    }

    #[test]
    fn the_border_mesh_lies_above_every_terrain_lod_and_faces_up() {
        let t = terrain();
        let tiles = TileOwnership::m1_around((2, 2));
        // Seen from far away the coarse LODs are on show.
        for cam in [None, Some(Vec3::new(5000.0, 9000.0, 5000.0))] {
            let m = border_mesh(&tiles, &t, cam, 4.0);
            assert!(faces_up(&m));
            assert!(m.positions.len() > 24_000 / 2 * 2, "tessellated every 2 m");
            for p in &m.positions {
                let q = bevy_math::Vec2::new(p[0], p[2]);
                let h = terrain_mesh_height(&t, q);
                assert!(p[1] >= h + BORDER_LIFT - 1e-3, "{p:?} vs {h}");
                if cam.is_some() {
                    for l in 1..crate::chunk::LOD_COUNT {
                        assert!(p[1] >= crate::road::lod_height(&t, l, q) + BORDER_LIFT - 1e-3);
                    }
                }
            }
        }
    }

    #[test]
    fn a_start_tile_in_the_corner_is_clipped_at_the_map_edge() {
        let t = terrain();
        let tiles = TileOwnership::m1_around((0, 0));
        let edges = border_edges(&tiles);
        // 2x2 open tiles: only the two inner sides facing closed tiles, 2 tiles long each.
        assert!((length(&edges) - 4.0 * TILE_SIZE).abs() < 1.0);
        for (a, b) in &edges {
            for p in [a, b] {
                assert!(p.x >= 0.0 && p.y >= 0.0 && p.x <= MAP_SIZE && p.y <= MAP_SIZE);
            }
        }
        let m = border_mesh(&tiles, &t, None, 4.0);
        assert!(!m.indices.is_empty());
        assert!(m.positions.iter().all(|p| p[0] >= -3.0 && p[2] >= -3.0));
        assert!(
            border_mesh(&TileOwnership::none(), &t, None, 4.0)
                .indices
                .is_empty()
        );
        // Everything open: no contour at all.
        let mut all = TileOwnership::none();
        for z in 0..5 {
            for x in 0..5 {
                all.set(x, z, true);
            }
        }
        assert!(border_edges(&all).is_empty());
    }

    #[test]
    fn the_veil_covers_exactly_the_tiles_that_are_not_open() {
        let t = terrain();
        let tiles = TileOwnership::m1_around((2, 2));
        let m = shade_mesh(&tiles, &t, None);
        assert!(faces_up(&m));
        let mut area = 0.0f64;
        for tri in m.indices.chunks(3) {
            let p = |i: u32| m.positions[i as usize];
            let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
            area += ((b[0] - a[0]) as f64 * (c[2] - a[2]) as f64
                - (c[0] - a[0]) as f64 * (b[2] - a[2]) as f64)
                .abs()
                * 0.5;
            let mid = bevy_math::Vec2::new((a[0] + b[0] + c[0]) / 3.0, (a[2] + b[2] + c[2]) / 3.0);
            assert!(
                !tiles.contains_point(mid),
                "veil over an open tile at {mid}"
            );
        }
        let closed = (25 - tiles.open_count()) as f64;
        assert!(
            (area - closed * (TILE_SIZE as f64).powi(2)).abs() < 1.0,
            "{area}"
        );
        for p in &m.positions {
            let h = terrain_mesh_height(&t, bevy_math::Vec2::new(p[0], p[2]));
            assert!(p[1] >= h + SHADE_LIFT - 1e-3);
        }
        assert!(
            shade_mesh(
                &TileOwnership::m1_around((2, 2)),
                &t,
                Some(Vec3::new(5000.0, 9000.0, 5000.0))
            )
            .indices
            .len()
                == m.indices.len()
        );
    }
}

/// T-119: the zone grid is built per chunk.
mod zone_chunks {
    use std::time::Duration;

    use super::*;
    use crate::overlay::zone_chunks::chunk_of;

    const NO_LIMIT: Duration = Duration::from_secs(3600);

    /// 8 m cells on a square lattice around the map centre, spanning 3 x 3 zone chunks; every
    /// third one painted, every seventh invalid.
    fn lattice() -> ZoneOverlay {
        let mut cells = Vec::new();
        for iz in 0..75 {
            for ix in 0..75 {
                let (x, z) = (4700.0 + ix as f32 * 8.0, 4700.0 + iz as f32 * 8.0);
                cells.push(ZoneCellView {
                    corners: [[x, z], [x + 8.0, z], [x + 8.0, z + 8.0], [x, z + 8.0]],
                    zone: if (ix + iz) % 3 == 0 {
                        ZoneType::ResLow
                    } else {
                        ZoneType::None
                    },
                    valid: (ix * 3 + iz) % 7 != 0,
                });
            }
        }
        ZoneOverlay { cells }
    }

    fn style(zoning: bool, x: f32, y: f32) -> ZoneStyle {
        ZoneStyle {
            zoning,
            cam: Some(Vec3::new(x, y, 5000.0)),
        }
    }

    /// Builds everything and returns the chunks' meshes concatenated: (fill, outline) triangles.
    fn build(c: &mut ZoneChunks, t: &Terrain, s: ZoneStyle) -> (usize, usize, usize) {
        let (mut fill, mut line, mut chunks) = (0, 0, 0);
        for u in c.update(t, 1, s, NO_LIMIT) {
            chunks += 1;
            fill += u.fill.map_or(0, |m| m.triangle_count());
            line += u.outline.map_or(0, |m| m.triangle_count());
        }
        (fill, line, chunks)
    }

    #[test]
    fn the_chunks_together_draw_what_the_whole_grid_draws() {
        let (t, zones) = (terrain(), lattice());
        for (zoning, x, y) in [(false, 5000.0, 300.0), (true, 1000.0, 2000.0)] {
            let s = style(zoning, x, y);
            let mut c = ZoneChunks::default();
            c.set_zones(&zones);
            let (fill, line, chunks) = build(&mut c, &t, s);
            let whole = zone_mesh_with(&zones, &t, zoning, s.cam);
            let width = outline_width_step(y);
            let outlines = zone_outline_mesh(&zones, &t, s.cam.unwrap(), width, zoning);
            assert_eq!(fill, whole.triangle_count(), "{zoning}");
            assert_eq!(line, outlines.triangle_count(), "{zoning}");
            assert!(chunks >= 9, "{chunks}");
        }
    }

    #[test]
    fn a_settled_grid_rebuilds_nothing_and_one_cell_rebuilds_one_chunk() {
        let (t, zones) = (terrain(), lattice());
        let s = style(false, 5000.0, 300.0);
        let mut c = ZoneChunks::default();
        c.set_zones(&zones);
        build(&mut c, &t, s);
        assert_eq!(c.dirty(&t, 1, s), 0);
        assert!(c.update(&t, 1, s, NO_LIMIT).is_empty());
        c.set_zones(&zones);
        assert_eq!(c.dirty(&t, 1, s), 0, "the same cells are not a change");

        let mut changed = zones.clone();
        changed.cells[2000].zone = ZoneType::Office;
        c.set_zones(&changed);
        let ups = c.update(&t, 1, s, NO_LIMIT);
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0].chunk, chunk_of(&changed.cells[2000]));
        assert!(c.update(&t, 1, s, NO_LIMIT).is_empty());

        // A cell that disappears empties its chunk's meshes.
        let mut fewer = changed.clone();
        fewer.cells.truncate(10);
        c.set_zones(&fewer);
        let ups = c.update(&t, 1, s, NO_LIMIT);
        assert!(ups.len() >= 3, "{}", ups.len());
        assert!(
            ups.iter()
                .filter(|u| u.chunk != chunk_of(&fewer.cells[0]))
                .all(|u| u.fill.as_ref().is_some_and(|m| m.indices.is_empty()))
        );
    }

    #[test]
    fn a_camera_step_only_touches_chunks_on_a_band_edge() {
        let (t, zones) = (terrain(), lattice());
        let mut c = ZoneChunks::default();
        c.set_zones(&zones);
        // High above the lattice every chunk is in the same terrain LOD band and the outlines
        // are out of range: a step of the camera changes nothing.
        let far = style(false, 5000.0, 9000.0);
        build(&mut c, &t, far);
        let step = style(false, 5000.0 + 260.0, 9000.0);
        assert_eq!(c.dirty(&t, 1, step), 0);
        // Close to the ground the chunks near the 1,500 m outline edge follow the camera, the
        // ones well inside do not.
        let near = style(false, 5000.0, 300.0);
        build(&mut c, &t, near);
        let step = style(false, 5000.0 + 260.0, 300.0);
        let d = c.dirty(&t, 1, step);
        assert!(d < 9, "{d}");
        // The zoning mode redraws all of them.
        assert!(c.dirty(&t, 1, style(true, 5000.0, 300.0)) >= 9);
    }

    #[test]
    fn a_frame_budget_spreads_the_work_nearest_chunk_first() {
        let (t, zones) = (terrain(), lattice());
        let s = style(false, 4750.0, 300.0);
        let mut c = ZoneChunks::default();
        c.set_zones(&zones);
        let mut frames = 0;
        let mut first = None;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let ups = c.update(&t, 1, s, Duration::ZERO);
            if ups.is_empty() {
                break;
            }
            assert_eq!(ups.len(), 1, "an exhausted budget still builds one chunk");
            first.get_or_insert(ups[0].chunk);
            assert!(seen.insert(ups[0].chunk));
            frames += 1;
        }
        assert!(frames >= 9, "{frames}");
        // The camera stands over this chunk.
        let under = ZoneCellView {
            corners: [[4750.0, 5000.0]; 4],
            ..zones.cells[0]
        };
        assert_eq!(first, Some(chunk_of(&under)));
        assert_eq!(seen.len(), frames);
    }
}

/// BUG-038: in a real new game (entry highway, only the start tile owned) a street built from the
/// end of the highway has valid zone cells beside it.
#[test]
fn a_street_from_the_new_game_highway_has_zone_cells() {
    use bevy::ecs::message::Messages;
    use cp_core::CommandOutcome;
    use cp_net::{ElevationMode, NetCommand, RoadPath};
    use cp_sim::SimCommand;
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = terrain();
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::clone(&t));
    sim.set_content(Arc::clone(&content));
    sim.new_game().unwrap();
    sim.tick_once();
    let stub = t.meta().entry_road.as_ref().expect("entry road").stub_end();
    let (tx, tz) = t.meta().start_tile;
    let centre = bevy_math::Vec2::new(
        (tx as f32 + 0.5) * cp_terrain::TILE_SIZE,
        (tz as f32 + 0.5) * cp_terrain::TILE_SIZE,
    );
    let dir = (centre - stub).normalize();
    let before = sim.view().overlays.zones.cells.len();
    let tick = sim.tick();
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: content.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: stub,
                to: stub + dir * 160.0,
            },
            elevation: ElevationMode::Ground,
        }),
        tick,
    );
    sim.tick_once();
    sim.tick_once();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    eprintln!("outcome {:?}", out.last().map(|o| &o.result));
    let view = sim.view();
    let valid = view.overlays.zones.cells.iter().filter(|c| c.valid).count();
    eprintln!(
        "stub {stub:?} segments={} cells {before} -> {} valid={valid}",
        view.net.segments().count(),
        view.overlays.zones.cells.len()
    );
    assert!(valid > 0, "no valid zone cell beside the new street");
}

/// BUG-038, whole path: a street built while the app runs makes the overlay systems spawn visible
/// zone outline and fill entities with vertices, and a second change rebuilds them.
#[test]
fn a_street_built_at_runtime_gets_visible_zone_layers() {
    use bevy::asset::AssetPlugin;
    use bevy::ecs::message::Messages;
    use cp_core::CommandOutcome;
    use cp_net::{ElevationMode, NetCommand, RoadPath};
    use cp_sim::SimCommand;
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = terrain();
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::clone(&t));
    sim.set_content(Arc::clone(&content));
    sim.new_game().unwrap();
    sim.tick_once();

    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Mesh>()
        .init_asset::<crate::material::VertexColorMaterial>()
        .init_resource::<OverlayState>()
        .init_resource::<ZoningMode>()
        .init_resource::<OverlayLayers>()
        .add_systems(
            Update,
            (sync_overlay_meshes, apply_overlay_visibility).chain(),
        );
    app.world_mut().spawn((
        Camera3d::default(),
        Transform::from_xyz(5120.0, 150.0, 4400.0)
            .looking_at(Vec3::new(5120.0, 0.0, 4500.0), Vec3::Y),
    ));
    app.insert_resource(sim.view());
    for _ in 0..3 {
        app.update();
    }
    let layers = |app: &mut App| -> Vec<(bool, bool, usize)> {
        let w = app.world_mut();
        let mut q = w.query::<(&OverlayLayer, Option<&ZoneOutlines>, &Visibility, &Mesh3d)>();
        let found: Vec<_> = q
            .iter(w)
            .filter(|(l, ..)| l.0 == Overlay::Zones)
            .map(|(_, o, v, m)| (o.is_some(), *v == Visibility::Visible, m.0.id()))
            .collect();
        let meshes = w.resource::<Assets<Mesh>>();
        found
            .into_iter()
            .map(|(o, v, id)| {
                let n = meshes.get(id).map_or(0, |m| m.count_vertices());
                (o, v, n)
            })
            .collect()
    };
    let before = layers(&mut app);

    let stub = t.meta().entry_road.as_ref().unwrap().stub_end();
    let (tx, tz) = t.meta().start_tile;
    let centre = bevy_math::Vec2::new(
        (tx as f32 + 0.5) * cp_terrain::TILE_SIZE,
        (tz as f32 + 0.5) * cp_terrain::TILE_SIZE,
    );
    let dir = (centre - stub).normalize();
    let tick = sim.tick();
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: content.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: stub,
                to: stub + dir * 160.0,
            },
            elevation: ElevationMode::Ground,
        }),
        tick,
    );
    sim.tick_once();
    sim.tick_once();
    sim.world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .clear();
    app.insert_resource(sim.view());
    for _ in 0..10 {
        app.update();
    }
    let after = layers(&mut app);
    eprintln!("zone layers before {before:?} after {after:?}");
    assert!(
        after
            .iter()
            .any(|&(outline, visible, n)| outline && visible && n > 0),
        "no visible zone outline mesh: {after:?}"
    );
    // Zoning mode (the Zones tool): the unpainted cells are filled too.
    app.world_mut().resource_mut::<ZoningMode>().0 = true;
    for _ in 0..10 {
        app.update();
    }
    let zoning = layers(&mut app);
    eprintln!("zoning mode {zoning:?}");
    assert!(
        zoning
            .iter()
            .any(|&(outline, visible, n)| !outline && visible && n > 0),
        "no visible zone fill mesh in zoning mode: {zoning:?}"
    );
}
