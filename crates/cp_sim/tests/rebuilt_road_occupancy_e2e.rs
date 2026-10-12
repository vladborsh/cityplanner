//! BUG-019: a road rebuilt along a removed one makes new zone blocks; the buildings that stayed
//! (`NO_ROAD`) must still occupy their cells there (and be re-bound to the new block), or the
//! spawner puts a new building on top of them.

use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_sim::SimRunner;
use cp_zone::{CELL_SIZE, ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn line(sim: &mut SimRunner, content: &ContentDb) -> SegmentId {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let seg = CubicBezier3::line(
        Vec3::new(3000.0, 0.0, 3000.0),
        Vec3::new(3200.0, 0.0, 3000.0),
    );
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(&def, road, seg, Elevation::Ground)
        .unwrap()
}

fn zone(sim: &mut SimRunner, seg: SegmentId) {
    let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
    let ids: Vec<_> = grid
        .blocks()
        .filter(|(_, b)| b.segment == seg)
        .map(|(id, b)| (id, b.cell_count()))
        .collect();
    for (id, cells) in ids {
        for cell in 0..cells {
            grid.set_zone(
                ZoneCellRef {
                    block: id,
                    cell: cell as u16,
                },
                ZoneType::ResLow,
            );
        }
    }
}

fn active(sim: &SimRunner) -> Vec<BuildingId> {
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .map(|(id, _)| id)
        .collect()
}

/// Every cell of every building's lot, with the building it belongs to.
fn cells_of(sim: &SimRunner) -> Vec<(BuildingId, ZoneCellRef)> {
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .flat_map(|(id, b)| b.lot.cells().map(move |c| (id, c)))
        .collect()
}

#[test]
fn survivors_keep_their_cells_in_the_blocks_of_a_rebuilt_road() {
    let content = content();
    let mut sim = SimRunner::new(7);
    sim.set_content(Arc::clone(&content));
    let seg = line(&mut sim, &content);
    sim.tick_once();
    zone(&mut sim, seg);
    while active(&sim).len() < 3 {
        sim.tick_once();
        assert!(sim.tick().0 < 6 * TICKS_PER_DAY, "no buildings");
    }
    for _ in 0..40 {
        sim.tick_once();
    }
    let survivors = active(&sim);

    sim.world_mut()
        .resource_mut::<NetGraph>()
        .remove_segment(seg)
        .unwrap();
    for _ in 0..40 {
        sim.tick_once();
    }
    let again = line(&mut sim, &content);
    sim.tick_once();
    zone(&mut sim, again);

    // Long enough for many spawn attempts on the new blocks.
    for _ in 0..(3 * TICKS_PER_DAY) {
        sim.tick_once();
    }

    let zones = sim.world().resource::<ZoneGrid>();
    let grid = sim.world().resource::<BuildingGrid>();
    for &id in &survivors {
        let b = grid.get(id).expect("the survivor is still there");
        assert!(
            zones.block(b.lot.block).is_some(),
            "re-bound to a live block"
        );
        assert!(
            b.lot.cells().all(|c| zones.is_occupied(c)),
            "its cells are taken in the new block"
        );
    }
    // No two lots overlap on the ground.
    let all = cells_of(&sim);
    assert!(
        all.len() > survivors.len() * 4,
        "the new blocks got buildings too"
    );
    for (i, (a, ca)) in all.iter().enumerate() {
        let pa = zones.cell_footprint(*ca).expect("live cell").c;
        for (b, cb) in &all[i + 1..] {
            if a == b {
                continue;
            }
            let pb = zones.cell_footprint(*cb).expect("live cell").c;
            assert!(
                pa.distance(pb) > CELL_SIZE * 0.9,
                "buildings {a:?} and {b:?} share ground at {pa}"
            );
        }
    }
}
