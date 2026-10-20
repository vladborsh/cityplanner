//! T-275 (spec v2, `architecture/system-design.md` §6.8): roads by whole cells. A player builds
//! a city of blocks with the real road tool ([`resolve_cursor`], [`RoadTool::click`],
//! [`chain_commit`], the ghost [`compute_ghost_alt`]) and a sloppy hand: every click lands up to
//! 3 m off the point meant and every stroke wobbles up to 2 degrees. Roads are whole cells wide
//! and a street ends a whole number of cells from where it starts (T-278; the sections of 10
//! cells between road centres are guides), so the tool must give:
//!
//! * every street square to the map (0.05 degrees);
//! * every block exactly its whole cells on one lattice, nothing over, nothing missing;
//! * after every stroke, every cell of the city on the one lattice of the first junction: a cell
//!   may vanish under a new street, never shift;
//! * the click == the ghost.
//!
//! The city: 3 x 3 blocks, streets 2 sections apart along `x` and 1 section apart along `y`
//! (2-cell roads: blocks of 18 x 8 cells; 4-cell roads: 16 x 6), long streets first (the top
//! street, the left street, the other long streets off the left street's marks), then the cross
//! streets off the top street's marks down across the long streets (X junctions inside, T
//! junctions on the edge).
//!
//! Run: `cargo nextest run -p cp_tools --test city_sweep --no-capture` prints every stroke.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::{CommandOutcome, Cost, Treasury};
use cp_net::{NetCommand, SegmentId};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::road::chain_commit;
use cp_tools::{CommandOutbox, RoadMode, RoadTool, compute_ghost_alt, resolve_cursor};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
/// The city's top-left corner, world.
const C: Vec2 = Vec2::new(4400.0, 4400.0);
const CELL: f32 = 8.0;
/// How far off the meant point a click lands, metres, and how far a stroke wobbles, degrees.
const SLOP_M: f32 = 3.0;
const WOBBLE_DEG: f32 = 2.0;

fn assets() -> (Arc<ContentDb>, Arc<Terrain>) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    (db, t)
}

/// A player with a sloppy hand.
struct Player {
    sim: SimRunner,
    road: &'static str,
    hw: f32,
    seed: u64,
    /// The city's top-left corner as built (the first click lands where it lands): the player
    /// aims every later stroke from what stands.
    origin: Vec2,
    /// A corner of the city's cell lattice (the first junction: road centres and road edges are
    /// whole cells from it).
    lattice: Vec2,
    /// What went wrong, per stroke.
    fails: Vec<String>,
    /// How sloppy the hand is: 1 the sloppy hand, 0 an exact one.
    slop_k: f32,
    /// Every straight street is checked to run square to the map.
    axes: bool,
    /// After every stroke every cell of a straight street stands on the city's lattice (a city
    /// with a curve is checked once it stands, see [`arc_city`]).
    lattice_each: bool,
    /// The ticks of the last stroke's ghost (T-278).
    ticks: Vec<cp_tools::lattice::CellTick>,
    /// The end is aimed from where the start landed (a street in the open: the player drags it
    /// out from the click), not from the point meant; so is the first street of a net.
    from_start: bool,
}

impl Player {
    fn new(road: &'static str, seed: u64) -> Self {
        let (db, t) = assets();
        let mut sim = SimRunner::new(1);
        sim.set_terrain(t);
        sim.set_content(Arc::clone(&db));
        sim.tick_once();
        sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(1_000_000_000.0);
        let hw = db.road(db.road_id(road).unwrap()).width / 2.0;
        Self {
            sim,
            road,
            hw,
            seed,
            origin: C,
            lattice: C,
            fails: Vec::new(),
            slop_k: 1.0,
            axes: true,
            lattice_each: true,
            ticks: Vec::new(),
            from_start: false,
        }
    }

    fn tool(&self) -> RoadTool {
        let db = self.sim.view().content.clone().unwrap();
        RoadTool {
            mode: RoadMode::Straight,
            road: Some(db.road_id(self.road).unwrap()),
            ..Default::default()
        }
    }

    /// A number in `-1..1`, the same for the same seed.
    fn rand(&mut self) -> f32 {
        self.seed = self
            .seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.seed >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    }

    fn slop(&mut self) -> Vec2 {
        Vec2::new(self.rand(), self.rand()) * SLOP_M * self.slop_k
    }

    /// One stroke from about `a` to about `b` (local metres, `x` east, `y` south): a click near
    /// `a`, the ghost at a cursor near `b`, a click there. Checks the ghost, the label and the
    /// built street; returns where its ends stand (local).
    fn stroke(&mut self, name: &str, a: Vec2, b: Vec2) -> Option<(Vec2, Vec2)> {
        self.draw(name, a, None, b)
    }

    /// A curve (T-268): a click near `a`, a click near its bend `c`, the ghost at a cursor near
    /// `b`, a click there.
    fn curve(&mut self, name: &str, a: Vec2, c: Vec2, b: Vec2) -> Option<(Vec2, Vec2)> {
        self.draw(name, a, Some(c), b)
    }

    fn draw(&mut self, name: &str, a: Vec2, bend: Option<Vec2>, b: Vec2) -> Option<(Vec2, Vec2)> {
        let o = self.origin;
        let mut t = self.tool();
        if bend.is_some() {
            t.mode = RoadMode::Curve;
        }
        let start = o + a + self.slop();
        let turn = Vec2::from_angle((self.rand() * WOBBLE_DEG * self.slop_k).to_radians());
        let curve = bend.is_some();
        let bend = bend.map(|c| o + a + turn.rotate(c - a) + self.slop());
        // A straight street's end: the stroke's direction wobbles (the square takes it up); a
        // curve's end is a point of its own (T-278: within half a cell of a guide it takes it).
        let empty = self.sim.view().net.segments().next().is_none();
        let end = if self.from_start || empty {
            start + turn.rotate(b - a) + self.slop()
        } else if curve {
            o + b + self.slop()
        } else {
            o + a + turn.rotate(b - a) + self.slop()
        };
        let view = self.sim.view();
        let q = resolve_cursor(&view, &t, start, false).map_or(start, |c| c.cursor);
        let _ = t.click(q);
        if let Some(c) = bend {
            let q = resolve_cursor(&view, &t, c, false).map_or(c, |c| c.cursor);
            let _ = t.click(q);
        }
        let Some(ghost) = compute_ghost_alt(&view, &t, end, false) else {
            self.fails.push(format!("{name}: no ghost"));
            return None;
        };
        if !ghost.valid {
            self.fails
                .push(format!("{name}: red ghost {:?}", ghost.errors));
            return None;
        }
        self.ticks = ghost.cell_ticks.clone();
        let before_ids: BTreeSet<SegmentId> = view.net.segments().map(|(id, _)| id).collect();
        let q = resolve_cursor(&view, &t, end, false).map_or(end, |c| c.cursor);
        let before = t.clone();
        let sent = t
            .click(q)
            .and_then(|req| chain_commit(Some(&view), &mut t, before, req, false));
        // Where the plan's ends stand, before it splits the streets they are on.
        let ends: Vec<Vec2> = sent
            .iter()
            .flat_map(|plan| plan.pieces.iter().flat_map(|p| [p.from, p.to]))
            .filter_map(|a| cp_tools::anchor_pos(&view, &a))
            .collect();
        drop(view);
        let Some(plan) = sent else {
            self.fails.push(format!("{name}: the click was refused"));
            return None;
        };
        if Some(&plan) != ghost.plan.as_ref() {
            self.fails
                .push(format!("{name}: the click is not the ghost"));
        }
        let mut outbox = CommandOutbox::default();
        outbox.push(SimCommand::Net(NetCommand::BuildPlan {
            plan: plan.clone(),
        }));
        outbox.flush_into(&mut self.sim);
        self.sim.tick_once();
        let out: Vec<CommandOutcome> = self
            .sim
            .world_mut()
            .resource_mut::<Messages<CommandOutcome>>()
            .drain()
            .collect();
        if let Some(e) = out.iter().find(|o| o.result.is_err()) {
            self.fails
                .push(format!("{name}: refused by the sim: {e:?}"));
            return None;
        }
        for _ in 0..4 {
            self.sim.tick_once();
        }
        // The new street: the new segments along the plan.
        let view = self.sim.view();
        let (pa, pb) = (ends[0], *ends.last().unwrap());
        let ids: Vec<SegmentId> = view
            .net
            .segments()
            .filter(|(id, s)| {
                let m = cp_core::curve::to_xz(s.curve.point(0.5));
                !before_ids.contains(id) && dist_seg(m, pa, pb) < 0.05
            })
            .map(|(id, _)| id)
            .collect();
        for &id in ids.iter().filter(|_| self.axes && bend.is_none()) {
            let s = view.net.segment(id).unwrap();
            let off = off_axes(cp_core::curve::to_xz(s.curve.p3 - s.curve.p0));
            if off > 0.05 {
                self.fails.push(format!(
                    "{name}: a piece runs {off:.3} deg off the map's axes"
                ));
            }
        }
        if bend.is_some() && self.axes {
            // A curve leaves and arrives square to the map at both its ends.
            for (id, s) in view
                .net
                .segments()
                .filter(|(id, _)| !before_ids.contains(id))
            {
                let _ = id;
                let c = &s.curve;
                for (p, t) in [(c.p0, 0.0), (c.p3, 1.0)] {
                    let p = cp_core::curve::to_xz(p);
                    if p.distance(pa) < 0.05 || p.distance(pb) < 0.05 {
                        let off = off_axes(cp_core::curve::to_xz(c.tangent(t)));
                        if off > 0.05 {
                            self.fails.push(format!(
                                "{name}: the curve leaves {off:.3} deg off the map's axes"
                            ));
                        }
                    }
                }
            }
        }
        println!("{} {name}: from {:?} to {:?}", self.road, pa - o, pb - o);
        // No cell shifts: every cell of the city stands on the lattice of the first junction.
        if self.lattice_each && (name != "top street" || self.origin != C) {
            let off = self.off_lattice(&view.zones, |_| true);
            if off > 0 {
                self.fails.push(format!(
                    "{name}: {off} cells off the city's lattice after it"
                ));
            }
        }
        Some((pa - o, pb - o))
    }

    /// The valid cells of straight streets with their centre (local) in `area` that are off the
    /// city's lattice.
    fn off_lattice(&self, zones: &cp_zone::ZoneGrid, area: impl Fn(Vec2) -> bool) -> usize {
        lattice_centres(zones)
            .into_iter()
            .filter(|c| area(*c - self.origin))
            .filter(|c| {
                let g = (*c - self.lattice) / CELL - Vec2::splat(0.5);
                (g - g.round()).abs().max_element() * CELL > 0.01
            })
            .count()
    }
}

/// Degrees between `d` and the nearest axis of the map.
fn off_axes(d: Vec2) -> f32 {
    let deg = d.y.atan2(d.x).to_degrees().rem_euclid(90.0);
    deg.min(90.0 - deg)
}

/// The centres of the valid cells.
fn valid_centres(zones: &cp_zone::ZoneGrid) -> Vec<Vec2> {
    cells(zones, |_| true)
}

/// The centres of the valid cells of straight streets (a curve's cells follow the curve).
fn lattice_centres(zones: &cp_zone::ZoneGrid) -> Vec<Vec2> {
    cells(zones, |b| straight(&b.curve))
}

fn cells(zones: &cp_zone::ZoneGrid, keep: impl Fn(&cp_zone::ZoneBlock) -> bool) -> Vec<Vec2> {
    zones
        .blocks()
        .filter(|(_, b)| keep(b))
        .flat_map(|(id, _)| zones.block_cells(id))
        .filter(|c| zones.is_valid(*c))
        .filter_map(|c| zones.cell_footprint(c).map(|o| o.c))
        .collect()
}

fn straight(c: &cp_core::curve::CubicBezier3) -> bool {
    let (a, b) = (cp_core::curve::to_xz(c.p0), cp_core::curve::to_xz(c.p3));
    let u = (b - a).normalize_or_zero();
    [c.p1, c.p2]
        .iter()
        .all(|p| (cp_core::curve::to_xz(*p) - a).perp_dot(u).abs() < 0.01)
}

/// No valid cell lies on a road (more than the zoning's own tolerance) or in another valid cell
/// (more than a block's own tolerance): what is wrong, if anything.
fn cells_clear(view: &cp_sim::SimView) -> Vec<String> {
    let db = view.content.clone().unwrap();
    let zones = &view.zones;
    let obbs: Vec<cp_zone::geom::Obb> = zones
        .blocks()
        .flat_map(|(id, _)| zones.block_cells(id))
        .filter(|c| zones.is_valid(*c))
        .filter_map(|c| zones.cell_footprint(c))
        .collect();
    let mut out = Vec::new();
    for (_, s) in view.net.segments() {
        let hw = db.road(s.road).width / 2.0;
        let pts: Vec<Vec2> = (0..=64)
            .map(|i| cp_core::curve::to_xz(s.curve.point(i as f32 / 64.0)))
            .collect();
        for o in &obbs {
            let d = pts
                .windows(2)
                .map(|w| o.distance_to_segment(w[0], w[1]))
                .fold(f32::MAX, f32::min);
            if d < hw - cp_zone::grid::ROAD_EPS - 0.02 {
                out.push(format!(
                    "a cell at {:?} lies {:.2} m on a road",
                    o.c,
                    hw - d
                ));
            }
        }
    }
    for (i, a) in obbs.iter().enumerate() {
        for b in &obbs[i + 1..] {
            if a.c.distance(b.c) < 2.0 * CELL
                && a.penetration(b) > cp_zone::grid::PENETRATION_EPS + 0.01
            {
                out.push(format!("the cells at {:?} and {:?} overlap", a.c, b.c));
            }
        }
    }
    out
}

fn dist_seg(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// The cells inside the rectangle `lo..hi` (local): `cols x rows` whole cells on the lattice of
/// its corner, every one there once, nothing else. What is wrong, if anything.
fn check_block(
    zones: &cp_zone::ZoneGrid,
    origin: Vec2,
    lo: Vec2,
    hi: Vec2,
    cols: u32,
    rows: u32,
) -> Vec<String> {
    let mut out = Vec::new();
    let size = hi - lo;
    let want = Vec2::new(cols as f32, rows as f32) * CELL;
    if (size - want).abs().max_element() > 0.01 {
        out.push(format!(
            "the block is {:.2} x {:.2} m between the edges, want {} x {} cells",
            size.x, size.y, cols, rows
        ));
    }
    let mut slots: BTreeSet<(i32, i32)> = BTreeSet::new();
    let (mut off, mut doubled) = (0, 0);
    for c in valid_centres(zones) {
        let l = c - origin;
        if l.x <= lo.x || l.x >= hi.x || l.y <= lo.y || l.y >= hi.y {
            continue;
        }
        let g = (l - lo) / CELL - Vec2::splat(0.5);
        let r = g.round();
        if (g - r).abs().max_element() * CELL > 0.01 {
            off += 1;
            continue;
        }
        if !slots.insert((r.x as i32, r.y as i32)) {
            doubled += 1;
        }
    }
    let empty = (cols * rows) as usize - slots.len().min((cols * rows) as usize);
    println!(
        "block {lo:?}..{hi:?}: {} cells on the lattice, {off} off, {doubled} doubled, {empty} empty",
        slots.len()
    );
    if off > 0 || doubled > 0 || empty > 0 {
        out.push(format!(
            "{} cells on the lattice of {} x {}: {off} off it, {doubled} doubled, {empty} empty",
            slots.len(),
            cols,
            rows
        ));
    }
    out
}

/// The 3 x 3 city: streets `sx` sections apart along `x`, `sy` along `y`, long streets first,
/// all of `road`. Returns what went wrong.
fn city(road: &'static str, seed: u64, sx: f32, sy: f32) -> Vec<String> {
    city_of(seed, sx, sy, |_| road, |_| road)
        .into_iter()
        .map(|f| format!("{road} {f}"))
        .collect()
}

/// The 3 x 3 city with the long street `j` of road `long(j)` and the cross street `i` of road
/// `cross(i)`: every block is the whole cells between the edges of its four streets.
fn city_of(
    seed: u64,
    sx: f32,
    sy: f32,
    long: impl Fn(usize) -> &'static str,
    cross: impl Fn(usize) -> &'static str,
) -> Vec<String> {
    let section = 10.0 * CELL;
    let xs: Vec<f32> = (0..4).map(|i| i as f32 * sx * section).collect();
    let ys: Vec<f32> = (0..4).map(|j| j as f32 * sy * section).collect();
    grid_city(seed, &xs, &ys, long, cross).1
}

/// A city of the streets at `xs` along `x` and `ys` along `y` (local metres, their centres; the
/// first of each at 0), long streets first: every block is the whole cells between the edges of
/// its four streets. The player who built it and what went wrong.
fn grid_city(
    seed: u64,
    xs: &[f32],
    ys: &[f32],
    long: impl Fn(usize) -> &'static str,
    cross: impl Fn(usize) -> &'static str,
) -> (Player, Vec<String>) {
    let mut pl = Player::new(long(0), seed);
    let v = Vec2::new;
    let (x1, y1) = (*xs.last().unwrap(), *ys.last().unwrap());
    if let Some((a, _)) = pl.stroke("top street", v(xs[0], ys[0]), v(x1, ys[0])) {
        pl.origin += a;
        pl.lattice = pl.origin;
    }
    pl.road = cross(0);
    pl.stroke("left street", v(xs[0], ys[0]), v(xs[0], y1));
    for (j, &y) in ys.iter().enumerate().skip(1) {
        pl.road = long(j);
        pl.stroke(&format!("long street {j}"), v(xs[0], y), v(x1, y));
    }
    for (i, &x) in xs.iter().enumerate().skip(1) {
        pl.road = cross(i);
        pl.stroke(&format!("cross street {i}"), v(x, ys[0]), v(x, y1));
    }
    let mut fails = std::mem::take(&mut pl.fails);
    let db = pl.sim.view().content.clone().unwrap();
    let hw = |r: &str| db.road(db.road_id(r).unwrap()).width / 2.0;
    {
        let zones = &pl.sim.view().zones;
        for i in 0..xs.len() - 1 {
            for j in 0..ys.len() - 1 {
                let lo = v(xs[i] + hw(cross(i)), ys[j] + hw(long(j)));
                let hi = v(xs[i + 1] - hw(cross(i + 1)), ys[j + 1] - hw(long(j + 1)));
                let cols = ((hi.x - lo.x) / CELL).round() as u32;
                let rows = ((hi.y - lo.y) / CELL).round() as u32;
                for e in check_block(zones, pl.origin, lo, hi, cols, rows) {
                    fails.push(format!("block ({i}, {j}) of {cols} x {rows}: {e}"));
                }
            }
        }
    }
    let fails = fails
        .into_iter()
        .map(|f| format!("seed {seed}: {f}"))
        .collect();
    (pl, fails)
}

#[test]
fn a_3x3_city_of_long_blocks_is_whole_cells_with_a_sloppy_hand() {
    let mut fails = Vec::new();
    for road in ["basic_2l", "medium_4l"] {
        for seed in [1, 2, 3] {
            fails.extend(city(road, seed, 2.0, 1.0));
        }
    }
    assert!(
        fails.is_empty(),
        "{} failures:\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// Acceptance 3: blocks of one section each way: 8 x 8 cells between 2-cell streets, 6 x 6
/// between 4-cell ones.
#[test]
fn a_3x3_city_of_8x8_blocks_is_whole_cells_with_a_sloppy_hand() {
    let mut fails = Vec::new();
    for road in ["basic_2l", "medium_4l"] {
        for seed in [4, 5] {
            fails.extend(city(road, seed, 1.0, 1.0));
        }
    }
    assert!(
        fails.is_empty(),
        "{} failures:\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// Acceptance 3: a 4-cell avenue beside a 2-cell street one section away leaves a block 7 cells
/// deep (the older street lays 4 rows, the other 3), whole cells on one lattice.
#[test]
fn a_city_of_avenues_and_streets_has_7_cell_blocks_on_one_lattice() {
    let mut fails = Vec::new();
    for seed in [6, 7] {
        fails.extend(city_of(
            seed,
            2.0,
            1.0,
            |j| if j % 2 == 0 { "medium_4l" } else { "basic_2l" },
            |_| "basic_2l",
        ));
    }
    assert!(
        fails.is_empty(),
        "{} failures:\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// Acceptance 3: an L turn is one sharp node; the cells inside the corner go right into it: the
/// 4 x 4 cells there are all laid, on one lattice, nothing over.
#[test]
fn the_cells_inside_an_l_go_right_into_the_corner() {
    for road in ["basic_2l", "medium_4l"] {
        let mut pl = Player::new(road, 8);
        let section = 10.0 * CELL;
        if let Some((a, _)) = pl.stroke("top street", Vec2::ZERO, Vec2::new(3.0 * section, 0.0)) {
            pl.origin += a;
            pl.lattice = pl.origin;
        }
        pl.stroke("left street", Vec2::ZERO, Vec2::new(0.0, 3.0 * section));
        assert!(pl.fails.is_empty(), "{road}: {:?}", pl.fails);
        let lo = Vec2::splat(pl.hw);
        let hi = lo + Vec2::splat(4.0 * CELL);
        let e = check_block(&pl.sim.view().zones, pl.origin, lo, hi, 4, 4);
        assert!(e.is_empty(), "{road}: the corner: {e:?}");
    }
}

// ---------- T-268: curves ----------

/// The 3 x 3 city of 18 x 8 blocks (T-268 acceptance (a)) with its top-left corner drawn as a
/// curve: the top street starts at its first cross street, the curve goes from there round the
/// corner (its bend clicked near the corner) down to the first long street, and the left street
/// goes on from the curve's end. Every block the curve does not touch is its whole cells on the
/// city's lattice; the curve's own block keeps cells on its inner side; no cell lies on a road or
/// in another cell.
fn arc_city(road: &'static str, seed: u64) -> Vec<String> {
    let mut pl = Player::new(road, seed);
    pl.lattice_each = false;
    let section = 10.0 * CELL;
    let xs: Vec<f32> = (0..4).map(|i| i as f32 * 2.0 * section).collect();
    let ys: Vec<f32> = (0..4).map(|j| j as f32 * section).collect();
    let v = Vec2::new;
    if let Some((a, _)) = pl.stroke("top street", v(xs[1], 0.0), v(xs[3], 0.0)) {
        pl.origin += a - v(xs[1], 0.0);
        pl.lattice = pl.origin;
    }
    pl.curve("corner curve", v(xs[1], 0.0), v(0.0, 0.0), v(0.0, ys[1]));
    pl.stroke("left street", v(0.0, ys[1]), v(0.0, ys[3]));
    for (j, &y) in ys.iter().enumerate().skip(1) {
        pl.stroke(&format!("long street {j}"), v(0.0, y), v(xs[3], y));
    }
    for (i, &x) in xs.iter().enumerate().skip(1) {
        pl.stroke(&format!("cross street {i}"), v(x, 0.0), v(x, ys[3]));
    }
    let mut fails = std::mem::take(&mut pl.fails);
    let view = pl.sim.view();
    let hw = pl.hw;
    for i in 0..3 {
        for j in 0..3 {
            if (i, j) == (0, 0) {
                continue;
            }
            let lo = v(xs[i] + hw, ys[j] + hw);
            let hi = v(xs[i + 1] - hw, ys[j + 1] - hw);
            let cols = ((hi.x - lo.x) / CELL).round() as u32;
            let rows = ((hi.y - lo.y) / CELL).round() as u32;
            for e in check_block(&view.zones, pl.origin, lo, hi, cols, rows) {
                fails.push(format!("block ({i}, {j}) of {cols} x {rows}: {e}"));
            }
        }
    }
    // The straight streets' cells inside the city, but for the curve's block, on the lattice.
    let in_city = |l: Vec2| l.x > 0.0 && l.y > 0.0 && l.x < xs[3] && l.y < ys[3];
    let in_corner = |l: Vec2| l.x < xs[1] && l.y < ys[1];
    let off = pl.off_lattice(&view.zones, |l| in_city(l) && !in_corner(l));
    if off > 0 {
        fails.push(format!(
            "{off} cells of straight streets off the city's lattice"
        ));
    }
    fails.extend(cells_clear(&view));
    // The curve's cells on its inner side, in the corner block.
    let inner = cells(&view.zones, |b| !straight(&b.curve))
        .into_iter()
        .filter(|c| {
            let l = *c - pl.origin;
            in_city(l) && in_corner(l)
        })
        .count();
    println!("{road} seed {seed}: {inner} cells inside the curve");
    if inner == 0 {
        fails.push("no cells inside the curve".into());
    }
    fails
        .into_iter()
        .map(|f| format!("{road} seed {seed}: {f}"))
        .collect()
}

#[test]
fn a_3x3_city_with_a_curved_corner_is_whole_cells_with_a_sloppy_hand() {
    let mut fails = Vec::new();
    for road in ["basic_2l", "medium_4l"] {
        for seed in [11, 12] {
            fails.extend(arc_city(road, seed));
        }
    }
    assert!(
        fails.is_empty(),
        "{} failures:\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// T-268 acceptance (b): an S between two parallel streets, drawn as two curves (the second
/// starts where the first ends and leaves along it): it leaves the first street square at a
/// section mark and meets the second square at its section mark, a T there.
#[test]
fn an_s_of_two_curves_joins_two_parallel_streets_square() {
    for road in ["basic_2l", "medium_4l"] {
        for seed in [13, 14] {
            let mut pl = Player::new(road, seed);
            pl.lattice_each = false;
            let section = 10.0 * CELL;
            // Each half turns a quarter at the smallest legs its road takes comfortably.
            let l = if pl.hw > 10.0 { section } else { section / 2.0 };
            let gap = 2.0 * l;
            let v = Vec2::new;
            if let Some((a, _)) = pl.stroke("first street", v(0.0, 0.0), v(4.0 * section, 0.0)) {
                pl.origin += a;
                pl.lattice = pl.origin;
            }
            pl.stroke("left street", v(0.0, 0.0), v(0.0, gap));
            pl.stroke("second street", v(0.0, gap), v(4.0 * section, gap));
            let x0 = section;
            pl.curve("S, first half", v(x0, 0.0), v(x0, l), v(x0 + l, l));
            let end = pl.curve(
                "S, second half",
                v(x0 + l, l),
                v(x0 + 2.0 * l, l),
                v(x0 + 2.0 * l, gap),
            );
            assert!(pl.fails.is_empty(), "{road} seed {seed}: {:?}", pl.fails);
            let (_, b) = end.unwrap();
            assert!(
                b.distance(v(x0 + 2.0 * l, gap)) < 0.01,
                "{road} seed {seed}: the S ends at {b:?}"
            );
            let view = pl.sim.view();
            let t = view
                .net
                .nodes()
                .find(|(_, n)| cp_core::curve::to_xz(n.pos).distance(pl.origin + b) < 0.01)
                .map(|(_, n)| n.segments.len());
            assert_eq!(t, Some(3), "{road} seed {seed}: a T at the S's end");
        }
    }
}

/// T-268 acceptance (c): a curve tighter than its road takes (3 cells of radius for a 2-cell
/// road, 5 for a 4-cell one) is red with the mark at its bend, and a click builds nothing; a
/// curve just wide enough builds.
#[test]
fn a_curve_tighter_than_its_road_takes_is_refused_at_its_bend() {
    for (road, tight, ok) in [("basic_2l", 24.0, 40.0), ("medium_4l", 40.0, 64.0)] {
        let mut pl = Player::new(road, 15);
        pl.slop_k = 0.0;
        let v = Vec2::new;
        let a = v(2.0 * 10.0 * CELL, 0.0);
        if let Some((s, _)) = pl.stroke("street", v(0.0, 0.0), a) {
            pl.origin += s;
        }
        let o = pl.origin;
        let mut t = pl.tool();
        t.mode = RoadMode::Curve;
        let view = pl.sim.view();
        for p in [a, a + v(tight, 0.0)] {
            let q = resolve_cursor(&view, &t, o + p, false).unwrap().cursor;
            let _ = t.click(q);
        }
        let g = compute_ghost_alt(&view, &t, o + a + v(tight, tight), false).unwrap();
        assert!(!g.valid, "{road}: a curve of legs {tight} m is not refused");
        assert!(
            g.errors.iter().any(|e| e.contains("too tight")),
            "{road}: {:?}",
            g.errors
        );
        let apex = o + a + v(0.75 * tight, 0.25 * tight);
        assert!(
            g.error_marks.iter().any(|m| m.distance(apex) < tight / 2.0),
            "{road}: the mark is not at the bend: {:?} (bend {apex:?})",
            g.error_marks
        );
        let before = t.clone();
        let q = resolve_cursor(&view, &t, o + a + v(tight, tight), false)
            .unwrap()
            .cursor;
        let req = t.click(q).unwrap();
        assert!(
            chain_commit(Some(&view), &mut t, before, req, false).is_none(),
            "{road}: the click builds the tight curve"
        );
        drop(view);
        let built = pl.curve("wide enough", a, a + v(ok, 0.0), a + v(ok, ok));
        assert!(
            built.is_some() && pl.fails.is_empty(),
            "{road}: {:?}",
            pl.fails
        );
    }
}

/// A quarter curve from the end of a street (2-cell road, exact hand): from `(160, 0)` round to
/// `(320, 160)`, its bend at `(320, 0)`; 260 m long, so the net cuts it in three.
fn street_and_curve(seed: u64) -> Player {
    let mut pl = Player::new("basic_2l", seed);
    pl.slop_k = 0.0;
    pl.lattice_each = false;
    let v = Vec2::new;
    if let Some((s, _)) = pl.stroke("street", v(0.0, 0.0), v(160.0, 0.0)) {
        pl.origin += s;
    }
    pl.curve("curve", v(160.0, 0.0), v(320.0, 0.0), v(320.0, 160.0));
    assert!(pl.fails.is_empty(), "{:?}", pl.fails);
    pl
}

/// T-268 acceptance (d): a street started from the middle of a curve (at its section mark)
/// leaves it square to the curve there, a T.
#[test]
fn a_street_from_a_curve_leaves_it_square() {
    for seed in [16, 17] {
        let mut pl = street_and_curve(seed);
        pl.slop_k = 1.0;
        pl.axes = false;
        let v = Vec2::new;
        // The player aims at the section mark the curve shows near its middle, square out of it.
        let (a, out) = {
            let view = pl.sim.view();
            let seg = view
                .net
                .segments()
                .find(|(_, s)| !straight(&s.curve))
                .unwrap()
                .0;
            let marks = cp_tools::lattice::section_marks(&view.net, seg);
            let mid = pl.origin + v(290.0, 40.0);
            let &(_, p, d) = marks
                .iter()
                .min_by(|x, y| x.1.distance(mid).total_cmp(&y.1.distance(mid)))
                .unwrap();
            (p - pl.origin, v(d.y, -d.x))
        };
        let (s, _) = pl
            .stroke("side street", a, a + out * 80.0)
            .unwrap_or_else(|| panic!("seed {seed}: {:?}", pl.fails));
        assert!(pl.fails.is_empty(), "seed {seed}: {:?}", pl.fails);
        let view = pl.sim.view();
        let (_, node) = view
            .net
            .nodes()
            .find(|(_, n)| cp_core::curve::to_xz(n.pos).distance(pl.origin + s) < 0.01)
            .expect("the side street's start is a node");
        assert_eq!(node.segments.len(), 3, "seed {seed}: a T on the curve");
        let dirs: Vec<(bool, Vec2)> = node
            .segments
            .iter()
            .map(|&id| {
                let sg = view.net.segment(id).unwrap();
                let d = if cp_core::curve::to_xz(sg.curve.p0).distance(pl.origin + s) < 0.01 {
                    sg.curve.tangent(0.0)
                } else {
                    -sg.curve.tangent(1.0)
                };
                (straight(&sg.curve), cp_core::curve::to_xz(d).normalize())
            })
            .collect();
        println!("T at {s:?}, aim {a:?} out {out:?}: {dirs:?}");
        let side = dirs.iter().find(|d| d.0).expect("the side street").1;
        for (_, d) in dirs.iter().filter(|d| !d.0) {
            let off = side.dot(*d).asin().to_degrees().abs();
            assert!(
                off < 0.05,
                "seed {seed}: {off:.3} deg off square to the curve"
            );
        }
    }
}

/// T-268 acceptance (e): a street across a curve makes an X on it, near the curve's own cut and
/// on it alike, and the net is the same whichever is drawn first.
#[test]
fn a_street_across_a_curve_makes_an_x_whichever_comes_first() {
    let v = Vec2::new;
    // The curve's lower cut (the net cuts it in three).
    let cut = {
        let pl = street_and_curve(18);
        let view = pl.sim.view();
        view.net
            .nodes()
            .filter(|(_, n)| n.segments.len() == 2)
            .map(|(_, n)| cp_core::curve::to_xz(n.pos) - pl.origin)
            .find(|p| p.x > 170.0 && p.y > 50.0 && p.y < 150.0)
            .expect("a cut of the curve")
    };
    // At the cut, 6 m from it (it is dissolved: the curve is cut anew), and clear of it.
    for y in [cut.y, cut.y + 6.0, cut.y + 30.0] {
        let across = |pl: &mut Player| {
            pl.stroke("across", v(400.0, y), v(240.0, y))
                .unwrap_or_else(|| panic!("y {y}: {:?}", pl.fails));
        };
        let mut a = street_and_curve(18);
        across(&mut a);
        let mut b = Player::new("basic_2l", 18);
        b.slop_k = 0.0;
        b.lattice_each = false;
        if let Some((s, _)) = b.stroke("street", v(0.0, 0.0), v(160.0, 0.0)) {
            b.origin += s;
        }
        across(&mut b);
        b.curve("curve", v(160.0, 0.0), v(320.0, 0.0), v(320.0, 160.0));
        for pl in [&a, &b] {
            assert!(pl.fails.is_empty(), "y {y}: {:?}", pl.fails);
            let net = &pl.sim.view().net;
            assert_eq!(
                net.check_invariants(),
                Vec::<cp_net::Violation>::new(),
                "y {y}"
            );
            let x = net.nodes().filter(|(_, n)| n.segments.len() == 4).count();
            assert_eq!(x, 1, "y {y}: one X");
        }
        // The same roads: every point of one net lies on the other.
        let (na, nb) = (&a.sim.view().net, &b.sim.view().net);
        for (n1, n2) in [(na, nb), (nb, na)] {
            for (_, s) in n1.segments() {
                for i in 0..=8 {
                    let p = cp_core::curve::to_xz(s.curve.point(i as f32 / 8.0));
                    let d = n2
                        .segments()
                        .map(|(_, t)| t.curve.nearest_xz(p).1)
                        .fold(f32::MAX, f32::min);
                    assert!(d < 0.05, "y {y}: {p:?} is {d:.3} m off the other net");
                }
            }
        }
    }
}

/// T-268 (user decision 2026-10-20): a curve's end near a guide lands exactly on it, one far
/// from every guide lands where the cursor is, on a whole cell from the curve's start.
#[test]
fn a_curve_end_takes_a_guide_near_it_else_the_nearest_whole_cell() {
    let v = Vec2::new;
    let fresh = || {
        let mut pl = Player::new("basic_2l", 19);
        pl.slop_k = 0.0;
        pl.lattice_each = false;
        if let Some((s, _)) = pl.stroke("street", v(0.0, 0.0), v(320.0, 0.0)) {
            pl.origin += s;
        }
        pl
    };
    // A whole section along and across from the start, 3.5 m and 3 m off (T-278: half a cell).
    let mut pl = fresh();
    let (_, b) = pl
        .curve(
            "to a section",
            v(160.0, 0.0),
            v(160.0, 60.0),
            v(240.0 + 3.5, 80.0 - 3.0),
        )
        .unwrap();
    assert!(b.distance(v(240.0, 80.0)) < 0.01, "a section: {b:?}");
    // The square through the street's end, 3.5 m off: on it, a whole cell down from the start.
    let mut pl = fresh();
    let (_, b) = pl
        .curve(
            "to a street's line",
            v(160.0, 0.0),
            v(160.0, 50.0),
            v(323.5, 58.0),
        )
        .unwrap();
    assert!(b.distance(v(320.0, 56.0)) < 0.01, "a street's line: {b:?}");
    // The same line 5 m off: past half a cell, the nearest whole cell from the start.
    let mut pl = fresh();
    let (_, b) = pl
        .curve(
            "past a street's line",
            v(160.0, 0.0),
            v(160.0, 50.0),
            v(325.0, 58.0),
        )
        .unwrap();
    assert!(
        b.distance(v(328.0, 56.0)) < 0.01,
        "past a street's line: {b:?}"
    );
    // Nothing near: the nearest whole cell.
    let mut pl = fresh();
    pl.axes = false;
    let (_, b) = pl
        .curve("free", v(160.0, 0.0), v(160.0, 50.0), v(290.5, 110.5))
        .unwrap();
    assert!(b.distance(v(288.0, 112.0)) < 0.01, "free: {b:?}");
    assert!(pl.fails.is_empty(), "{:?}", pl.fails);
}

// ---------- T-278: a street by whole cells ----------

/// T-278 acceptance: a street drawn in the open towards N cells (the cursor near N cells from its
/// start, a sloppy hand) is exactly N cells long, for every N from 1 to 25, on 2- and 4-cell
/// roads; its ghost has a tick at every cell, every 10th a major one.
#[test]
fn a_street_of_every_length_from_1_to_25_cells_lands_exactly() {
    let mut fails = Vec::new();
    for (road, seed) in [("basic_2l", 21), ("medium_4l", 22)] {
        let mut pl = Player::new(road, seed);
        pl.lattice_each = false;
        pl.from_start = true;
        for n in 1..=25u32 {
            // A hand 3 m off over a cell or two aims wider than the 7 degrees of the square.
            pl.axes = n >= 3;
            // Far apart, so that no street is a guide for another.
            let a = Vec2::new(((n - 1) % 5) as f32 * 320.0, ((n - 1) / 5) as f32 * 120.0);
            let want = n as f32 * CELL;
            let Some((s, e)) = pl.stroke(&format!("{n} cells"), a, a + Vec2::new(want, 0.0)) else {
                continue;
            };
            let len = s.distance(e);
            if (len - want).abs() > 0.01 {
                fails.push(format!("{road}: {n} cells: {len:.3} m"));
            }
            let majors = pl.ticks.iter().filter(|t| t.major).count();
            if pl.ticks.len() != n as usize || majors != n as usize / 10 {
                fails.push(format!(
                    "{road}: {n} cells: {} ticks, {majors} major",
                    pl.ticks.len()
                ));
            }
        }
        fails.extend(pl.fails.iter().map(|f| format!("{road}: {f}")));
    }
    assert!(
        fails.is_empty(),
        "{} failures:\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// T-278 acceptance: a cursor within half a cell of the 10, 20 or 30 cell mark lands on the mark;
/// past half a cell, on the cell beside it.
#[test]
fn a_cursor_within_half_a_cell_of_a_section_mark_lands_on_it() {
    let mut pl = Player::new("basic_2l", 23);
    pl.slop_k = 0.0;
    pl.lattice_each = false;
    let mut row = 0.0;
    for k in 1..=3 {
        for (off, cells) in [(-3.5, 0), (3.5, 0), (4.5, 1), (-4.5, -1)] {
            let a = Vec2::new(0.0, row);
            row += 80.0;
            let mark = k as f32 * 10.0 * CELL;
            let (s, e) = pl
                .stroke("to the mark", a, a + Vec2::new(mark + off, 0.0))
                .unwrap_or_else(|| panic!("{:?}", pl.fails));
            let want = mark + cells as f32 * CELL;
            assert!(
                (s.distance(e) - want).abs() < 0.01,
                "{k}0 cells {off:+} m: {:.3} m, want {want}",
                s.distance(e)
            );
        }
    }
    assert!(pl.fails.is_empty(), "{:?}", pl.fails);
}

/// The streets' centres for blocks of `blocks` cells in a row between roads of half width `hw`.
fn centres(blocks: impl IntoIterator<Item = u32>, hw: f32) -> Vec<f32> {
    let mut out = vec![0.0];
    for b in blocks {
        out.push(out.last().unwrap() + b as f32 * CELL + 2.0 * hw);
    }
    out
}

/// T-278 acceptance: blocks of every width from 1 to 18 cells, a row of them between two long
/// streets a section apart, each its whole cells on the one lattice (2- and 4-cell roads).
#[test]
fn blocks_of_1_to_18_cells_are_whole_on_one_lattice() {
    let mut fails = Vec::new();
    for (road, hw) in [("basic_2l", CELL), ("medium_4l", 2.0 * CELL)] {
        for (blocks, seed) in [(1..=9, 24), (10..=18, 25)] {
            let xs = centres(blocks, hw);
            let ys = [0.0, 10.0 * CELL];
            let (_, f) = grid_city(seed, &xs, &ys, |_| road, |_| road);
            fails.extend(f.into_iter().map(|f| format!("{road} {f}")));
        }
    }
    assert!(
        fails.is_empty(),
        "{} failures:\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// T-278 shot city: blocks of 18 x 8, 5 x 8 and 8 x 8 cells, and 18 x 3, 5 x 3 and 8 x 3 below
/// them, each its whole cells on the one lattice.
#[test]
fn a_city_of_mixed_blocks_is_whole_cells_with_a_sloppy_hand() {
    let mut fails = Vec::new();
    for seed in [26, 27] {
        let xs = centres([18, 5, 8], CELL);
        let ys = centres([8, 3], CELL);
        fails.extend(grid_city(seed, &xs, &ys, |_| "basic_2l", |_| "basic_2l").1);
    }
    assert!(
        fails.is_empty(),
        "{} failures:\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// T-278 acceptance: a street branches from any whole cell of a street, drawn in any order, and
/// starts exactly there.
#[test]
fn a_street_branches_from_any_cell_of_a_street() {
    for (road, seed) in [("basic_2l", 28), ("medium_4l", 29)] {
        let mut pl = Player::new(road, seed);
        let v = Vec2::new;
        if let Some((a, _)) = pl.stroke("top street", v(0.0, 0.0), v(32.0 * CELL, 0.0)) {
            pl.origin += a;
            pl.lattice = pl.origin;
        }
        for k in [14, 4, 27, 9, 21] {
            let x = k as f32 * CELL;
            let (s, _) = pl
                .stroke(&format!("branch at {k}"), v(x, 0.0), v(x, 5.0 * CELL))
                .unwrap_or_else(|| panic!("{road}: {:?}", pl.fails));
            assert!(
                s.distance(v(x, 0.0)) < 0.01,
                "{road}: the branch at {k} cells starts at {s:?}"
            );
        }
        assert!(pl.fails.is_empty(), "{road}: {:?}", pl.fails);
    }
}

/// T-278: a street's end within a junction's pull (1 cell, 2 where a 4-cell road meets) goes to
/// its centre: the corner closes exactly.
#[test]
fn a_street_end_within_the_pull_closes_on_the_junction() {
    for (road, off) in [("basic_2l", 5.0), ("medium_4l", 11.0)] {
        let mut pl = Player::new(road, 30);
        pl.slop_k = 0.0;
        let v = Vec2::new;
        if let Some((a, _)) = pl.stroke("top street", v(0.0, 0.0), v(20.0 * CELL, 0.0)) {
            pl.origin += a;
            pl.lattice = pl.origin;
        }
        pl.stroke(
            "cross street",
            v(10.0 * CELL, 0.0),
            v(10.0 * CELL, 10.0 * CELL),
        );
        let (_, e) = pl
            .stroke(
                "up to the T",
                v(0.0, 10.0 * CELL),
                v(10.0 * CELL - off, 10.0 * CELL - off),
            )
            .unwrap_or_else(|| panic!("{road}: {:?}", pl.fails));
        assert!(
            e.distance(v(10.0 * CELL, 10.0 * CELL)) < 0.01,
            "{road}: the end is at {e:?}"
        );
        assert!(pl.fails.is_empty(), "{road}: {:?}", pl.fails);
    }
}
