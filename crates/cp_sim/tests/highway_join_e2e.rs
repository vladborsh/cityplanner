//! BUG-041: a street the player joins to the entry highway is connected to it (EntryReach) and
//! the "City not connected to the highway" signal goes off once it is, with or without houses.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_content::{ContentDb, RoadClass, ZoneType};
use cp_core::treasury::Treasury;
use cp_core::{Cost, SimLimits};
use cp_net::{ElevationMode, EntryReach, NetCommand, NetGraph, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_zone::{ZoneCellRef, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn start(name: &str) -> (SimRunner, Arc<Terrain>, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(format!("{ROOT}/../../assets/content")).unwrap());
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps").join(name)).unwrap());
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(Arc::clone(&terrain));
    sim.set_content(Arc::clone(&db));
    sim.new_game().unwrap();
    sim.world_mut()
        .insert_resource(TileOwnership::only(terrain.meta().start_tile));
    sim.world_mut().resource_mut::<Treasury>().balance = Cost(1_000_000_000);
    sim.tick_once();
    (sim, terrain, db)
}

fn build(sim: &mut SimRunner, db: &ContentDb, from: Vec2, to: Vec2) {
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: db.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight { from, to },
            elevation: ElevationMode::Ground,
        }),
        sim.tick(),
    );
    sim.tick_once();
    let outcomes: Vec<_> = sim
        .world_mut()
        .resource_mut::<bevy_ecs::message::Messages<cp_core::CommandOutcome>>()
        .drain()
        .collect();
    assert!(
        outcomes.last().unwrap().result.is_ok(),
        "{:?}",
        outcomes.last()
    );
    sim.tick_once();
}

fn zone_streets(sim: &mut SimRunner, db: &ContentDb) {
    let basic = db.road_id("basic_2l").unwrap();
    let segs: Vec<_> = sim
        .world()
        .resource::<NetGraph>()
        .segments()
        .filter(|(_, s)| s.road == basic)
        .map(|(id, _)| id)
        .collect();
    let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
    for seg in segs {
        let blocks: Vec<_> = grid
            .blocks_of(seg)
            .into_iter()
            .flatten()
            .map(|id| (id, grid.block(id).unwrap().cell_count()))
            .collect();
        for (id, cells) in blocks {
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
}

fn streets_reached(sim: &SimRunner, db: &ContentDb) -> (usize, usize) {
    let net = sim.world().resource::<NetGraph>();
    let reach = sim.world().resource::<EntryReach>();
    let basic = db.road_id("basic_2l").unwrap();
    let all: Vec<_> = net.segments().filter(|(_, s)| s.road == basic).collect();
    let _ = RoadClass::Local;
    (
        all.iter().filter(|(id, _)| reach.reaches(*id)).count(),
        all.len(),
    )
}

fn warning(sim: &mut SimRunner) -> bool {
    for _ in 0..128 {
        sim.tick_once();
    }
    !sim.view().city.connected_to_highway
}

#[test]
fn a_street_joined_to_the_highway_is_connected_on_both_maps_at_the_end_and_mid_way() {
    for name in ["flatlands", "river_valley"] {
        for (case, offset) in [
            ("stub end", 0.0f32),
            ("mid stub", -24.0),
            ("mid highway", -100.0),
        ] {
            let (mut sim, terrain, db) = start(name);
            let e = terrain.meta().entry_road.clone().unwrap();
            let (dir, junction) = (e.direction(), e.junction());
            let side = Vec2::new(-dir.y, dir.x);
            let at = e.stub_end() + dir * offset;
            let (a, b) = (
                at + side * 8.0 * if offset < -100.0 { 6.0 } else { 0.0 },
                at + side * 140.0,
            );
            let _ = junction;
            build(&mut sim, &db, at, b);
            let _ = a;
            let (reached, all) = streets_reached(&sim, &db);
            assert!(
                all > 0 && reached == all,
                "{name} {case}: {reached}/{all} streets reached"
            );
            zone_streets(&mut sim, &db);
            assert!(
                !warning(&mut sim),
                "{name} {case}: warning with a joined street"
            );
        }
    }
}

#[test]
fn an_unjoined_street_still_warns() {
    let (mut sim, terrain, db) = start("flatlands");
    let e = terrain.meta().entry_road.clone().unwrap();
    let far = e.stub_end() + Vec2::new(400.0, 400.0);
    build(&mut sim, &db, far, far + Vec2::new(150.0, 0.0));
    zone_streets(&mut sim, &db);
    assert!(warning(&mut sim));
}

/// Zones painted or not, houses grown or not: a street that joins the highway lifts the warning.
#[test]
fn the_warning_does_not_wait_for_houses() {
    for name in ["flatlands", "river_valley"] {
        let (mut sim, terrain, db) = start(name);
        let e = terrain.meta().entry_road.clone().unwrap();
        let side = Vec2::new(-e.direction().y, e.direction().x);
        build(&mut sim, &db, e.stub_end(), e.stub_end() + side * 140.0);
        assert!(
            !warning(&mut sim),
            "{name}: warning with a joined street and no zones"
        );
    }
}
