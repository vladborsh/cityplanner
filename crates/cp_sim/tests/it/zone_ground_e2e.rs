//! T-062: on river_valley a road along the shore has zone cells over water that are invalid,
//! and the building spawner never uses them.

use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingGrid, Demand};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_terrain::Terrain;
use cp_zone::{MAX_CELL_SLOPE, ZoneCellRef, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn slope(t: &Terrain, p: Vec2) -> f32 {
    let ny = t.normal(p).y;
    (1.0 - ny * ny).max(0.0).sqrt() / ny
}

/// A level, dry 160 m stretch whose neighbourhood (8..40 m off the axis) has both water and
/// dry ground: a road along the shore.
fn shore(t: &Terrain) -> Option<(Vec2, Vec2)> {
    let mut z = 300.0;
    while z < 9800.0 {
        let mut x = 300.0;
        while x < 9800.0 {
            let on_road = |k: f32| Vec2::new(x + k, z);
            let road_ok = (0..=20).all(|i| {
                let p = on_road(i as f32 * 8.0);
                !t.is_water(p) && slope(t, p) < 0.15
            });
            if road_ok {
                let (mut wet, mut dry) = (0, 0);
                for i in 0..=20 {
                    for off in [-36.0, -28.0, -20.0, -12.0, 12.0, 20.0, 28.0, 36.0] {
                        let p = on_road(i as f32 * 8.0) + Vec2::new(0.0, off);
                        if t.is_water(p) {
                            wet += 1;
                        } else {
                            dry += 1;
                        }
                    }
                }
                if wet >= 8 && dry >= 24 {
                    return Some((Vec2::new(x, z), Vec2::new(x + 160.0, z)));
                }
            }
            x += 40.0;
        }
        z += 40.0;
    }
    None
}

#[test]
fn cells_over_the_river_are_invalid_and_no_building_stands_on_them() {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/river_valley")).unwrap());
    let (a, b) = shore(&terrain).expect("river_valley has a shore with a level dry bank");
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let mut sim = SimRunner::new(5);
    sim.set_terrain(Arc::clone(&terrain));
    sim.set_content(Arc::clone(&content));
    let road = content.road_id("basic_2l").unwrap();
    let seg = sim
        .world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &content.road(road).clone(),
            road,
            CubicBezier3::line(Vec3::new(a.x, 0.0, a.y), Vec3::new(b.x, 0.0, b.y)),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();

    let cells: Vec<ZoneCellRef> = {
        let g = sim.world().resource::<ZoneGrid>();
        g.blocks()
            .filter(|(_, b)| b.segment == seg)
            .flat_map(|(id, b)| {
                (0..b.cell_count()).map(move |i| ZoneCellRef {
                    block: id,
                    cell: i as u16,
                })
            })
            .collect()
    };
    {
        let g = sim.world().resource::<ZoneGrid>();
        let (mut wet_invalid, mut dry_valid) = (0, 0);
        for &c in &cells {
            let p = g.cell_footprint(c).unwrap().c;
            if terrain.is_water(p) {
                assert!(!g.is_valid(c), "{c:?} over water at {p}");
                wet_invalid += 1;
            } else if g.is_valid(c) {
                dry_valid += 1;
            }
        }
        assert!(
            wet_invalid > 0 && dry_valid > 0,
            "{wet_invalid} {dry_valid}"
        );
    }
    {
        let mut g = sim.world_mut().resource_mut::<ZoneGrid>();
        for &c in &cells {
            g.set_zone(c, ZoneType::ResLow);
        }
    }
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(1.0);
    for _ in 0..(4 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    let zones = sim.world().resource::<ZoneGrid>();
    let buildings = sim.world().resource::<BuildingGrid>();
    assert!(!buildings.is_empty(), "the dry bank got buildings");
    for (id, b) in buildings.iter() {
        for c in b.lot().expect("a lot").cells() {
            let p = zones.cell_footprint(c).expect("cell").c;
            assert!(!terrain.is_water(p), "{id:?} stands in the river at {p}");
            assert!(
                slope(&terrain, p) <= MAX_CELL_SLOPE + 1e-3,
                "{id:?} on a slope"
            );
        }
    }
}
