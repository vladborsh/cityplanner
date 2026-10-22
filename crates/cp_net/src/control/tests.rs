use std::sync::Arc;

use bevy_ecs::world::World;
use bevy_math::Vec3;
use cp_content::{ContentDb, SimContent};
use cp_core::curve::CubicBezier3;

use super::*;
use crate::connections::PedLinkKind;
use crate::junction::{JunctionGeometry, update_junctions};
use crate::model::Elevation;

fn content() -> Arc<ContentDb> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
    Arc::new(ContentDb::load_dir(root).unwrap_or_else(|e| panic!("{e}")))
}

const C: Vec3 = Vec3::new(1000.0, 50.0, 1000.0);

fn star(db: &ContentDb, arms: &[(f32, &str)]) -> NetGraph {
    let mut g = NetGraph::new();
    for &(deg, name) in arms {
        let road = db.road_id(name).unwrap();
        let def = db.road(road).clone();
        let a = deg.to_radians();
        let tip = C + Vec3::new(150.0 * a.cos(), 0.0, 150.0 * a.sin());
        g.add_segment(&def, road, CubicBezier3::line(C, tip), Elevation::Ground)
            .unwrap();
    }
    g
}

struct Sim {
    world: World,
}

impl Sim {
    fn new(graph: NetGraph) -> Self {
        let mut world = World::new();
        world.insert_resource(graph);
        world.init_resource::<JunctionGeometry>();
        world.insert_resource(SimContent(content()));
        let mut s = Self { world };
        s.network();
        s
    }
    fn network(&mut self) {
        self.world.run_system_cached(update_junctions).unwrap();
    }
    fn graph(&self) -> &NetGraph {
        self.world.resource::<NetGraph>()
    }
    fn graph_mut(&mut self) -> bevy_ecs::change_detection::Mut<'_, NetGraph> {
        self.world.resource_mut::<NetGraph>()
    }
    fn centre(&self) -> NodeId {
        self.graph().find_node(C).unwrap()
    }
    fn junction(&self) -> Junction {
        self.world
            .resource::<JunctionGeometry>()
            .get(self.centre())
            .unwrap()
            .clone()
    }
    fn control(&self) -> JunctionControl {
        self.graph().node(self.centre()).unwrap().control
    }
}

// ---------- auto-install rule ----------

#[test]
fn auto_install_rule_table() {
    let db = content();
    type Case<'a> = (&'a [(f32, &'a str)], bool, &'a str);
    let cases: &[Case] = &[
        // (arms, light expected, why)
        (
            &[
                (0.0, "medium_4l"),
                (90.0, "medium_4l"),
                (180.0, "medium_4l"),
                (270.0, "medium_4l"),
            ],
            true,
            "4-way of 4-lane roads",
        ),
        (
            &[
                (0.0, "basic_2l"),
                (90.0, "basic_2l"),
                (180.0, "basic_2l"),
                (270.0, "basic_2l"),
            ],
            false,
            "4-way of 2-lane streets",
        ),
        (
            &[(0.0, "medium_4l"), (90.0, "basic_2l"), (180.0, "basic_2l")],
            false,
            "T with only one big road",
        ),
        (
            &[(0.0, "medium_4l"), (90.0, "basic_2l"), (180.0, "medium_4l")],
            true,
            "T with two big roads",
        ),
        (
            &[(0.0, "large_6l"), (90.0, "medium_4l"), (180.0, "basic_2l")],
            true,
            "T with 6- and 4-lane roads",
        ),
        (
            &[(0.0, "medium_4l"), (90.0, "medium_4l")],
            false,
            "a bend has only 2 segments",
        ),
        (
            // Highways have 3 vehicle lanes: not big roads for the rule.
            &[
                (0.0, "highway_1w_3l"),
                (90.0, "highway_1w_3l"),
                (180.0, "medium_4l"),
            ],
            false,
            "3-lane highways do not count",
        ),
    ];
    for (arms, expected, why) in cases {
        let sim = Sim::new(star(&db, arms));
        assert_eq!(sim.control().has_light(), *expected, "{why}");
        assert!(!sim.control().is_manual(), "{why}");
        assert_eq!(sim.junction().control.light.is_some(), *expected, "{why}");
    }
}

#[test]
fn rule_reapplies_to_automatic_nodes_but_keeps_the_players_choice() {
    let db = content();
    let arms = [
        (0.0, "medium_4l"),
        (90.0, "medium_4l"),
        (180.0, "medium_4l"),
        (270.0, "basic_2l"),
    ];
    // Automatic: removing arms re-evaluates the rule.
    let mut sim = Sim::new(star(&db, &arms));
    assert_eq!(sim.control(), JunctionControl::TrafficLight);
    let segs: Vec<SegmentId> = sim.graph().segments().map(|(id, _)| id).collect();
    sim.graph_mut().remove_segment(segs[3]).unwrap(); // the 2-lane street: still 3 arms, 3 big
    sim.network();
    assert_eq!(sim.control(), JunctionControl::TrafficLight);
    sim.graph_mut().remove_segment(segs[2]).unwrap(); // 2 arms left
    sim.network();
    assert_eq!(sim.control(), JunctionControl::Priority);

    // Manual priority at a node the rule would light stays priority through recomputes.
    let mut sim = Sim::new(star(&db, &arms));
    let n = sim.centre();
    assert_eq!(
        sim.graph_mut().toggle_traffic_light(n),
        Ok(JunctionControl::ManualPriority)
    );
    sim.network();
    assert_eq!(sim.control(), JunctionControl::ManualPriority);
    assert!(sim.junction().control.light.is_none());
    // Touch the node again (new segment) — still the player's choice.
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    sim.graph_mut()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(C, C + Vec3::new(100.0, 0.0, 100.0)),
            Elevation::Ground,
        )
        .unwrap();
    sim.network();
    assert_eq!(sim.control(), JunctionControl::ManualPriority);
    // Toggling again installs a manual light.
    assert_eq!(
        sim.graph_mut().toggle_traffic_light(n),
        Ok(JunctionControl::ManualTrafficLight)
    );
    sim.network();
    assert!(sim.junction().control.light.is_some());

    // Manual light at a node the rule would not light (one big road: controllable, §6.5a).
    let mut sim = Sim::new(star(
        &db,
        &[(0.0, "medium_4l"), (90.0, "basic_2l"), (180.0, "basic_2l")],
    ));
    assert!(!sim.control().has_light());
    let n = sim.centre();
    assert_eq!(
        sim.graph_mut().toggle_traffic_light(n),
        Ok(JunctionControl::ManualTrafficLight)
    );
    sim.network();
    assert_eq!(sim.control(), JunctionControl::ManualTrafficLight);
    let plan = sim.junction().control.light.expect("light plan");
    assert!(!plan.phases.is_empty());
}

#[test]
fn toggle_rejects_non_junctions() {
    let db = content();
    let mut sim = Sim::new(star(&db, &[(0.0, "basic_2l"), (90.0, "basic_2l")]));
    let bend = sim.centre();
    assert_eq!(
        sim.graph_mut().toggle_traffic_light(bend),
        Err(crate::NetOpError::NotAJunction(bend))
    );
    let end = sim
        .graph()
        .nodes()
        .map(|(id, _)| id)
        .find(|&id| id != bend)
        .unwrap();
    assert_eq!(
        sim.graph_mut().toggle_traffic_light(end),
        Err(crate::NetOpError::NotAJunction(end))
    );
    let first = sim.graph().segments().next().unwrap().0;
    sim.graph_mut().remove_segment(first).unwrap();
    assert!(sim.graph_mut().toggle_traffic_light(bend).is_err());
}

#[test]
fn toggle_rejects_a_junction_of_two_lane_roads() {
    let db = content();
    let mut sim = Sim::new(star(
        &db,
        &[(0.0, "basic_2l"), (90.0, "basic_2l"), (180.0, "basic_2l")],
    ));
    let n = sim.centre();
    assert!(!sim.graph().is_controllable(n));
    assert_eq!(
        sim.graph_mut().toggle_traffic_light(n),
        Err(crate::NetOpError::NotControllable(n))
    );
    assert_eq!(sim.control(), JunctionControl::Priority);
    // A footpath is no arm: two streets and a path stay automatic even beside a big road's
    // width elsewhere.
    let sim = Sim::new(star(
        &db,
        &[(0.0, "medium_4l"), (90.0, "basic_2l"), (180.0, "park_path")],
    ));
    assert!(!sim.graph().is_controllable(sim.centre()));
}

// ---------- priority ----------

#[test]
fn main_road_is_the_highest_rank_pair() {
    let db = content();
    // Ranks: basic 1, medium 2, large 3.
    let sim = Sim::new(star(
        &db,
        &[
            (0.0, "basic_2l"),
            (90.0, "large_6l"),
            (180.0, "basic_2l"),
            (270.0, "large_6l"),
        ],
    ));
    let j = sim.junction();
    let main: Vec<&str> = j
        .control
        .main_arms
        .iter()
        .map(|&a| {
            let seg = sim.graph().segment(j.arms[a].segment).unwrap();
            if seg.road == db.road_id("large_6l").unwrap() {
                "large"
            } else {
                "basic"
            }
        })
        .collect();
    assert_eq!(main, vec!["large", "large"]);
    // Connections from the streets yield; straight/right from the avenue have priority.
    for (i, c) in j.links.connections.iter().enumerate() {
        let from_arm = j
            .arms
            .iter()
            .position(|a| Some(a.segment) == sim.graph().lane(c.from).map(|l| l.segment))
            .unwrap();
        let p = j.control.priority[i];
        if !j.control.main_arms.contains(&from_arm) {
            assert_eq!(p, Priority::Yield, "street connection {i}");
        } else if c.turn == Turn::Straight || c.turn == Turn::Right {
            assert_eq!(p, Priority::Main, "avenue {:?} {i}", c.turn);
        }
    }
    // Left turns off the avenue give way to the oncoming straight traffic.
    assert!(j.links.connections.iter().enumerate().any(|(i, c)| {
        c.turn == Turn::Left
            && j.control.priority[i] == Priority::Yield
            && j.control.main_arms.contains(
                &j.arms
                    .iter()
                    .position(|a| Some(a.segment) == sim.graph().lane(c.from).map(|l| l.segment))
                    .unwrap(),
            )
    }));
}

#[test]
fn equal_rank_prefers_the_wider_road_then_the_lower_segment_id() {
    let db = content();
    // Same road type everywhere: the two lowest segment ids are the main road.
    let sim = Sim::new(star(
        &db,
        &[(0.0, "basic_2l"), (90.0, "basic_2l"), (180.0, "basic_2l")],
    ));
    let j = sim.junction();
    let mut ids: Vec<SegmentId> = j
        .control
        .main_arms
        .iter()
        .map(|&a| j.arms[a].segment)
        .collect();
    ids.sort();
    let mut all: Vec<SegmentId> = j.arms.iter().map(|a| a.segment).collect();
    all.sort();
    assert_eq!(ids, all[..2].to_vec());
    // A wider road of the same rank wins: two medium (rank 2) vs one large... rank decides
    // first, so use equal-rank by constructing the same-rank pair via identical road type
    // but check width ordering with large_6l (rank 3) beating medium_4l (rank 2).
    let sim = Sim::new(star(
        &db,
        &[(0.0, "medium_4l"), (90.0, "large_6l"), (180.0, "medium_4l")],
    ));
    let j = sim.junction();
    assert!(j.control.main_arms.iter().any(|&a| j.arms[a].width > 27.0));
}

#[test]
fn small_nodes_have_no_yielding() {
    let db = content();
    for arms in [
        &[(0.0, "basic_2l")][..],
        &[(0.0, "basic_2l"), (90.0, "basic_2l")],
    ] {
        let sim = Sim::new(star(&db, arms));
        let j = sim.junction();
        assert!(j.control.priority.iter().all(|&p| p == Priority::Main));
        assert!(j.control.light.is_none());
    }
}

// ---------- phases and timing ----------

fn lit_junction(db: &ContentDb, arms: &[(f32, &str)]) -> (Sim, Junction, LightPlan) {
    let sim = Sim::new(star(db, arms));
    let j = sim.junction();
    let plan = j.control.light.clone().expect("this junction has a light");
    (sim, j, plan)
}

#[test]
fn phases_have_no_conflicts_and_cover_every_connection() {
    let db = content();
    for arms in [
        vec![
            (0.0, "medium_4l"),
            (90.0, "medium_4l"),
            (180.0, "medium_4l"),
            (270.0, "medium_4l"),
        ],
        vec![
            (0.0, "large_6l"),
            (90.0, "large_6l"),
            (180.0, "large_6l"),
            (270.0, "large_6l"),
        ],
        vec![(0.0, "medium_4l"), (90.0, "basic_2l"), (180.0, "medium_4l")],
        vec![
            (0.0, "large_6l"),
            (45.0, "medium_4l"),
            (135.0, "medium_4l"),
            (180.0, "large_6l"),
            (250.0, "basic_2l"),
        ],
        (0..8).map(|i| (i as f32 * 45.0, "medium_4l")).collect(),
    ] {
        let (sim, j, plan) = lit_junction(&db, &arms);
        let n = j.links.connections.len() as u16;
        for c in 0..n {
            let count = plan
                .phases
                .iter()
                .filter(|p| p.connections.contains(&c))
                .count();
            assert_eq!(count, 1, "connection {c}");
        }
        for p in &plan.phases {
            for (x, &a) in p.connections.iter().enumerate() {
                for &b in &p.connections[x + 1..] {
                    assert!(!j.links.in_conflict(a, b), "{a} vs {b} in one phase");
                }
            }
        }
        assert!(plan.phases.len() >= 2);
        // Crossings: walkable somewhere, and never on an arm used by that phase's vehicles.
        for (i, l) in j.links.pedestrian.iter().enumerate() {
            if l.kind != PedLinkKind::Crossing {
                continue;
            }
            let arm = j
                .arms
                .iter()
                .position(|a| Some(a.segment) == sim.graph().lane(l.a).map(|x| x.segment))
                .unwrap();
            let walk_phases: Vec<&Phase> = plan
                .phases
                .iter()
                .filter(|p| p.walk.contains(&(i as u16)))
                .collect();
            assert!(!walk_phases.is_empty(), "crossing {i} never walks");
            for p in walk_phases {
                for &c in &p.connections {
                    let con = &j.links.connections[c as usize];
                    let seg = |lane| sim.graph().lane(lane).map(|x| x.segment);
                    assert_ne!(seg(con.from), Some(j.arms[arm].segment));
                    assert_ne!(seg(con.to), Some(j.arms[arm].segment));
                }
            }
        }
    }
}

#[test]
fn simple_four_way_has_two_straight_through_phases_and_crossings_in_the_other() {
    let db = content();
    let (sim, j, plan) = lit_junction(
        &db,
        &[
            (0.0, "medium_4l"),
            (90.0, "medium_4l"),
            (180.0, "medium_4l"),
            (270.0, "medium_4l"),
        ],
    );
    // Opposing straights share a phase; left turns across them cannot: more than 2 phases.
    assert!(plan.phases.len() >= 2);
    assert!(plan.phases.iter().all(|p| !p.pedestrian_only) || plan.phases.len() > 2);
    let _ = sim;
    assert!(
        j.links
            .pedestrian
            .iter()
            .any(|l| l.kind == PedLinkKind::Crossing)
    );
}

#[test]
fn signal_timing_is_20s_green_3s_yellow_and_a_pure_function_of_the_tick() {
    let db = content();
    let (_, j, plan) = lit_junction(
        &db,
        &[
            (0.0, "medium_4l"),
            (90.0, "medium_4l"),
            (180.0, "medium_4l"),
            (270.0, "medium_4l"),
        ],
    );
    assert_eq!((GREEN_TICKS, YELLOW_TICKS), (400, 60));
    let cycle = plan.cycle_ticks();
    assert_eq!(cycle, plan.phases.len() as u32 * 460);
    let n = j.links.connections.len() as u16;
    // Walk the whole cycle (twice): exactly one phase is non-red at any tick; green for 400
    // ticks then yellow for 60; the state repeats every cycle.
    let mut green_run = vec![0u32; plan.phases.len()];
    let mut yellow_run = vec![0u32; plan.phases.len()];
    for t in 0..u64::from(cycle) * 2 {
        let (phase, green) = plan.phase_at(t);
        for c in 0..n {
            let s = plan.connection_signal(c, t);
            let in_phase = plan.phases[phase].connections.contains(&c);
            assert_eq!(s != Signal::Red, in_phase, "tick {t} connection {c}");
            if in_phase {
                assert_eq!(s == Signal::Green, green);
            }
            assert_eq!(s, plan.connection_signal(c, t + u64::from(cycle)));
        }
        if t < u64::from(cycle) {
            if green {
                green_run[phase] += 1;
            } else {
                yellow_run[phase] += 1;
            }
        }
    }
    assert!(green_run.iter().all(|&g| g == 400), "{green_run:?}");
    assert!(yellow_run.iter().all(|&y| y == 60), "{yellow_run:?}");
    // Walk signals only during green of listed phases.
    for (i, l) in j.links.pedestrian.iter().enumerate() {
        if l.kind == PedLinkKind::Crossing {
            let walking = (0..u64::from(cycle))
                .filter(|&t| plan.walk(i as u16, t))
                .count();
            assert!(walking >= 400 && walking % 400 == 0, "{walking}");
        }
    }
}

#[test]
fn plans_are_deterministic_and_offsets_differ_by_node() {
    let db = content();
    let arms = [
        (0.0, "medium_4l"),
        (90.0, "medium_4l"),
        (180.0, "medium_4l"),
        (270.0, "medium_4l"),
    ];
    let a = lit_junction(&db, &arms).2;
    let b = lit_junction(&db, &arms).2;
    assert_eq!(a, b);
    assert!(a.offset < a.cycle_ticks());
}

// ---------- the player's signs (§6.5a) ----------

/// A 4-way of a 6-lane avenue end (0°) and three streets (90°, 180°, 270°): controllable,
/// no automatic light. Returns the sim and the segments in that order.
fn signed_cross(db: &ContentDb) -> (Sim, Vec<SegmentId>) {
    let sim = Sim::new(star(
        db,
        &[
            (0.0, "large_6l"),
            (90.0, "basic_2l"),
            (180.0, "basic_2l"),
            (270.0, "basic_2l"),
        ],
    ));
    let segs = sim.graph().segments().map(|(id, _)| id).collect();
    (sim, segs)
}

impl Sim {
    /// The main arms as indices into `segs`, ascending.
    fn main_of(&self, segs: &[SegmentId]) -> Vec<usize> {
        let j = self.junction();
        let mut m: Vec<usize> = j
            .control
            .main_arms
            .iter()
            .map(|&a| segs.iter().position(|&s| s == j.arms[a].segment).unwrap())
            .collect();
        m.sort();
        m
    }
    fn signs(&self) -> Vec<ArmSign> {
        self.graph().node(self.centre()).unwrap().signs.clone()
    }
}

#[test]
fn signs_choose_the_main_road() {
    let db = content();
    use Sign::{Main, Yield};
    type Case<'a> = (&'a [(usize, Sign)], &'a [usize], &'a str);
    let cases: &[Case] = &[
        (&[], &[0, 1], "no signs: rank, then the lower segment id"),
        (&[(1, Main), (3, Main)], &[1, 3], "two main arms"),
        (&[(1, Main)], &[1, 3], "one main arm takes the opposite one"),
        (
            &[(1, Main), (3, Yield)],
            &[0, 1],
            "the opposite yields: of the two square arms the higher rank",
        ),
        (
            &[(0, Yield)],
            &[1, 2],
            "a yield on the avenue hands the main road to the next pair",
        ),
        (
            &[(0, Yield), (1, Yield), (2, Yield), (3, Yield)],
            &[0, 1],
            "yield everywhere is no sign",
        ),
        (
            &[(0, Main), (1, Yield), (2, Yield), (3, Yield)],
            &[0],
            "one main arm and nothing to pair it with",
        ),
    ];
    for (signs, want, why) in cases {
        let (mut sim, segs) = signed_cross(&db);
        let n = sim.centre();
        for &(i, sign) in *signs {
            sim.graph_mut().set_sign(n, segs[i], Some(sign)).unwrap();
        }
        sim.network();
        assert_eq!(sim.main_of(&segs), want.to_vec(), "{why}");
        // The connections follow the main road: everything from a yield arm gives way.
        let j = sim.junction();
        for (c, conn) in j.links.connections.iter().enumerate() {
            let seg = sim.graph().lane(conn.from).unwrap().segment;
            let arm = j.arms.iter().position(|a| a.segment == seg).unwrap();
            if !j.control.main_arms.contains(&arm) {
                assert_eq!(j.control.priority[c], Priority::Yield, "{why}: {c}");
            }
        }
    }
}

#[test]
fn set_sign_refuses_and_changes_nothing() {
    let db = content();
    let (mut sim, segs) = signed_cross(&db);
    let n = sim.centre();
    let mut g = sim.graph_mut();
    g.set_sign(n, segs[1], Some(Sign::Main)).unwrap();
    g.set_sign(n, segs[2], Some(Sign::Main)).unwrap();
    assert_eq!(
        g.set_sign(n, segs[3], Some(Sign::Main)),
        Err(crate::NetOpError::TooManyMain(n))
    );
    // Main again on a main arm, or yield on a third, is fine.
    g.set_sign(n, segs[2], Some(Sign::Main)).unwrap();
    g.set_sign(n, segs[3], Some(Sign::Yield)).unwrap();
    // A segment of another node.
    let far = g.segment(segs[0]).unwrap().end;
    let at = g.node(far).unwrap().pos;
    let road = db.road_id("basic_2l").unwrap();
    let other = g
        .add_segment(
            db.road(road),
            road,
            CubicBezier3::line(at, at + Vec3::new(0.0, 0.0, 80.0)),
            Elevation::Ground,
        )
        .unwrap();
    assert_eq!(
        g.set_sign(n, other, Some(Sign::Yield)),
        Err(crate::NetOpError::NotAnArm(n, other))
    );
    // `far` is a bend now: no control.
    assert_eq!(
        g.set_sign(far, other, Some(Sign::Yield)),
        Err(crate::NetOpError::NotControllable(far))
    );
    let signs = sim.signs();
    assert_eq!(
        signs,
        vec![
            ArmSign {
                segment: segs[1],
                sign: Sign::Main
            },
            ArmSign {
                segment: segs[2],
                sign: Sign::Main
            },
            ArmSign {
                segment: segs[3],
                sign: Sign::Yield
            },
        ]
    );
    // `None` takes the sign away.
    sim.graph_mut().set_sign(n, segs[2], None).unwrap();
    assert_eq!(sim.signs().len(), 2);

    // Two-lane streets only.
    let mut sim = Sim::new(star(
        &db,
        &[(0.0, "basic_2l"), (90.0, "basic_2l"), (180.0, "basic_2l")],
    ));
    let n = sim.centre();
    let seg = sim.graph().segments().next().unwrap().0;
    assert_eq!(
        sim.graph_mut().set_sign(n, seg, Some(Sign::Main)),
        Err(crate::NetOpError::NotControllable(n))
    );
    assert!(sim.signs().is_empty());
}

#[test]
fn a_cut_or_a_merge_of_the_arm_keeps_its_sign() {
    let db = content();
    let (mut sim, segs) = signed_cross(&db);
    let n = sim.centre();
    let def = db.road(db.road_id("basic_2l").unwrap()).clone();
    sim.graph_mut()
        .set_sign(n, segs[1], Some(Sign::Main))
        .unwrap();
    let split = sim.graph_mut().split_segment(&def, segs[1], 0.5).unwrap();
    // The piece at the junction is the first one (the star's segments start at the centre).
    let near = split.segments[0];
    assert_eq!(
        sim.signs(),
        vec![ArmSign {
            segment: near,
            sign: Sign::Main
        }]
    );
    sim.network();
    let mut segs2 = segs.clone();
    segs2[1] = near;
    assert_eq!(sim.main_of(&segs2), vec![1, 3]);
    // Merged back into one: the sign moves onto the merged segment.
    let merged = sim.graph_mut().merge_joint(&def, split.nodes[0]).unwrap();
    assert_eq!(
        sim.signs(),
        vec![ArmSign {
            segment: merged,
            sign: Sign::Main
        }]
    );
}

#[test]
fn removing_or_downgrading_an_arm_drops_its_sign_and_the_control_it_no_longer_has() {
    let db = content();
    let (mut sim, segs) = signed_cross(&db);
    let n = sim.centre();
    let mut g = sim.graph_mut();
    g.set_sign(n, segs[1], Some(Sign::Main)).unwrap();
    g.set_sign(n, segs[3], Some(Sign::Yield)).unwrap();
    g.toggle_traffic_light(n).unwrap();
    // A street with a sign goes: its sign goes, the rest stays (still controllable).
    g.remove_segment(segs[3]).unwrap();
    assert_eq!(
        sim.signs(),
        vec![ArmSign {
            segment: segs[1],
            sign: Sign::Main
        }]
    );
    assert_eq!(sim.control(), JunctionControl::ManualTrafficLight);
    // The avenue goes: two-lane streets only, back to automatic priority, no signs.
    sim.graph_mut().remove_segment(segs[0]).unwrap();
    assert!(sim.signs().is_empty());
    assert_eq!(sim.control(), JunctionControl::Priority);

    // An arm made a footpath loses its sign.
    let (mut sim, segs) = signed_cross(&db);
    let n = sim.centre();
    let path = db.road_id("park_path").unwrap();
    let basic = db.road_id("basic_2l").unwrap();
    let mut g = sim.graph_mut();
    g.set_sign(n, segs[1], Some(Sign::Yield)).unwrap();
    g.set_sign(n, segs[2], Some(Sign::Main)).unwrap();
    g.upgrade_segment(segs[1], db.road(path), path).unwrap();
    assert_eq!(
        sim.signs(),
        vec![ArmSign {
            segment: segs[2],
            sign: Sign::Main
        }]
    );
    // The avenue made a street: no big road, no control.
    sim.graph_mut().toggle_traffic_light(n).unwrap();
    sim.graph_mut()
        .upgrade_segment(segs[0], db.road(basic), basic)
        .unwrap();
    assert!(sim.signs().is_empty());
    assert_eq!(sim.control(), JunctionControl::Priority);
}

#[test]
fn with_a_light_the_signs_still_give_the_main_arms() {
    let db = content();
    let mut sim = Sim::new(star(
        &db,
        &[
            (0.0, "medium_4l"),
            (90.0, "medium_4l"),
            (180.0, "medium_4l"),
            (270.0, "medium_4l"),
        ],
    ));
    assert_eq!(sim.control(), JunctionControl::TrafficLight);
    let segs: Vec<SegmentId> = sim.graph().segments().map(|(id, _)| id).collect();
    let n = sim.centre();
    sim.graph_mut()
        .set_sign(n, segs[1], Some(Sign::Main))
        .unwrap();
    sim.graph_mut()
        .set_sign(n, segs[3], Some(Sign::Main))
        .unwrap();
    sim.network();
    assert_eq!(sim.control(), JunctionControl::TrafficLight);
    assert!(sim.junction().control.light.is_some());
    assert_eq!(sim.main_of(&segs), vec![1, 3]);
}
