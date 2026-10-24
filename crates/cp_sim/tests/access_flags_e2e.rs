//! T-061 acceptance: `NO_ROAD` after a road is removed (buildings stay, the flag clears when
//! a road is built along the old one) and `NO_PATH` from a failed trip (cleared once the
//! network is connected). Both bits are part of the checksum.

use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{BuildingFlags, BuildingGrid, BuildingId, BuildingState};
use cp_citizen::{
    CitizenGrid, Location, RoutineSettings, Trip, TripBook, TripMode, TripPurpose, TripState,
};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_sim::SimRunner;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn line(sim: &mut SimRunner, content: &ContentDb, a: (f32, f32), b: (f32, f32)) -> SegmentId {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let seg = CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1));
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(&def, road, seg, Elevation::Ground)
        .unwrap()
}

fn zone(sim: &mut SimRunner, seg: SegmentId, kind: ZoneType) {
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
                kind,
            );
        }
    }
}

fn ticks(sim: &mut SimRunner, n: u64) {
    for _ in 0..n {
        sim.tick_once();
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

fn flags(sim: &SimRunner, id: BuildingId) -> BuildingFlags {
    sim.world()
        .resource::<BuildingGrid>()
        .get(id)
        .unwrap()
        .flags
}

#[test]
fn buildings_keep_their_lots_and_get_no_road_until_a_road_runs_along_them_again() {
    let content = content();
    let mut sim = SimRunner::new(7);
    sim.set_content(Arc::clone(&content));
    let seg = line(&mut sim, &content, (3000.0, 3000.0), (3200.0, 3000.0));
    sim.tick_once();
    zone(&mut sim, seg, ZoneType::ResLow);
    while active(&sim).len() < 3 {
        sim.tick_once();
        assert!(sim.tick().0 < 6 * TICKS_PER_DAY, "no buildings");
    }
    ticks(&mut sim, 40);
    let before = active(&sim);
    assert!(
        before
            .iter()
            .all(|&b| !flags(&sim, b).has(BuildingFlags::NO_ROAD)),
        "a road serves them"
    );
    let sum_before = sim.checksum();

    sim.world_mut()
        .resource_mut::<NetGraph>()
        .remove_segment(seg)
        .unwrap();
    ticks(&mut sim, 40);
    let after = active(&sim);
    assert!(
        before.iter().all(|b| after.contains(b)),
        "nobody is demolished"
    );
    let zones = sim.world().resource::<ZoneGrid>();
    for &b in &after {
        let bd = sim.world().resource::<BuildingGrid>().get(b).unwrap();
        assert!(flags(&sim, b).has(BuildingFlags::NO_ROAD));
        assert!(
            bd.lot()
                .expect("a lot")
                .cells()
                .all(|c| zones.is_occupied(c)),
            "cells stay taken"
        );
    }
    assert_ne!(sim.checksum(), sum_before, "the bit is in the checksum");

    let again = line(&mut sim, &content, (3000.0, 3000.0), (3200.0, 3000.0));
    ticks(&mut sim, 40);
    for &b in &after {
        assert!(!flags(&sim, b).has(BuildingFlags::NO_ROAD), "flag cleared");
        let bd = sim.world().resource::<BuildingGrid>().get(b).unwrap();
        assert_eq!(bd.access().expect("an access point").segment, again);
    }
}

#[test]
fn a_trip_into_an_isolated_network_flags_no_path_until_it_is_connected() {
    let content = content();
    let mut sim = SimRunner::new(11);
    sim.set_content(Arc::clone(&content));
    let a = line(&mut sim, &content, (3000.0, 3000.0), (3200.0, 3000.0));
    let b = line(&mut sim, &content, (3300.0, 3000.0), (3500.0, 3000.0));
    sim.tick_once();
    zone(&mut sim, a, ZoneType::ResHigh);
    zone(&mut sim, b, ZoneType::Industrial);
    let on = |sim: &SimRunner, seg: SegmentId| -> Vec<BuildingId> {
        let g = sim.world().resource::<BuildingGrid>();
        active(sim)
            .into_iter()
            .filter(|&id| {
                g.get(id)
                    .unwrap()
                    .access()
                    .expect("an access point")
                    .segment
                    == seg
            })
            .collect()
    };
    loop {
        sim.tick_once();
        assert!(sim.tick().0 < 12 * TICKS_PER_DAY, "town never grew");
        let homed = sim.world().resource::<CitizenGrid>().citizen_count() >= 2;
        if homed && !on(&sim, b).is_empty() {
            break;
        }
    }
    ticks(&mut sim, TICKS_PER_DAY);
    sim.world_mut().resource_mut::<RoutineSettings>().decide = false;
    ticks(&mut sim, 2 * TICKS_PER_DAY);
    let target = on(&sim, b)[0];

    let go = |sim: &mut SimRunner| -> BuildingId {
        let (cid, home) = sim
            .world()
            .resource::<CitizenGrid>()
            .citizens()
            .filter(|(_, c)| c.is_adult() && c.location == Location::Home)
            .find_map(|(id, c)| c.home.map(|h| (id, h)))
            .expect("an adult at home");
        sim.world_mut()
            .resource_scope(|w, mut book: bevy_ecs::change_detection::Mut<TripBook>| {
                book.ensure_derived();
                book.add(Trip {
                    citizen: cid,
                    from: home,
                    to: target,
                    purpose: TripPurpose::Work,
                    mode: TripMode::Car,
                    state: TripState::Planned,
                    started: cp_core::Tick(0),
                })
                .expect("trip");
                w.resource_mut::<CitizenGrid>()
                    .set_location(cid, Location::Moving);
            });
        home
    };
    let home = go(&mut sim);
    ticks(&mut sim, 60);
    assert!(flags(&sim, target).has(BuildingFlags::NO_PATH));

    // Connect the two pieces and try again.
    line(&mut sim, &content, (3200.0, 3000.0), (3300.0, 3000.0));
    ticks(&mut sim, 3 * TICKS_PER_DAY);
    let home2 = go(&mut sim);
    ticks(&mut sim, 60);
    assert!(!flags(&sim, target).has(BuildingFlags::NO_PATH), "cleared");
    assert!(!flags(&sim, home2).has(BuildingFlags::NO_PATH));
    let _ = home;
}
