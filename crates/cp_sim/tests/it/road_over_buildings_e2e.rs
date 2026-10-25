//! BUG-096: a road laid or widened over buildings takes them down. Upgrading a 2-cell street to a
//! 4-cell road eats the first row of cells on each side: the houses on it are demolished, no lot
//! is left on the road, and homes grow again on the new frontage. A protected building (a police station) under
//! the new road makes a plain road command refused, nothing changed; `NetDemolish` (the player
//! confirmed) takes it down with the houses.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_building::{
    Building, BuildingFlags, BuildingGrid, BuildingState, Demand, Footprint, buildings_under_road,
};
use cp_content::{ContentDb, ZoneType};
use cp_core::{CommandOutcome, Cost, Tick, Treasury};
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath, SegmentId};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_zone::{ZoneCellRef, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
/// The street runs east from here, 20 cells.
const A: Vec2 = Vec2::new(4400.0, 4400.0);
const LEN: f32 = 160.0;

fn sim() -> (SimRunner, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(96);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    sim.tick_once();
    sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(1_000_000_000.0);
    (sim, db)
}

/// Runs `cmd` now; its outcome.
fn run(sim: &mut SimRunner, cmd: SimCommand) -> Result<Cost, String> {
    let at = sim.tick();
    sim.push_command(cmd, at);
    sim.tick_once();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    let o = out.last().expect("an outcome");
    o.result.clone().map_err(|e| e.to_string())
}

fn street(sim: &mut SimRunner, db: &ContentDb) {
    let road = db.road_id("basic_2l").unwrap();
    run(
        sim,
        SimCommand::Net(NetCommand::BuildRoad {
            road,
            path: RoadPath::Straight {
                from: A,
                to: A + Vec2::new(LEN, 0.0),
            },
            elevation: ElevationMode::Ground,
        }),
    )
    .expect("the street");
    for _ in 0..4 {
        sim.tick_once();
    }
}

/// Homes on both sides of the street, grown with the demand held full.
fn grow_homes(sim: &mut SimRunner) {
    {
        let mut zones = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = zones.blocks().map(|(id, b)| (id, b.cell_count())).collect();
        for (id, cells) in ids {
            for cell in 0..cells {
                zones.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    ZoneType::ResLow,
                );
            }
        }
    }
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(1.0);
    for _ in 0..6000 {
        sim.tick_once();
        if sim.world().resource::<BuildingGrid>().len() >= 16 {
            break;
        }
    }
}

fn segments(sim: &SimRunner) -> Vec<SegmentId> {
    sim.world()
        .resource::<NetGraph>()
        .segments()
        .map(|(id, _)| id)
        .collect()
}

fn upgrade_all(sim: &SimRunner, db: &ContentDb) -> Vec<NetCommand> {
    let road = db.road_id("medium_4l").unwrap();
    segments(sim)
        .into_iter()
        .map(|seg| NetCommand::UpgradeSegment { seg, road })
        .collect()
}

/// Every road's strip as it stands.
fn strips(sim: &SimRunner, db: &ContentDb) -> Vec<(cp_core::curve::CubicBezier3, f32)> {
    sim.world()
        .resource::<NetGraph>()
        .segments()
        .map(|(_, s)| (s.curve, db.road(s.road).width * 0.5))
        .collect()
}

#[test]
fn upgrading_a_street_demolishes_exactly_the_houses_under_its_new_width() {
    let (mut sim, db) = sim();
    street(&mut sim, &db);
    grow_homes(&mut sim);
    let before = sim.world().resource::<BuildingGrid>().len();
    let mut hit: Vec<_> = upgrade_all(&sim, &db)
        .iter()
        .flat_map(|c| cp_sim::buildings_under(sim.world(), c))
        .collect();
    hit.sort();
    hit.dedup();
    assert!(
        before >= 8 && !hit.is_empty(),
        "{before} buildings, {} under the new width",
        hit.len()
    );
    for c in upgrade_all(&sim, &db) {
        run(&mut sim, SimCommand::Net(c)).expect("the upgrade");
    }
    for _ in 0..8 {
        sim.tick_once();
    }
    let grid = sim.world().resource::<BuildingGrid>();
    let on_road = buildings_under_road(grid, &strips(&sim, &db));
    assert!(on_road.is_empty(), "buildings on the road: {on_road:?}");
    // Exactly the hit ones are gone (a house's lot starts at the road edge, so every house that
    // faces the street loses its front cell to the wider road).
    assert!(hit.iter().all(|id| grid.get(*id).is_none()));
    assert_eq!(grid.len(), before - hit.len());
    // The zoning keeps no cell on the wider road.
    let zones = sim.world().resource::<ZoneGrid>();
    let hw = db.road(db.road_id("medium_4l").unwrap()).width * 0.5;
    for (id, _) in zones.blocks() {
        for c in zones
            .block_cells(id)
            .into_iter()
            .filter(|c| zones.is_valid(*c))
        {
            let o = zones.cell_footprint(c).unwrap();
            let d = o.distance_to_segment(A, A + Vec2::new(LEN, 0.0));
            assert!(
                d >= hw - cp_zone::grid::ROAD_EPS - 0.02,
                "a cell at {:?} lies {:.2} m on the road",
                o.c,
                hw - d
            );
        }
    }
    // The frontage of the wider road grows homes again, clear of it.
    for _ in 0..6000 {
        sim.tick_once();
        if sim.world().resource::<BuildingGrid>().len() > before - hit.len() {
            break;
        }
    }
    let grid = sim.world().resource::<BuildingGrid>();
    assert!(
        grid.len() > before - hit.len(),
        "no home grew on the wider road"
    );
    assert!(buildings_under_road(grid, &strips(&sim, &db)).is_empty());
}

#[test]
fn a_police_station_under_the_new_width_asks_first() {
    let (mut sim, db) = sim();
    street(&mut sim, &db);
    // A 2 x 2 station on the first row of cells north of the street (8..24 m off its axis).
    let def = db.building_id("police_station").unwrap();
    let police = sim
        .world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
            def,
            footprint: Footprint::Free { w: 2, d: 2 },
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos: A + Vec2::new(LEN / 2.0, -16.5),
            rot: 0.0,
            access: None,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
    let ups = upgrade_all(&sim, &db);
    let seg = segments(&sim);
    let target = db.road_id("medium_4l").unwrap();
    let road_of = |sim: &SimRunner, s: SegmentId| {
        sim.world()
            .resource::<NetGraph>()
            .segment(s)
            .map(|s| s.road)
    };
    let mut refused = 0;
    for c in &ups {
        if let Err(e) = run(&mut sim, SimCommand::Net(c.clone())) {
            assert!(e.contains("would demolish"), "{e}");
            refused += 1;
        }
    }
    assert!(refused > 0, "no upgrade was refused over the station");
    assert!(
        sim.world().resource::<BuildingGrid>().get(police).is_some(),
        "the station is still there"
    );
    // Confirmed: the station goes, every piece is the wide road.
    for s in &seg {
        if road_of(&sim, *s) != Some(target) {
            run(
                &mut sim,
                SimCommand::NetDemolish(NetCommand::UpgradeSegment {
                    seg: *s,
                    road: target,
                }),
            )
            .expect("the confirmed upgrade");
        }
    }
    assert!(sim.world().resource::<BuildingGrid>().get(police).is_none());
    assert!(seg.iter().all(|s| road_of(&sim, *s) == Some(target)));
}

#[test]
fn a_new_road_across_houses_demolishes_them() {
    let (mut sim, db) = sim();
    street(&mut sim, &db);
    grow_homes(&mut sim);
    let road = db.road_id("basic_2l").unwrap();
    // A cross street through the middle of the homes south of the street, to the street.
    let mid = A + Vec2::new(LEN / 2.0, 0.0);
    let cmd = NetCommand::BuildRoad {
        road,
        path: RoadPath::Straight {
            from: mid + Vec2::new(0.0, 80.0),
            to: mid,
        },
        elevation: ElevationMode::Ground,
    };
    let hits = cp_sim::buildings_under(sim.world(), &cmd);
    assert!(!hits.is_empty(), "the cross street hits no house");
    run(&mut sim, SimCommand::Net(cmd)).expect("the cross street");
    let grid = sim.world().resource::<BuildingGrid>();
    assert!(hits.iter().all(|id| grid.get(*id).is_none()));
    assert!(buildings_under_road(grid, &strips(&sim, &db)).is_empty());
}
