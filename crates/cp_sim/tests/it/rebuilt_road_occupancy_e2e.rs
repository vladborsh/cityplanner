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
        .flat_map(|(id, b)| b.lot().expect("a lot").cells().map(move |c| (id, c)))
        .collect()
}

/// A road with survivors, removed and rebuilt along the same line (new blocks, zoned).
fn rebuilt() -> (SimRunner, Vec<BuildingId>) {
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

    (sim, survivors)
}

#[test]
fn survivors_keep_their_cells_in_the_blocks_of_a_rebuilt_road() {
    let (mut sim, survivors) = rebuilt();
    // Long enough for many spawn attempts on the new blocks.
    for _ in 0..(3 * TICKS_PER_DAY) {
        sim.tick_once();
    }

    let zones = sim.world().resource::<ZoneGrid>();
    let grid = sim.world().resource::<BuildingGrid>();
    for &id in &survivors {
        let b = grid.get(id).expect("the survivor is still there");
        assert!(
            zones.block(b.lot().expect("a lot").block).is_some(),
            "re-bound to a live block"
        );
        assert!(
            b.lot()
                .expect("a lot")
                .cells()
                .all(|c| zones.is_occupied(c)),
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

#[test]
fn the_zone_revision_is_stable_once_the_survivors_are_settled() {
    let (mut sim, _) = rebuilt();
    // Stop spawning so only the rebind logic could still touch the grid.
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(0.0);
    // A survivor whose cells never form its lot rectangle stays unbound for good: it must
    // not touch the zone grid every tick.
    {
        let (id, _) = sim
            .world()
            .resource::<BuildingGrid>()
            .iter()
            .next()
            .unwrap();
        let mut grid = sim.world_mut().resource_mut::<BuildingGrid>();
        let lot = grid.get_mut(id).unwrap().lot_mut().expect("a lot");
        lot.block = cp_core::Id::new(9999, 0);
        lot.w += 1;
    }
    for _ in 0..40 {
        sim.tick_once();
    }
    let rev = sim.world().resource::<ZoneGrid>().revision();
    for _ in 0..200 {
        sim.tick_once();
    }
    assert_eq!(sim.world().resource::<ZoneGrid>().revision(), rev);
}

#[test]
fn demolishing_an_unbound_survivor_releases_the_cells_it_holds() {
    use cp_building::BuildingCommand;
    use cp_core::Id;
    let (mut sim, survivors) = rebuilt();
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(0.0);
    for _ in 0..40 {
        sim.tick_once();
    }
    let id = survivors[0];
    let held: Vec<ZoneCellRef> = {
        let b = sim.world().resource::<BuildingGrid>().get(id).unwrap();
        b.lot().expect("a lot").cells().collect()
    };
    assert!(
        held.iter()
            .all(|c| sim.world().resource::<ZoneGrid>().is_occupied(*c))
    );
    // Make the lot unbound, as if the rebuilt cells had not formed its rectangle.
    sim.world_mut()
        .resource_mut::<BuildingGrid>()
        .get_mut(id)
        .unwrap()
        .lot_mut()
        .expect("a lot")
        .block = Id::new(9999, 0);
    let command = cp_sim::SimCommand::Building(BuildingCommand::Demolish { building: id });
    sim.push_command(command, sim.tick());
    sim.apply_due_commands();
    assert!(sim.world().resource::<BuildingGrid>().get(id).is_none());
    let zones = sim.world().resource::<ZoneGrid>();
    assert!(
        held.iter().all(|c| !zones.is_occupied(*c)),
        "the cells under the unbound lot are free again"
    );
}
