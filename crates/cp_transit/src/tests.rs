//! The rules of stops, the route of a line, and what a change of the network does to them.

use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule};
use bevy_ecs::world::World;
use bevy_math::Vec3;
use cp_content::{ContentDb, SimContent};
use cp_core::curve::CubicBezier3;
use cp_core::{Cost, Tick, Treasury};
use cp_net::{Elevation, JunctionGeometry, NetGraph, SegmentId, update_junctions};
use cp_traffic::{PathNetwork, PathService, Side, update_path_network};

use crate::*;

struct Town {
    world: World,
    schedule: Schedule,
    db: std::sync::Arc<ContentDb>,
    /// The four sides of a 100 m square, counter-clockwise from the origin.
    sides: Vec<SegmentId>,
}

fn town() -> Town {
    let db = std::sync::Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    );
    let mut world = World::new();
    world.insert_resource(Tick(0));
    world.insert_resource(SimContent(db.clone()));
    world.init_resource::<NetGraph>();
    world.init_resource::<JunctionGeometry>();
    world.init_resource::<PathNetwork>();
    world.init_resource::<TransitNet>();
    world.init_resource::<LinePlans>();
    world.init_resource::<Treasury>();
    world.insert_resource(PathService::new());
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let corners = [
        (0.0, 0.0),
        (100.0, 0.0),
        (100.0, 100.0),
        (0.0, 100.0),
        (0.0, 0.0),
    ];
    let sides = corners
        .windows(2)
        .map(|w| {
            let curve = CubicBezier3::line(
                Vec3::new(w[0].0, 0.0, w[0].1),
                Vec3::new(w[1].0, 0.0, w[1].1),
            );
            world
                .resource_mut::<NetGraph>()
                .add_segment(&def, road, curve, Elevation::Ground)
                .unwrap()
        })
        .collect();
    let mut schedule = Schedule::default();
    schedule.add_systems((
        update_junctions,
        update_path_network.after(update_junctions),
        update_transit.after(update_path_network),
    ));
    let mut t = Town {
        world,
        schedule,
        db,
        sides,
    };
    t.tick();
    t
}

impl Town {
    fn tick(&mut self) {
        self.schedule.run(&mut self.world);
        self.world.resource_mut::<Tick>().advance();
    }

    fn cmd(&mut self, c: TransitCommand) -> Result<Cost, TransitError> {
        apply(&mut self.world, &c)
    }

    fn stop_at(&self, side: usize, s: f32) -> StopRef {
        StopRef::NewRoad {
            seg: self.sides[side],
            s,
            side: Side::Right,
        }
    }

    fn only_line(&self) -> (LineId, Line) {
        let (id, l) = self
            .world
            .resource::<TransitNet>()
            .lines
            .iter()
            .next()
            .unwrap();
        (id, l.clone())
    }
}

#[test]
fn three_new_stops_make_a_ring_of_three_legs_and_are_charged_once() {
    let mut t = town();
    let before = t.world.resource::<Treasury>().balance;
    let stops = vec![t.stop_at(0, 50.0), t.stop_at(1, 50.0), t.stop_at(2, 50.0)];
    let cost = t
        .cmd(TransitCommand::CreateLine {
            mode: TransitMode::Bus,
            stops,
        })
        .unwrap();
    let per_stop = Cost::from_units(t.db.balance().bus_stop_cost);
    assert_eq!(cost, Cost(per_stop.0 * 3));
    assert_eq!(t.world.resource::<Treasury>().balance, before - cost);
    let (id, line) = t.only_line();
    assert_eq!(line.stops.len(), 3);
    assert_eq!(line.state, LineState::Ok);
    let plan = t.world.resource::<LinePlans>().get(id).unwrap();
    assert!(plan.is_ok());
    assert_eq!(plan.legs.len(), 3, "the ring closes");
    assert!(plan.length_m > 0.0 && plan.cycle_s > 0.0);
    assert!(plan.legs.iter().all(|l| l.length_m > 0.0));
    // The stops know their line.
    let net = t.world.resource::<TransitNet>();
    assert!(net.stops.iter().all(|(_, s)| s.lines == vec![id]));
}

#[test]
fn a_stop_near_a_junction_or_a_neighbour_or_on_a_highway_is_refused() {
    let mut t = town();
    let near_node = t.stop_at(0, 5.0);
    let ok = t.stop_at(0, 50.0);
    let close = t.stop_at(0, 60.0);
    let far = t.stop_at(1, 50.0);
    let mk = |stops| TransitCommand::CreateLine {
        mode: TransitMode::Bus,
        stops,
    };
    assert!(matches!(
        t.cmd(mk(vec![near_node, far])),
        Err(TransitError::Stop(StopError::NearNode))
    ));
    assert!(matches!(
        t.cmd(mk(vec![ok, close])),
        Err(TransitError::NewStopsTooClose)
    ));
    assert!(
        t.world.resource::<TransitNet>().is_empty(),
        "nothing was changed"
    );
    // A highway.
    let hw = t.db.road_id("highway_1w_2l").unwrap();
    let def = t.db.road(hw).clone();
    let seg = t
        .world
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            hw,
            CubicBezier3::line(Vec3::new(0.0, 0.0, 300.0), Vec3::new(200.0, 0.0, 300.0)),
            Elevation::Ground,
        )
        .unwrap();
    let on_highway = StopRef::NewRoad {
        seg,
        s: 100.0,
        side: Side::Right,
    };
    assert!(matches!(
        t.cmd(mk(vec![on_highway, far])),
        Err(TransitError::Stop(StopError::Highway))
    ));
    // The accepted case, and an existing stop is not a new one: 24 m is enough.
    assert!(t.cmd(mk(vec![ok, far])).is_ok());
}

#[test]
fn demolishing_the_road_under_a_stop_removes_it_and_breaks_a_two_stop_line() {
    let mut t = town();
    let stops = vec![t.stop_at(0, 50.0), t.stop_at(2, 50.0)];
    t.cmd(TransitCommand::CreateLine {
        mode: TransitMode::Bus,
        stops,
    })
    .unwrap();
    t.tick();
    assert_eq!(t.only_line().1.state, LineState::Ok);
    let seg = t.sides[0];
    t.world
        .resource_mut::<NetGraph>()
        .remove_segment(seg)
        .unwrap();
    t.tick();
    let (_, line) = t.only_line();
    assert_eq!(line.stops.len(), 1, "the stop is gone from the line");
    assert_eq!(line.state, LineState::Broken);
    assert_eq!(t.world.resource::<TransitNet>().stops.len(), 1);
}

#[test]
fn cutting_a_segment_keeps_the_stop_on_the_new_piece() {
    let mut t = town();
    let stops = vec![t.stop_at(0, 50.0), t.stop_at(2, 50.0)];
    t.cmd(TransitCommand::CreateLine {
        mode: TransitMode::Bus,
        stops,
    })
    .unwrap();
    let (_, line) = t.only_line();
    let stop = line.stops[0];
    let road = t.db.road_id("basic_2l").unwrap();
    let def = t.db.road(road).clone();
    let seg = t.sides[0];
    // Cut it at 30 %: the stop at 50 m ends up on the second piece.
    t.world
        .resource_mut::<NetGraph>()
        .split_segment(&def, seg, 0.3)
        .unwrap();
    t.tick();
    let net = t.world.resource::<TransitNet>();
    let st = net.stops.get(stop).expect("still there");
    let StopKind::Road { seg: now, s } = st.kind else {
        panic!("a road stop")
    };
    assert_ne!(now, seg, "re-attached to a piece");
    assert!(
        (s - 20.0).abs() < 0.5,
        "50 m on the old segment = 20 m on the piece, got {s}"
    );
    assert_eq!(t.only_line().1.state, LineState::Ok);
}
