//! T-250: a grid sweep that builds like a player, along the ticks and guides the road tool
//! offers (BUG-047, -056, -067, -073).
//!
//! Every step goes through the tool's own code: the cursor goes through
//! [`block_hover`](cp_tools::zone_preview::block_hover), [`RoadTool::click`] and
//! [`chain_commit`](cp_tools::road::chain_commit) (the quantizing, the joins on the 8-cell
//! ticks, the control lines), the ghost is [`compute_ghost_alt`] and the result is sent as
//! `BuildRoad`. A tick is picked where the player sees one: an 8th column boundary of the cells
//! of an existing street ([`cp_zone::ZoneGrid::frontage_marks`], what the joins snap to) or of
//! the ghost's own strip; the cursor is put near it (seeded jitter, a cell's width from the
//! edge) and the tool snaps.
//!
//! Scenarios, each at 0, 17, 45 and 90 degrees, 2 and 4 lanes, seeds 0 (no jitter), 7 and 42,
//! bends sharp (two straights) and rounded (a curve between them):
//!
//! * `l_close` (BUG-073 shot 1): an L, a street up from the bottom street's 16-cell tick to its
//!   own 8-cell tick, then a cross street from the left street's 8-cell tick to that end;
//! * `parallel_cross` (shot 3): a street, two parallels 8 cells apart off it, the block closed
//!   from the upper street's end to the lower one's;
//! * `extend_bend` (shot 2): an L, then the bottom street extended straight on;
//! * `t_mid_block`: a 16 x 8 block closed from ticks, then a T from the middle tick across it;
//! * `lattice_2x2`: four 8 x 8 blocks, rows first, columns first, ring then the inner cross.
//!
//! Checks (each a category in the report): `lattice` every closed block is one rectangular
//! lattice of 8 m cells (no gap, no overlap, the cell count the ticks promised), `size` its depth
//! and length are the promised multiples of 8 m, `opposite` the ticks on the two sides of a
//! street stand opposite each other, `perp` a cross street meets the street at a right angle
//! (< 0.5 deg), `lands` it lands on the opposite street's tick or end (< 0.5 m), `tick` the tool
//! offers a tick where 8 cells were promised, `preview` the ghost's cells are the built cells,
//! `moved` the road is built where the tool sent it (the validator snapped no end elsewhere),
//! `refused` the tool built what was clicked.
//!
//! T-261 (§6.8.5) adds the U of BUG-081 (`u_shape`, 1-3 sections, sharp or rounded corners,
//! through the tool) and blocks the player draws off the guides, built by command (seed 0 only):
//! `on_tick` (8 x 8, the control), `off_tick` (70 x 86 m), `skewed` (10 degrees), `trapezoid`,
//! `long` (240 m), `mixed` (`medium_4l` top and bottom), `one_way`, `upgrade` (the bottom street
//! upgraded to `large_6l`), `rebuild` (a side demolished and drawn again). Their checks:
//! `stretch` a cell in the block is not an 8 m square, `overlap` two cells overlap, `fill` less
//! than 75 % of the block within 4 rows of a street is cells, `history` the rebuilt block's cells
//! differ from the first build's.
//!
//! Run: `cargo nextest run -p cp_tools --test grid_sweep`. `GRID_SWEEP_REPORT=1` prints every
//! failing case with its numbers (`--no-capture`), `GRID_SWEEP_TRACE=1` every click and what it
//! built, `GRID_SWEEP_ONLY=<case prefix>` runs only those cases (debugging; the tests then fail). Cases that fail today are listed in
//! [`KNOWN_RED`] and checked by `known_red_cases_still_fail`: when a fix makes one pass, that test
//! says so and the case moves to the green set.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::{ContentDb, RoadDefId};
use cp_core::CommandOutcome;
use cp_core::curve::to_xz;
use cp_net::{NetCommand, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::road::chain_commit;
use cp_tools::{CommandOutbox, Ghost, RoadMode, RoadTool, compute_ghost_alt, resolve_cursor};
use cp_zone::Mark;
use cp_zone::geom::Obb;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const CELL: f32 = 8.0;
const BLOCK: f32 = 8.0 * CELL;
const ANGLES: [f32; 4] = [0.0, 15.0, 45.0, 90.0];
const ROADS: [&str; 2] = ["basic_2l", "medium_4l"];
const SEEDS: [u64; 3] = [0, 7, 42];
/// Radius of the rounded bends (the curve's corner control point is this far from its ends): the
/// quadratic is exactly 40 m long, so the length step leaves it whole and its end tangent exact.
const BEND_R: f32 = 24.642_298;

/// Cases that fail today (T-250 report, BUG-073), as prefixes of `scenario/road/variant/angle/seed`:
/// every case under one of them is expected red. Narrow a prefix (or list single cases) as fixes
/// land; `known_red_cases_still_fail` names the cases that pass.
///
/// T-261 (2026-10-19, 100 of 432 red, §6.8.1 R4): the U of BUG-081, the blocks off the ticks,
/// skewed, trapezoid and upgraded; layout v3 (T-265, 8 m cells never stretched) made them green,
/// and the legs of a rounded corner counting from its virtual corner (§6.8.3 item 4, ruling on
/// T-266) the `l_close` with a rounded corner. Empty.
const KNOWN_RED: &[&str] = &[];

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bend {
    Sharp,
    Rounded,
}

/// One built piece: its end points in the local frame, as the tool sent them.
#[derive(Clone, Copy, Debug)]
struct Piece {
    a: Vec2,
    b: Vec2,
}

impl Piece {
    fn x(&self) -> f32 {
        (self.a.x + self.b.x) / 2.0
    }
    fn y(&self) -> f32 {
        (self.a.y + self.b.y) / 2.0
    }
    /// Unit direction a -> b.
    fn dir(&self) -> Vec2 {
        (self.b - self.a).normalize_or_zero()
    }
    /// Unit direction across the piece.
    fn across(&self) -> Vec2 {
        let d = self.dir();
        Vec2::new(-d.y, d.x)
    }
}

/// What a click that built something left behind.
struct Built {
    piece: Piece,
    /// World points a cross street may land on, taken before it was built: the opposite
    /// street's 8-cell ticks moved by the half width either way (the near edge stands on the
    /// tick, BUG-067) and dead ends.
    targets: Vec<Vec2>,
    ghost: Option<Ghost>,
}

/// A cell as `GRID_SWEEP_TRACE` remembers it: corner key, valid, segment, row.
type TracedCell = ([(i32, i32); 4], bool, cp_net::SegmentId, usize);

/// A player in a fresh city: builds with the road tool in a frame turned by `angle` about `o`.
struct Player {
    sim: SimRunner,
    db: Arc<ContentDb>,
    road: RoadDefId,
    hw: f32,
    o: Vec2,
    rot: Vec2,
    rng: u64,
    jitter: bool,
    /// Radius round the corner of the axes within which a block may lack cells: the bite the
    /// arc of a rounded corner takes out of the lattice (BUG-073b).
    bite: Option<f32>,
    tool: RoadTool,
    fails: Vec<(String, String)>,
    /// `GRID_SWEEP_TRACE`: every cell before the last click (key, valid, segment, row).
    before_all: Vec<TracedCell>,
}

impl Player {
    fn new(road: &str, angle: f32, seed: u64) -> Self {
        let (db, t) = assets();
        let mut sim = SimRunner::new(1);
        sim.set_terrain(t);
        sim.set_content(Arc::clone(&db));
        sim.tick_once();
        let road = db.road_id(road).unwrap();
        let hw = db.road(road).width / 2.0;
        let a = angle.to_radians();
        Self {
            sim,
            hw,
            road,
            db,
            o: Vec2::new(4600.0, 4600.0),
            rot: Vec2::new(a.cos(), a.sin()),
            rng: seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407),
            jitter: seed != 0,
            bite: None,
            tool: RoadTool::default(),
            fails: Vec::new(),
            before_all: Vec::new(),
        }
    }

    fn fail(&mut self, cat: &str, msg: String) {
        self.fails.push((cat.to_string(), msg));
    }

    /// Local frame to world.
    fn w(&self, l: Vec2) -> Vec2 {
        self.o + self.wv(l)
    }
    fn wv(&self, l: Vec2) -> Vec2 {
        Vec2::new(
            l.x * self.rot.x - l.y * self.rot.y,
            l.x * self.rot.y + l.y * self.rot.x,
        )
    }
    /// World to local frame.
    fn l(&self, w: Vec2) -> Vec2 {
        self.lv(w - self.o)
    }
    fn lv(&self, d: Vec2) -> Vec2 {
        Vec2::new(
            d.x * self.rot.x + d.y * self.rot.y,
            -d.x * self.rot.y + d.y * self.rot.x,
        )
    }

    /// Uniform in `[-m, m]`, or 0 without jitter.
    fn jit(&mut self, m: f32) -> f32 {
        if !self.jitter {
            return 0.0;
        }
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let u = ((self.rng >> 40) as f32) / ((1u64 << 24) as f32);
        (u * 2.0 - 1.0) * m
    }

    fn jv(&mut self, m: f32) -> Vec2 {
        let (x, y) = (self.jit(m), self.jit(m));
        Vec2::new(x, y)
    }

    /// Takes the tool up fresh in `mode` (Esc, then the mode).
    fn start(&mut self, mode: RoadMode) {
        self.tool = RoadTool {
            mode,
            road: Some(self.road),
            ..Default::default()
        };
    }

    /// Switches the mode in the middle of a chain (the player clicks the chain's end again).
    fn mode(&mut self, mode: RoadMode) {
        self.tool.mode = mode;
    }

    /// Valid cells, as corner lists rounded to 2 cm.
    fn cell_keys(&self) -> BTreeSet<[(i32, i32); 4]> {
        self.cells().iter().map(|o| key(o.corners())).collect()
    }

    fn cells(&self) -> Vec<Obb> {
        let view = self.sim.view();
        let mut out = Vec::new();
        for (id, b) in view.zones.blocks() {
            for (i, ok) in b.valid.iter().enumerate() {
                if *ok {
                    out.extend(view.zones.cell_footprint(cp_zone::ZoneCellRef {
                        block: id,
                        cell: i as u16,
                    }));
                }
            }
        }
        out
    }

    /// Every column boundary of every street, with the half width of its road.
    fn marks(&self) -> Vec<(Mark, f32)> {
        let view = self.sim.view();
        let roads = cp_zone::road_zoning(&self.db);
        let mut out = Vec::new();
        for (id, s) in view.net.segments() {
            let hw = self.db.road(s.road).width / 2.0;
            for side in view.zones.frontage_marks(&view.net, &roads, id) {
                for m in side {
                    // Every boundary plainly, and the ticks: the centreline station of a cross
                    // street of this road whose near edge stands on an 8-cell line (T-265).
                    out.push((Mark { major: false, ..m }, hw));
                    if m.major && !m.end {
                        out.push((
                            Mark {
                                axis: m.axis + m.onward * self.hw,
                                ..m
                            },
                            hw,
                        ));
                    }
                }
            }
        }
        out
    }

    /// The 8-cell tick nearest the local point `nominal` (on a street axis) whose cells lie
    /// towards `side` (local): where the player clicks to follow the ticks. A missing tick is a
    /// failure; the nominal point stands in.
    fn tick(&mut self, what: &str, nominal: Vec2, side: Vec2) -> Mark {
        let (p, sw) = (self.w(nominal), self.wv(side));
        let mut best: Option<(f32, Mark)> = None;
        let mut nearest_any: Option<(f32, Mark)> = None;
        for (m, _) in self.marks() {
            if m.v.dot(sw) < 0.9 {
                continue;
            }
            // Along the street; the nominal point is only roughly on the axis.
            let r = m.axis - p;
            if r.dot(m.v).abs() > 3.0 {
                continue;
            }
            let d = r.perp_dot(m.v).abs();
            if nearest_any.is_none_or(|(bd, _)| d < bd) {
                nearest_any = Some((d, m));
            }
            // Since T-275 a dead end keeps a half width for its cross street (§6.8.3 item 4 (д)),
            // so a tick counted from one stands a half width further on than the nominal point.
            if m.major && !m.end && d <= 2.0 * self.hw + 3.0 && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, m));
            }
        }
        if std::env::var_os("GRID_SWEEP_TRACE").is_some() {
            let mut all: Vec<String> = self
                .marks()
                .into_iter()
                .filter(|(m, _)| m.v.dot(sw) >= 0.9 && (m.axis - p).dot(m.v).abs() <= 3.0)
                .map(|(m, _)| {
                    let q = self.l(m.axis);
                    let tag = if m.end {
                        "E"
                    } else if m.major {
                        "M"
                    } else {
                        ""
                    };
                    format!("({:.1},{:.1}){tag}", q.x, q.y)
                })
                .collect();
            all.sort();
            println!("  {what}: boundaries there: {}", all.join(" "));
        }
        if let Some((_, m)) = best {
            return m;
        }
        let near = nearest_any.map_or("none".to_string(), |(d, m)| {
            format!(
                "nearest boundary {d:.2} m away at {:?} (major {})",
                self.l(m.axis),
                m.major
            )
        });
        self.fail(
            "tick",
            format!("{what}: no 8-cell tick within 6 m of {nominal:?}; {near}"),
        );
        Mark {
            axis: p,
            v: sw,
            onward: Vec2::new(sw.y, -sw.x),
            major: true,
            end: false,
            arc: 0.0,
        }
    }

    /// Where the player puts the cursor to click a tick: inside the band of cells, 2.5 m off the
    /// road edge, a little along the street.
    fn at_tick(&mut self, m: Mark) -> Vec2 {
        let t = Vec2::new(-m.v.y, m.v.x);
        let j = self.jit(1.5);
        m.axis + m.v * (self.hw + 2.5) + t * j
    }

    /// The points a cross street may land on now (see [`Built::targets`]).
    fn targets(&self) -> Vec<Vec2> {
        let mut out: Vec<Vec2> = Vec::new();
        for (m, _) in self.marks() {
            if m.major && !m.end {
                let t = Vec2::new(-m.v.y, m.v.x);
                let _ = t;
                out.push(m.axis);
            }
        }
        let view = self.sim.view();
        out.extend(
            view.net
                .nodes()
                .filter(|(_, n)| n.segments.len() == 1)
                .map(|(_, n)| to_xz(n.pos)),
        );
        out
    }

    /// A click at the world point `p`, as the tool takes it; when it completes a piece, the
    /// piece is built and checked against its ghost.
    fn click(&mut self, what: &str, p: Vec2) -> Option<Built> {
        let before = self.tool.clone();
        let view = self.sim.view();
        let ghost = compute_ghost_alt(&view, &self.tool, p, false);
        // The click takes the point the ghost put the cursor on (T-264), as the plugin does.
        let cur = resolve_cursor(&view, &self.tool, p, false);
        let q = cur.as_ref().map_or(p, |c| c.cursor);
        drop(view);
        let req = self.tool.click(q)?;
        let view = self.sim.view();
        let Some(plan) = chain_commit(Some(&view), &mut self.tool, before, req, false) else {
            drop(view);
            self.fail(
                "refused",
                format!("{what}: the tool refused the piece at {:?}", self.l(p)),
            );
            return None;
        };
        // The ends as sent: where the anchors of the plan stand.
        let (a, b) = (
            cp_tools::anchor_pos(&view, &plan.pieces[0].from).unwrap(),
            cp_tools::anchor_pos(&view, &plan.pieces.last().unwrap().to).unwrap(),
        );
        drop(view);
        let targets = self.targets();
        let old = self.cell_keys();
        if std::env::var_os("GRID_SWEEP_TRACE").is_some() {
            let view = self.sim.view();
            let mut all = Vec::new();
            for (id, b) in view.zones.blocks() {
                for i in 0..b.cell_count() {
                    if let Some(o) = view.zones.cell_footprint(cp_zone::ZoneCellRef {
                        block: id,
                        cell: i as u16,
                    }) {
                        all.push((key(o.corners()), b.valid[i], b.segment, i % 4));
                    }
                }
            }
            self.before_all = all;
            for (_, b) in view.zones.blocks() {
                let p = self.l(to_xz(b.curve.p0));
                let q = self.l(to_xz(b.curve.p3));
                if p.y.abs() < 1.0 && q.y.abs() < 1.0 {
                    println!(
                        "    before block seg {:?} {:?} s0 {} cols {} {p:?}->{q:?}",
                        b.segment, b.side, b.s0, b.cols
                    );
                }
            }
        }
        let mut outbox = CommandOutbox::default();
        outbox.push(SimCommand::Net(NetCommand::BuildPlan { plan }));
        outbox.flush_into(&mut self.sim);
        self.sim.tick_once();
        let out: Vec<CommandOutcome> = self
            .sim
            .world_mut()
            .resource_mut::<Messages<CommandOutcome>>()
            .drain()
            .collect();
        if let Some(Err(e)) = out.last().map(|o| &o.result) {
            self.fail("refused", format!("{what}: BuildPlan failed: {e:?}"));
            return None;
        }
        self.sim.tick_once();
        // Where the ends were built: the nodes there (the validator may snap a sent end to a
        // node or a segment nearby, BUG-073).
        let node_at = |q: Vec2| {
            let view = self.sim.view();
            view.net
                .nodes()
                .map(|(_, n)| to_xz(n.pos))
                .min_by(|x, y| x.distance(q).total_cmp(&y.distance(q)))
                .filter(|n| n.distance(q) < 16.0)
                .unwrap_or(q)
        };
        let (ba, bb) = (node_at(a), node_at(b));
        for (end, sent, built) in [("start", a, ba), ("end", b, bb)] {
            if sent.distance(built) > 0.05 {
                self.fail(
                    "moved",
                    format!(
                        "{what}: the {end} was sent at {:?} but built at {:?} ({:.2} m)",
                        self.l(sent),
                        self.l(built),
                        sent.distance(built)
                    ),
                );
            }
        }
        let piece = Piece {
            a: self.l(ba),
            b: self.l(bb),
        };
        if std::env::var_os("GRID_SWEEP_TRACE").is_some() {
            println!(
                "  {what}: cursor {:?} -> built {:?} -> {:?}",
                self.l(p),
                piece.a,
                piece.b
            );
        }
        if let Some(g) = &ghost {
            self.check_preview(what, g, &old);
        }
        Some(Built {
            piece,
            targets,
            ghost,
        })
    }

    /// Clicks that must complete a piece.
    fn piece(&mut self, what: &str, p: Vec2) -> Option<Piece> {
        let b = self.click(what, p);
        if b.is_none() && !self.fails.iter().any(|(c, _)| c == "refused") {
            self.fail("refused", format!("{what}: no piece from the click"));
        }
        b.map(|b| b.piece)
    }

    /// `preview`: the cells the ghost showed are the cells the build added.
    fn check_preview(&mut self, what: &str, g: &Ghost, old: &BTreeSet<[(i32, i32); 4]>) {
        let now = self.cell_keys();
        let shown: BTreeSet<_> = g
            .zone
            .cells
            .iter()
            .filter(|c| c.valid)
            .map(|c| key(c.corners))
            .collect();
        let added: Vec<_> = now.difference(old).collect();
        let unshown = added.iter().filter(|k| !shown.contains(**k)).count();
        let unbuilt = shown.iter().filter(|k| !now.contains(*k)).count();
        if unshown + unbuilt > 0 {
            if std::env::var_os("GRID_SWEEP_TRACE").is_some() {
                let c = |k: &[(i32, i32); 4]| {
                    let q = Vec2::new(
                        k.iter().map(|p| p.0 as f32).sum::<f32>() / 200.0,
                        k.iter().map(|p| p.1 as f32).sum::<f32>() / 200.0,
                    );
                    let l = self.l(q);
                    format!("({:.1},{:.1})", l.x, l.y)
                };
                let un: Vec<String> = shown.iter().filter(|k| !now.contains(*k)).map(c).collect();
                println!("  ghost cells not built: {}", un.join(" "));
                let hidden: Vec<String> = added
                    .iter()
                    .filter(|k| !shown.contains(**k))
                    .map(|k| c(k))
                    .collect();
                println!("  built cells not in the ghost: {}", hidden.join(" "));
                let view = self.sim.view();
                for (_, b) in view.zones.blocks() {
                    let p = self.l(to_xz(b.curve.p0));
                    let q = self.l(to_xz(b.curve.p3));
                    if p.y.abs() < 1.0 && q.y.abs() < 1.0 {
                        println!(
                            "    after block seg {:?} {:?} s0 {} cols {} {p:?}->{q:?}",
                            b.segment, b.side, b.s0, b.cols
                        );
                    }
                }
                for (id, b) in view.zones.blocks() {
                    for i in 0..b.cell_count() {
                        let Some(o) = view.zones.cell_footprint(cp_zone::ZoneCellRef {
                            block: id,
                            cell: i as u16,
                        }) else {
                            continue;
                        };
                        if b.valid[i]
                            && added
                                .iter()
                                .any(|k| **k == key(o.corners()) && !shown.contains(*k))
                        {
                            let k = key(o.corners());
                            let before: Vec<_> = self
                                .before_all
                                .iter()
                                .filter(|e| e.0 == k)
                                .map(|e| (e.1, e.2, e.3))
                                .collect();
                            println!("    before: {before:?}");
                            println!(
                                "    hidden: block {id:?} segment {:?} order {} side {:?} col {} row {} s0 {:.2} cols {}",
                                b.segment,
                                view.net.segment(b.segment).map_or(0, |s| s.order),
                                b.side,
                                i / 4,
                                i % 4,
                                b.s0,
                                b.cols
                            );
                        }
                    }
                }
                let real: Vec<Obb> = self.cells();
                let near: Vec<String> = shown
                    .iter()
                    .filter(|k| !now.contains(*k))
                    .take(3)
                    .map(|k| {
                        let q = Vec2::new(
                            k.iter().map(|p| p.0 as f32).sum::<f32>() / 200.0,
                            k.iter().map(|p| p.1 as f32).sum::<f32>() / 200.0,
                        );
                        let o: Vec<String> = real
                            .iter()
                            .filter(|r| r.c.distance(q) < 7.0)
                            .map(|r| format!("{:?}@{:.1},{:.1}", r.u, self.l(r.c).x, self.l(r.c).y))
                            .collect();
                        format!("{} overlapped by real [{}]", c(k), o.join(" "))
                    })
                    .collect();
                println!("  {}", near.join("; "));
            }
            self.fail(
                "preview",
                format!(
                    "{what}: {unshown} of {} built cells not in the ghost, {unbuilt} of {} ghost cells not built",
                    added.len(),
                    shown.len()
                ),
            );
        }
    }

    /// `perp`: the piece runs along local `dir` (either way) within 0.5 deg.
    fn check_perp(&mut self, what: &str, p: Piece, dir: Vec2) {
        let d = (p.b - p.a).normalize_or_zero();
        let err = d.dot(dir).abs().clamp(0.0, 1.0).acos().to_degrees();
        if err >= 0.5 {
            self.fail(
                "perp",
                format!(
                    "{what}: {err:.2} deg off the perpendicular ({:?} -> {:?})",
                    p.a, p.b
                ),
            );
        }
    }

    /// `lands`: the piece's end is within 0.5 m of a tick (edge on it) or a dead end of before.
    fn check_lands(&mut self, what: &str, b: &Built) {
        let end = self.w(b.piece.b);
        let d = b
            .targets
            .iter()
            .map(|t| t.distance(end))
            .fold(f32::INFINITY, f32::min);
        if d >= 0.5 {
            let near = b
                .targets
                .iter()
                .min_by(|x, y| x.distance(end).total_cmp(&y.distance(end)))
                .map(|t| self.l(*t));
            self.fail(
                "lands",
                format!(
                    "{what}: ends at {:?}, {d:.2} m from the nearest tick/end {near:?}",
                    b.piece.b
                ),
            );
        }
    }

    /// `size` and `lattice` for the block between the axes `bottom`/`top` (local y) and
    /// `left`/`right` (local x): `nx` x `ny` cells of 8 m from the road edges, one each.
    fn check_block(&mut self, what: &str, axes: [f32; 4], nx: u32, ny: u32) {
        let [bottom, top, left, right] = axes;
        let hw = self.hw;
        let (x0, x1, y0, y1) = (left + hw, right - hw, bottom + hw, top - hw);
        let (w, d) = (x1 - x0, y1 - y0);
        let (ew, ed) = (nx as f32 * CELL, ny as f32 * CELL);
        if (w - ew).abs() > 0.1 || (d - ed).abs() > 0.1 {
            self.fail(
                "size",
                format!("{what}: block {w:.2} x {d:.2} m between the edges, want {ew} x {ed}"),
            );
        }
        let mut slots: BTreeMap<(i32, i32), u32> = BTreeMap::new();
        let (mut off, mut worst, mut odd) = (0, 0.0f32, 0);
        let mut inside = 0;
        for o in self.cells() {
            let c = self.l(o.c);
            if c.x < x0 + 0.5 || c.x > x1 - 0.5 || c.y < y0 + 0.5 || c.y > y1 - 0.5 {
                continue;
            }
            inside += 1;
            let u = self.lv(o.u);
            let square = (o.half - CELL / 2.0).abs() < 0.05 && (o.half_v - CELL / 2.0).abs() < 0.05;
            let aligned = u.x.abs().max(u.y.abs()) > 0.5f32.to_radians().cos();
            if !square || !aligned {
                odd += 1;
            }
            let (i, j) = ((c.x - x0 - 4.0) / CELL, (c.y - y0 - 4.0) / CELL);
            let e = ((i - i.round()).abs().max((j - j.round()).abs())) * CELL;
            if e > 0.1 {
                off += 1;
                worst = worst.max(e);
            }
            *slots
                .entry((i.round() as i32, j.round() as i32))
                .or_default() += 1;
        }
        if std::env::var_os("GRID_SWEEP_TRACE").is_some() {
            let mut rows: BTreeMap<i32, Vec<String>> = BTreeMap::new();
            for o in self.cells() {
                let c = self.l(o.c);
                if c.x > x0 - 8.0 && c.x < x1 + 8.0 && c.y > y0 - 8.0 && c.y < y1 + 8.0 {
                    rows.entry((c.y * 2.0).round() as i32)
                        .or_default()
                        .push(format!("{:.1}", c.x));
                }
            }
            println!("  {what}: x0 {x0:.2} x1 {x1:.2} y0 {y0:.2} y1 {y1:.2}; cell centres by row:");
            for (y, xs) in rows {
                println!("    y {:.1}: {}", y as f32 / 2.0, xs.join(" "));
            }
        }
        let doubled = slots.values().filter(|n| **n > 1).count();
        let gaps: Vec<(i32, i32)> = (0..nx as i32)
            .flat_map(|i| (0..ny as i32).map(move |j| (i, j)))
            .filter(|s| !slots.contains_key(s))
            .collect();
        // Missing cells are only allowed in the bite of a rounded corner (the axes cross at the
        // local origin), never elsewhere.
        let bitten = |&(i, j): &(i32, i32)| {
            self.bite.is_some_and(|r| {
                Vec2::new(x0 + 4.0 + i as f32 * CELL, y0 + 4.0 + j as f32 * CELL).length() <= r
            })
        };
        let allowed = gaps.iter().filter(|s| bitten(s)).count();
        let missing = gaps.len() - allowed;
        let stray = slots
            .keys()
            .filter(|(i, j)| *i < 0 || *j < 0 || *i >= nx as i32 || *j >= ny as i32)
            .count();
        if off + odd + doubled + missing + stray > 0 || inside + allowed != (nx * ny) as usize {
            self.fail(
                "lattice",
                format!(
                    "{what}: {inside} cells, want {}; {off} off the lattice (worst {worst:.2} m), {odd} not 8 m squares along the block, {doubled} slots doubled, {missing} empty, {stray} outside",
                    nx * ny
                ),
            );
        }
    }

    /// `opposite` for the ticks of the built street through the local points `a` -> `b`.
    fn check_opposite_built(&mut self, what: &str, a: Vec2, b: Vec2) {
        let (wa, wb) = (self.w(a), self.w(b));
        let dir = (wb - wa).normalize_or_zero();
        let len = wa.distance(wb);
        let mut sides: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
        for (m, _) in self.marks() {
            let r = m.axis - wa;
            let along = r.dot(dir);
            if r.perp_dot(dir).abs() > 0.5 || !(-0.5..=len + 0.5).contains(&along) {
                continue;
            }
            if m.major && !m.end {
                sides[usize::from(dir.perp_dot(m.v) > 0.0)].push(along);
            }
        }
        self.compare_sides(what, sides);
    }

    /// `opposite` for the ticks the ghost of a built piece showed beside it.
    fn check_opposite_ghost(&mut self, what: &str, b: &Built) {
        let Some(g) = &b.ghost else { return };
        let (wa, wb) = (self.w(b.piece.a), self.w(b.piece.b));
        let dir = (wb - wa).normalize_or_zero();
        let len = wa.distance(wb);
        let mut sides: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
        for t in &g.zone.ticks {
            let r = t[0] - wa;
            let along = r.dot(dir);
            let lat = dir.perp_dot(r);
            if lat.abs() > self.hw + 1.0 || !(-0.5..=len + 0.5).contains(&along) {
                continue;
            }
            sides[usize::from(dir.perp_dot(t[1] - t[0]) > 0.0)].push(along);
        }
        self.compare_sides(&format!("{what} (ghost)"), sides);
    }

    fn compare_sides(&mut self, what: &str, mut sides: [Vec<f32>; 2]) {
        for s in &mut sides {
            s.sort_by(f32::total_cmp);
        }
        let [l, r] = &sides;
        if l.is_empty() || r.is_empty() {
            return;
        }
        let lo = l[0].max(r[0]) - 4.0;
        let hi = l[l.len() - 1].min(r[r.len() - 1]) + 4.0;
        let mut worst = 0.0f32;
        for (a, b) in [(l, r), (r, l)] {
            for x in a.iter().filter(|x| (lo..=hi).contains(*x)) {
                let d = b
                    .iter()
                    .map(|y| (y - x).abs())
                    .fold(f32::INFINITY, f32::min);
                worst = worst.max(d);
            }
        }
        if worst >= 0.5 {
            let f = |v: &Vec<f32>| {
                v.iter()
                    .map(|x| format!("{x:.1}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            self.fail(
                "opposite",
                format!(
                    "{what}: ticks {worst:.2} m apart across the street; one side at [{}], the other at [{}]",
                    f(l),
                    f(r)
                ),
            );
        }
    }

    /// An L from `top` (local, on the y axis) down to the corner at the origin and on to `right`
    /// (local, on the x axis), sharp or rounded. Returns the left and the bottom straight pieces.
    fn draw_l(&mut self, bend: Bend, top: f32, right: f32) -> Option<(Piece, Piece)> {
        // A rounded corner starts its bend where the length step of the first street puts it.
        let top = if matches!(bend, Bend::Rounded) {
            top - 24.0 + BEND_R
        } else {
            top
        };
        let j = self.jv(2.0);
        // A rounded corner needs the first street to end exactly where the bend starts: only the
        // sideways jitter stays.
        let j = if matches!(bend, Bend::Rounded) {
            Vec2::new(j.x, 0.0)
        } else {
            j
        };
        let (start, end) = (
            Vec2::new(0.0, top) + j,
            Vec2::new(right, 0.0) + self.jv(3.0),
        );
        self.start(RoadMode::Straight);
        self.click("L start", self.w(start));
        match bend {
            Bend::Sharp => {
                let left = self.piece("L left", self.w(Vec2::ZERO))?;
                let bottom = self.piece("L bottom", self.w(end))?;
                Some(self.onto_bottom(left, bottom))
            }
            Bend::Rounded => {
                let left = self.piece("L left", self.w(Vec2::new(0.0, BEND_R)))?;
                self.mode(RoadMode::Curve);
                // The bend's corner is on the street the player drew (its cursor jitter moved it).
                let x = left.x();
                self.click("L bend control", self.w(Vec2::new(x, 0.0)));
                self.piece("L bend", self.w(Vec2::new(x + BEND_R, 0.0)))?;
                self.mode(RoadMode::Straight);
                let bottom = self.piece("L bottom", self.w(end))?;
                Some(self.onto_bottom(left, bottom))
            }
        }
    }

    /// The frame moved along its `y` so the L's bottom street is its `x` axis: the length step
    /// of the first stroke (whole cells from its open start, T-275) puts the corner where the
    /// cells say, and the player builds on from where it stands.
    fn onto_bottom(&mut self, left: Piece, bottom: Piece) -> (Piece, Piece) {
        let dy = Vec2::new(0.0, bottom.y());
        self.o = self.w(dy);
        let shift = |p: Piece| Piece {
            a: p.a - dy,
            b: p.b - dy,
        };
        (shift(left), shift(bottom))
    }

    /// A straight street from the tick `m` to the local point `to` (the cursor there).
    /// The cursor is jittered by up to `jit` metres at the end.
    fn street_from_tick(&mut self, what: &str, m: Mark, to: Vec2, jit: f32) -> Option<Built> {
        self.start(RoadMode::Straight);
        let p = self.at_tick(m);
        self.click(&format!("{what} start"), p);
        let to = to + self.jv(jit);
        let b = self.click(what, self.w(to));
        if b.is_none() {
            self.fail("refused", format!("{what}: not built"));
        }
        b
    }

    /// A straight street between two local points, the cursor jittered at its end.
    fn free(&mut self, what: &str, from: Vec2, to: Vec2) -> Option<Piece> {
        self.start(RoadMode::Straight);
        self.click(&format!("{what} start"), self.w(from));
        let to = to + self.jv(3.0);
        self.piece(what, self.w(to))
    }

    // ---------- T-261: blocks drawn off the guides (built by command, not through the tool) ----------

    /// Builds a straight street `a` -> `b` (local) of type `road` with `BuildRoad`, as sent: the
    /// player drew it off the guides.
    fn line(&mut self, what: &str, road: &str, a: Vec2, b: Vec2) -> bool {
        let id = self.db.road_id(road).unwrap();
        let mut outbox = CommandOutbox::default();
        outbox.push(SimCommand::Net(NetCommand::BuildRoad {
            road: id,
            path: RoadPath::Straight {
                from: self.w(a),
                to: self.w(b),
            },
            elevation: cp_net::ElevationMode::Ground,
        }));
        self.send(what, outbox)
    }

    fn send(&mut self, what: &str, outbox: CommandOutbox) -> bool {
        let mut outbox = outbox;
        outbox.flush_into(&mut self.sim);
        self.sim.tick_once();
        let out: Vec<CommandOutcome> = self
            .sim
            .world_mut()
            .resource_mut::<Messages<CommandOutcome>>()
            .drain()
            .collect();
        self.sim.tick_once();
        if let Some(Err(e)) = out.last().map(|o| &o.result) {
            self.fail("refused", format!("{what}: {e:?}"));
            return false;
        }
        true
    }

    /// The segments whose middle lies on the local street `a` -> `b` (within 0.5 m).
    fn segments_on(&self, a: Vec2, b: Vec2) -> Vec<cp_net::SegmentId> {
        let view = self.sim.view();
        view.net
            .segments()
            .filter(|(_, s)| {
                let m = self.l(to_xz(s.curve.point(0.5)));
                let ab = b - a;
                let t = (m - a).dot(ab) / ab.length_squared();
                (0.0..=1.0).contains(&t) && (a + ab * t).distance(m) < 0.5
            })
            .map(|(id, _)| id)
            .collect()
    }

    /// The closed block of streets through the local axis points `axes` (counter-clockwise),
    /// each side `i` (from `axes[i]` to `axes[i + 1]`) of half width `hw[i]` and a street when
    /// `street[i]`: `stretch` every cell inside is an 8 m square (no cell is stretched to fit,
    /// §6.8.3 item 4), `overlap` no two cells overlap, `fill` at least 75 % of the block within
    /// 4 rows (32 m) of a street edge is covered by cells.
    fn check_strips(&mut self, what: &str, axes: &[Vec2], hw: &[f32], street: &[bool]) {
        let poly = inset(axes, hw);
        let inside = |p: Vec2| in_convex(p, &poly);
        let cells: Vec<Obb> = self
            .cells()
            .into_iter()
            .filter(|o| inside(self.l(o.c)))
            .collect();
        let stretched = cells
            .iter()
            .filter(|o| (o.half - CELL / 2.0).abs() > 0.05 || (o.half_v - CELL / 2.0).abs() > 0.05)
            .count();
        if stretched > 0 {
            let o = cells
                .iter()
                .find(|o| {
                    (o.half - CELL / 2.0).abs() > 0.05 || (o.half_v - CELL / 2.0).abs() > 0.05
                })
                .unwrap();
            self.fail(
                "stretch",
                format!(
                    "{what}: {stretched} of {} cells are not 8 m squares, e.g. {:.2} x {:.2} m",
                    cells.len(),
                    o.half * 2.0,
                    o.half_v * 2.0
                ),
            );
        }
        let mut overlaps = 0;
        for i in 0..cells.len() {
            for j in i + 1..cells.len() {
                if cells[i].c.distance(cells[j].c) < CELL - 0.1 {
                    overlaps += 1;
                }
            }
        }
        if overlaps > 0 {
            self.fail(
                "overlap",
                format!("{what}: {overlaps} pairs of cells overlap"),
            );
        }
        let n = poly.len();
        let lo = poly
            .iter()
            .fold(Vec2::splat(f32::INFINITY), |a, p| a.min(*p));
        let hi = poly
            .iter()
            .fold(Vec2::splat(f32::NEG_INFINITY), |a, p| a.max(*p));
        let (mut want, mut have) = (0u32, 0u32);
        let world_cells: Vec<Obb> = cells.clone();
        let mut y = lo.y + 0.5;
        while y < hi.y {
            let mut x = lo.x + 0.5;
            while x < hi.x {
                let p = Vec2::new(x, y);
                let near_street = (0..n)
                    .any(|i| street[i] && dist_seg(p, poly[i], poly[(i + 1) % n]) <= 4.0 * CELL);
                if inside(p) && near_street {
                    want += 1;
                    let w = self.w(p);
                    if world_cells.iter().any(|o| o.distance_to_point(w) <= 0.0) {
                        have += 1;
                    }
                }
                x += 1.0;
            }
            y += 1.0;
        }
        if want > 0 && (have as f32) < 0.75 * want as f32 {
            self.fail(
                "fill",
                format!(
                    "{what}: cells cover {:.0} % of the block within 4 rows of a street (want 75 %)",
                    100.0 * have as f32 / want as f32
                ),
            );
        }
    }

    /// The id of the frame's road.
    fn road_name(&self) -> &'static str {
        ROADS
            .into_iter()
            .find(|r| self.db.road_id(r) == Some(self.road))
            .unwrap()
    }
}

fn key(c: [Vec2; 4]) -> [(i32, i32); 4] {
    c.map(|p| ((p.x * 50.0).round() as i32, (p.y * 50.0).round() as i32))
}

const X: Vec2 = Vec2::X;
const Y: Vec2 = Vec2::Y;

// ---------- scenarios ----------

/// BUG-073 shot 1.
fn l_close(p: &mut Player, bend: Bend) -> Option<()> {
    let hw = p.hw;
    let (left, bottom) = p.draw_l(bend, 200.0, 216.0)?;
    if matches!(bend, Bend::Rounded) {
        p.bite = Some(BEND_R + 2.0 * hw + CELL);
    }
    // The right street: up from the bottom street's 16-cell tick (counted from the left street's
    // edge) to its own 8-cell tick.
    let m = p.tick(
        "right street on the bottom tick",
        Vec2::new(hw + BLOCK, bottom.y()),
        Y,
    );
    let up = p.l(m.axis).x;
    let right = p.street_from_tick("right street", m, Vec2::new(up, 2.0 * hw + BLOCK), 1.5)?;
    p.check_perp("right street", right.piece, bottom.across());
    // The cross street from the left street's 8-cell tick to the right street's end.
    let m = p.tick(
        "cross street on the left tick",
        Vec2::new(left.x(), hw + BLOCK),
        X,
    );
    let cross = p.street_from_tick("cross street", m, right.piece.b, 1.5)?;
    p.check_perp("cross street", cross.piece, left.across());
    p.check_lands("cross street", &cross);
    let nx = ((right.piece.x() - left.x() - 2.0 * hw) / CELL).round() as u32;
    p.check_block(
        "the closed block",
        [bottom.y(), cross.piece.y(), left.x(), right.piece.x()],
        nx,
        8,
    );
    Some(())
}

/// BUG-073 shot 3.
fn parallel_cross(p: &mut Player) -> Option<()> {
    let hw = p.hw;
    let v = p.free("left street", Vec2::new(0.0, -40.0), Vec2::new(0.0, 200.0))?;
    // The first street off it: the cursor near y = 0 (the join takes the nearest boundary).
    p.start(RoadMode::Straight);
    let j = p.jit(1.5);
    let s = p.w(Vec2::new(v.x() + hw + 2.5, j));
    p.click("lower street start", s);
    let to = Vec2::new(200.0, 0.0) + p.jv(3.0);
    let lower = p.piece("lower street", p.w(to))?;
    // The upper street off the left street's 8-cell tick above the lower one, to its end.
    let m = p.tick(
        "upper street on the left tick",
        Vec2::new(v.x(), lower.y() + hw + BLOCK),
        X,
    );
    let to = Vec2::new(lower.b.x, p.l(m.axis).y + hw);
    let upper = p.street_from_tick("upper street", m, to, 2.0)?;
    p.check_perp("upper street", upper.piece, X);
    if (upper.piece.b.x - lower.b.x).abs() >= 0.5 {
        p.fail(
            "lands",
            format!(
                "upper street ends at x {:.2}, the lower one at {:.2}",
                upper.piece.b.x, lower.b.x
            ),
        );
    }
    // Close the block from the upper street's end down to the lower street's end.
    p.start(RoadMode::Straight);
    let s = upper.piece.b + p.jv(1.5);
    p.click("closing street start", p.w(s));
    let to = lower.b + p.jv(1.5);
    let close = p.click("closing street", p.w(to));
    let Some(close) = close else {
        p.fail("refused", "closing street: not built".into());
        return None;
    };
    p.check_perp("closing street", close.piece, Y);
    p.check_lands("closing street", &close);
    let nx = ((close.piece.x() - v.x() - 2.0 * hw) / CELL).round() as u32;
    p.check_block(
        "the closed block",
        [lower.y(), upper.piece.y(), v.x(), close.piece.x()],
        nx,
        8,
    );
    Some(())
}

/// BUG-073 shot 2.
fn extend_bend(p: &mut Player, bend: Bend) -> Option<()> {
    let (_, bottom) = p.draw_l(bend, 160.0, 96.0)?;
    // Straight on from the end of the L.
    let to = Vec2::new(bottom.b.x + 160.0, bottom.y()) + p.jv(3.0);
    let ext = p.click("extension", p.w(to));
    let Some(ext) = ext else {
        p.fail("refused", "extension: not built".into());
        return None;
    };
    p.check_perp("extension", ext.piece, bottom.dir());
    p.check_opposite_ghost("extension", &ext);
    let a = Vec2::new(bottom.a.x, bottom.y());
    p.check_opposite_built("bottom street + extension", a, ext.piece.b);
    Some(())
}

/// A 16 x 8 block closed from ticks, then a T from its middle tick across it.
fn t_mid_block(p: &mut Player) -> Option<()> {
    let hw = p.hw;
    let b0 = p.free("bottom street", Vec2::ZERO, Vec2::new(280.0, 0.0))?;
    // Three streets up from the bottom street, each on the next tick: the 2nd 8 cells (64 m
    // between the edges) from the 1st, the 3rd from the 2nd (BUG-073: a tick is the centreline
    // of the next cross street, not a boundary 16 cells away).
    let m = p.tick(
        "left street on the bottom tick",
        Vec2::new(BLOCK, b0.y()),
        Y,
    );
    let up = p.l(m.axis).x;
    let left = p.street_from_tick("left street", m, Vec2::new(up, 120.0), 3.0)?;
    let top_y = 2.0 * hw + BLOCK;
    let m = p.tick(
        "middle street on the bottom tick",
        Vec2::new(left.piece.x() + hw + BLOCK, b0.y()),
        Y,
    );
    let x = p.l(m.axis).x;
    let mid = p.street_from_tick("middle street", m, Vec2::new(x, top_y), 1.5)?;
    p.check_perp("middle street", mid.piece, Y);
    let m = p.tick(
        "right street on the bottom tick",
        Vec2::new(mid.piece.x() + hw + BLOCK, b0.y()),
        Y,
    );
    let x = p.l(m.axis).x;
    let right = p.street_from_tick("right street", m, Vec2::new(x, top_y), 1.5)?;
    p.check_perp("right street", right.piece, Y);
    // The top street from the left street's tick, on through the middle street's end (a T) to
    // the right street's end, in one chain.
    let m = p.tick(
        "top street on the left tick",
        Vec2::new(left.piece.x(), hw + BLOCK),
        X,
    );
    let top = p.street_from_tick("top street", m, mid.piece.b, 1.5)?;
    p.check_perp("top street", top.piece, X);
    p.check_lands("top street", &top);
    let to = right.piece.b + p.jv(1.5);
    let on = p.click("top street on", p.w(to));
    let Some(on) = on else {
        p.fail("refused", "top street on: not built".into());
        return None;
    };
    p.check_perp("top street on", on.piece, X);
    p.check_lands("top street on", &on);
    p.check_block(
        "the left block",
        [b0.y(), top.piece.y(), left.piece.x(), mid.piece.x()],
        8,
        8,
    );
    p.check_block(
        "the right block",
        [b0.y(), top.piece.y(), mid.piece.x(), right.piece.x()],
        8,
        8,
    );
    Some(())
}

#[derive(Clone, Copy, Debug)]
enum Order {
    RowsFirst,
    ColumnsFirst,
    RingThenCross,
}

/// Four 8 x 8 blocks between three streets each way.
fn lattice_2x2(p: &mut Player, order: Order) -> Option<()> {
    let hw = p.hw;
    let pitch = BLOCK + 2.0 * hw;
    let (rows, cols): (Vec<f32>, Vec<f32>) = match order {
        // Three parallel streets, the 2nd and 3rd on the 8-cell block guide of the one before,
        // then three across from the first one's ticks.
        Order::RowsFirst | Order::ColumnsFirst => {
            // In the columns-first order the frame is turned by 90 degrees: the same moves.
            let (a, b) = if matches!(order, Order::RowsFirst) {
                (X, Y)
            } else {
                (Y, -X)
            };
            let along = |s: f32, t: f32| a * s + b * t;
            let mut rows = Vec::new();
            for k in 0..3 {
                let r = p.free(
                    &format!("parallel {k}"),
                    along(-24.0, k as f32 * pitch),
                    along(2.0 * pitch + 72.0, k as f32 * pitch),
                )?;
                rows.push((r.a + r.b).dot(b) / 2.0);
            }
            let mut cols = Vec::new();
            // The first one on the first tick of the first street (it starts at -24).
            let mut at = BLOCK - 24.0;
            for k in 0..3 {
                let m = p.tick(
                    &format!("cross {k} on the first tick"),
                    along(at, rows[0]),
                    b,
                );
                let s = (p.l(m.axis) - along(0.0, rows[0])).dot(a);
                let c =
                    p.street_from_tick(&format!("cross {k}"), m, along(s, rows[2] + 24.0), 3.0)?;
                p.check_perp(&format!("cross {k}"), c.piece, b);
                let x = (c.piece.a + c.piece.b).dot(a) / 2.0;
                cols.push(x);
                at = x + hw + BLOCK;
            }
            if matches!(order, Order::RowsFirst) {
                (rows, cols)
            } else {
                // Rows ran along y (their x is -t), the crosses along x.
                (cols, rows.iter().map(|r| -r).collect())
            }
        }
        // Three streets up from the bottom one, each on the next tick, then two rows across
        // them from the first one's ticks (BUG-073: ticks restart after every street).
        Order::RingThenCross => {
            let b0 = p.free("bottom street", Vec2::ZERO, Vec2::new(280.0, 0.0))?;
            let mut cols: Vec<Built> = Vec::new();
            let mut at = BLOCK;
            for k in 0..3 {
                let m = p.tick(
                    &format!("column {k} on the bottom tick"),
                    Vec2::new(at, b0.y()),
                    Y,
                );
                let up = p.l(m.axis).x;
                let c = p.street_from_tick(
                    &format!("column {k}"),
                    m,
                    Vec2::new(up, 2.0 * pitch + 24.0),
                    3.0,
                )?;
                p.check_perp(&format!("column {k}"), c.piece, Y);
                at = c.piece.x() + hw + BLOCK;
                cols.push(c);
            }
            let mut rows = vec![b0.y()];
            for k in 0..2 {
                let nominal = rows[k] + hw + BLOCK;
                let m = p.tick(
                    &format!("row {k} on the left tick"),
                    Vec2::new(cols[0].piece.x(), nominal),
                    X,
                );
                let y = p.l(m.axis).y;
                let first = p.street_from_tick(
                    &format!("row {k}"),
                    m,
                    Vec2::new(cols[1].piece.x(), y),
                    1.5,
                )?;
                p.check_perp(&format!("row {k}"), first.piece, X);
                p.check_lands(&format!("row {k}"), &first);
                let to = Vec2::new(cols[2].piece.x(), first.piece.b.y) + p.jv(1.5);
                let on = p.click(&format!("row {k} on"), p.w(to));
                let Some(on) = on else {
                    p.fail("refused", format!("row {k} on: not built"));
                    return None;
                };
                p.check_perp(&format!("row {k} on"), on.piece, X);
                p.check_lands(&format!("row {k} on"), &on);
                rows.push(first.piece.y());
            }
            (rows, cols.iter().map(|c| c.piece.x()).collect())
        }
    };
    let (mut rows, mut cols) = (rows, cols);
    rows.sort_by(f32::total_cmp);
    cols.sort_by(f32::total_cmp);
    for i in 0..2 {
        for j in 0..2 {
            p.check_block(
                &format!("block ({i}, {j})"),
                [rows[j], rows[j + 1], cols[i], cols[i + 1]],
                8,
                8,
            );
        }
    }
    Some(())
}

/// The polygon of a block between the street edges: each side of the axis polygon `axes`
/// (counter-clockwise) moved inwards by its half width.
fn inset(axes: &[Vec2], hw: &[f32]) -> Vec<Vec2> {
    let n = axes.len();
    let line = |i: usize| {
        let (a, b) = (axes[i], axes[(i + 1) % n]);
        let d = (b - a).normalize();
        let inn = Vec2::new(-d.y, d.x);
        (a + inn * hw[i], d)
    };
    (0..n)
        .map(|i| {
            let (p, d) = line((i + n - 1) % n);
            let (q, e) = line(i);
            let t = (q - p).perp_dot(e) / d.perp_dot(e);
            p + d * t
        })
        .collect()
}

fn in_convex(p: Vec2, poly: &[Vec2]) -> bool {
    let n = poly.len();
    (0..n).all(|i| (poly[(i + 1) % n] - poly[i]).perp_dot(p - poly[i]) >= 0.0)
}

fn dist_seg(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// A closed block of four streets through the local axis corners `c` (counter-clockwise), each
/// of its own road type, built side by side off the guides; then [`Player::check_strips`].
fn block_of(p: &mut Player, what: &str, c: [Vec2; 4], roads: [&str; 4]) -> Option<()> {
    for i in 0..4 {
        if !p.line(&format!("{what} side {i}"), roads[i], c[i], c[(i + 1) % 4]) {
            return None;
        }
    }
    let hw = roads.map(|r| p.db.road(p.db.road_id(r).unwrap()).width / 2.0);
    p.check_strips(what, &c, &hw, &[true; 4]);
    Some(())
}

/// A rectangle whose interior between the edges is `w x h` metres, of the frame's road.
fn rect(p: &Player, w: f32, h: f32) -> [Vec2; 4] {
    let s = 2.0 * p.hw;
    [
        Vec2::ZERO,
        Vec2::new(w + s, 0.0),
        Vec2::new(w + s, h + s),
        Vec2::new(0.0, h + s),
    ]
}

/// T-261 `off_tick`: a block 70 x 86 m between the edges, not a whole number of cells.
fn off_tick(p: &mut Player) -> Option<()> {
    let r = p.road_name();
    block_of(p, "off-tick block", rect(p, 70.0, 86.0), [r; 4])
}

/// T-261 `on_tick`: a block 64 x 64 m between the edges built off the tool: the control case.
fn on_tick(p: &mut Player) -> Option<()> {
    let r = p.road_name();
    block_of(p, "8 x 8 block", rect(p, 64.0, 64.0), [r; 4])
}

/// T-261 `skewed`: a parallelogram, the cross streets 10 degrees off square.
fn skewed(p: &mut Player) -> Option<()> {
    let r = p.road_name();
    let s = 80.0 * 10f32.to_radians().tan();
    let c = [
        Vec2::ZERO,
        Vec2::new(96.0, 0.0),
        Vec2::new(96.0 + s, 80.0),
        Vec2::new(s, 80.0),
    ];
    block_of(p, "skewed block", c, [r; 4])
}

/// T-261 `trapezoid`: the top street shorter than the bottom one.
fn trapezoid(p: &mut Player) -> Option<()> {
    let r = p.road_name();
    let c = [
        Vec2::ZERO,
        Vec2::new(128.0, 0.0),
        Vec2::new(104.0, 88.0),
        Vec2::new(24.0, 88.0),
    ];
    block_of(p, "trapezoid block", c, [r; 4])
}

/// T-261 `long`: a block 64 x 240 m, longer than the layout's 198 m reach.
fn long_block(p: &mut Player) -> Option<()> {
    let r = p.road_name();
    block_of(p, "long block", rect(p, 240.0, 64.0), [r; 4])
}

/// T-261 `mixed`: an 8 x 8 block with `medium_4l` along the bottom and top and the frame's road
/// on the sides.
fn mixed(p: &mut Player) -> Option<()> {
    let r = p.road_name();
    let (hm, hr) = (
        p.db.road(p.db.road_id("medium_4l").unwrap()).width / 2.0,
        p.hw,
    );
    let c = [
        Vec2::ZERO,
        Vec2::new(64.0 + 2.0 * hr, 0.0),
        Vec2::new(64.0 + 2.0 * hr, 64.0 + 2.0 * hm),
        Vec2::new(0.0, 64.0 + 2.0 * hm),
    ];
    block_of(p, "mixed block", c, ["medium_4l", r, "medium_4l", r])
}

/// T-261 `one_way`: an 8 x 8 block of one-way streets round it.
fn one_way(p: &mut Player) -> Option<()> {
    let hw = p.db.road(p.db.road_id("basic_1w_2l").unwrap()).width / 2.0;
    let s = 2.0 * hw;
    let c = [
        Vec2::ZERO,
        Vec2::new(64.0 + s, 0.0),
        Vec2::new(64.0 + s, 64.0 + s),
        Vec2::new(0.0, 64.0 + s),
    ];
    block_of(p, "one-way block", c, ["basic_1w_2l"; 4])
}

/// T-261 `upgrade`: an 8 x 16 block, then its bottom street upgraded to `large_6l`.
fn upgrade(p: &mut Player) -> Option<()> {
    let r = p.road_name();
    let c = rect(p, 128.0, 64.0);
    for i in 0..4 {
        if !p.line(&format!("side {i}"), r, c[i], c[(i + 1) % 4]) {
            return None;
        }
    }
    let large = p.db.road_id("large_6l").unwrap();
    for seg in p.segments_on(c[0], c[1]) {
        let mut outbox = CommandOutbox::default();
        outbox.push(SimCommand::Net(NetCommand::UpgradeSegment {
            seg,
            road: large,
        }));
        if !p.send("the upgrade", outbox) {
            return None;
        }
    }
    let hl = p.db.road(large).width / 2.0;
    p.check_strips("upgraded block", &c, &[hl, p.hw, p.hw, p.hw], &[true; 4]);
    Some(())
}

/// T-261 `rebuild`: an 8 x 8 block; its left street demolished and drawn again: the cells are
/// those of the first build (`history`: the layout is a function of the network, not of the
/// order it was built in).
fn rebuild(p: &mut Player) -> Option<()> {
    let r = p.road_name();
    let c = rect(p, 64.0, 64.0);
    for i in 0..4 {
        if !p.line(&format!("side {i}"), r, c[i], c[(i + 1) % 4]) {
            return None;
        }
    }
    let before = p.cell_keys();
    for seg in p.segments_on(c[3], c[0]) {
        let mut outbox = CommandOutbox::default();
        outbox.push(SimCommand::Net(NetCommand::RemoveSegment { seg }));
        if !p.send("the demolition", outbox) {
            return None;
        }
    }
    if !p.line("the left street again", r, c[3], c[0]) {
        return None;
    }
    let after = p.cell_keys();
    let (lost, new) = (
        before.difference(&after).count(),
        after.difference(&before).count(),
    );
    if lost + new > 0 {
        p.fail(
            "history",
            format!("rebuilt block: {lost} cells of the first build gone, {new} new ones"),
        );
    }
    p.check_strips("rebuilt block", &c, &[p.hw; 4], &[true; 4]);
    Some(())
}

/// BUG-081 (from dev-1's WIP): a U drawn in one chain along the ticks: up the left street,
/// across the top for `sections` tick sections, down the right street (open at the bottom); the
/// 8 rows under the top street, between the side streets, are checked as strips.
fn u_shape(p: &mut Player, bend: Bend, sections: u32) -> Option<()> {
    let hw = p.hw;
    let r = if matches!(bend, Bend::Rounded) {
        BEND_R
    } else {
        0.0
    };
    let k = sections as f32;
    let right_x = k * (BLOCK + 2.0 * hw);
    let top_y = 3.0 * BLOCK;
    p.start(RoadMode::Straight);
    p.click("U start", p.w(Vec2::new(0.0, 0.0)));
    let left = p.piece("U left", p.w(Vec2::new(0.0, top_y - r)))?;
    if matches!(bend, Bend::Rounded) {
        p.mode(RoadMode::Curve);
        p.click("U corner 1 control", p.w(Vec2::new(left.x(), top_y)));
        p.piece("U corner 1", p.w(Vec2::new(left.x() + r, top_y)))?;
        p.mode(RoadMode::Straight);
    }
    let top = p.piece("U top", p.w(Vec2::new(right_x - r, top_y)))?;
    if matches!(bend, Bend::Rounded) {
        p.mode(RoadMode::Curve);
        p.click("U corner 2 control", p.w(Vec2::new(right_x, top.y())));
        p.piece("U corner 2", p.w(Vec2::new(right_x, top_y - r)))?;
        p.mode(RoadMode::Straight);
    }
    let right = p.piece("U right", p.w(Vec2::new(right_x, top_y - 2.0 * BLOCK)))?;
    // The axes of the U, closed by a line 8 rows under the top street's edge (not a street).
    let bottom = top.y() - hw - BLOCK - hw;
    let axes = [
        Vec2::new(left.x(), bottom),
        Vec2::new(right.x(), bottom),
        Vec2::new(right.x(), top.y()),
        Vec2::new(left.x(), top.y()),
    ];
    if matches!(bend, Bend::Sharp) {
        p.check_strips(
            "the block inside the U",
            &axes,
            &[hw; 4],
            &[false, true, true, true],
        );
    } else {
        // The bites of the rounded corners are allowed: only stretch and overlap count.
        p.check_strips("the block inside the U", &axes, &[hw; 4], &[false; 4]);
    }
    Some(())
}

// ---------- the sweep ----------

/// A case's name and its failures, `(check, message)`.
type Case = (String, Vec<(String, String)>);

/// Runs one scenario over angles, roads and seeds: `(case, failures)` per case.
fn sweep(
    scenario: &str,
    variants: &[&str],
    run: impl Fn(&mut Player, &str) -> Option<()>,
) -> Vec<Case> {
    let mut out = Vec::new();
    for variant in variants {
        for road in ROADS {
            for angle in ANGLES {
                for seed in SEEDS {
                    let case = format!("{scenario}/{road}/{variant}/{angle}/{seed}");
                    if let Ok(only) = std::env::var("GRID_SWEEP_ONLY")
                        && !case.starts_with(&only)
                    {
                        continue;
                    }
                    if std::env::var_os("GRID_SWEEP_TRACE").is_some() {
                        println!("{case}");
                    }
                    let mut p = Player::new(road, angle, seed);
                    if run(&mut p, variant).is_none() && p.fails.is_empty() {
                        p.fail("refused", "the scenario stopped".into());
                    }
                    out.push((case, p.fails));
                }
            }
        }
    }
    out
}

/// [`sweep`] for the blocks built by command (T-261): no jitter, so seed 0 only.
fn sweep_direct(scenario: &str, run: fn(&mut Player) -> Option<()>) -> Vec<Case> {
    let mut out = Vec::new();
    for road in ROADS {
        for angle in ANGLES {
            let case = format!("{scenario}/{road}/-/{angle}/0");
            if let Ok(only) = std::env::var("GRID_SWEEP_ONLY")
                && !case.starts_with(&only)
            {
                continue;
            }
            let mut p = Player::new(road, angle, 0);
            if run(&mut p).is_none() && p.fails.is_empty() {
                p.fail("refused", "the scenario stopped".into());
            }
            out.push((case, p.fails));
        }
    }
    out
}

fn all_cases() -> Vec<Case> {
    let mut out = Vec::new();
    out.extend(sweep("l_close", &["sharp", "rounded"], |p, v| {
        l_close(
            p,
            if v == "sharp" {
                Bend::Sharp
            } else {
                Bend::Rounded
            },
        )
    }));
    for sections in [1u32, 2, 3] {
        for (bend, name) in [(Bend::Sharp, "sharp"), (Bend::Rounded, "rounded")] {
            out.extend(sweep("u_shape", &[&format!("{name}{sections}")], |p, _| {
                u_shape(p, bend, sections)
            }));
        }
    }
    type Direct = fn(&mut Player) -> Option<()>;
    let direct: [(&str, Direct); 9] = [
        ("on_tick", on_tick),
        ("off_tick", off_tick),
        ("skewed", skewed),
        ("trapezoid", trapezoid),
        ("long", long_block),
        ("mixed", mixed),
        ("one_way", one_way),
        ("upgrade", upgrade),
        ("rebuild", rebuild),
    ];
    for (name, run) in direct {
        out.extend(sweep_direct(name, run));
    }
    out.extend(sweep("parallel_cross", &["-"], |p, _| parallel_cross(p)));
    out.extend(sweep("extend_bend", &["sharp", "rounded"], |p, v| {
        extend_bend(
            p,
            if v == "sharp" {
                Bend::Sharp
            } else {
                Bend::Rounded
            },
        )
    }));
    out.extend(sweep("t_mid_block", &["-"], |p, _| t_mid_block(p)));
    out.extend(sweep(
        "lattice_2x2",
        &["rows", "columns", "ring"],
        |p, v| {
            let order = match v {
                "rows" => Order::RowsFirst,
                "columns" => Order::ColumnsFirst,
                _ => Order::RingThenCross,
            };
            lattice_2x2(p, order)
        },
    ));
    out
}

fn report(cases: &[Case]) -> String {
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

fn results() -> &'static [Case] {
    static R: OnceLock<Vec<Case>> = OnceLock::new();
    R.get_or_init(|| {
        let r = all_cases();
        if std::env::var_os("GRID_SWEEP_REPORT").is_some() {
            println!("{}", report(&r));
            for (c, f) in &r {
                println!("CASE {} {c}", if f.is_empty() { "green" } else { "red" });
            }
        }
        r
    })
}

/// Every case not listed in [`KNOWN_RED`] builds clean blocks along the guides.
#[test]
fn the_green_cases_build_clean_blocks_along_the_guides() {
    let red: Vec<_> = results()
        .iter()
        .filter(|(c, f)| !f.is_empty() && !known_red(c))
        .cloned()
        .collect();
    assert!(red.is_empty(), "{}", report(&red));
}

/// The cases under [`KNOWN_RED`] still fail (they are the open bugs, BUG-073). One that passes
/// now is fixed: narrow its prefix so it joins the green set.
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
