//! `validate_free_placement` (T-107): one test per error and one for success.

use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_content::{BuildingDefId, ContentDb};
use cp_core::curve::CubicBezier3;
use cp_core::{Id, Tick};
use cp_net::{Elevation, NetGraph};
use cp_terrain::{GRID, MAX_HEIGHT, MapMeta, SPACING, Terrain, TileOwnership};
use cp_zone::ZoneGrid;

use crate::{
    Building, BuildingError, BuildingFlags, BuildingGrid, BuildingState, Footprint, Lot,
    validate_free_placement,
};

/// A free-standing 2×2 tower (a 16 m square) next to the real road defs.
fn db() -> ContentDb {
    let src = |p: &str, name: &str| {
        (
            format!("content/{name}"),
            std::fs::read_to_string(format!(
                "{}/../../assets/content/{name}",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap_or_else(|e| panic!("{p}: {e}")),
        )
    };
    ContentDb::from_sources(vec![
        src("balance", "balance.ron"),
        src("road", "roads/basic_2l.ron"),
        src("style", "styles/service_water_tower.ron"),
        (
            "content/buildings/tower.ron".to_string(),
            r#"Building((id: "tower", name: "Tower", zone: Industrial, level: 1,
            lot: (2, 2), jobs: (0, 0, 0, 0), consumption: {}, tax_base: 0.0,
            cost: 5000.0, upkeep_week: 1.0,
            styles: ["service_water_tower"],
            service: Some("tower_service")))"#
                .to_string(),
        ),
        (
            "content/services/tower.ron".to_string(),
            r#"Service((id: "tower_service", name: "Tower", kind: Water,
            radius_m: 100.0, capacity: 10, road_access: false, vehicles: []))"#
                .to_string(),
        ),
    ])
    .unwrap()
}

fn enc(m: f32) -> u16 {
    (m / MAX_HEIGHT * 65535.0).round() as u16
}

fn terrain(sea: f32, f: impl Fn(f32, f32) -> f32) -> Arc<Terrain> {
    let mut hs = vec![0u16; GRID * GRID];
    for iz in 0..GRID {
        for ix in 0..GRID {
            hs[iz * GRID + ix] = enc(f(ix as f32 * SPACING, iz as f32 * SPACING));
        }
    }
    let meta = MapMeta {
        name: "t".into(),
        sea_level: sea,
        start_tile: (2, 2),
        external_connections: vec![],
        entry_road: None,
    };
    Arc::new(Terrain::from_samples(meta, &hs, &vec![0; GRID * GRID]).unwrap())
}

struct Fixture {
    db: ContentDb,
    tower: BuildingDefId,
    net: NetGraph,
    zones: ZoneGrid,
    buildings: BuildingGrid,
    terrain: Arc<Terrain>,
    tiles: TileOwnership,
}

impl Fixture {
    /// Flat dry map, the middle tiles owned, a 200 m road along +X at z = 5000.
    fn new() -> Self {
        let db = db();
        let road = db.road_id("basic_2l").unwrap();
        let mut net = NetGraph::new();
        net.add_segment(
            &db.road(road).clone(),
            road,
            CubicBezier3::line(
                Vec3::new(5000.0, 100.0, 5000.0),
                Vec3::new(5200.0, 100.0, 5000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
        Self {
            tower: db.building_id("tower").unwrap(),
            db,
            net,
            zones: ZoneGrid::new(),
            buildings: BuildingGrid::new(),
            terrain: terrain(0.0, |_, _| 100.0),
            tiles: TileOwnership::m1_around((2, 2)),
        }
    }

    fn check(&self, pos: Vec2, rot: f32) -> Result<crate::PlacementPlan, BuildingError> {
        validate_free_placement(
            &self.db,
            &self.zones,
            &self.net,
            &self.buildings,
            Some(&self.terrain),
            Some(&self.tiles),
            self.tower,
            pos,
            rot,
        )
    }

    fn building(&self, footprint: Footprint, pos: Vec2, rot: f32) -> Building {
        Building {
            def: self.tower,
            footprint,
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos,
            rot,
            access: None,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        }
    }
}

#[test]
fn a_field_100_m_from_the_road_is_fine_and_costs_the_def_price() {
    let f = Fixture::new();
    let plan = f.check(Vec2::new(5100.0, 5100.0), 0.3).unwrap();
    assert_eq!(plan.footprint, Footprint::Free { w: 2, d: 2 });
    assert_eq!(plan.access, None);
    assert!(plan.cells().is_empty());
    assert_eq!((plan.pos, plan.rot), (Vec2::new(5100.0, 5100.0), 0.3));
    assert_eq!(plan.cost, cp_core::Cost::from_units(5000.0));
}

#[test]
fn two_metres_from_the_road_is_road_and_twenty_is_fine() {
    let f = Fixture::new();
    // The road is 12 m wide (half 6) around z = 5000; the tower is 16 m deep (half 8) at rot 0.
    // Half a metre of clearance (edge at 5006.5) is too close: 1 m is the rule.
    assert_eq!(
        f.check(Vec2::new(5100.0, 5014.5), 0.0),
        Err(BuildingError::Road)
    );
    // On the road itself.
    assert_eq!(
        f.check(Vec2::new(5100.0, 5002.0), 0.0),
        Err(BuildingError::Road)
    );
    // Just beyond the 1 m clearance (edge at 5007 + 0.5).
    assert!(
        f.check(Vec2::new(5100.0, 5000.0 + 6.0 + 1.5 + 8.0), 0.0)
            .is_ok()
    );
    // 20 m from the road edge.
    assert!(
        f.check(Vec2::new(5100.0, 5000.0 + 6.0 + 20.0 + 8.0), 0.0)
            .is_ok()
    );
    // Turned by 45°, the corner is what comes close.
    assert_eq!(
        f.check(
            Vec2::new(5100.0, 5006.5 + 8.0 * 2f32.sqrt()),
            std::f32::consts::FRAC_PI_4
        ),
        Err(BuildingError::Road)
    );
    // Across the road, on the other side.
    assert_eq!(
        f.check(Vec2::new(5100.0, 4986.5), 0.0),
        Err(BuildingError::Road)
    );
}

#[test]
fn an_elevated_road_does_not_block_the_ground_below() {
    let mut f = Fixture::new();
    let road = f.db.road_id("basic_2l").unwrap();
    let mut net = NetGraph::new();
    net.add_segment(
        &f.db.road(road).clone(),
        road,
        CubicBezier3::line(
            Vec3::new(5000.0, 110.0, 5000.0),
            Vec3::new(5200.0, 110.0, 5000.0),
        ),
        Elevation::Elevated,
    )
    .unwrap();
    f.net = net;
    assert!(f.check(Vec2::new(5100.0, 5000.0), 0.0).is_ok());
}

#[test]
fn water_under_any_sample_is_water() {
    let mut f = Fixture::new();
    // A pond: sea level 100, ground 99 within 6 m of (5100, 5100), dry elsewhere.
    f.terrain = terrain(100.0, |x, z| {
        if (x - 5100.0).abs() < 15.0 && (z - 5100.0).abs() < 15.0 {
            99.0
        } else {
            101.0
        }
    });
    assert_eq!(
        f.check(Vec2::new(5100.0, 5100.0), 0.0),
        Err(BuildingError::Water)
    );
    assert!(f.check(Vec2::new(5100.0, 5300.0), 0.0).is_ok());
}

#[test]
fn a_forty_percent_slope_is_too_steep_and_ten_percent_is_fine() {
    let mut f = Fixture::new();
    f.terrain = terrain(0.0, |_, z| 100.0 + 0.4 * (z - 5100.0));
    assert_eq!(
        f.check(Vec2::new(5100.0, 5100.0), 0.0),
        Err(BuildingError::TooSteep)
    );
    f.terrain = terrain(0.0, |_, z| 100.0 + 0.1 * (z - 5100.0));
    assert!(f.check(Vec2::new(5100.0, 5100.0), 0.0).is_ok());
}

#[test]
fn a_tile_the_player_does_not_own_is_not_owned() {
    let mut f = Fixture::new();
    f.tiles = TileOwnership::only((0, 0));
    assert_eq!(
        f.check(Vec2::new(5100.0, 5100.0), 0.0),
        Err(BuildingError::NotOwned)
    );
    // Straddling an owned and a closed tile is also refused.
    f.tiles = TileOwnership::none();
    f.tiles.set(2, 2, true);
    let edge = 2.0 * cp_terrain::TILE_SIZE;
    assert_eq!(
        f.check(Vec2::new(edge + 4.0, 5100.0), 0.0),
        Err(BuildingError::NotOwned)
    );
    assert!(f.check(Vec2::new(edge + 40.0, 5100.0), 0.0).is_ok());
}

#[test]
fn on_top_of_a_lot_or_another_free_building_is_blocked() {
    let mut f = Fixture::new();
    let lot = Footprint::Lot(Lot {
        block: Id::new(0, 0),
        col: 0,
        w: 2,
        d: 2,
    });
    f.buildings
        .insert(f.building(lot, Vec2::new(5100.0, 5100.0), 0.0));
    assert_eq!(
        f.check(Vec2::new(5108.0, 5104.0), 0.2),
        Err(BuildingError::Blocked)
    );
    // Side by side, touching, is fine.
    assert!(f.check(Vec2::new(5116.0, 5100.0), 0.0).is_ok());

    let mut g = Fixture::new();
    g.buildings.insert(g.building(
        Footprint::Free { w: 2, d: 2 },
        Vec2::new(5100.0, 5100.0),
        0.7,
    ));
    assert_eq!(
        g.check(Vec2::new(5100.0, 5110.0), 0.0),
        Err(BuildingError::Blocked)
    );
    assert!(g.check(Vec2::new(5100.0, 5140.0), 0.0).is_ok());
}

#[test]
fn a_lot_cell_held_by_a_building_blocks_even_without_a_listed_building() {
    use cp_zone::{RoadZoning, ZoneCellRef};
    let mut f = Fixture::new();
    f.zones.sync(
        &f.net,
        &|id| {
            let d = f.db.road(id);
            RoadZoning {
                allows_zoning: d.allows_zoning,
                half_width: d.width / 2.0,
            }
        },
        &|_, s| s.curve,
    );
    let (block, _) = f.zones.blocks().next().unwrap();
    let cell = ZoneCellRef {
        block,
        cell: 4 * 5 + 3,
    };
    let centre = f.zones.cell_footprint(cell).unwrap().c;
    f.zones.occupy(&[cell]);
    assert_eq!(f.check(centre, 0.0), Err(BuildingError::Blocked));
}

#[test]
fn bad_input_and_non_free_defs_are_refused() {
    let f = Fixture::new();
    assert_eq!(
        f.check(Vec2::new(f32::NAN, 0.0), 0.0),
        Err(BuildingError::BadCoordinates)
    );
    assert_eq!(
        f.check(Vec2::new(5100.0, 5100.0), f32::INFINITY),
        Err(BuildingError::BadCoordinates)
    );
    let real =
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap();
    let pump = real.building_id("water_pump_station").unwrap();
    let r = validate_free_placement(
        &real,
        &f.zones,
        &f.net,
        &f.buildings,
        None,
        None,
        pump,
        Vec2::new(5100.0, 5100.0),
        0.0,
    );
    assert_eq!(r, Err(BuildingError::NotPlaceable));
}
