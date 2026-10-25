//! T-332c (§18.1 decision B1): the diamond stamped by the interchange tool on a dual highway, as
//! the player does it: the template in hand, the cursor on carriageway A, a click. The batch
//! builds; each carriageway reaches the crossing road and comes back onto itself; the
//! carriageways and the crossing road share no node; the counts are the template's; the command
//! log replays to the same checksum.

use std::collections::BTreeSet;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::{ContentDb, LaneKind, TemplatePoint};
use cp_core::{CommandOutcome, Cost, Treasury};
use cp_net::{
    Anchor, BuildPlan, ElevationMode, LaneId, NetCommand, NetGraph, PlanPiece, SegmentId,
};
use cp_sim::{SimCommand, SimRunner};
use cp_tools::{interchange_command, interchange_preview};

const C: f32 = 5100.0;
/// Carriageway B, metres to the left of A (A runs +x, its left is -z).
const B: f32 = 20.0;

fn world() -> (SimRunner, Arc<ContentDb>) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
    let db = Arc::new(ContentDb::load_dir(format!("{dir}/content")).unwrap());
    let terrain = Arc::new(
        cp_terrain::Terrain::load(&std::path::Path::new(dir).join("maps/flatlands")).unwrap(),
    );
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(&db));
    sim.world_mut()
        .insert_resource(cp_terrain::TileOwnership::only((2, 2)));
    sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(10_000_000.0);
    sim.tick_once();
    (sim, db)
}

fn run(sim: &mut SimRunner, c: SimCommand) -> Result<(), String> {
    sim.push_command(c, sim.tick());
    sim.tick_once();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last()
        .expect("an outcome")
        .result
        .clone()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The two carriageways, 1200 m of `highway_1w_2l` each: A along +x at z = C, B back along -x
/// `B` metres to its left.
fn highway(db: &ContentDb) -> Vec<SimCommand> {
    let road = db.road_id("highway_1w_2l").unwrap();
    [
        (Vec2::new(C - 600.0, C), Vec2::new(C + 600.0, C)),
        (Vec2::new(C + 600.0, C - B), Vec2::new(C - 600.0, C - B)),
    ]
    .into_iter()
    .map(|(a, b)| {
        SimCommand::Net(NetCommand::BuildPlan {
            plan: BuildPlan {
                road,
                elevation: ElevationMode::Ground,
                pieces: vec![PlanPiece::straight(Anchor::Free(a), Anchor::Free(b), a, b)],
                trees: false,
            },
        })
    })
    .collect()
}

fn graph(sim: &SimRunner) -> &NetGraph {
    sim.world().resource::<NetGraph>()
}

fn road_of(g: &NetGraph, db: &ContentDb, seg: SegmentId) -> String {
    db.road(g.segment(seg).unwrap().road).id.clone()
}

/// The segments the car lanes of `from` reach through the junctions.
fn reach(g: &NetGraph, from: SegmentId) -> BTreeSet<SegmentId> {
    let mut next: std::collections::BTreeMap<LaneId, Vec<LaneId>> = Default::default();
    for (id, _) in g.nodes() {
        if let Some(j) = cp_net::compute_junction(g, id, 0) {
            for c in &j.links.connections {
                next.entry(c.from).or_default().push(c.to);
            }
        }
    }
    let car = |l: &LaneId| g.lane(*l).is_some_and(|l| l.kind == LaneKind::Car);
    let mut todo: Vec<LaneId> = g
        .segment(from)
        .unwrap()
        .lanes
        .iter()
        .copied()
        .filter(car)
        .collect();
    let mut seen: BTreeSet<LaneId> = todo.iter().copied().collect();
    while let Some(l) = todo.pop() {
        for n in next.get(&l).into_iter().flatten() {
            if seen.insert(*n) {
                todo.push(*n);
            }
        }
    }
    seen.iter()
        .filter_map(|l| g.lane(*l))
        .map(|l| l.segment)
        .collect()
}

/// The carriageway segment whose curve passes nearest `p`.
fn segment_at(g: &NetGraph, p: Vec2) -> SegmentId {
    g.segments()
        .min_by(|a, b| {
            a.1.curve
                .nearest_xz(p)
                .1
                .total_cmp(&b.1.curve.nearest_xz(p).1)
        })
        .unwrap()
        .0
}

#[test]
fn the_diamond_stamps_on_a_dual_highway_connects_it_and_replays() {
    let (mut sim, db) = world();
    let mut log = highway(&db);
    for c in log.clone() {
        run(&mut sim, c).unwrap();
    }
    let (n0, s0) = (graph(&sim).nodes().count(), graph(&sim).segments().count());
    let def = db.interchange("diamond").expect("the diamond template");

    // The player: the diamond in hand, the cursor on A a few metres off its axis.
    let view = sim.view();
    let preview = interchange_preview(&view, def, Vec2::new(C, C + 3.0));
    drop(view);
    let frame = preview.frame.expect("a frame on the highway");
    assert!(
        frame.axis.distance(Vec2::X) < 1e-3,
        "the axis is A's direction: {frame:?}"
    );
    assert!(
        (frame.b - B).abs() < 0.5,
        "B is found {B} m left: {frame:?}"
    );
    assert!(preview.valid, "the stamp checks: {:?}", preview.error);
    let cmd = interchange_command(&preview).expect("a batch");
    run(&mut sim, cmd.clone()).unwrap_or_else(|e| panic!("the batch is refused: {e}"));
    log.push(cmd);

    let g = graph(&sim);
    // (iii) The counts the template implies: a node per vertex of its own (not a reference to
    // an earlier piece's), a segment per leg, a leg over 96 m cut in even pieces (a node and a
    // segment per cut; legs measured on their chords), and each cut of a carriageway splits it
    // once more.
    let own = |p: &TemplatePoint| !matches!(p, TemplatePoint::Piece(..));
    let cuts = def
        .pieces
        .iter()
        .flat_map(|p| &p.points)
        .filter(|p| matches!(p, TemplatePoint::OnA(_) | TemplatePoint::OnB(_)))
        .count();
    let nodes: usize = def
        .pieces
        .iter()
        .map(|p| p.points.iter().filter(|q| own(q)).count())
        .sum();
    let even: usize = (0..def.pieces.len())
        .flat_map(|k| {
            let at = move |j| {
                let (u, v) = def.point_uv(k, j, frame.b).unwrap();
                frame.at(u, v)
            };
            (1..def.pieces[k].points.len()).map(move |j| (at(j - 1), at(j)))
        })
        .map(|(a, b)| cp_net::build::even_cuts(a.distance(b)).len())
        .sum();
    let nodes = nodes + even;
    let legs: usize = def.pieces.iter().map(|p| p.points.len() - 1).sum::<usize>() + even;
    println!(
        "nodes {n0} -> {}, segments {s0} -> {}",
        g.nodes().count(),
        g.segments().count()
    );
    assert_eq!(g.nodes().count(), n0 + nodes, "nodes");
    assert_eq!(g.segments().count(), s0 + legs + cuts, "segments");

    // (ii) Grade separation: no node of a carriageway is a node of the crossing road.
    let nodes_of = |road: &str| -> BTreeSet<cp_net::NodeId> {
        g.segments()
            .filter(|(id, _)| road_of(g, &db, *id) == road)
            .flat_map(|(_, s)| [s.start, s.end])
            .collect()
    };
    let carriageways = nodes_of("highway_1w_2l");
    let crossing = nodes_of("medium_4l");
    assert!(!crossing.is_empty());
    assert!(
        carriageways.is_disjoint(&crossing),
        "a carriageway meets the crossing road at grade"
    );

    // (i) Each carriageway reaches the crossing road from before the interchange and is reached
    // from it after; both ends of the crossing road reach both carriageways.
    let crossing_segs: BTreeSet<SegmentId> = g
        .segments()
        .filter(|(id, _)| road_of(g, &db, *id) == "medium_4l")
        .map(|(id, _)| id)
        .collect();
    let a_up = segment_at(g, Vec2::new(C - 500.0, C));
    let a_down = segment_at(g, Vec2::new(C + 500.0, C));
    let b_up = segment_at(g, Vec2::new(C + 500.0, C - B));
    let b_down = segment_at(g, Vec2::new(C - 500.0, C - B));
    for (name, up) in [("A", a_up), ("B", b_up)] {
        assert!(
            !reach(g, up).is_disjoint(&crossing_segs),
            "{name} does not reach the crossing road"
        );
    }
    let far_a = segment_at(g, Vec2::new(C, C + 110.0));
    let far_b = segment_at(g, Vec2::new(C, C - B - 110.0));
    for end in [far_a, far_b] {
        assert!(crossing_segs.contains(&end), "{end:?} is the crossing road");
        let r = reach(g, end);
        assert!(r.contains(&a_down), "the crossing road does not reach A");
        assert!(r.contains(&b_down), "the crossing road does not reach B");
    }

    // (iv) The log replays to the same state.
    let want = sim.checksum();
    let (mut again, _) = world();
    for c in log {
        run(&mut again, c).unwrap();
    }
    assert_eq!(again.checksum(), want, "the replay differs");
}
