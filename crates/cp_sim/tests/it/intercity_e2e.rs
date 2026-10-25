//! T-167: intercity trains. A new game with a train station joined to the railway gets a train
//! every `intercity_interval_h`; a share of the households moves in by train (they step off at
//! the station and walk home), the rest by car; tourists come by train too. Without a station
//! everything comes by car.

use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::BuildingCommand;
use cp_citizen::{CitizenGrid, TripBook, TripMode};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_core::treasury::Treasury;
use cp_core::{Cost, SimLimits};
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_transit::{Intercity, Tourists, TrainPhase};
use cp_zone::{ZoneCellRef, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// A new game on flatlands with houses and shops on a street joined to the entry highway, and
/// (with `station`) a train station on the railway with a street to the houses.
fn world(station: bool) -> SimRunner {
    let db = Arc::new(ContentDb::load_dir(format!("{ROOT}/../../assets/content")).unwrap());
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(Arc::clone(&terrain));
    sim.set_content(Arc::clone(&db));
    sim.new_game().unwrap();
    // T-283: the train station waits for Town; this test is about the station, not the unlock.
    sim.world_mut().insert_resource(cp_sim::Progress::ALL);
    sim.world_mut().insert_resource(TileOwnership::only((2, 2)));
    sim.world_mut().resource_mut::<Treasury>().balance = Cost(1_000_000_000);
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(1.0);
    *sim.world_mut().resource_mut::<cp_economy::Decay>() = cp_economy::Decay::disabled();
    let e = terrain.meta().entry_road.clone().unwrap();
    let end = terrain.meta().entry_rail.as_ref().unwrap().dead_end();
    let (stub, dir) = (e.stub_end(), e.direction());
    let side = Vec2::new(-dir.y, dir.x);
    let y = |p: Vec2| Vec3::new(p.x, terrain.height(p), p.y);
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let a = stub + side * 120.0;
    let mut streets: Vec<SegmentId> = Vec::new();
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let mut add = |p: Vec2, q: Vec2| {
            g.add_segment(
                &def,
                road,
                CubicBezier3::line(y(p), y(q)),
                Elevation::Ground,
            )
            .unwrap()
        };
        add(stub, a);
        for k in 0..3 {
            streets.push(add(
                a + dir * (160.0 * k as f32),
                a + dir * (160.0 * (k + 1) as f32),
            ));
        }
        if station {
            // The station's street, and the way from it to the houses.
            let z = end.y + 28.5;
            add(Vec2::new(end.x - 150.0, z), Vec2::new(end.x - 30.0, z));
            let mut p = Vec2::new(end.x - 150.0, z);
            let town = Vec2::new(a.x, z);
            while p.x > town.x + 0.5 {
                let q = Vec2::new((p.x - 100.0).max(town.x), z);
                add(p, q);
                p = q;
            }
            let mut p = town;
            let top = a + dir * 480.0;
            while p.y > top.y + 0.5 {
                let q = Vec2::new(town.x, (p.y - 100.0).max(top.y));
                add(p, q);
                p = q;
            }
        }
    }
    sim.tick_once();
    if station {
        sim.push_command(
            SimCommand::Building(BuildingCommand::Place {
                def: db.building_id("train_station").unwrap(),
                pos: Vec2::new(end.x - 64.0, end.y),
                rot: 0.0,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
    let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
    for street in streets {
        let blocks: Vec<_> = grid
            .blocks_of(street)
            .into_iter()
            .flatten()
            .map(|id| (id, grid.block(id).unwrap().cell_count()))
            .collect();
        for (id, cells) in blocks {
            for cell in 0..cells {
                let zone = if cell % 5 == 0 {
                    ZoneType::ComLow
                } else {
                    ZoneType::ResLow
                };
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    zone,
                );
            }
        }
    }
    sim
}

fn households(sim: &SimRunner) -> usize {
    sim.world().resource::<CitizenGrid>().household_count()
}

/// With a station: trains come and go, some households arrive by train and the rest do not, and
/// tourists get off the train and walk.
#[test]
fn trains_run_between_the_edge_and_the_station_and_bring_households_and_tourists() {
    let mut sim = world(true);
    let (mut dwelled, mut walkers) = (false, false);
    for _ in 0..4 * TICKS_PER_DAY {
        sim.tick_once();
        let ic = sim.world().resource::<Intercity>();
        if matches!(ic.train.map(|t| t.phase), Some(TrainPhase::Dwell { .. })) {
            dwelled = true;
            let runs = sim.world().resource::<cp_traffic::ServiceRuns>();
            let t = sim.world().resource::<Tourists>();
            walkers |= t.visits.keys().any(|id| {
                runs.get(*id)
                    .is_some_and(|r| r.kind == cp_traffic::WALK_KIND)
            });
        }
    }
    let ic = sim.world().resource::<Intercity>().clone();
    assert!(dwelled, "a train stood at the station");
    assert!(ic.trains_done >= 1, "a train came and left: {ic:?}");
    assert!(ic.moved_in > 0, "households stepped off a train: {ic:?}");
    assert!(
        households(&sim) as u32 > ic.moved_in,
        "the others came by car: {} households, {} by train",
        households(&sim),
        ic.moved_in
    );
    assert!(walkers, "tourists walked from the station");
}

/// Without a station nothing comes by train.
#[test]
fn without_a_station_everything_comes_by_car() {
    let mut sim = world(false);
    for _ in 0..3 * TICKS_PER_DAY {
        sim.tick_once();
    }
    let ic = sim.world().resource::<Intercity>();
    assert!(ic.is_empty(), "{ic:?}");
    assert!(households(&sim) > 0, "households came");
    let book = sim.world().resource::<TripBook>();
    assert!(book.iter().all(|(_, t)| t.mode != TripMode::Transit));
}

#[test]
fn the_train_service_is_deterministic() {
    let run = || {
        let mut sim = world(true);
        for _ in 0..2 * TICKS_PER_DAY {
            sim.tick_once();
        }
        (sim.checksum(), sim.world().resource::<Intercity>().clone())
    };
    assert_eq!(run(), run());
}

/// The client can draw the train: while it is out, `SimView::intercity_train` has the leg and
/// the head moves along it; at the station it stands at the end of the ride in.
#[test]
fn the_view_has_the_train_moving_along_its_leg_and_standing_at_the_station() {
    let mut sim = world(true);
    let (mut moved, mut stood) = (false, false);
    let mut last: Option<f32> = None;
    for _ in 0..2 * TICKS_PER_DAY {
        sim.tick_once();
        let phase = sim.world().resource::<Intercity>().train.map(|t| t.phase);
        let view = sim.view();
        match (phase, view.intercity_train.as_deref()) {
            (None, pose) => assert!(pose.is_none(), "no train, no pose"),
            (Some(TrainPhase::Inbound), Some(p)) => {
                assert!(p.s >= 0.0 && p.s <= p.length() + 0.01 && p.next_s >= p.s - 0.01);
                if last.is_some_and(|l| p.s > l + 0.01) {
                    moved = true;
                }
                last = Some(p.s);
            }
            (Some(TrainPhase::Dwell { .. }), Some(p)) => {
                assert!((p.s - p.length()).abs() < 0.01, "stands at the platform");
                stood = true;
            }
            _ => {}
        }
    }
    assert!(moved && stood, "moved {moved}, stood {stood}");
}
