//! T-156: buses leave the depot, call at the stops of their line in order and stand there for
//! `dwell_s`; surplus buses go home; no depot means `NoDepot`; a line that breaks sends its
//! buses home.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::{Id, Tick};
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_sim::SimRunner;
use cp_traffic::{Agents, Side};
use cp_transit::{
    BusId, BusPhase, LineId, LineState, StopRef, TransitCommand, TransitFleet, TransitMode,
    TransitNet, apply,
};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

struct Town {
    sim: SimRunner,
    /// The four sides of a 200 m square.
    sides: Vec<SegmentId>,
    /// `(first tick, last tick)` of every dwell, by bus and stop index, in order of occurrence.
    dwells: BTreeMap<BusId, Vec<(u16, u64, u64)>>,
}

fn town() -> Town {
    let db = Arc::new(ContentDb::load_dir(format!("{ROOT}/../../assets/content")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(&db));
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let c = [
        (0.0, 0.0),
        (200.0, 0.0),
        (200.0, 200.0),
        (0.0, 200.0),
        (0.0, 0.0),
    ];
    let sides = c
        .windows(2)
        .map(|w| {
            sim.world_mut()
                .resource_mut::<NetGraph>()
                .add_segment(
                    &def,
                    road,
                    CubicBezier3::line(
                        Vec3::new(w[0].0, 0.0, w[0].1),
                        Vec3::new(w[1].0, 0.0, w[1].1),
                    ),
                    Elevation::Ground,
                )
                .unwrap()
        })
        .collect();
    sim.tick_once();
    Town {
        sim,
        sides,
        dwells: BTreeMap::new(),
    }
}

impl Town {
    fn depot(&mut self) -> BuildingId {
        let db = self
            .sim
            .world()
            .resource::<cp_content::SimContent>()
            .0
            .clone();
        self.sim
            .world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(Building {
                def: db.building_id("bus_depot").unwrap(),
                footprint: Footprint::Lot(Lot {
                    block: Id::new(0, 0),
                    col: 0,
                    w: 1,
                    d: 1,
                }),
                level: 1,
                state: BuildingState::Active,
                ready_at: Tick(0),
                pos: Vec2::new(100.0, 8.0),
                rot: 0.0,
                access: Some(AccessPoint {
                    segment: self.sides[0],
                    t: 0.5,
                }),
                flags: BuildingFlags(0),
                level_progress: 0.0,
            })
    }

    fn line(&mut self, sides: &[usize]) -> LineId {
        let stops = sides
            .iter()
            .map(|&i| StopRef::NewRoad {
                seg: self.sides[i],
                s: 100.0,
                side: Side::Right,
            })
            .collect();
        apply(
            self.sim.world_mut(),
            &TransitCommand::CreateLine {
                mode: TransitMode::Bus,
                stops,
            },
        )
        .unwrap();
        self.sim
            .world()
            .resource::<TransitNet>()
            .lines
            .iter()
            .last()
            .unwrap()
            .0
    }

    fn cmd(&mut self, c: TransitCommand) {
        apply(self.sim.world_mut(), &c).unwrap();
    }

    fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.sim.tick_once();
            let now = self.sim.tick().0;
            let fleet = self.sim.world().resource::<TransitFleet>();
            for (id, b) in fleet.buses.iter() {
                if let BusPhase::Dwell { idx, .. } = b.phase {
                    let v = self.dwells.entry(id).or_default();
                    match v.last_mut() {
                        Some(l) if l.0 == idx && l.2 + 1 >= now => l.2 = now,
                        _ => v.push((idx, now, now)),
                    }
                }
            }
        }
    }

    fn buses(&self) -> usize {
        self.sim.world().resource::<TransitFleet>().buses.len()
    }

    fn state(&self, line: LineId) -> LineState {
        self.sim
            .world()
            .resource::<TransitNet>()
            .lines
            .get(line)
            .unwrap()
            .state
    }
}

#[test]
fn two_buses_call_at_all_four_stops_in_order_and_stand_for_the_dwell() {
    let mut t = town();
    t.depot();
    let line = t.line(&[0, 1, 2, 3]);
    t.run(260);
    assert_eq!(t.buses(), 2);
    assert_eq!(t.state(line), LineState::Ok);
    assert_eq!(t.dwells.len(), 2);
    let dwell_ticks = (t
        .sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .balance()
        .dwell_s
        / cp_core::time::MOVE_SECONDS_PER_TICK)
        .ceil() as u64;
    for (bus, calls) in &t.dwells {
        let idxs: Vec<u16> = calls.iter().map(|c| c.0).collect();
        assert!(idxs.len() >= 4, "{bus:?} called {idxs:?}");
        assert!(
            idxs.windows(2).all(|w| w[1] == (w[0] + 1) % 4),
            "in ring order: {idxs:?}"
        );
        assert_eq!(idxs[0], 0, "starts at the first stop");
        // Every finished call lasted the dwell (the last one may still be going on).
        for c in &calls[..calls.len() - 1] {
            assert!(c.2 - c.1 + 1 >= dwell_ticks, "{bus:?} stood {c:?}");
        }
    }
    assert_eq!(
        t.sim.world().resource::<Agents>().vehicle_count(),
        2,
        "the buses are vehicles of the pool"
    );
}

#[test]
fn taking_a_vehicle_off_the_line_sends_one_bus_home_and_removes_it() {
    let mut t = town();
    t.depot();
    let line = t.line(&[0, 1, 2, 3]);
    t.run(120);
    assert_eq!(t.buses(), 2);
    t.cmd(TransitCommand::SetLineVehicles { line, n: 1 });
    t.run(300);
    assert_eq!(t.buses(), 1);
    assert_eq!(t.sim.world().resource::<Agents>().vehicle_count(), 1);
}

#[test]
fn without_a_depot_the_line_has_no_depot_and_no_buses() {
    let mut t = town();
    let line = t.line(&[0, 1, 2, 3]);
    t.run(60);
    assert_eq!(t.state(line), LineState::NoDepot);
    assert_eq!(t.buses(), 0);
    // A depot appears: the buses come.
    t.depot();
    t.run(60);
    assert_eq!(t.state(line), LineState::Ok);
    assert!(t.buses() >= 1);
}

#[test]
fn a_broken_line_sends_its_buses_home() {
    let mut t = town();
    t.depot();
    let line = t.line(&[0, 2]);
    t.run(100);
    assert!(t.buses() >= 1);
    // The road under the second stop goes: the line is left with one stop.
    let seg = t.sides[2];
    t.sim
        .world_mut()
        .resource_mut::<NetGraph>()
        .remove_segment(seg)
        .unwrap();
    t.run(400);
    assert_eq!(t.state(line), LineState::Broken);
    assert_eq!(t.buses(), 0);
    assert_eq!(t.sim.world().resource::<Agents>().vehicle_count(), 0);
}

#[test]
fn each_bus_out_on_a_line_costs_its_weekly_upkeep_at_the_end_of_the_week() {
    let mut t = town();
    t.depot();
    t.line(&[0, 1, 2, 3]);
    t.run(120);
    let buses = t.buses() as f64;
    assert!(buses >= 1.0);
    let before = t.sim.world().resource::<cp_transit::TransitLedger>().upkeep;
    assert_eq!(before, cp_core::Cost::ZERO);
    t.sim
        .world_mut()
        .insert_resource(Tick(cp_core::time::TICKS_PER_WEEK - 1));
    t.run(3);
    let per_bus = t
        .sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .balance()
        .bus_upkeep_week;
    // The weekly settlement (T-160) takes the ledger into the report.
    let booked = t
        .sim
        .world()
        .resource::<cp_economy::EconomyState>()
        .last_week
        .transit_upkeep;
    assert_eq!(booked, cp_core::Cost::scaled(per_bus, buses));
}
