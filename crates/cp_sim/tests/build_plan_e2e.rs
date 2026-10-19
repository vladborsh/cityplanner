//! T-262 step 1: `NetCommand::BuildPlan` — the contract types and their application through the
//! existing path. A plan of straight pieces builds what `BuildRoad` builds; anchors name nodes and
//! segments of the graph; a bad anchor is refused and changes nothing.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_core::{CommandOutcome, Cost, Treasury};
use cp_net::{Anchor, BuildPlan, ElevationMode, NetCommand, NetGraph, PlanPiece, RoadPath};
use cp_sim::{SimCommand, SimRunner};

fn world() -> (SimRunner, cp_content::RoadDefId) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
    let db = Arc::new(cp_content::ContentDb::load_dir(format!("{dir}/content")).unwrap());
    let terrain = Arc::new(
        cp_terrain::Terrain::load(&std::path::Path::new(dir).join("maps/flatlands")).unwrap(),
    );
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(&db));
    sim.world_mut()
        .insert_resource(cp_terrain::TileOwnership::only((2, 2)));
    sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(1_000_000.0);
    (sim, db.road_id("basic_2l").unwrap())
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

fn shape(sim: &SimRunner) -> Vec<(Vec<f32>, f32)> {
    let g = sim.world().resource::<NetGraph>();
    g.segments()
        .map(|(_, s)| {
            (
                vec![s.curve.p0.x, s.curve.p0.z, s.curve.p3.x, s.curve.p3.z],
                s.length,
            )
        })
        .collect()
}

fn pieces(points: &[Vec2]) -> Vec<PlanPiece> {
    points
        .windows(2)
        .map(|w| PlanPiece::straight(Anchor::Free(w[0]), Anchor::Free(w[1]), w[0], w[1]))
        .collect()
}

#[test]
fn a_plan_of_straight_pieces_builds_what_build_road_builds() {
    let (mut a, road) = world();
    let (mut b, _) = world();
    let (p, q, r) = (
        Vec2::new(5100.0, 5100.0),
        Vec2::new(5200.0, 5100.0),
        Vec2::new(5200.0, 5200.0),
    );
    for (from, to) in [(p, q), (q, r)] {
        run(
            &mut a,
            NetCommand::BuildRoad {
                road,
                path: RoadPath::Straight { from, to },
                elevation: ElevationMode::Ground,
            },
        )
        .unwrap();
    }
    run(
        &mut b,
        NetCommand::BuildPlan {
            plan: BuildPlan {
                road,
                elevation: ElevationMode::Ground,
                pieces: pieces(&[p, q, r]),
            },
        },
    )
    .unwrap();
    assert_eq!(shape(&a), shape(&b));
    assert_eq!(
        a.world().resource::<Treasury>().balance,
        b.world().resource::<Treasury>().balance
    );
}

#[test]
fn anchors_name_nodes_and_segments_of_the_graph() {
    let (mut sim, road) = world();
    let (p, q) = (Vec2::new(5100.0, 5100.0), Vec2::new(5300.0, 5100.0));
    run(
        &mut sim,
        NetCommand::BuildRoad {
            road,
            path: RoadPath::Straight { from: p, to: q },
            elevation: ElevationMode::Ground,
        },
    )
    .unwrap();
    let (seg, start, mid) = {
        let g = sim.world().resource::<NetGraph>();
        let (id, s) = g.segments().next().unwrap();
        let m = s.curve.point(0.5);
        (id, s.end, Vec2::new(m.x, m.z))
    };
    // From the middle of the first piece to the north; and from its last node to the south.
    let before = sim.world().resource::<NetGraph>().segment_count();
    let up = mid + Vec2::new(0.0, 96.0);
    run(
        &mut sim,
        NetCommand::BuildPlan {
            plan: BuildPlan {
                road,
                elevation: ElevationMode::Ground,
                pieces: vec![PlanPiece::straight(
                    Anchor::OnSegment(seg, 0.5),
                    Anchor::Free(up),
                    mid,
                    up,
                )],
            },
        },
    )
    .unwrap();
    assert!(sim.world().resource::<NetGraph>().segment_count() >= before + 2);
    let q = {
        let n = sim.world().resource::<NetGraph>().node(start).unwrap().pos;
        Vec2::new(n.x, n.z)
    };
    let down = q + Vec2::new(0.0, -96.0);
    run(
        &mut sim,
        NetCommand::BuildPlan {
            plan: BuildPlan {
                road,
                elevation: ElevationMode::Ground,
                pieces: vec![PlanPiece::straight(
                    Anchor::Node(start),
                    Anchor::Free(down),
                    q,
                    down,
                )],
            },
        },
    )
    .unwrap();
    let g = sim.world().resource::<NetGraph>();
    assert_eq!(g.node(start).unwrap().segments.len(), 3);
    assert!(
        g.segments()
            .any(|(_, s)| s.curve.p3.z < q.y - 50.0 || s.curve.p0.z < q.y - 50.0)
    );
}

#[test]
fn a_bad_plan_is_refused_and_changes_nothing() {
    let (mut sim, road) = world();
    let plan = |pieces| NetCommand::BuildPlan {
        plan: BuildPlan {
            road,
            elevation: ElevationMode::Ground,
            pieces,
        },
    };
    let balance = sim.world().resource::<Treasury>().balance;
    // Empty, not finite, a node that does not exist.
    assert!(run(&mut sim, plan(vec![])).is_err());
    let nan = Vec2::new(f32::NAN, 5100.0);
    let p = Vec2::new(5100.0, 5100.0);
    assert!(run(&mut sim, plan(pieces(&[p, nan]))).is_err());
    let ghost = {
        let (mut other, r2) = world();
        run(
            &mut other,
            NetCommand::BuildRoad {
                road: r2,
                path: RoadPath::Straight {
                    from: p,
                    to: Vec2::new(5200.0, 5100.0),
                },
                elevation: ElevationMode::Ground,
            },
        )
        .unwrap();
        other
            .world()
            .resource::<NetGraph>()
            .segments()
            .next()
            .unwrap()
            .1
            .start
    };
    let q = Vec2::new(5200.0, 5100.0);
    assert!(
        run(
            &mut sim,
            plan(vec![PlanPiece::straight(
                Anchor::Node(ghost),
                Anchor::Free(q),
                p,
                q
            )])
        )
        .is_err()
    );
    // The second piece is bad (too short): the first is not kept.
    let tiny = Vec2::new(5205.0, 5100.0);
    let mut two = pieces(&[p, q]);
    two.extend(pieces(&[q, tiny]));
    assert!(run(&mut sim, plan(two)).is_err());
    assert_eq!(sim.world().resource::<NetGraph>().segment_count(), 0);
    assert_eq!(sim.world().resource::<Treasury>().balance, balance);
}
