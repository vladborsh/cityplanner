//! T-302b, T-302c: trams and trolleybuses. A line over a square of `tram_4l` (`trolley_4l`)
//! carries carless residents from home to work; the vehicles drive on their own lanes from the
//! mode's depot; a stop on a road without rails (wires) is refused; turning the road back into
//! `medium_4l` breaks the line; the run replays alike.

use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_citizen::{CitizenGrid, Location, Trip, TripBook, TripMode, TripPurpose, TripState};
use cp_content::{ContentDb, LaneKind, VehicleKind};
use cp_core::curve::CubicBezier3;
use cp_core::{Id, Tick};
use cp_net::{Elevation, NetCommand, NetGraph, SegmentId};
use cp_sim::SimRunner;
use cp_traffic::{Agents, Side};
use cp_transit::{
    LineState, StopError, StopRef, TransitCommand, TransitError, TransitFleet, TransitMode,
    TransitNet, apply,
};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const SIDE: f32 = 800.0;

/// A mode under test: its road, depot, vehicles and lanes.
#[derive(Clone, Copy)]
struct Mode {
    mode: TransitMode,
    road: &'static str,
    depot: &'static str,
    vehicle: VehicleKind,
    lane: LaneKind,
}

const TRAM: Mode = Mode {
    mode: TransitMode::Tram,
    road: "tram_4l",
    depot: "tram_depot",
    vehicle: VehicleKind::Tram,
    lane: LaneKind::Tram,
};

const TROLLEY: Mode = Mode {
    mode: TransitMode::Trolley,
    road: "trolley_4l",
    depot: "trolley_depot",
    vehicle: VehicleKind::Trolley,
    lane: LaneKind::Trolley,
};

struct Town {
    m: Mode,
    sim: SimRunner,
    db: Arc<ContentDb>,
    sides: Vec<SegmentId>,
    /// A plain street off the first corner, without rails.
    street: SegmentId,
    home: BuildingId,
    work: BuildingId,
}

/// An 800 m square of the mode's road with its depot on the first side, a home by the middle of
/// the first side, a workplace by the middle of the third and a `basic_2l` street off a corner.
fn town(m: Mode) -> Town {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(&db));
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    // The square lies on the first tile, the player's.
    sim.world_mut()
        .insert_resource(cp_terrain::TileOwnership::only((0, 0)));
    let road = |sim: &mut SimRunner, name: &str, a: (f32, f32), b: (f32, f32)| {
        let id = db.road_id(name).unwrap();
        let def = db.road(id).clone();
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                id,
                CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1)),
                Elevation::Ground,
            )
            .unwrap()
    };
    let c = [
        (0.0, 0.0),
        (SIDE, 0.0),
        (SIDE, SIDE),
        (0.0, SIDE),
        (0.0, 0.0),
    ];
    let sides: Vec<SegmentId> = c
        .windows(2)
        .map(|w| road(&mut sim, m.road, w[0], w[1]))
        .collect();
    let street = road(&mut sim, "basic_2l", (0.0, 0.0), (-300.0, 0.0));
    let mut put = |name: &str, pos: Vec2, seg: SegmentId, t: f32| {
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
                access: Some(AccessPoint { segment: seg, t }),
                flags: BuildingFlags(0),
                level_progress: 0.0,
            })
    };
    put(m.depot, Vec2::new(200.0, 20.0), sides[0], 0.25);
    let home = put("house_low_1", Vec2::new(SIDE / 2.0, 20.0), sides[0], 0.5);
    let work = put(
        "com_low_1",
        Vec2::new(SIDE / 2.0, SIDE - 20.0),
        sides[2],
        0.5,
    );
    // The test sends the citizens itself.
    sim.world_mut()
        .insert_resource(cp_citizen::RoutineSettings { decide: false });
    sim.tick_once();
    Town {
        m,
        sim,
        db,
        sides,
        street,
        home,
        work,
    }
}

impl Town {
    fn create(&mut self, stops: Vec<StopRef>) -> Result<(), TransitError> {
        apply(
            self.sim.world_mut(),
            &TransitCommand::CreateLine {
                mode: self.m.mode,
                stops,
            },
        )
        .map(|_| ())
    }

    fn line(&mut self) {
        let stops = (0..4)
            .map(|i| StopRef::NewRoad {
                seg: self.sides[i],
                s: SIDE / 2.0,
                side: Side::Right,
            })
            .collect();
        self.create(stops).unwrap();
    }

    /// A household of `n` carless adults at the home.
    fn household(&mut self, n: usize) {
        let home = self.home;
        let mut g = self.sim.world_mut().resource_mut::<CitizenGrid>();
        g.ensure_derived();
        g.add_household(home, 0, Tick(0), &vec![(30 * 365, 0, false); n]);
        let ids: Vec<_> = g.citizens().map(|(i, _)| i).collect();
        for id in ids {
            g.set_location(id, Location::Home);
        }
    }

    /// Sends every adult at home to the workplace.
    fn send_all(&mut self) -> usize {
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
        movers
            .into_iter()
            .filter(|&citizen| {
                w.resource_mut::<TripBook>()
                    .add(Trip {
                        citizen,
                        from: home,
                        to: work,
                        purpose: TripPurpose::Work,
                        mode: TripMode::Walk,
                        state: TripState::Scheduled { depart_at: Tick(0) },
                        started: Tick(0),
                    })
                    .is_some()
            })
            .count()
    }

    fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.sim.tick_once();
        }
    }

    fn arrived(&self) -> usize {
        self.sim
            .world()
            .resource::<CitizenGrid>()
            .citizens()
            .filter(|(_, c)| c.location == Location::Work)
            .count()
    }

    fn line_state(&self) -> LineState {
        let net = self.sim.world().resource::<TransitNet>();
        net.lines.iter().next().unwrap().1.state
    }

    /// The mode's vehicles on the map: the lane kind each stands on.
    fn vehicles(&self) -> Vec<LaneKind> {
        let net = self.sim.world().resource::<NetGraph>();
        self.sim
            .world()
            .resource::<Agents>()
            .vehicles()
            .filter(|(_, v)| v.spawned && v.kind_id() == self.m.vehicle as u8)
            .filter_map(|(_, v)| net.lane(v.lane()).map(|l| l.kind))
            .collect()
    }
}

fn carless_residents_ride_and_keep_to_their_lanes(m: Mode) {
    let mut t = town(m);
    t.line();
    t.household(3);
    t.run(120);
    assert!(!t.sim.world().resource::<TransitFleet>().buses.is_empty());
    assert_eq!(t.line_state(), LineState::Ok);
    assert_eq!(t.send_all(), 3);
    t.run(2);
    let modes: Vec<TripMode> = t
        .sim
        .world()
        .resource::<TripBook>()
        .iter()
        .map(|(_, trip)| trip.mode)
        .collect();
    assert_eq!(modes, vec![TripMode::Transit; 3]);
    let mut seen = Vec::new();
    for _ in 0..30 {
        t.run(50);
        seen.extend(t.vehicles());
    }
    assert!(!seen.is_empty(), "the line's vehicles drove");
    assert!(
        seen.iter().all(|&k| k == m.lane),
        "only on {:?} lanes: {seen:?}",
        m.lane
    );
    assert_eq!(t.arrived(), 3, "all three reached the workplace");
    let riders: u32 = t
        .sim
        .world()
        .resource::<TransitNet>()
        .stats
        .values()
        .map(|s| s.riders_today)
        .sum();
    assert!(riders > 0);
}

#[test]
fn carless_residents_ride_the_tram_and_the_trams_keep_to_the_rails() {
    carless_residents_ride_and_keep_to_their_lanes(TRAM);
}

#[test]
fn carless_residents_ride_the_trolleybus_and_it_keeps_under_the_wires() {
    carless_residents_ride_and_keep_to_their_lanes(TROLLEY);
}

fn a_stop_needs_the_lane_and_a_road_without_it_breaks_the_line(m: Mode) {
    let mut t = town(m);
    let street = t.street;
    let off_lane = t.create(vec![
        StopRef::NewRoad {
            seg: t.sides[0],
            s: SIDE / 2.0,
            side: Side::Right,
        },
        StopRef::NewRoad {
            seg: street,
            s: 150.0,
            side: Side::Right,
        },
    ]);
    assert_eq!(off_lane, Err(TransitError::Stop(StopError::NoLane)));

    t.line();
    t.run(60);
    assert_eq!(t.line_state(), LineState::Ok);
    // The third side loses its lanes: the ring has no way round for the line.
    let plain = t.db.road_id("medium_4l").unwrap();
    let seg = t.sides[2];
    cp_net::apply(
        t.sim.world_mut(),
        &NetCommand::UpgradeSegment { seg, road: plain },
    )
    .unwrap();
    t.run(5);
    let now = t
        .sim
        .world()
        .resource::<NetGraph>()
        .segment(seg)
        .map(|s| s.road);
    assert_eq!(now, Some(plain), "upgraded");
    assert_eq!(t.line_state(), LineState::Broken);
}

#[test]
fn a_tram_stop_needs_rails_and_a_road_without_them_breaks_the_line() {
    a_stop_needs_the_lane_and_a_road_without_it_breaks_the_line(TRAM);
}

#[test]
fn a_trolleybus_stop_needs_wires_and_a_road_without_them_breaks_the_line() {
    a_stop_needs_the_lane_and_a_road_without_it_breaks_the_line(TROLLEY);
}

/// Two runs of the same town give the same state.
fn replays_alike(m: Mode) {
    let run = || {
        let mut t = town(m);
        t.line();
        t.household(3);
        t.run(120);
        t.send_all();
        t.run(600);
        t.sim.checksum()
    };
    assert_eq!(run(), run());
}

#[test]
fn the_tram_town_replays_alike() {
    replays_alike(TRAM);
}

#[test]
fn the_trolleybus_town_replays_alike() {
    replays_alike(TROLLEY);
}
