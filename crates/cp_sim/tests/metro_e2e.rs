//! T-163: a metro line of three stations through a tunnel. Two trains each run A → B → C → B → A,
//! turn round at both ends and stand at every station; riders between A and C take the metro.

use std::collections::BTreeMap;
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
use cp_transit::{
    BusId, BusPhase, StopKind, StopRef, TransitCommand, TransitFleet, TransitMode, TransitNet,
    apply,
};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

struct Metro {
    sim: SimRunner,
    stations: Vec<BuildingId>,
    db: Arc<ContentDb>,
}

fn build() -> Metro {
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
    for k in 0..4 {
        let x = 2600.0 + 200.0 * k as f32;
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(x, 0.0, 3000.0), Vec3::new(x + 200.0, 0.0, 3000.0)),
                Elevation::Ground,
            )
            .unwrap();
    }
    sim.tick_once();
    // The test sends the citizens itself.
    sim.world_mut()
        .insert_resource(cp_citizen::RoutineSettings { decide: false });
    let mut stations = Vec::new();
    for x in [2700.0f32, 2900.0, 3300.0] {
        let before = sim.world().resource::<BuildingGrid>().len();
        sim.push_command(
            SimCommand::Building(BuildingCommand::Place {
                def: db.building_id("metro_station").unwrap(),
                pos: Vec2::new(x, 3010.0),
                rot: 0.0,
            }),
            sim.tick(),
        );
        sim.tick_once();
        let why: Vec<_> = sim
            .world_mut()
            .resource_mut::<bevy_ecs::message::Messages<cp_core::CommandOutcome>>()
            .drain()
            .map(|o| format!("{:?}", o.result))
            .collect();
        let g = sim.world().resource::<BuildingGrid>();
        assert_eq!(g.len(), before + 1, "station at {x} placed: {why:?}");
        stations.push(g.iter().map(|(id, _)| id).last().unwrap());
    }
    let mut m = Metro { sim, stations, db };
    m.tunnels();
    m
}

impl Metro {
    fn platform(&self, b: BuildingId) -> SegmentId {
        self.sim
            .world()
            .resource::<TransitNet>()
            .stops
            .iter()
            .find_map(|(_, s)| match s.kind {
                StopKind::Station { building, seg, .. } if building == b => Some(seg),
                _ => None,
            })
            .unwrap()
    }

    /// Ends of a platform as `(west, east)` points on the ground plane.
    fn ends(&self, seg: SegmentId) -> (Vec2, Vec2) {
        let net = self.sim.world().resource::<NetGraph>();
        let s = net.segment(seg).unwrap();
        let (a, b) = (net.node(s.start).unwrap().pos, net.node(s.end).unwrap().pos);
        let (a, b) = (Vec2::new(a.x, a.z), Vec2::new(b.x, b.z));
        if a.x < b.x { (a, b) } else { (b, a) }
    }

    /// Tunnels between the platforms, node to node.
    fn tunnels(&mut self) {
        let metro = self.db.road_id("metro_track").unwrap();
        let depth = 12.0;
        for w in self.stations.windows(2) {
            let (_, east) = self.ends(self.platform(w[0]));
            let (west, _) = self.ends(self.platform(w[1]));
            self.sim.push_command(
                SimCommand::Net(NetCommand::BuildRoad {
                    road: metro,
                    path: RoadPath::Straight {
                        from: east,
                        to: west,
                    },
                    elevation: ElevationMode::Tunnel(depth),
                }),
                self.sim.tick(),
            );
            self.sim.tick_once();
        }
    }

    fn line(&mut self, stations: &[BuildingId]) {
        apply(
            self.sim.world_mut(),
            &TransitCommand::CreateLine {
                mode: TransitMode::Metro,
                stops: stations.iter().map(|&b| StopRef::Station(b)).collect(),
            },
        )
        .unwrap();
        self.sim.tick_once();
    }
}

#[test]
fn two_trains_run_out_and_back_with_a_turn_at_both_ends() {
    let mut m = build();
    let all = m.stations.clone();
    m.line(&all);
    let line = m
        .sim
        .world()
        .resource::<TransitNet>()
        .lines
        .iter()
        .next()
        .unwrap()
        .0;
    assert_eq!(
        m.sim
            .world()
            .resource::<TransitNet>()
            .lines
            .get(line)
            .unwrap()
            .state,
        cp_transit::LineState::Ok
    );
    // 342 s of movement (one game hour before T-270, §2 v1.2): every train's stands, by
    // position of the ride (0 = A, 1 = B, 2 = C, 3 = B).
    let mut stands: BTreeMap<BusId, Vec<u16>> = BTreeMap::new();
    for _ in 0..(342.0 / cp_core::time::MOVE_SECONDS_PER_TICK).ceil() as u64 {
        m.sim.tick_once();
        for (id, bus) in m.sim.world().resource::<TransitFleet>().buses.iter() {
            if let BusPhase::Dwell { idx, .. } = bus.phase {
                let v = stands.entry(id).or_default();
                if v.last() != Some(&idx) {
                    v.push(idx);
                }
            }
        }
    }
    eprintln!("STANDS {stands:?}");
    assert_eq!(stands.len(), 2, "two trains: {stands:?}");
    for (id, v) in &stands {
        let ride = [0u16, 1, 2, 3, 0];
        assert!(
            v.windows(5).any(|w| w == ride),
            "train {id:?} stood at {v:?}: A B C B A with a turn at both ends"
        );
    }
}

/// A home by the first station and a workplace by the last: carless residents ride the metro.
#[test]
fn residents_between_the_first_and_the_last_station_ride_the_metro() {
    let mut m = build();
    let all = m.stations.clone();
    m.line(&all);
    let (home_seg, work_seg) = {
        let net = m.sim.world().resource::<NetGraph>();
        let seg_at = |x: f32| {
            net.segments()
                .find(|(_, s)| {
                    s.elevation == Elevation::Ground && s.curve.p0.x <= x && x <= s.curve.p3.x
                })
                .map(|(id, _)| id)
                .unwrap()
        };
        (seg_at(2700.0), seg_at(3300.0))
    };
    let db = Arc::clone(&m.db);
    let mut put = |name: &str, pos: Vec2, seg: SegmentId| {
        m.sim
            .world_mut()
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
    let home = put("house_low_1", Vec2::new(2690.0, 3008.0), home_seg);
    let work = put("com_low_1", Vec2::new(3310.0, 3008.0), work_seg);
    {
        let members = vec![(30 * 365, 0, false); 6];
        let mut g = m.sim.world_mut().resource_mut::<CitizenGrid>();
        g.ensure_derived();
        g.add_household(home, 0, Tick(0), &members);
        let ids: Vec<_> = g.citizens().map(|(i, _)| i).collect();
        for id in ids {
            g.set_location(id, Location::Home);
        }
    }
    let movers: Vec<_> = m
        .sim
        .world()
        .resource::<CitizenGrid>()
        .citizens()
        .map(|(id, _)| id)
        .collect();
    m.sim
        .world_mut()
        .resource_mut::<TripBook>()
        .ensure_derived();
    for citizen in movers {
        m.sim.world_mut().resource_mut::<TripBook>().add(Trip {
            citizen,
            from: home,
            to: work,
            purpose: TripPurpose::Work,
            mode: TripMode::Walk,
            state: TripState::Scheduled { depart_at: Tick(0) },
            started: Tick(0),
        });
    }
    // Trains first, riders next: the metro must run before a trip can choose it.
    for _ in 0..60 {
        m.sim.tick_once();
    }
    let mut max_riding = 0;
    for _ in 0..300 {
        m.sim.tick_once();
        let t = m.sim.world().resource::<TransitNet>();
        max_riding = max_riding.max(t.riders.len());
    }
    let t = m.sim.world().resource::<TransitNet>();
    let (_, line) = t.lines.iter().next().unwrap();
    let riders: u32 = t.stats.values().map(|s| s.riders_week).sum();
    eprintln!("RIDERS {riders} max {max_riding} line {:?}", line.state);
    assert!(riders > 0, "the metro carried nobody");
}
