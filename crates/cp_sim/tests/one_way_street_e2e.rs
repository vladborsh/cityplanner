//! T-172: a one-way street with sidewalks (`basic_1w_2l`) is zoned on both sides and the
//! buildings spawn on both.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{BuildingGrid, Demand};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_terrain::Terrain;
use cp_zone::{ZoneCellRef, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn both_sides_of_a_one_way_street_are_zoned_and_built() {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let mut sim = SimRunner::new(5);
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(&content));
    let road = content.road_id("basic_1w_2l").unwrap();
    assert!(content.road(road).is_one_way());
    let seg = sim
        .world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &content.road(road).clone(),
            road,
            CubicBezier3::line(
                Vec3::new(4000.0, 0.0, 4000.0),
                Vec3::new(4300.0, 0.0, 4000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();

    let cells: Vec<ZoneCellRef> = {
        let g = sim.world().resource::<ZoneGrid>();
        let sides = g.blocks_of(seg);
        assert!(sides.iter().all(Option::is_some), "a block on each side");
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
        let mut g = sim.world_mut().resource_mut::<ZoneGrid>();
        for &c in &cells {
            g.set_zone(c, ZoneType::ResLow);
        }
    }
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(1.0);
    for _ in 0..(6 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    let zones = sim.world().resource::<ZoneGrid>();
    let mut per_side = std::collections::BTreeMap::new();
    for (_, b) in sim.world().resource::<BuildingGrid>().iter() {
        let block = b.lot().expect("a lot").block;
        *per_side
            .entry(format!("{:?}", zones.block(block).unwrap().side))
            .or_insert(0) += 1;
    }
    assert_eq!(per_side.len(), 2, "buildings on both sides: {per_side:?}");
}
