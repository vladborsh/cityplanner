//! T-162: a station lays its platform under itself, together with the building and its stop;
//! a platform that cannot be built refuses the building; the bulldozer takes platform and stop
//! with the building and does not take the platform alone.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingCommand, BuildingGrid, BuildingId};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::treasury::Treasury;
use cp_core::{CommandError, CommandOutcome};
use cp_net::{Elevation, ElevationMode, NetCommand, NetGraph, RoadPath, SegmentId};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_transit::{StopKind, StopRef, TransitCommand, TransitMode, TransitNet};
use cp_zone::{ZoneCellRef, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn world() -> (SimRunner, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(format!("{ROOT}/../../assets/content")).unwrap());
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(&db));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3200.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.world_mut().resource_mut::<Treasury>().balance = cp_core::Cost(100_000_000);
    sim.tick_once();
    (sim, db)
}

fn run(sim: &mut SimRunner, cmd: SimCommand) -> Result<(), String> {
    sim.push_command(cmd, sim.tick());
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    match out.last().unwrap().result.clone() {
        Ok(_) => Ok(()),
        Err(CommandError::Rejected { reason, .. }) => Err(reason),
        Err(e) => panic!("{e:?}"),
    }
}

fn spot(sim: &SimRunner, col: u16) -> Vec2 {
    let zones = sim.world().resource::<ZoneGrid>();
    let (id, _) = zones.blocks().next().unwrap();
    zones
        .cell_footprint(ZoneCellRef {
            block: id,
            cell: col * 4 + 1,
        })
        .unwrap()
        .c
}

fn place(sim: &mut SimRunner, db: &ContentDb, at: Vec2) -> Result<(), String> {
    let def = db.building_id("metro_station").unwrap();
    run(
        sim,
        SimCommand::Building(BuildingCommand::Place {
            def,
            pos: at,
            rot: 0.0,
        }),
    )
}

fn tunnels(sim: &SimRunner) -> Vec<SegmentId> {
    sim.world()
        .resource::<NetGraph>()
        .segments()
        .filter(|(_, s)| s.elevation == Elevation::Tunnel)
        .map(|(id, _)| id)
        .collect()
}

fn station(sim: &SimRunner) -> (BuildingId, SegmentId) {
    let t = sim.world().resource::<TransitNet>();
    let (_, st) = t.stops.iter().next().expect("the station has a stop");
    match st.kind {
        StopKind::Station { building, seg, .. } => (building, seg),
        k => panic!("{k:?}"),
    }
}

#[test]
fn a_station_lays_a_platform_of_its_length_and_depth_and_a_tunnel_joins_it() {
    let (mut sim, db) = world();
    let at = spot(&sim, 5);
    place(&mut sim, &db, at).unwrap();
    let (building, seg) = station(&sim);
    assert!(
        sim.world()
            .resource::<BuildingGrid>()
            .get(building)
            .is_some()
    );

    let track = db
        .building(db.building_id("metro_station").unwrap())
        .track
        .clone()
        .unwrap();
    assert_eq!(tunnels(&sim), vec![seg]);
    let (start, end, length) = {
        let net = sim.world().resource::<NetGraph>();
        let s = net.segment(seg).unwrap();
        assert_eq!(db.road(s.road).id, track.road);
        assert!((s.length - track.length_m).abs() < 0.5, "{}", s.length);
        (s.start, s.end, s.length)
    };
    let terrain = sim.world().resource::<cp_terrain::SimTerrain>().0.clone();
    {
        let net = sim.world().resource::<NetGraph>();
        for n in [start, end] {
            let p = net.node(n).unwrap().pos;
            let ground = terrain.height(Vec2::new(p.x, p.z));
            assert!((ground - p.y - track.depth).abs() < 0.01, "depth at {p:?}");
        }
    }
    // The stop stands halfway along the platform.
    let t = sim.world().resource::<TransitNet>();
    assert!((t.stops.iter().next().unwrap().1.kind.s() - length * 0.5).abs() < 0.01);

    // A tunnel built from the platform's end joins its node.
    let p = sim.world().resource::<NetGraph>().node(end).unwrap().pos;
    let metro = db.road_id("metro_track").unwrap();
    run(
        &mut sim,
        SimCommand::Net(NetCommand::BuildRoad {
            road: metro,
            path: RoadPath::Straight {
                from: Vec2::new(p.x, p.z),
                to: Vec2::new(p.x + 80.0, p.z + 40.0),
            },
            elevation: ElevationMode::Tunnel(track.depth),
        }),
    )
    .unwrap();
    assert_eq!(tunnels(&sim).len(), 2);
    let net = sim.world().resource::<NetGraph>();
    assert_eq!(
        net.node(end).unwrap().segments.len(),
        2,
        "one node, two tunnels"
    );

    // The station is a stop a line may name (the lines themselves come with T-163).
    let err = run(
        &mut sim,
        SimCommand::Transit(TransitCommand::CreateLine {
            mode: TransitMode::Metro,
            stops: vec![StopRef::Station(building), StopRef::Station(building)],
        }),
    );
    assert!(err.is_err());
}

#[test]
fn a_platform_that_cannot_be_built_refuses_the_building_and_touches_nothing() {
    let (mut sim, db) = world();
    let at = spot(&sim, 5);
    // A tunnel 3 m off the depth of the platform crosses its path: no node, no room to pass.
    let metro = db.road_id("metro_track").unwrap();
    run(
        &mut sim,
        SimCommand::Net(NetCommand::BuildRoad {
            road: metro,
            path: RoadPath::Straight {
                from: Vec2::new(at.x + 10.0, at.y - 45.0),
                to: Vec2::new(at.x + 10.0, at.y + 45.0),
            },
            elevation: ElevationMode::Tunnel(15.0),
        }),
    )
    .unwrap();
    let net_before = sim.world().resource::<NetGraph>().clone();
    let funds = *sim.world().resource::<Treasury>();
    let err = place(&mut sim, &db, at).unwrap_err();
    assert!(err.contains("platform"), "{err}");
    assert!(sim.world().resource::<BuildingGrid>().is_empty());
    assert!(sim.world().resource::<NetGraph>() == &net_before);
    assert!(sim.world().resource::<TransitNet>().stops.is_empty());
    assert!(sim.world().resource::<Treasury>() == &funds);
    let z = sim.world().resource::<ZoneGrid>();
    let (id, _) = z.blocks().next().unwrap();
    assert!(!z.is_occupied(ZoneCellRef {
        block: id,
        cell: 5 * 4 + 1
    }));
}

#[test]
fn demolishing_the_station_takes_platform_and_stop_and_the_platform_alone_stays() {
    let (mut sim, db) = world();
    let at = spot(&sim, 5);
    place(&mut sim, &db, at).unwrap();
    let (building, seg) = station(&sim);

    let err = run(&mut sim, SimCommand::Net(NetCommand::RemoveSegment { seg })).unwrap_err();
    assert!(err.contains("platform"), "{err}");
    assert_eq!(tunnels(&sim), vec![seg]);

    run(
        &mut sim,
        SimCommand::Building(BuildingCommand::Demolish { building }),
    )
    .unwrap();
    assert!(tunnels(&sim).is_empty());
    assert!(sim.world().resource::<TransitNet>().stops.is_empty());
    assert!(sim.world().resource::<BuildingGrid>().is_empty());
}
