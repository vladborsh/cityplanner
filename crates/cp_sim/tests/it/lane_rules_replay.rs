//! T-313b: a command log with `SetLaneRule` replays to the same checksum (the rules are part of
//! the net and of its hash), and a rule changes the state it was applied to.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_core::{CommandOutcome, Cost, Tick, Treasury};
use cp_net::{Anchor, BuildPlan, ElevationMode, Exit, NetCommand, NetGraph, PlanPiece};
use cp_sim::{SimCommand, SimRunner};

const C: f32 = 5100.0;

fn world() -> SimRunner {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
    let db = Arc::new(cp_content::ContentDb::load_dir(format!("{dir}/content")).unwrap());
    let terrain = Arc::new(
        cp_terrain::Terrain::load(&std::path::Path::new(dir).join("maps/flatlands")).unwrap(),
    );
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain);
    sim.set_content(db);
    sim.world_mut()
        .insert_resource(cp_terrain::TileOwnership::only((2, 2)));
    sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(1_000_000.0);
    sim
}

fn run(sim: &mut SimRunner, c: NetCommand) -> Result<(), String> {
    sim.push_command(SimCommand::Net(c), sim.tick());
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last()
        .unwrap()
        .result
        .clone()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// A cross of four 90 m medium streets.
fn cross(sim: &mut SimRunner) {
    let road = sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .road_id("medium_4l")
        .unwrap();
    for (a, b) in [
        (Vec2::new(-90.0, 0.0), Vec2::new(0.0, 0.0)),
        (Vec2::new(0.0, 0.0), Vec2::new(90.0, 0.0)),
        (Vec2::new(0.0, -90.0), Vec2::new(0.0, 0.0)),
        (Vec2::new(0.0, 0.0), Vec2::new(0.0, 90.0)),
    ] {
        let (a, b) = (Vec2::new(C, C) + a, Vec2::new(C, C) + b);
        let plan = BuildPlan {
            road,
            elevation: ElevationMode::Ground,
            pieces: vec![PlanPiece::straight(Anchor::Free(a), Anchor::Free(b), a, b)],
            trees: false,
        };
        run(sim, NetCommand::BuildPlan { plan }).unwrap();
    }
}

/// The rule: the first inbound lane of the first arm may go to every other arm.
fn rule(sim: &SimRunner) -> NetCommand {
    let g = sim.world().resource::<NetGraph>();
    let node = g
        .find_node(bevy_math::Vec3::new(C, 0.0, C))
        .unwrap_or_else(|| {
            g.nodes()
                .find(|(_, n)| n.segments.len() == 4)
                .map(|(id, _)| id)
                .unwrap()
        });
    let arms = g.node(node).unwrap().segments.clone();
    let segment = arms[0];
    let inbound = g
        .segment(segment)
        .unwrap()
        .lanes
        .iter()
        .filter_map(|&l| g.lane(l))
        .filter(|l| l.kind.is_road_vehicle())
        .find(|l| {
            (l.dir == cp_content::LaneDir::Forward) != (g.segment(segment).unwrap().start == node)
        })
        .unwrap()
        .index;
    NetCommand::SetLaneRule {
        node,
        segment,
        lane: inbound,
        exits: Some(
            arms[1..]
                .iter()
                .map(|&arm| Exit { arm, lane: None })
                .collect(),
        ),
    }
}

fn play(with_rule: bool) -> (u64, u64) {
    let mut sim = world();
    cross(&mut sim);
    for _ in 0..40 {
        sim.tick_once();
    }
    let before = sim.checksum();
    if with_rule {
        let r = rule(&sim);
        run(&mut sim, r).expect("the sim accepts the rule");
    }
    while sim.tick() < Tick(400) {
        sim.tick_once();
    }
    (before, sim.checksum())
}

#[test]
fn a_lane_rule_replays_to_the_same_checksum_and_changes_the_state() {
    let (b1, a1) = play(true);
    let (b2, a2) = play(true);
    assert_eq!((b1, a1), (b2, a2), "two runs of one log agree");
    let (_, bare) = play(false);
    assert_ne!(a1, bare, "the rule is part of the hashed state");
}
