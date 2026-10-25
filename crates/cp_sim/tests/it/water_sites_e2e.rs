//! T-102: the water buildings can be placed on a real map. Since T-062 the zone cells over
//! water are invalid; the pump (site `OnWater`) must still be placeable with a cell over the
//! water, the outflow (`Shore`) on the dry bank, and a land building never on the water.

use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingError, validate_placement};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_terrain::Terrain;
use cp_zone::ZoneGrid;

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
                    // The first two rows of the blocks: cell centres 10 and 18 m off the axis.
                    for off in [-18.0, -10.0, 10.0, 18.0] {
                        let p = on_road(i as f32 * 8.0) + Vec2::new(0.0, off);
                        if t.is_water(p) {
                            wet += 1;
                        } else {
                            dry += 1;
                        }
                    }
                }
                if wet >= 4 && dry >= 40 {
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
fn the_pump_goes_on_the_water_the_outflow_on_the_bank_and_a_land_building_not_on_the_water() {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/river_valley")).unwrap());
    let (a, b) = shore(&terrain).expect("river_valley has a shore with a level dry bank");
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let mut sim = SimRunner::new(5);
    sim.set_terrain(Arc::clone(&terrain));
    sim.set_content(Arc::clone(&content));
    let road = content.road_id("basic_2l").unwrap();
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &content.road(road).clone(),
            road,
            CubicBezier3::line(Vec3::new(a.x, 0.0, a.y), Vec3::new(b.x, 0.0, b.y)),
            Elevation::Ground,
        )
        .unwrap();
    // The zone grid after its sync: cells over water are invalid now.
    sim.tick_once();
    sim.tick_once();
    let view = sim.view();
    let zones = sim.world().resource::<ZoneGrid>();
    let try_def = |name: &str, p: Vec2| {
        validate_placement(
            &content,
            zones,
            &view.net,
            Some(&terrain),
            None,
            content.building_id(name).unwrap(),
            p,
            0.0,
        )
    };
    // Every cell centre of every block is a candidate cursor position.
    let mut cells: Vec<Vec2> = Vec::new();
    for (id, block) in zones.blocks() {
        for cell in 0..block.cell_count() {
            let r = cp_zone::ZoneCellRef {
                block: id,
                cell: cell as u16,
            };
            if let Some(f) = zones.cell_footprint(r) {
                cells.push(f.c);
            }
        }
    }
    assert!(
        cells.iter().any(|&c| terrain.is_water(c)),
        "the blocks reach the water"
    );
    let mut errs: std::collections::BTreeMap<String, usize> = Default::default();
    for &c in cells.iter().filter(|&&c| terrain.is_water(c)) {
        *errs
            .entry(format!("{:?}", try_def("water_pump_station", c)))
            .or_default() += 1;
    }
    eprintln!(
        "DBG pump at wet cells: {errs:?}; total cells {}",
        cells.len()
    );
    let ok = |name: &str| cells.iter().filter(|&&c| try_def(name, c).is_ok()).count();
    assert!(
        ok("water_pump_station") > 0,
        "no spot for the pump over the water"
    );
    assert!(
        ok("sewage_outflow_station") > 0,
        "no spot for the outflow on the bank"
    );
    // A land building never covers water: where it can be placed at all, every cell of its
    // lot is dry; with a lot cell over the water it is refused.
    let mut refused_over_water = 0;
    for &c in &cells {
        match try_def("coal_power_plant", c) {
            Ok(plan) => {
                for cell in plan.cells() {
                    let f = zones.cell_footprint(cell).unwrap();
                    assert!(
                        !terrain.is_water(f.c),
                        "a land building over the water at {c:?}"
                    );
                }
            }
            Err(BuildingError::Water) => refused_over_water += 1,
            Err(_) => {}
        }
    }
    assert!(
        refused_over_water > 0,
        "no lot with a wet cell was refused as water"
    );
}
