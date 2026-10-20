//! T-261: the junction mesh sweep (`architecture/system-design.md` §6.8.5): every node of the
//! junction gallery ([`cp_sim::road_cases::join_cases`]: bends of every turn and road, curved
//! arms, T, X and Y junctions, width changes, street to highway, automatic cuts) is drawn with
//! the near meshes of its pieces and held against the ideal road outline: the band of half the
//! road's width on either side of each arm's curve, from the node out.
//!
//! Checks (categories of the report), per node of degree 2 or more:
//! * `gap`: a point of the ideal outline that no upward face covers (cracks between pieces, the
//!   chord notch of a shallow bend, BUG-080; a corner cut short); where two widths meet at a
//!   node of degree 2 the wider one is checked from [`TAPER`] on;
//! * `sidewalk`: a sidewalk face over a point of an arm's carriageway band, ahead of the node
//!   (the sidewalk in the mouth of a highway, BUG-023/065; the taper is free, as for `gap`);
//! * `spill`: a junction face farther from every arm than its half width and a kerb radius
//!   (spikes, BUG-054);
//! * `marking`: at a node of degree 2 between two pieces of one road with a centre line, the
//!   line does not run through the node (the patch has no markings, §6.8.1 R5).
//!
//! Each case is built square to the map and turned by 30 degrees. Run:
//! `cargo nextest run -p cp_render junction_sweep`; `JUNCTION_SWEEP_REPORT=1` prints every
//! failing case (`--no-capture`), `JUNCTION_SWEEP_ONLY=<prefix>` runs only those. Cases that fail
//! today are in [`KNOWN_RED`]; `known_red_cases_still_fail` says when one passes.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use bevy_math::{Vec2, Vec3};
use cp_content::{ContentDb, LaneKind};
use cp_core::curve::to_xz;
use cp_net::RoadPath;
use cp_sim::SimRunner;
use cp_sim::road_cases::{JoinCase, build_roads, join_cases};
use cp_terrain::Terrain;

use super::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const TURNS: [f32; 2] = [0.0, 30.0];
/// Sample spacing over the ideal outline, metres.
const STEP_S: f32 = 0.25;
/// Points this close to the edge of a band are not sampled (f32 and kerb lines), metres.
const MARGIN: f32 = 0.15;
/// How far out from the node the arms are checked, beyond three half widths, metres.
const REACH: f32 = 12.0;
/// At a node of degree 2 between roads of different widths, the wider road may narrow to the
/// other over this length from the node (a transition, §6.8.3 item 6): its outline is checked
/// from there on, metres.
const TAPER: f32 = 8.0;

/// Cases that fail today, as prefixes of `case/turn` (a prefix ending in `/` covers every case
/// under it). T-261 recorded 95 of 152 (chord notches on the outside of shallow bends, centre
/// lines broken at every node of degree 2, sidewalks in the mouths of highways, spikes on wide
/// bends); node render v2 (T-263) leaves 2: a 12 m street at 45 degrees into a 7.5 m highway,
/// whose band at the node reaches past the highway's far edge, where the highway's flat side
/// runs straight (`gap`).
const KNOWN_RED: &[&str] = &["highway_t/45/"];

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

type Tri2 = [Vec2; 3];

fn in_tri(p: Vec2, t: &Tri2) -> bool {
    let (p, t) = (p - t[0], t.map(|v| v - t[0]));
    let s = |a: Vec2, b: Vec2| (b - a).perp_dot(p - a);
    let (d0, d1, d2) = (s(t[0], t[1]), s(t[1], t[2]), s(t[2], t[0]));
    let eps = -0.02;
    (d0 >= eps && d1 >= eps && d2 >= eps) || (d0 <= -eps && d1 <= -eps && d2 <= -eps)
}

/// Upward faces in plan, bucketed by 2 m cells for the point tests.
#[derive(Default)]
struct Faces {
    tris: Vec<(Tri2, [f32; 4])>,
    cells: BTreeMap<(i32, i32), Vec<usize>>,
}

const CELL: f32 = 2.0;

impl Faces {
    fn add(&mut self, m: &MeshData) {
        for t in 0..m.triangle_count() {
            let p = |k: usize| Vec3::from_array(m.positions[m.indices[t * 3 + k] as usize]);
            let (a, b, c) = (p(0), p(1), p(2));
            if (b - a).cross(c - a).normalize_or_zero().y.abs() < 0.3 {
                continue;
            }
            let tri = [a, b, c].map(|v| Vec2::new(v.x, v.z));
            let colour = m.colors[m.indices[t * 3] as usize];
            let i = self.tris.len();
            self.tris.push((tri, colour));
            let lo = tri[0].min(tri[1]).min(tri[2]);
            let hi = tri[0].max(tri[1]).max(tri[2]);
            for x in (lo.x / CELL).floor() as i32..=(hi.x / CELL).floor() as i32 {
                for z in (lo.y / CELL).floor() as i32..=(hi.y / CELL).floor() as i32 {
                    self.cells.entry((x, z)).or_default().push(i);
                }
            }
        }
    }

    fn at(&self, p: Vec2) -> impl Iterator<Item = &(Tri2, [f32; 4])> {
        let k = ((p.x / CELL).floor() as i32, (p.y / CELL).floor() as i32);
        self.cells
            .get(&k)
            .into_iter()
            .flatten()
            .map(|&i| &self.tris[i])
            .filter(move |(t, _)| in_tri(p, t))
    }

    fn covered(&self, p: Vec2) -> bool {
        self.at(p).next().is_some()
    }
}

/// One arm of a node: the curve walked from the node out, its road's half width and the half
/// width of its carriageway.
struct ArmLine {
    pts: Vec<Vec2>,
    hw: f32,
    cw: f32,
    road: cp_content::RoadDefId,
}

fn carriageway_half(db: &ContentDb, road: cp_content::RoadDefId) -> f32 {
    db.road(road)
        .lanes
        .iter()
        .filter(|l| l.kind != LaneKind::Pedestrian)
        .map(|l| l.offset.abs() + l.width / 2.0)
        .fold(0.0, f32::max)
}

fn dist_to(p: Vec2, pts: &[Vec2]) -> f32 {
    pts.windows(2)
        .map(|w| {
            let ab = w[1] - w[0];
            let t = ((p - w[0]).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
            p.distance(w[0] + ab * t)
        })
        .fold(f32::INFINITY, f32::min)
}

/// [`dist_to`], but only for points ahead of the arm's start (not behind the node): the
/// carriageway of an arm does not reach back over the node.
fn dist_ahead(p: Vec2, pts: &[Vec2]) -> f32 {
    let d = pts[1] - pts[0];
    if (p - pts[0]).dot(d) < 0.0 {
        return f32::INFINITY;
    }
    dist_to(p, pts)
}

/// The sample points of a band: along `pts` every [`STEP_S`], across within `half - MARGIN`.
fn band_points(pts: &[Vec2], half: f32, from: f32) -> Vec<(Vec2, f32)> {
    let mut out = Vec::new();
    let mut s = 0.0;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = a.distance(b);
        if len < 1e-6 {
            continue;
        }
        let d = (b - a) / len;
        let n = Vec2::new(-d.y, d.x);
        let mut u = (s / STEP_S).ceil() * STEP_S - s;
        while u <= len {
            let c = a + d * u;
            if s + u < from {
                u += STEP_S;
                continue;
            }
            let k = ((half - MARGIN) / STEP_S).floor() as i32;
            for j in -k..=k {
                out.push((c + n * (j as f32 * STEP_S), s + u));
            }
            u += STEP_S;
        }
        s += len;
    }
    out
}

struct Run {
    name: String,
    fails: Vec<(String, String)>,
}

fn run_case(case: &JoinCase, turn: f32) -> Run {
    let name = format!("{}/{turn}", case.name);
    let mut fails = Vec::new();
    let (db, terrain) = assets();
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::clone(&terrain));
    sim.set_content(Arc::clone(&db));
    sim.tick_once();
    let rot = Vec2::from_angle(turn.to_radians());
    let roads: Vec<_> = case
        .roads
        .iter()
        .map(|(r, p)| {
            let p = match p {
                RoadPath::Straight { from, to } => RoadPath::Straight {
                    from: rot.rotate(*from),
                    to: rot.rotate(*to),
                },
                RoadPath::Curve { from, control, to } => RoadPath::Curve {
                    from: rot.rotate(*from),
                    control: rot.rotate(*control),
                    to: rot.rotate(*to),
                },
                RoadPath::Free(v) => RoadPath::Free(v.iter().map(|q| rot.rotate(*q)).collect()),
            };
            (*r, p)
        })
        .collect();
    let origin = Vec2::new(4600.0, 4600.0);
    for (i, e) in build_roads(&mut sim, origin, &roads) {
        fails.push(("refused".into(), format!("road {i}: {e}")));
    }
    let view = sim.view();
    let mut set = RoadSet::default();
    set.update(&view.net, &view.junctions, &terrain, &db);
    let mut faces = Faces::default();
    let mut junction_faces: BTreeMap<NodeId, Faces> = BTreeMap::new();
    for (id, _) in view.net.segments() {
        if let Some(p) = set.piece(PieceKey::Segment(id)) {
            faces.add(&p.near);
        }
    }
    for (node, _) in view.junctions.iter() {
        if let Some(p) = set.piece(PieceKey::Junction(*node)) {
            faces.add(&p.near);
            junction_faces.entry(*node).or_default().add(&p.near);
        }
    }
    let local = |p: Vec2| Vec2::new(rot.x, -rot.y).rotate(p - origin);
    for (nid, n) in view.net.nodes() {
        if n.segments.len() < 2 {
            continue;
        }
        let c = to_xz(n.pos);
        let at = local(c);
        let what = format!("node at ({:.1}, {:.1})", at.x, at.y);
        let arms: Vec<ArmLine> = n
            .segments
            .iter()
            .filter_map(|&sid| {
                let s = view.net.segment(sid)?;
                let hw = db.road(s.road).width / 2.0;
                let k = (s.length / 0.5).ceil().max(2.0) as usize;
                let mut pts: Vec<Vec2> = (0..=k)
                    .map(|i| to_xz(s.curve.point(i as f32 / k as f32)))
                    .collect();
                if s.start != nid {
                    pts.reverse();
                }
                Some(ArmLine {
                    pts,
                    hw,
                    cw: carriageway_half(&db, s.road),
                    road: s.road,
                })
            })
            .collect();
        let reach = 3.0 * arms.iter().map(|a| a.hw).fold(0.0, f32::max) + REACH;
        // Each arm up to `reach` from the node (or its half length when it is short).
        let near: Vec<Vec<Vec2>> = arms
            .iter()
            .map(|a| {
                let total: f32 = a.pts.windows(2).map(|w| w[0].distance(w[1])).sum();
                let lim = reach.min(total / 2.0);
                let mut s = 0.0;
                let mut out = vec![a.pts[0]];
                for w in a.pts.windows(2) {
                    if s >= lim {
                        break;
                    }
                    s += w[0].distance(w[1]);
                    out.push(w[1]);
                }
                out
            })
            .collect();
        // gap
        let mut gaps = Vec::new();
        let narrow = arms.iter().map(|a| a.hw).fold(f32::INFINITY, f32::min);
        for (a, pts) in arms.iter().zip(&near) {
            let from = if arms.len() == 2 && a.hw > narrow + 0.01 {
                TAPER
            } else {
                0.0
            };
            for (p, s) in band_points(pts, a.hw, from) {
                if !faces.covered(p) {
                    gaps.push((p, s));
                }
            }
        }
        if let Some((p, s)) = gaps.first() {
            let q = local(*p);
            fails.push((
                "gap".into(),
                format!(
                    "{what}: {} points of the outline uncovered, e.g. ({:.2}, {:.2}), {s:.1} m along an arm",
                    gaps.len(),
                    q.x,
                    q.y
                ),
            ));
        }
        // sidewalk
        let mut over = Vec::new();
        for (tri, colour) in &faces.tris {
            if *colour != SIDEWALK {
                continue;
            }
            let cen = (tri[0] + tri[1] + tri[2]) / 3.0;
            if cen.distance(c) > reach {
                continue;
            }
            let probes = [
                cen,
                (cen + tri[0]) / 2.0,
                (cen + tri[1]) / 2.0,
                (cen + tri[2]) / 2.0,
            ];
            for p in probes {
                if let Some(a) = arms.iter().zip(&near).find(|(a, pts)| {
                    let tapering = arms.len() == 2 && a.hw > narrow + 0.01;
                    let along = (p - pts[0]).dot((pts[1] - pts[0]).normalize_or_zero());
                    !(tapering && along < TAPER) && dist_ahead(p, pts) < a.cw - MARGIN
                }) {
                    over.push((p, a.0.cw));
                    break;
                }
            }
        }
        if let Some((p, cw)) = over.first() {
            let q = local(*p);
            fails.push((
                "sidewalk".into(),
                format!(
                    "{what}: {} sidewalk probes on a carriageway, e.g. ({:.2}, {:.2}) (carriageway half {cw})",
                    over.len(),
                    q.x,
                    q.y
                ),
            ));
        }
        // spill
        if let Some(jf) = junction_faces.get(&nid) {
            // The kerb curve of a wide junction's obtuse corner bows out in proportion to the
            // width (T-275: 4-cell roads are 32 m): a kerb radius, or a quarter of the half width.
            let hw = arms.iter().map(|a| a.hw).fold(0.0, f32::max);
            let allow = hw + node::CURB_RADIUS.max(hw * 0.25) + 0.5;
            let worst = jf
                .tris
                .iter()
                .flat_map(|(t, _)| t.iter())
                .map(|p| {
                    (
                        arms.iter()
                            .map(|a| dist_to(*p, &a.pts))
                            .fold(f32::INFINITY, f32::min),
                        *p,
                    )
                })
                .filter(|(d, _)| *d > allow)
                .max_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((d, p)) = worst {
                let q = local(p);
                fails.push((
                    "spill".into(),
                    format!("{what}: a junction vertex {d:.2} m from every arm at ({:.2}, {:.2}), allowed {allow:.2}", q.x, q.y),
                ));
            }
        }
        // marking
        if arms.len() == 2 && arms[0].road == arms[1].road {
            let line_at = |p: Vec2| faces.at(p).any(|(_, col)| *col == YELLOW);
            let point_at = |pts: &[Vec2], s: f32| {
                let mut acc = 0.0;
                for w in pts.windows(2) {
                    let l = w[0].distance(w[1]);
                    if acc + l >= s {
                        return w[0] + (w[1] - w[0]) * ((s - acc) / l.max(1e-6));
                    }
                    acc += l;
                }
                *pts.last().unwrap()
            };
            let has_line = near.iter().all(|pts| line_at(point_at(pts, 12.0)));
            if has_line {
                let missing: Vec<f32> = near
                    .iter()
                    .flat_map(|pts| (0..=16).map(move |i| (pts, i as f32 * 0.25)))
                    .filter(|(pts, s)| !line_at(point_at(pts, *s)))
                    .map(|(_, s)| s)
                    .collect();
                if !missing.is_empty() {
                    fails.push((
                        "marking".into(),
                        format!(
                            "{what}: the centre line misses {} of 34 points within 4 m of the node",
                            missing.len()
                        ),
                    ));
                }
            }
        }
    }
    Run { name, fails }
}

fn results() -> &'static [Run] {
    static R: OnceLock<Vec<Run>> = OnceLock::new();
    R.get_or_init(|| {
        let mut out = Vec::new();
        for case in join_cases() {
            for turn in TURNS {
                let name = format!("{}/{turn}", case.name);
                if let Ok(only) = std::env::var("JUNCTION_SWEEP_ONLY")
                    && !name.starts_with(&only)
                {
                    continue;
                }
                out.push(run_case(&case, turn));
            }
        }
        if std::env::var_os("JUNCTION_SWEEP_REPORT").is_some() {
            println!("{}", report(&out));
            for r in &out {
                println!(
                    "CASE {} {}",
                    if r.fails.is_empty() { "green" } else { "red" },
                    r.name
                );
            }
        }
        out
    })
}

fn report(runs: &[Run]) -> String {
    let mut by_cat: BTreeMap<&str, usize> = BTreeMap::new();
    let mut s = String::new();
    for r in runs.iter().filter(|r| !r.fails.is_empty()) {
        s.push_str(&format!("{}\n", r.name));
        for (cat, msg) in &r.fails {
            *by_cat.entry(cat).or_default() += 1;
            s.push_str(&format!("    [{cat}] {msg}\n"));
        }
    }
    let red = runs.iter().filter(|r| !r.fails.is_empty()).count();
    format!(
        "{red} of {} cases red; failures by check: {by_cat:?}\n{s}",
        runs.len()
    )
}

/// Every case not in [`KNOWN_RED`] draws clean joins.
#[test]
fn junction_sweep_green_cases_draw_clean_joins() {
    let red: Vec<&Run> = results()
        .iter()
        .filter(|r| !r.fails.is_empty() && !known_red(&r.name))
        .collect();
    let owned: Vec<Run> = red
        .iter()
        .map(|r| Run {
            name: r.name.clone(),
            fails: r.fails.clone(),
        })
        .collect();
    assert!(owned.is_empty(), "{}", report(&owned));
}

/// The cases under [`KNOWN_RED`] still fail; one that passes is fixed: narrow its prefix.
#[test]
fn junction_sweep_known_red_cases_still_fail() {
    let fixed: Vec<&str> = results()
        .iter()
        .filter(|r| r.fails.is_empty() && known_red(&r.name))
        .map(|r| r.name.as_str())
        .collect();
    assert!(
        fixed.is_empty(),
        "{} cases pass now, take them out of KNOWN_RED: {fixed:?}",
        fixed.len()
    );
}
