//! BUG-082: a service building must fit in a block's corner.

use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::validate_placement_over;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_zone::ZoneGrid;

fn crossing() -> (SimRunner, Arc<ContentDb>) {
    let db = Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    );
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(&db));
    sim.world_mut()
        .insert_resource(cp_terrain::TileOwnership::only((1, 1)));
    let c = Vec3::new(3100.0, 0.0, 3100.0);
    for end in [
        Vec3::new(3000.0, 0.0, 3100.0),
        Vec3::new(3200.0, 0.0, 3100.0),
        Vec3::new(3100.0, 0.0, 3000.0),
        Vec3::new(3100.0, 0.0, 3200.0),
    ] {
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(&def, road, CubicBezier3::line(end, c), Elevation::Ground)
            .unwrap();
    }
    sim.tick_once();
    (sim, db)
}

/// Every cursor cell of a four-way crossing: a service building fits wherever a lot of free valid
/// cells holds the cursor column (a corner shifts the lot along the street), and what is refused
/// is named as a street cutting the lot, never as a building in the way.
#[test]
fn services_fit_in_block_corners_and_refusals_are_exact() {
    let (sim, db) = crossing();
    let w = sim.world();
    let zones = w.resource::<ZoneGrid>();
    let net = w.resource::<NetGraph>();
    let buildings = w.resource::<cp_building::BuildingGrid>();
    let tiles = w.resource::<cp_terrain::TileOwnership>();
    for name in ["coal_power_plant", "clinic"] {
        let def = db.building_id(name).unwrap();
        for (bid, b) in zones.blocks() {
            let (mut ok_corner, mut ok_mid) = (0, 0);
            for cell in 0..b.cols * cp_zone::BLOCK_DEPTH {
                let Some(fp) = zones.cell_footprint(cp_zone::ZoneCellRef { block: bid, cell })
                else {
                    continue;
                };
                let at_end = cell / cp_zone::BLOCK_DEPTH + 3 >= b.cols;
                match validate_placement_over(
                    &db,
                    zones,
                    net,
                    buildings,
                    None,
                    Some(tiles),
                    def,
                    fp.c,
                    0.0,
                ) {
                    Ok(_) if at_end => ok_corner += 1,
                    Ok(_) => ok_mid += 1,
                    Err(e) => {
                        assert_eq!(e, cp_building::BuildingError::Cut, "{name} {bid:?} {cell}");
                        assert!(at_end, "{name} refused mid-block: {bid:?} {cell}");
                    }
                }
            }
            assert!(
                ok_mid > 0 && ok_corner > 0,
                "{name} {bid:?}: {ok_mid} {ok_corner}"
            );
        }
    }
}
