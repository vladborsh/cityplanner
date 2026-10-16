//! T-165: a change between a bus line and a metro line. Homes by the bus line L1, work by a
//! metro station of L2, a stop of L1 70 m from the station: the trip goes L1, on foot, L2.
//! Without L2 the trip is on foot again.

use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingCommand, BuildingFlags, BuildingGrid, BuildingId, BuildingState,
    Footprint, Lot,
};
use cp_citizen::{CitizenGrid, Location, Trip, TripBook, TripMode, TripPurpose, TripState};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::treasury::Treasury;
use cp_core::{Cost, Id, Tick};
use cp_net::{Elevation, ElevationMode, NetCommand, NetGraph, RoadPath, SegmentId};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_traffic::Side;
use cp_transit::{LineId, StopKind, StopRef, TransitCommand, TransitMode, TransitNet, apply};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

struct World {
    sim: SimRunner,
    home: BuildingId,
    work: BuildingId,
    stations: Vec<BuildingId>,
    sides: Vec<SegmentId>,
}

fn world() -> World {
    let db = Arc::new(ContentDb::load_dir(format!("{ROOT}/../../assets/content")).unwrap());
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(&db));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    sim.world_mut().resource_mut::<Treasury>().balance = Cost(1_000_000_000);
    let sides: Vec<SegmentId> = (0..9)
        .map(|k| {
            let x = 2000.0 + 200.0 * k as f32;
            sim.world_mut()
                .resource_mut::<NetGraph>()
                .add_segment(
                    &def,
                    road,
                    CubicBezier3::line(
                        Vec3::new(x, 0.0, 3000.0),
                        Vec3::new(x + 200.0, 0.0, 3000.0),
                    ),
                    Elevation::Ground,
                )
                .unwrap()
        })
        .collect();
    sim.tick_once();
    sim.world_mut()
        .insert_resource(cp_citizen::RoutineSettings { decide: false });
    let mut stations = Vec::new();
    for x in [2700.0f32, 3500.0] {
        sim.push_command(
            SimCommand::Building(BuildingCommand::Place {
                def: db.building_id("metro_station").unwrap(),
                pos: Vec2::new(x, 3010.0),
                rot: 0.0,
            }),
            sim.tick(),
        );
        sim.tick_once();
        stations.push(
            sim.world()
                .resource::<BuildingGrid>()
                .iter()
                .map(|(id, _)| id)
                .last()
                .unwrap(),
        );
    }
    // The tunnel between the platforms.
    let ends: Vec<(Vec2, Vec2)> = {
        let transit = sim.world().resource::<TransitNet>();
        let net = sim.world().resource::<NetGraph>();
        let mut v: Vec<_> = transit
            .stops
            .iter()
            .filter_map(|(_, s)| match s.kind {
                StopKind::Station { seg, .. } => net.segment(seg),
                _ => None,
            })
            .map(|s| {
                let (a, b) = (net.node(s.start).unwrap().pos, net.node(s.end).unwrap().pos);
                let (a, b) = (Vec2::new(a.x, a.z), Vec2::new(b.x, b.z));
                if a.x < b.x { (a, b) } else { (b, a) }
            })
            .collect();
        v.sort_by(|p, q| p.0.x.total_cmp(&q.0.x));
        v
    };
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: db.road_id("metro_track").unwrap(),
            path: RoadPath::Straight {
                from: ends[0].1,
                to: ends[1].0,
            },
            elevation: ElevationMode::Tunnel(12.0),
        }),
        sim.tick(),
    );
    sim.tick_once();
    let mut put = |name: &str, pos: Vec2, seg: SegmentId| {
        sim.world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(Building {
                def: db.building_id(name).unwrap(),
                footprint: Footprint::Lot(Lot {
                    block: Id::new(0, 0),
                    col: 0,
                    w: 1,
                    d: 1,
                }),
                level: 1,
                state: BuildingState::Active,
                ready_at: Tick(0),
                pos,
                rot: 0.0,
                access: Some(AccessPoint {
                    segment: seg,
                    t: 0.5,
                }),
                flags: BuildingFlags(0),
                level_progress: 0.0,
            })
    };
    put("bus_depot", Vec2::new(2300.0, 3008.0), sides[1]);
    let home = put("house_low_1", Vec2::new(2100.0, 3008.0), sides[0]);
    let work = put("com_low_1", Vec2::new(3510.0, 3008.0), sides[7]);
    World {
        sim,
        home,
        work,
        stations,
        sides,
    }
}

impl World {
    fn lines(&mut self) -> (LineId, LineId) {
        // L1: two stops, by the home and 70 m from the first station.
        apply(
            self.sim.world_mut(),
            &TransitCommand::CreateLine {
                mode: TransitMode::Bus,
                stops: vec![
                    StopRef::NewRoad {
                        seg: self.sides[0],
                        s: 100.0,
                        side: Side::Right,
                    },
                    StopRef::NewRoad {
                        seg: self.sides[3],
                        s: 170.0,
                        side: Side::Right,
                    },
                ],
            },
        )
        .unwrap();
        apply(
            self.sim.world_mut(),
            &TransitCommand::CreateLine {
                mode: TransitMode::Metro,
                stops: self.stations.iter().map(|&b| StopRef::Station(b)).collect(),
            },
        )
        .unwrap();
        self.sim.tick_once();
        let net = self.sim.world().resource::<TransitNet>();
        let mut ids = net.lines.iter().map(|(id, _)| id);
        (ids.next().unwrap(), ids.next().unwrap())
    }

    fn household(&mut self, n: usize) {
        let members = vec![(30 * 365, 0, false); n];
        let home = self.home;
        let mut g = self.sim.world_mut().resource_mut::<CitizenGrid>();
        g.ensure_derived();
        g.add_household(home, 0, Tick(0), &members);
        let ids: Vec<_> = g.citizens().map(|(i, _)| i).collect();
        for id in ids {
            g.set_location(id, Location::Home);
        }
    }

    fn send_all(&mut self) {
        let movers: Vec<_> = self
            .sim
            .world()
            .resource::<CitizenGrid>()
            .citizens()
            .filter(|(_, c)| c.location == Location::Home)
            .map(|(id, _)| id)
            .collect();
        let (home, work) = (self.home, self.work);
        let w = self.sim.world_mut();
        w.resource_mut::<TripBook>().ensure_derived();
        for citizen in movers {
            w.resource_mut::<TripBook>().add(Trip {
                citizen,
                from: home,
                to: work,
                purpose: TripPurpose::Work,
                mode: TripMode::Walk,
                state: TripState::Scheduled { depart_at: Tick(0) },
            });
        }
    }

    fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.sim.tick_once();
        }
    }

    fn at_work(&self) -> usize {
        self.sim
            .world()
            .resource::<CitizenGrid>()
            .citizens()
            .filter(|(_, c)| c.location == Location::Work)
            .count()
    }
}

/// Runs the whole scenario and returns a summary of what happened (for the determinism check).
fn scenario() -> String {
    let mut w = world();
    let (l1, l2) = w.lines();
    w.household(3);
    // Buses and trains come out first.
    w.run(300);
    w.send_all();
    w.run(2);
    let net = w.sim.world().resource::<TransitNet>();
    assert_eq!(net.riders.len(), 3, "three journeys");
    for j in net.riders.values() {
        let lines: Vec<_> = j.legs.iter().map(|r| r.line).collect();
        assert_eq!(lines, vec![l1, l2], "L1, then L2: {j:?}");
    }
    let mut changed_on_foot = false;
    for _ in 0..3000 {
        w.sim.tick_once();
        let net = w.sim.world().resource::<TransitNet>();
        changed_on_foot |= net
            .riders
            .values()
            .any(|j| j.leg == 1 && matches!(j.stage, cp_transit::Stage::WalkTo { .. }));
        if net.riders.is_empty() && w.at_work() == 3 {
            break;
        }
    }
    assert!(changed_on_foot, "somebody walked between the two lines");
    assert_eq!(w.at_work(), 3, "all three arrived");
    let net = w.sim.world().resource::<TransitNet>();
    for line in [l1, l2] {
        let s = net.stats.get(&line).copied().unwrap_or_default();
        assert!(s.riders_week > 0, "line {line:?} carried nobody: {s:?}");
    }
    format!("{:?} {:?}", net.stats, w.sim.tick())
}

#[test]
fn a_trip_changes_from_the_bus_to_the_metro_on_foot() {
    scenario();
}

#[test]
fn the_same_world_gives_the_same_trips() {
    assert_eq!(scenario(), scenario());
}

#[test]
fn without_the_metro_the_trip_is_on_foot_again() {
    let mut w = world();
    let (_, l2) = w.lines();
    w.household(2);
    w.run(300);
    apply(w.sim.world_mut(), &TransitCommand::DeleteLine { line: l2 }).unwrap();
    w.run(2);
    w.send_all();
    w.run(2);
    let modes: Vec<_> = w
        .sim
        .world()
        .resource::<TripBook>()
        .iter()
        .map(|(_, t)| t.mode)
        .collect();
    assert_eq!(modes.len(), 2);
    assert!(modes.iter().all(|&m| m != TripMode::Transit), "{modes:?}");
}
