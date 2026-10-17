//! T-236: low-density homes and industry fill the strip: the spawner prefers big lots, so most
//! buildings of a grown block are four rows deep.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_building::{BuildingGrid, Demand, pick_def, spawnable};
use cp_content::{ContentDb, ZoneType};
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_zone::{ZoneCommand, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

#[test]
fn the_picker_takes_a_four_deep_def_most_of_the_time_when_a_4x4_lot_is_free() {
    let db = content();
    for zone in [ZoneType::ResLow, ZoneType::Industrial] {
        let fitting = spawnable(&db, zone, None);
        assert!(
            fitting.iter().any(|d| db.building(*d).lot.1 == 4),
            "{zone:?} has depth-4 defs"
        );
        let mut deep = 0;
        for roll in 0..2000u32 {
            let d = pick_def(&db, &fitting, roll.wrapping_mul(2_654_435_761));
            deep += u32::from(db.building(d).lot.1 == 4);
        }
        assert!(deep >= 1200, "{zone:?}: {deep} of 2000 picks are 4 deep");
    }
}

fn grown(zone: ZoneType, seed: u64) -> (usize, usize) {
    let db = content();
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(seed);
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(&db));
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance = cp_core::Cost(1_000_000_000);
    // A street of 128 m: a strip of 16 cells, four rows deep, on each side (8 x 16 blocks).
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: db.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: Vec2::new(5000.0, 5000.0),
                to: Vec2::new(5128.0, 5000.0),
            },
            elevation: ElevationMode::Ground,
        }),
        sim.tick(),
    );
    for _ in 0..3 {
        sim.tick_once();
    }
    let cells: Vec<_> = {
        let z = sim.world().resource::<ZoneGrid>();
        z.blocks()
            .flat_map(|(id, _)| z.block_cells(id))
            .filter(|c| z.is_valid(*c))
            .collect()
    };
    sim.push_command(
        SimCommand::Zone(ZoneCommand::PaintZones { cells, zone }),
        sim.tick(),
    );
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(1.0);
    for _ in 0..6000 {
        sim.tick_once();
    }
    let b = sim.world().resource::<BuildingGrid>();
    let total = b.iter().filter(|(_, b)| b.lot().is_some()).count();
    let deep = b
        .iter()
        .filter(|(_, b)| b.lot().is_some_and(|l| l.d == 4))
        .count();
    (deep, total)
}

#[test]
fn most_buildings_of_a_grown_block_are_four_rows_deep() {
    for zone in [ZoneType::ResLow, ZoneType::Industrial] {
        let (mut deep, mut total) = (0, 0);
        for seed in 1..=3 {
            let (d, t) = grown(zone, seed);
            deep += d;
            total += t;
        }
        assert!(total >= 12, "{zone:?}: only {total} buildings");
        assert!(
            deep * 10 >= total * 6,
            "{zone:?}: {deep} of {total} are 4 deep"
        );
    }
}
