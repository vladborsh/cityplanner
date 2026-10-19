//! T-264 step C: the road tool through the one resolver. The ghost is the plan a click sends
//! (`BuildPlan`, applied literally by the sim); a click takes the point the ghost put the cursor on;
//! snaps off (`Alt`, the button) is free placement but for a direct hit on a node or a street;
//! errors carry their place on the map; the length shows in cells with the angle. And the sweep
//! "built = aimed" through the tool: every street the tool builds has its vertices where the
//! resolver put them and leaves the graph clean (`NetGraph::check_invariants`).
//!
//! §6.8.5: this file is the "through the tool" half of `net_sweep` (`cp_sim` cannot depend on
//! `cp_tools`); the other half, the cases of the audit built as plans, is
//! `crates/cp_sim/tests/net_sweep.rs`.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::{CommandOutcome, Cost, Treasury};
use cp_net::{Anchor, NetCommand, NetGraph};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::road::chain_commit;
use cp_tools::{CommandOutbox, RoadMode, RoadTool, compute_ghost_alt, resolve_cursor};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const C: f32 = 4600.0;

fn world() -> (SimRunner, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    sim.tick_once();
    sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(1_000_000_000.0);
    (sim, db)
}

fn p(x: f32, z: f32) -> Vec2 {
    Vec2::new(C + x, C + z)
}

fn tool(db: &ContentDb, road: &str) -> RoadTool {
    RoadTool {
        mode: RoadMode::Straight,
        road: Some(db.road_id(road).unwrap()),
        ..Default::default()
    }
}

type Plans = (Option<cp_net::BuildPlan>, Option<cp_net::BuildPlan>);

/// What the plugin does for one click: the cursor of the ghost, the click, the commit; the plan is
/// sent as a `BuildPlan`. Returns the plan, and the ghost's own plan for the same cursor.
fn click(sim: &mut SimRunner, tool: &mut RoadTool, at: Vec2, free: bool) -> Plans {
    click_ends(sim, tool, at, free).0
}

/// [`click`], and where the ends of the piece stand before it is built.
fn click_ends(
    sim: &mut SimRunner,
    tool: &mut RoadTool,
    at: Vec2,
    free: bool,
) -> (Plans, Option<(Vec2, Vec2)>) {
    let view = sim.view();
    let ghost = compute_ghost_alt(&view, tool, at, free);
    let q = resolve_cursor(&view, tool, at, free).map_or(at, |c| c.cursor);
    let before = tool.clone();
    let sent = tool
        .click(q)
        .and_then(|req| chain_commit(Some(&view), tool, before, req, free));
    let ends = sent.as_ref().and_then(|plan| {
        Some((
            cp_tools::anchor_pos(&view, &plan.pieces[0].from)?,
            cp_tools::anchor_pos(&view, &plan.pieces.last()?.to)?,
        ))
    });
    drop(view);
    if let Some(plan) = &sent {
        let mut outbox = CommandOutbox::default();
        outbox.push(SimCommand::Net(NetCommand::BuildPlan {
            plan: plan.clone(),
        }));
        outbox.flush_into(sim);
        sim.tick_once();
        let out: Vec<CommandOutcome> = sim
            .world_mut()
            .resource_mut::<Messages<CommandOutcome>>()
            .drain()
            .collect();
        assert!(out.iter().all(|o| o.result.is_ok()), "{out:?}");
    }
    ((sent, ghost.and_then(|g| g.plan)), ends)
}

fn graph(sim: &SimRunner) -> Arc<NetGraph> {
    sim.view().net.clone()
}

fn node_at(sim: &SimRunner, at: Vec2) -> bool {
    graph(sim)
        .nodes()
        .any(|(_, n)| Vec2::new(n.pos.x, n.pos.z).distance(at) < 0.05)
}

#[test]
fn a_click_sends_the_plan_the_ghost_showed() {
    let (mut sim, db) = world();
    let mut t = tool(&db, "basic_2l");
    click(&mut sim, &mut t, p(0.0, 0.0), true);
    // Free: the point is where the cursor was; the second click completes the piece.
    let (sent, ghost) = click(&mut sim, &mut t, p(100.0, 3.0), true);
    assert_eq!(sent, ghost, "the click sends the ghost's plan");
    let plan = sent.expect("a piece");
    assert_eq!(plan.pieces.len(), 1);
    assert!(matches!(plan.pieces[0].from, Anchor::Free(a) if a == p(0.0, 0.0)));
    assert!(matches!(plan.pieces[0].to, Anchor::Free(b) if b == p(100.0, 3.0)));
    // With snaps on the point is stepped to whole cells (between the half widths its dead ends
    // keep, T-275), and the plan is still the ghost's.
    let (mut sim, db) = world();
    let mut t = tool(&db, "basic_2l");
    click(&mut sim, &mut t, p(0.0, 0.0), false);
    let (sent, ghost) = click(&mut sim, &mut t, p(101.0, 0.5), false);
    assert_eq!(sent, ghost);
    let Anchor::Free(end) = sent.unwrap().pieces[0].to else {
        panic!("free end")
    };
    let cells = ((end - p(0.0, 0.0)).length() - 12.0) / 8.0;
    assert!((cells - cells.round()).abs() < 0.01, "{end:?}");
}

#[test]
fn a_curve_click_sends_the_plan_the_ghost_showed() {
    let (mut sim, db) = world();
    let mut t = tool(&db, "basic_2l");
    t.mode = RoadMode::Curve;
    click(&mut sim, &mut t, p(0.0, 0.0), true);
    // The control point, then the end: the ghost already draws the curve from the control.
    let (sent, _) = click(&mut sim, &mut t, p(50.0, 40.0), true);
    assert!(sent.is_none(), "the control point completes nothing");
    let (sent, ghost) = click(&mut sim, &mut t, p(100.0, 0.0), true);
    assert_eq!(sent, ghost, "the click sends the ghost's plan");
    let plan = sent.expect("a curve");
    assert_eq!(plan.pieces.len(), 1);
    let piece = plan.pieces[0];
    // Bent: the control points are off the chord.
    let chord = p(100.0, 0.0) - p(0.0, 0.0);
    assert!(
        chord.perp_dot(piece.c1 - p(0.0, 0.0)).abs() > 1.0,
        "{piece:?}"
    );
    assert!(
        chord.perp_dot(piece.c2 - p(0.0, 0.0)).abs() > 1.0,
        "{piece:?}"
    );
}

#[test]
fn free_placement_keeps_the_point_where_it_is_snaps_move_it() {
    let (mut sim, db) = world();
    let mut t = tool(&db, "basic_2l");
    click(&mut sim, &mut t, p(0.0, 0.0), true);
    let odd = p(61.3, 17.7);
    let view = sim.view();
    let free = compute_ghost_alt(&view, &t, odd, true).unwrap();
    assert_eq!(free.resolved, Some(odd), "free: not moved");
    let snapped = compute_ghost_alt(&view, &t, odd, false).unwrap();
    assert_ne!(snapped.resolved, Some(odd), "snaps on: stepped");
    assert_eq!(free.snap.kind, cp_net::SnapReportKind::None);
}

#[test]
fn a_direct_hit_on_a_node_or_a_street_stays_when_snaps_are_off() {
    let (mut sim, db) = world();
    let mut t = tool(&db, "basic_2l");
    click(&mut sim, &mut t, p(-100.0, 0.0), true);
    click(&mut sim, &mut t, p(100.0, 0.0), true);
    t.cancel();
    // 3 m beside the axis of the street (half width 4): the street takes the point, onto its axis.
    click(&mut sim, &mut t, p(31.0, 60.0), true);
    let (sent, ghost) = click(&mut sim, &mut t, p(31.0, 3.0), true);
    assert_eq!(sent, ghost);
    let plan = sent.unwrap();
    assert!(
        matches!(plan.pieces[0].to, Anchor::OnSegment(..)),
        "{plan:?}"
    );
    let g = graph(&sim);
    assert!(
        g.nodes()
            .any(|(_, n)| (n.pos.x - (C + 31.0)).abs() < 0.05 && (n.pos.z - C).abs() < 0.05),
        "a T on the axis"
    );
}

#[test]
fn errors_have_a_place_on_the_map_and_the_length_shows_in_cells() {
    let (sim, db) = world();
    let mut t = tool(&db, "basic_2l");
    t.points = vec![p(0.0, 0.0)];
    let view = sim.view();
    // 5 m is under the shortest piece: red, with its place.
    let short = compute_ghost_alt(&view, &t, p(5.0, 0.0), true).unwrap();
    assert!(!short.valid && !short.errors.is_empty());
    assert!(
        short
            .error_marks
            .iter()
            .any(|m| m.distance(p(0.0, 0.0)) < 10.0),
        "{:?}",
        short.error_marks
    );
    // 80 m drawn free: the cells it gets, 8 (68 m between the half widths its dead ends keep,
    // T-275); no road to measure an angle against.
    let ok = compute_ghost_alt(&view, &t, p(80.0, 0.0), true).unwrap();
    assert!(ok.valid, "{:?}", ok.errors);
    assert_eq!(ok.label.as_deref(), Some("8 cells"));
}

#[test]
fn the_label_gives_the_angle_to_the_road_a_street_leaves() {
    let (mut sim, db) = world();
    let mut t = tool(&db, "basic_2l");
    click(&mut sim, &mut t, p(-100.0, 0.0), true);
    click(&mut sim, &mut t, p(100.0, 0.0), true);
    t.cancel();
    t.points = vec![p(0.0, 0.0)];
    let view = sim.view();
    let g = compute_ghost_alt(&view, &t, p(0.0, 64.0), true).unwrap();
    assert!(g.valid, "{:?}", g.errors);
    // 64 m drawn free from the axis: its cells start at the street's edge (6 m) and stop a half
    // width short of its open end, 6 whole cells (T-275: the label is the cells it gets).
    assert_eq!(g.label.as_deref(), Some("6 cells · 90°"));
}

/// The sweep "built = aimed" through the tool: a side street drawn from the axis of a street at
/// `d` metres from a T, a dead end or a cut; free, as a player with snaps off, or snapped. Every
/// vertex of what is built is a node of the graph, and the graph keeps its invariants.
#[test]
fn what_the_tool_builds_is_what_the_resolver_aimed_at_and_the_graph_stays_clean() {
    let mut cases = 0;
    for road in ["basic_2l", "medium_4l", "large_6l"] {
        for d in [40.0_f32, 56.0, 72.0] {
            for free in [true, false] {
                let (mut sim, db) = world();
                let mut t = tool(&db, road);
                // The street, and a street across it at x = 0 (a T).
                click(&mut sim, &mut t, p(-120.0, 0.0), true);
                click(&mut sim, &mut t, p(120.0, 0.0), true);
                t.cancel();
                click(&mut sim, &mut t, p(0.0, -100.0), true);
                click(&mut sim, &mut t, p(0.0, 0.0), true);
                t.cancel();
                // A side street from the axis of the street, `d` m from the T, to the south.
                click(&mut sim, &mut t, p(-d, 0.0), free);
                let end = p(-d, 64.0);
                let why = compute_ghost_alt(&sim.view(), &t, end, free)
                    .map(|g| g.errors)
                    .unwrap_or_default();
                let ((sent, ghost), ends) = click_ends(&mut sim, &mut t, end, free);
                assert_eq!(sent, ghost, "{road} {d} {free}: the click is the ghost");
                sent.unwrap_or_else(|| panic!("{road} {d} {free}: refused: {why:?}"));
                // The vertices of the plan are nodes of the graph.
                let (a, b) = ends.expect("ends");
                for at in [a, b] {
                    assert!(node_at(&sim, at), "{road} {d} {free}: no node at {at:?}");
                }
                assert_eq!(
                    graph(&sim).check_invariants(),
                    Vec::<cp_net::Violation>::new(),
                    "{road} {d} {free}"
                );
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 18);
}

/// Where the last point of the ghost's plan stands: a node, a point of a street, or free.
fn last_anchor(sim: &SimRunner, t: &RoadTool, at: Vec2, free: bool) -> Option<Anchor> {
    resolve_cursor(&sim.view(), t, at, free)
        .and_then(|c| c.plan)
        .and_then(|plan| plan.pieces.last().map(|p| p.to))
}

/// A rounded L (BUG-085 shot 1): a street up, a bend of radius `r`, a street to the west.
fn rounded_l(road: &str, r: f32) -> SimRunner {
    let (mut sim, db) = world();
    let mut t = tool(&db, road);
    click(&mut sim, &mut t, p(0.0, 150.0), true);
    click(&mut sim, &mut t, p(0.0, r), true);
    t.cancel();
    t.mode = RoadMode::Curve;
    click(&mut sim, &mut t, p(0.0, r), true);
    click(&mut sim, &mut t, p(0.0, 0.0), true);
    click(&mut sim, &mut t, p(-r, 0.0), true);
    t.cancel();
    t.mode = RoadMode::Straight;
    click(&mut sim, &mut t, p(-r, 0.0), true);
    click(&mut sim, &mut t, p(-150.0, 0.0), true);
    t.cancel();
    sim
}

/// BUG-085: a street dragged to the outside of a rounded corner from the side ends ON the
/// corner (the bend's end node, or a point of the bend or the street), never at the kerb on a
/// guide or a length step where it is refused; also beside a straight street and a T, on a 2-lane
/// and a 6-lane road, with snaps on and off.
#[test]
fn a_street_dragged_to_a_road_ends_on_it() {
    for road in ["basic_2l", "large_6l"] {
        let db = world().1;
        let hw = db.road(db.road_id(road).unwrap()).width / 2.0;
        // The user's corner on a 2-lane street; a wide road gets a bend it can take.
        let r = if hw > 6.0 { 48.0 } else { 24.0 };
        let k = std::f32::consts::FRAC_1_SQRT_2;
        let targets = [
            ("beside the bend node", p(hw + 2.0, r)),
            // 2 m off the outer kerb of the bend at t = 0.3 (the bend is the quadratic
            // (0, r) -> (0, 0) -> (-r, 0): at t its point is (-r t^2, r (1 - t)^2), its
            // outward normal (1 - t, -t)).
            ("beside the bend", {
                let n = Vec2::new(0.7, -0.3).normalize();
                p(-r * 0.09, r * 0.49) + n * (hw + 2.0)
            }),
            (
                "on the bend's outer sidewalk",
                p(-r / 4.0 + k * (hw - 0.5), r / 4.0 - k * (hw - 0.5)),
            ),
            ("beside the street", p(hw + 3.0, 60.0)),
        ];
        for (name, to) in targets {
            for free in [true, false] {
                let mut sim = rounded_l(road, r);
                let mut t = tool(&db, road);
                click(&mut sim, &mut t, p(120.0, to.y - C), free);
                let case = format!("{road} {name} free={free}");
                let g = compute_ghost_alt(&sim.view(), &t, to, free).unwrap();
                assert!(g.valid, "{case}: red ghost {:?}", g.errors);
                let end = last_anchor(&sim, &t, to, free);
                assert!(
                    matches!(end, Some(Anchor::Node(_) | Anchor::OnSegment(..))),
                    "{case}: the end is not on the road: {end:?}"
                );
                let (sent, ghost) = click(&mut sim, &mut t, to, free);
                assert_eq!(sent, ghost, "{case}");
                assert!(sent.is_some(), "{case}: refused");
                assert_eq!(
                    graph(&sim).check_invariants(),
                    Vec::<cp_net::Violation>::new(),
                    "{case}"
                );
            }
        }
        // A sharp T: a side street to just off the kerb of a street, and to just off the kerb
        // 3 m from the T (the junction takes it).
        for (name, x) in [("off the kerb", 40.0), ("off the kerb by the T", 3.0)] {
            for free in [true, false] {
                let (mut sim, db) = world();
                let mut t = tool(&db, road);
                click(&mut sim, &mut t, p(-120.0, 0.0), true);
                click(&mut sim, &mut t, p(120.0, 0.0), true);
                t.cancel();
                click(&mut sim, &mut t, p(0.0, -100.0), true);
                click(&mut sim, &mut t, p(0.0, 0.0), true);
                t.cancel();
                click(&mut sim, &mut t, p(x, 90.0), free);
                let to = p(x, hw + 2.0);
                let case = format!("{road} T {name} free={free}");
                let g = compute_ghost_alt(&sim.view(), &t, to, free).unwrap();
                assert!(g.valid, "{case}: red ghost {:?}", g.errors);
                let end = last_anchor(&sim, &t, to, free);
                if x < 8.0 {
                    assert!(matches!(end, Some(Anchor::Node(_))), "{case}: {end:?}");
                } else {
                    assert!(
                        matches!(end, Some(Anchor::OnSegment(..))),
                        "{case}: {end:?}"
                    );
                }
                let (sent, _) = click(&mut sim, &mut t, to, free);
                assert!(sent.is_some(), "{case}: refused");
                assert_eq!(
                    graph(&sim).check_invariants(),
                    Vec::<cp_net::Violation>::new(),
                    "{case}"
                );
            }
        }
    }
}

/// BUG-086: on an axis-aligned grid a street drawn east from beside a junction runs along the
/// axis: its guide and its built street are square to the cross street (label 90°), wherever near
/// the junction the first click lands.
#[test]
fn a_street_from_a_junction_on_a_square_grid_is_square() {
    for road in ["basic_2l", "medium_4l"] {
        for start in [
            (0.0, 0.0),
            (3.0, 2.0),
            (6.0, -3.0),
            (9.0, 1.0),
            (12.0, 4.0),
            (-4.0, 5.0),
        ] {
            for cursor in [(100.0, 4.0), (93.0, -6.0), (150.0, 9.0)] {
                let (mut sim, db) = world();
                let mut t = tool(&db, road);
                // A T at the origin: a street along z, and one along x ending on it from the
                // west; the new street goes on east across it.
                click(&mut sim, &mut t, p(-200.0, 0.0), true);
                click(&mut sim, &mut t, p(0.0, 0.0), true);
                t.cancel();
                click(&mut sim, &mut t, p(0.0, -200.0), true);
                click(&mut sim, &mut t, p(0.0, 200.0), true);
                t.cancel();
                let case = format!("{road} start {start:?} cursor {cursor:?}");
                click(&mut sim, &mut t, p(start.0, start.1), false);
                let to = p(cursor.0, cursor.1);
                let g = compute_ghost_alt(&sim.view(), &t, to, false).unwrap();
                let plan = resolve_cursor(&sim.view(), &t, to, false)
                    .and_then(|c| c.plan)
                    .unwrap_or_else(|| panic!("{case}: no plan"));
                assert!(g.valid, "{case}: red {:?} {:?}", g.errors, plan.pieces);
                let view = sim.view();
                let a = cp_tools::anchor_pos(&view, &plan.pieces[0].from).unwrap();
                let b = cp_tools::anchor_pos(&view, &plan.pieces.last().unwrap().to).unwrap();
                let d = b - a;
                let off = d.y.atan2(d.x).to_degrees();
                assert!(
                    off.abs() < 0.1,
                    "{case}: the street runs {off:.2} deg off the axis ({a:?} -> {b:?}), label {:?}, guide {:?}",
                    g.label,
                    g.snap.guide
                );
                if let Some((g0, g1)) = g.snap.guide {
                    let gd = g1 - g0;
                    let ga = gd.y.atan2(gd.x).to_degrees().rem_euclid(90.0);
                    assert!(
                        ga.min(90.0 - ga) < 0.1,
                        "{case}: the guide is {ga:.2} deg off the grid"
                    );
                }
            }
        }
    }
}

/// A layout of BUG-088: its name, two streets, the start, the square end, and the direction of
/// the cursor's error.
type Layout = (
    &'static str,
    [[(f32, f32); 2]; 2],
    (f32, f32),
    (f32, f32),
    (f32, f32),
);

/// BUG-088: a street that closes a block from a dead end to a side street, aimed a few degrees
/// off square, ends where its square direction meets the side street, not at the cursor's foot:
/// on the extension of the dead-end street (shot 2: the side street runs on lower than the start)
/// and square to it (shot 1: down from a street's end to the street below). Click == ghost, the
/// built street is square.
#[test]
fn a_street_closing_a_block_along_the_guides_is_square() {
    for road in ["basic_2l", "medium_4l"] {
        // (name, the streets, the start, the square end, the direction of the cursor's error)
        let layouts: [Layout; 2] = [
            (
                "extension",
                [[(-150.0, 0.0), (0.0, 0.0)], [(72.0, 60.0), (72.0, -120.0)]],
                (0.0, 0.0),
                (72.0, 0.0),
                (0.0, -1.0),
            ),
            (
                "down from an end",
                [[(-150.0, 72.0), (0.0, 72.0)], [(-150.0, 0.0), (150.0, 0.0)]],
                (0.0, 72.0),
                (0.0, 0.0),
                (1.0, 0.0),
            ),
        ];
        for (name, streets, start, end, err) in layouts {
            for off in [2.0_f32, 3.0, -3.0, 5.0] {
                let (mut sim, db) = world();
                let mut t = tool(&db, road);
                for [a, b] in streets {
                    click(&mut sim, &mut t, p(a.0, a.1), true);
                    click(&mut sim, &mut t, p(b.0, b.1), true);
                    t.cancel();
                }
                let case = format!("{road} {name} {off} m off");
                let start = p(start.0, start.1);
                let end = p(end.0, end.1);
                let to = end + Vec2::new(err.0, err.1) * off;
                click(&mut sim, &mut t, start, false);
                let g = compute_ghost_alt(&sim.view(), &t, to, false).unwrap();
                assert!(g.valid, "{case}: red {:?}", g.errors);
                let ((sent, ghost), ends) = click_ends(&mut sim, &mut t, to, false);
                assert_eq!(sent, ghost, "{case}: the click is the ghost");
                let (a, b) = ends.unwrap_or_else(|| panic!("{case}: refused"));
                assert!(a.distance(start) < 0.05, "{case}: starts at {a:?}");
                assert!(
                    b.distance(end) < 0.05,
                    "{case}: ends at {b:?}, not square at {end:?} (label {:?})",
                    g.label
                );
                assert_eq!(
                    graph(&sim).check_invariants(),
                    Vec::<cp_net::Violation>::new(),
                    "{case}"
                );
            }
        }
    }
}
