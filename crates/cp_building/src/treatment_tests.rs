//! T-110: the sewage treatment plant stands on dry land beside a road, not on water.

use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_terrain::{GRID, MAX_HEIGHT, MapMeta, SPACING, Terrain, TileOwnership};
use cp_zone::{RoadZoning, ZoneCellRef, ZoneGrid};

use crate::{BuildingError, validate_placement};

fn terrain() -> Arc<Terrain> {
    let enc = |m: f32| (m / MAX_HEIGHT * 65535.0).round() as u16;
    let mut hs = vec![0u16; GRID * GRID];
    for iz in 0..GRID {
        for ix in 0..GRID {
            let (x, z) = (ix as f32 * SPACING, iz as f32 * SPACING);
            // A pond of 100 m beside the road around x = 5100; dry everywhere else.
            let wet = (5060.0..5160.0).contains(&x) && z > 4980.0 && z < 5100.0;
            hs[iz * GRID + ix] = enc(if wet { 90.0 } else { 101.0 });
        }
    }
    let meta = MapMeta {
        name: "t".into(),
        sea_level: 100.0,
        start_tile: (2, 2),
        external_connections: vec![],
        entry_road: None,
        entry_rail: None,
    };
    Arc::new(Terrain::from_samples(meta, &hs, &vec![0; GRID * GRID]).unwrap())
}

#[test]
fn the_plant_is_placed_on_land_beside_the_road_and_told_water_on_a_pond() {
    let db =
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap();
    let plant = db.building_id("sewage_treatment_plant").unwrap();
    assert_eq!(db.building(plant).lot, (3, 2));
    let road = db.road_id("basic_2l").unwrap();
    let mut net = NetGraph::new();
    net.add_segment(
        &db.road(road).clone(),
        road,
        CubicBezier3::line(
            Vec3::new(5000.0, 101.0, 5000.0),
            Vec3::new(5200.0, 101.0, 5000.0),
        ),
        Elevation::Ground,
    )
    .unwrap();
    let t = terrain();
    let mut zones = ZoneGrid::new();
    zones.set_terrain(Arc::clone(&t));
    zones.sync(&net, &|id| {
        let d = db.road(id);
        RoadZoning {
            allows_zoning: d.allows_zoning,
            half_width: d.width / 2.0,
            footpath: d.class == cp_content::RoadClass::Path,
        }
    });
    let tiles = TileOwnership::m1_around((2, 2));
    let (block, _) = zones
        .blocks()
        .find(|(_, b)| b.side == cp_zone::Side::Right)
        .unwrap();
    let at = |col: u16| -> Vec2 {
        zones
            .cell_footprint(ZoneCellRef {
                block,
                cell: col * 4 + 1,
            })
            .unwrap()
            .c
    };
    let place =
        |pos| validate_placement(&db, &zones, &net, Some(&t), Some(&tiles), plant, pos, 0.0);
    // Dry land at the west end of the road.
    let plan = place(at(3)).unwrap();
    assert_eq!(plan.cells().len(), 6);
    assert!(plan.access.is_some());
    // The same lot over the pond.
    assert_eq!(place(at(15)), Err(BuildingError::Water));
}
