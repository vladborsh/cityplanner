//! T-261: the road network sweep (`architecture/system-design.md` §6.8.5): what is built is
//! what was aimed at, and the graph stays clean after every build.
//!
//! Each case sends `BuildPlan` (T-262) with the points resolved the way the tool resolves them
//! (§6.8.3 item 1, `road_cases::resolve_anchor`): a node, a point on a street's axis, a free
//! point; the sim applies them as they are.
//! Scenarios, each at 0, 15, 45 and 90 degrees with `basic_2l`, `medium_4l` and `large_6l` as
//! the existing street:
//!
//! * `near_node/<kind>/<d>`: a side street from the existing street's axis, `d` metres from a
//!   T (`t`, on the side away from its arm), a dead end (`end`) or a 20-degree bend (`bend`),
//!   square to it;
//! * `wide_join/<where>`: a `basic_2l` ending on the existing street: on its axis (`axis`), on
//!   its asphalt short of the axis (`short`) or past it (`past`);
//! * `cross_near_node/<d>`: a street across the existing one at 60 degrees, `d` metres from a T;
//! * `through_end/<off>`: a street across the end of a dead end, through it (`0`) or 3 m beside;
//! * `cut_cross/<d>`: a street across a long street `d` metres past its first automatic cut
//!   (BUG-069 merges the cut away).
//!
//! Checks (categories of the report): `moved` a point that was aimed at is not a node where it
//! was sent (more than 5 cm off; the validator snapped it elsewhere, §6.8.1 R1); `short` /
//! `long` a segment under 8 m or over 100 m; `cross` two segments cross without a node;
//! `dangling` a node lies on the asphalt of a street it is not joined to; `refused` a build
//! that must be accepted was not.
//!
//! Run: `cargo nextest run -p cp_sim --test net_sweep`. `NET_SWEEP_REPORT=1` prints every
//! failing case (`--no-capture`), `NET_SWEEP_ONLY=<case prefix>` runs only those. Cases that
//! fail today are in [`KNOWN_RED`]; `known_red_cases_still_fail` says when one passes.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::curve::to_xz;
use cp_net::{NetGraph, RoadPath};
use cp_sim::SimRunner;
use cp_sim::road_cases::{CaseRoad, build_roads_plan};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const ANGLES: [f32; 4] = [0.0, 15.0, 45.0, 90.0];
const ROADS: [&str; 3] = ["basic_2l", "medium_4l", "large_6l"];
/// Shortest and longest segment the graph may hold (§6.8.3 item 2).
const MIN_LEN: f32 = 8.0;
const MAX_LEN: f32 = 100.0;

/// Cases that fail today (T-264 step A; the five prefixes below are step B's). T-262 took the sim's snaps away (R1, R2); T-264 gave the
/// resolver of the road tool the v7 rule (§6.8.3 items 1 and 5: a point inside a junction's reach
/// is the node, a point within the half width of a street is a point of its axis) and made the
/// sim refuse a plan whose node stands on the asphalt of a road it does not join (`through_end/3`).
/// Prefixes of `scenario/.../road/angle` (a prefix ending in `/` covers every case under it);
/// `known_red_cases_still_fail` names the cases that pass.
const KNOWN_RED: &[&str] = &[
    // Owner T-264 step B (the junction footprint test, architect review of step A): two
    // junctions 8-12 m apart on a wide road (a T 10 m from the dead end of a 6-lane street; a
    // crossing 9.3 m from a cut of one) leave a node inside the footprint of the other. The
    // reach of a dead end or a plain cut is 8 m, so the point is not pulled onto it; the check
    // that a point inside any arm's asphalt within the junction snaps comes with the resolver.
    "near_node/end/10/large_6l/",
    "near_node/end/12/large_6l/",
    "near_node/end/8/large_6l/15",
    "near_node/end/8/medium_4l/15",
    "cross_near_node/24/large_6l/",
];

fn known_red(case: &str) -> bool {
    KNOWN_RED.iter().any(|k| case.starts_with(k))
}

fn assets() -> (Arc<ContentDb>, Arc<Terrain>) {
    static A: OnceLock<(Arc<ContentDb>, Arc<Terrain>)> = OnceLock::new();
    A.get_or_init(|| {
        let db = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
        let t = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
        (Arc::new(db), Arc::new(t))
    })
    .clone()
}

/// A fresh city with a frame turned by `angle` about a point in the middle of the map.
struct Case {
    sim: SimRunner,
    db: Arc<ContentDb>,
    o: Vec2,
    rot: Vec2,
    /// Half width of the existing street.
    hw: f32,
    road: &'static str,
    fails: Vec<(String, String)>,
}

impl Case {
    fn new(road: &'static str, angle: f32) -> Self {
        let (db, t) = assets();
        let mut sim = SimRunner::new(1);
        sim.set_terrain(t);
        sim.set_content(Arc::clone(&db));
        sim.tick_once();
        let hw = db.road(db.road_id(road).unwrap()).width / 2.0;
        Self {
            sim,
            db,
            o: Vec2::new(4600.0, 4600.0),
            rot: Vec2::from_angle(angle.to_radians()),
            hw,
            road,
            fails: Vec::new(),
        }
    }

    fn fail(&mut self, cat: &str, msg: String) {
        self.fails.push((cat.into(), msg));
    }

    fn w(&self, l: Vec2) -> Vec2 {
        self.o + self.rot.rotate(l)
    }

    fn l(&self, w: Vec2) -> Vec2 {
        Vec2::new(self.rot.x, -self.rot.y).rotate(w - self.o)
    }

    /// Builds `roads` (local frame); `must` says a refusal is a failure.
    fn build(&mut self, what: &str, roads: Vec<CaseRoad>, must: bool) -> bool {
        let world: Vec<CaseRoad> = roads
            .into_iter()
            .map(|(r, p)| {
                let p = match p {
                    RoadPath::Straight { from, to } => RoadPath::Straight {
                        from: self.rot.rotate(from),
                        to: self.rot.rotate(to),
                    },
                    RoadPath::Curve { from, control, to } => RoadPath::Curve {
                        from: self.rot.rotate(from),
                        control: self.rot.rotate(control),
                        to: self.rot.rotate(to),
                    },
                    RoadPath::Free(v) => {
                        RoadPath::Free(v.iter().map(|q| self.rot.rotate(*q)).collect())
                    }
                };
                (r, p)
            })
            .collect();
        let refused = build_roads_plan(&mut self.sim, self.o, &world);
        if must {
            for (i, e) in &refused {
                self.fail("refused", format!("{what}: road {i} refused: {e}"));
            }
        }
        refused.is_empty()
    }

    fn line(&self, a: Vec2, b: Vec2) -> CaseRoad {
        (self.road, RoadPath::Straight { from: a, to: b })
    }

    /// Where a point aimed at (local frame) ends up (§6.8.3 items 1 and 5), the rule stated here
    /// on its own, not read from `cp_net::junction_reach`: a point within 8 m of a node is that
    /// node; inside the junction of a junction (three roads or more) or a bend (two roads that
    /// turn more than 10 degrees, or of two types), within the half width of the widest road
    /// there as well. Anything else stays.
    fn through(&self, p: Vec2) -> Vec2 {
        let view = self.sim.view();
        let net = &view.net;
        let w = self.w(p);
        let reach = |n: &cp_net::Node, id: cp_net::NodeId| {
            let _ = id;
            let segs: Vec<&cp_net::Segment> =
                n.segments.iter().filter_map(|&s| net.segment(s)).collect();
            let straight = match segs.as_slice() {
                [a, b] => {
                    let away = |s: &cp_net::Segment| {
                        if to_xz(n.pos).distance(to_xz(s.curve.p0)) < 0.05 {
                            to_xz(s.curve.tangent(0.0)).normalize()
                        } else {
                            -to_xz(s.curve.tangent(1.0)).normalize()
                        }
                    };
                    a.road == b.road && away(a).dot(away(b)) < -0.985
                }
                _ => segs.len() <= 1,
            };
            if straight {
                8.0
            } else {
                let hw = segs
                    .iter()
                    .map(|s| self.db.road(s.road).width / 2.0)
                    .fold(0.0, f32::max);
                hw.max(8.0)
            }
        };
        net.nodes()
            .filter(|(_, n)| n.elevation == cp_net::Elevation::Ground)
            .map(|(id, n)| (to_xz(n.pos), n, id))
            .filter(|(q, n, id)| q.distance(w) <= reach(n, *id))
            .min_by(|a, b| a.0.distance(w).total_cmp(&b.0.distance(w)))
            .map_or(p, |(q, ..)| self.l(q))
    }

    /// `moved`: a node stands at each local point.
    fn nodes_at(&mut self, what: &str, points: &[Vec2]) {
        let nodes: Vec<Vec2> = self
            .sim
            .view()
            .net
            .nodes()
            .map(|(_, n)| to_xz(n.pos))
            .collect();
        for &p in points {
            let w = self.w(p);
            let (d, at) = nodes
                .iter()
                .map(|n| (n.distance(w), *n))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .unwrap_or((f32::INFINITY, w));
            if d > 0.05 {
                let at = self.l(at);
                self.fail(
                    "moved",
                    format!("{what}: aimed at {p:?}, the nearest node is at ({:.2}, {:.2}), {d:.2} m off", at.x, at.y),
                );
            }
        }
    }

    /// The graph invariants of §6.8.3 item 2, over the whole network.
    fn invariants(&mut self) {
        let view = self.sim.view();
        let net: &NetGraph = &view.net;
        let mut fails = Vec::new();
        let polys: Vec<(cp_net::SegmentId, Vec<Vec2>, f32, [Vec2; 2])> = net
            .segments()
            .map(|(id, s)| {
                let n = (s.length / 0.5).ceil().max(2.0) as usize;
                let pts: Vec<Vec2> = (0..=n)
                    .map(|i| to_xz(s.curve.point(i as f32 / n as f32)))
                    .collect();
                let ends = [pts[0], pts[n]];
                (id, pts, self.db.road(s.road).width / 2.0, ends)
            })
            .collect();
        for (id, s) in net.segments() {
            let c = self.l(to_xz(s.curve.point(0.5)));
            if s.length < MIN_LEN - 0.01 {
                fails.push((
                    "short",
                    format!(
                        "segment {id:?} at ({:.1}, {:.1}) is {:.2} m",
                        c.x, c.y, s.length
                    ),
                ));
            }
            if s.length > MAX_LEN + 0.01 {
                fails.push((
                    "long",
                    format!(
                        "segment {id:?} at ({:.1}, {:.1}) is {:.2} m",
                        c.x, c.y, s.length
                    ),
                ));
            }
        }
        // Crossings without a node: polyline pieces that cross away from both curves' ends.
        for i in 0..polys.len() {
            for j in i + 1..polys.len() {
                let (a, b) = (&polys[i], &polys[j]);
                if let Some(p) = crossing(&a.1, &b.1, &[a.3[0], a.3[1], b.3[0], b.3[1]]) {
                    let q = self.l(p);
                    fails.push((
                        "cross",
                        format!(
                            "{:?} and {:?} cross at ({:.1}, {:.1}) without a node",
                            a.0, b.0, q.x, q.y
                        ),
                    ));
                }
            }
        }
        // A node on the asphalt of a street it does not belong to. A segment that runs straight
        // on from one of the node's own segments is the same street, not another one (a short
        // piece between two cuts of a wide street, T-262).
        let runs_on = |n: &cp_net::Node, sid: cp_net::SegmentId| {
            let Some(s) = net.segment(sid) else {
                return false;
            };
            n.segments.iter().filter_map(|&i| net.segment(i)).any(|i| {
                [s.start, s.end].iter().any(|&m| {
                    if i.start != m && i.end != m {
                        return false;
                    }
                    // Only through a plain cut: two junctions 8 m apart on a wide road stay red.
                    if !net.node(m).is_some_and(|x| x.segments.len() == 2) {
                        return false;
                    }
                    let away = |g: &cp_net::Segment| {
                        if g.start == m {
                            to_xz(g.curve.tangent(0.0)).normalize_or_zero()
                        } else {
                            -to_xz(g.curve.tangent(1.0)).normalize_or_zero()
                        }
                    };
                    away(i).dot(away(s)) < -0.98
                })
            })
        };
        for (nid, n) in net.nodes() {
            let p = to_xz(n.pos);
            for (sid, pts, hw, _) in &polys {
                if n.segments.contains(sid) || runs_on(n, *sid) {
                    continue;
                }
                let d = dist_to_polyline(p, pts);
                if d < hw - 0.3 {
                    let q = self.l(p);
                    fails.push(("dangling", format!("node {nid:?} at ({:.1}, {:.1}) lies {d:.2} m from the axis of {sid:?} (half width {hw}) without joining it", q.x, q.y)));
                }
            }
        }
        drop(view);
        for (c, m) in fails {
            self.fail(c, m);
        }
    }

    /// The nodes of degree 2 along the local x axis, by x.
    fn cuts(&self) -> Vec<f32> {
        let view = self.sim.view();
        let mut xs: Vec<f32> = view
            .net
            .nodes()
            .filter(|(_, n)| n.segments.len() == 2)
            .map(|(_, n)| self.l(to_xz(n.pos)))
            .filter(|p| p.y.abs() < 0.1)
            .map(|p| p.x)
            .collect();
        xs.sort_by(f32::total_cmp);
        xs
    }
}

fn dist_to_polyline(p: Vec2, pts: &[Vec2]) -> f32 {
    pts.windows(2)
        .map(|w| {
            let ab = w[1] - w[0];
            let t = ((p - w[0]).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
            p.distance(w[0] + ab * t)
        })
        .fold(f32::INFINITY, f32::min)
}

/// A point where the two polylines cross, at least 1 m from every end in `ends`.
fn crossing(a: &[Vec2], b: &[Vec2], ends: &[Vec2]) -> Option<Vec2> {
    for p in a.windows(2) {
        for q in b.windows(2) {
            let (r, s) = (p[1] - p[0], q[1] - q[0]);
            let den = r.perp_dot(s);
            if den.abs() < 1e-6 {
                continue;
            }
            let t = (q[0] - p[0]).perp_dot(s) / den;
            let u = (q[0] - p[0]).perp_dot(r) / den;
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                let x = p[0] + r * t;
                if ends.iter().all(|e| e.distance(x) > 1.0) {
                    return Some(x);
                }
            }
        }
    }
    None
}

// ---------- scenarios ----------

/// The existing street of `near_node`, with its node at the local origin.
fn base(c: &mut Case, kind: &str) {
    let x = Vec2::X;
    let roads = match kind {
        "t" => vec![
            c.line(-x * 100.0, x * 100.0),
            c.line(Vec2::ZERO, Vec2::Y * 100.0),
        ],
        "end" => vec![c.line(-x * 100.0, Vec2::ZERO)],
        _ => vec![
            c.line(-x * 100.0, Vec2::ZERO),
            c.line(Vec2::ZERO, Vec2::from_angle(20f32.to_radians()) * 100.0),
        ],
    };
    c.build("the existing street", roads, true);
}

fn near_node(c: &mut Case, kind: &str, d: f32) {
    base(c, kind);
    let (a, b) = (Vec2::new(-d, 0.0), Vec2::new(-d, -80.0));
    let start = c.through(a);
    c.build(
        "the side street",
        vec![("basic_2l", RoadPath::Straight { from: a, to: b })],
        d >= MIN_LEN,
    );
    if d >= MIN_LEN {
        // A start inside the junction of the T, the dead end or the bend is on its node.
        c.nodes_at("the side street", &[start, b]);
    }
    c.invariants();
}

fn wide_join(c: &mut Case, at: &str) {
    c.build(
        "the existing street",
        vec![c.line(-Vec2::X * 100.0, Vec2::X * 100.0)],
        true,
    );
    let y = match at {
        "axis" => 0.0,
        "short" => c.hw * 0.6,
        _ => -c.hw * 0.6,
    };
    let (a, b) = (Vec2::new(0.0, 80.0), Vec2::new(0.0, y));
    let built = c.build(
        "the street onto it",
        vec![("basic_2l", RoadPath::Straight { from: a, to: b })],
        at == "axis",
    );
    if at == "axis" || built {
        c.nodes_at("the street onto it", &[a]);
    }
    if at == "axis" {
        c.nodes_at("the street onto it", &[b]);
    }
    c.invariants();
}

fn cross_near_node(c: &mut Case, d: f32) {
    base(c, "t");
    let m = Vec2::new(d, 0.0);
    let v = Vec2::from_angle(60f32.to_radians()) * 80.0;
    let (a, b) = (m - v, m + v);
    let must = d >= 12.0;
    let crossing = c.through(m);
    c.build(
        "the street across",
        vec![("basic_2l", RoadPath::Straight { from: a, to: b })],
        must,
    );
    c.nodes_at("the street across", &[a, b]);
    if must {
        c.nodes_at("the crossing", &[crossing]);
    }
    c.invariants();
}

fn through_end(c: &mut Case, off: f32) {
    base(c, "end");
    let (a, b) = (Vec2::new(off, -80.0), Vec2::new(off, 80.0));
    c.build(
        "the street across the end",
        vec![("basic_2l", RoadPath::Straight { from: a, to: b })],
        off == 0.0,
    );
    if off == 0.0 {
        c.nodes_at("the street across the end", &[a, b, Vec2::ZERO]);
    }
    c.invariants();
}

fn cut_cross(c: &mut Case, d: f32) {
    c.build(
        "the long street",
        vec![c.line(-Vec2::X * 150.0, Vec2::X * 150.0)],
        true,
    );
    let Some(&cut) = c.cuts().first() else {
        c.fail("refused", "the long street has no automatic cut".into());
        return;
    };
    let x = cut + d;
    let (a, b) = (Vec2::new(x, -80.0), Vec2::new(x, 80.0));
    let crossing_at = c.through(Vec2::new(x, 0.0));
    c.build(
        "the street across",
        vec![("basic_2l", RoadPath::Straight { from: a, to: b })],
        true,
    );
    // A crossing within the reach of the cut goes through the cut (§6.8.3 item 2): the node stays
    // and the street across bends onto it; further out the crossing is where it was aimed.
    let crossing = crossing_at;
    c.nodes_at("the street across", &[a, b, crossing]);
    c.invariants();
}

// ---------- the sweep ----------

type Outcome = (String, Vec<(String, String)>);

fn sweep(out: &mut Vec<Outcome>, scenario: &str, run: impl Fn(&mut Case)) {
    for road in ROADS {
        for angle in ANGLES {
            let name = format!("{scenario}/{road}/{angle}");
            if let Ok(only) = std::env::var("NET_SWEEP_ONLY")
                && !name.starts_with(&only)
            {
                continue;
            }
            let mut c = Case::new(road, angle);
            run(&mut c);
            out.push((name, c.fails));
        }
    }
}

fn all_cases() -> Vec<Outcome> {
    let mut out = Vec::new();
    for kind in ["t", "end", "bend"] {
        for d in [4.0, 8.0, 10.0, 12.0, 16.0] {
            sweep(&mut out, &format!("near_node/{kind}/{d}"), |c| {
                near_node(c, kind, d)
            });
        }
    }
    for at in ["axis", "short", "past"] {
        sweep(&mut out, &format!("wide_join/{at}"), |c| wide_join(c, at));
    }
    for d in [3.0, 6.0, 12.0, 24.0] {
        sweep(&mut out, &format!("cross_near_node/{d}"), |c| {
            cross_near_node(c, d)
        });
    }
    for off in [0.0, 3.0] {
        sweep(&mut out, &format!("through_end/{off}"), |c| {
            through_end(c, off)
        });
    }
    for d in [5.0, 38.0] {
        sweep(&mut out, &format!("cut_cross/{d}"), |c| cut_cross(c, d));
    }
    out
}

fn report(cases: &[Outcome]) -> String {
    let mut by_cat: BTreeMap<&str, usize> = BTreeMap::new();
    let mut s = String::new();
    for (case, fails) in cases.iter().filter(|(_, f)| !f.is_empty()) {
        s.push_str(&format!("{case}\n"));
        for (cat, msg) in fails {
            *by_cat.entry(cat).or_default() += 1;
            s.push_str(&format!("    [{cat}] {msg}\n"));
        }
    }
    let red = cases.iter().filter(|(_, f)| !f.is_empty()).count();
    format!(
        "{red} of {} cases red; failures by check: {by_cat:?}\n{s}",
        cases.len()
    )
}

fn results() -> &'static [Outcome] {
    static R: OnceLock<Vec<Outcome>> = OnceLock::new();
    R.get_or_init(|| {
        let r = all_cases();
        if std::env::var_os("NET_SWEEP_REPORT").is_some() {
            println!("{}", report(&r));
            for (c, f) in &r {
                println!("CASE {} {c}", if f.is_empty() { "green" } else { "red" });
            }
        }
        r
    })
}

/// Every case not in [`KNOWN_RED`] builds what was aimed at and leaves a clean graph.
#[test]
fn the_green_cases_build_what_was_aimed_at() {
    let red: Vec<_> = results()
        .iter()
        .filter(|(c, f)| !f.is_empty() && !known_red(c))
        .cloned()
        .collect();
    assert!(red.is_empty(), "{}", report(&red));
}

/// The cases under [`KNOWN_RED`] still fail; one that passes is fixed: narrow its prefix.
#[test]
fn known_red_cases_still_fail() {
    let fixed: Vec<&str> = results()
        .iter()
        .filter(|(c, f)| f.is_empty() && known_red(c))
        .map(|(c, _)| c.as_str())
        .collect();
    assert!(
        fixed.is_empty(),
        "{} cases pass now, take them out of KNOWN_RED: {fixed:?}",
        fixed.len()
    );
}
