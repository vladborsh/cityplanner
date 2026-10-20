//! T-275 (spec v2, `architecture/system-design.md` §6.8): roads by whole cells. A player builds
//! a city of blocks with the real road tool ([`resolve_cursor`], [`RoadTool::click`],
//! [`chain_commit`], the ghost [`compute_ghost_alt`]) and a sloppy hand: every click lands up to
//! 3 m off the point meant and every stroke wobbles up to 2 degrees. Roads are whole cells wide
//! and a street ends a whole number of sections (10 cells between road centres) from where it
//! starts, so the tool must give:
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
        Vec2::new(self.rand(), self.rand()) * SLOP_M
    }

    /// One stroke from about `a` to about `b` (local metres, `x` east, `y` south): a click near
    /// `a`, the ghost at a cursor near `b`, a click there. Checks the ghost, the label and the
    /// built street; returns the new street's segments.
    fn stroke(&mut self, name: &str, a: Vec2, b: Vec2) -> Option<(Vec2, Vec2)> {
        let o = self.origin;
        let mut t = self.tool();
        let start = o + a + self.slop();
        let turn = Vec2::from_angle((self.rand() * WOBBLE_DEG).to_radians());
        let end = o + a + turn.rotate(b - a) + self.slop();
        let view = self.sim.view();
        let q = resolve_cursor(&view, &t, start, false).map_or(start, |c| c.cursor);
        let _ = t.click(q);
        let Some(ghost) = compute_ghost_alt(&view, &t, end, false) else {
            self.fails.push(format!("{name}: no ghost"));
            return None;
        };
        if !ghost.valid {
            self.fails
                .push(format!("{name}: red ghost {:?}", ghost.errors));
            return None;
        }
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
        for &id in &ids {
            let s = view.net.segment(id).unwrap();
            let d = cp_core::curve::to_xz(s.curve.p3 - s.curve.p0);
            let deg = d.y.atan2(d.x).to_degrees().rem_euclid(90.0);
            let off = deg.min(90.0 - deg);
            if off > 0.05 {
                self.fails.push(format!(
                    "{name}: a piece runs {off:.3} deg off the map's axes"
                ));
            }
        }
        println!("{} {name}: from {:?} to {:?}", self.road, pa - o, pb - o);
        // No cell shifts: every cell of the city stands on the lattice of the first junction.
        if name != "top street" || self.origin != C {
            let off = valid_centres(&view.zones)
                .into_iter()
                .filter(|c| {
                    let g = (*c - self.lattice) / CELL - Vec2::splat(0.5);
                    (g - g.round()).abs().max_element() * CELL > 0.01
                })
                .count();
            if off > 0 {
                self.fails.push(format!(
                    "{name}: {off} cells off the city's lattice after it"
                ));
            }
        }
        Some((pa - o, pb - o))
    }
}

fn valid_centres(zones: &cp_zone::ZoneGrid) -> Vec<Vec2> {
    zones
        .blocks()
        .flat_map(|(id, _)| zones.block_cells(id))
        .filter(|c| zones.is_valid(*c))
        .filter_map(|c| zones.cell_footprint(c).map(|o| o.c))
        .collect()
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
    let mut pl = Player::new(long(0), seed);
    let section = 10.0 * CELL;
    let xs: Vec<f32> = (0..4).map(|i| i as f32 * sx * section).collect();
    let ys: Vec<f32> = (0..4).map(|j| j as f32 * sy * section).collect();
    let v = Vec2::new;
    if let Some((a, _)) = pl.stroke("top street", v(xs[0], ys[0]), v(xs[3], ys[0])) {
        pl.origin += a;
        pl.lattice = pl.origin;
    }
    pl.road = cross(0);
    pl.stroke("left street", v(xs[0], ys[0]), v(xs[0], ys[3]));
    for (j, &y) in ys.iter().enumerate().skip(1) {
        pl.road = long(j);
        pl.stroke(&format!("long street {j}"), v(xs[0], y), v(xs[3], y));
    }
    for (i, &x) in xs.iter().enumerate().skip(1) {
        pl.road = cross(i);
        pl.stroke(&format!("cross street {i}"), v(x, ys[0]), v(x, ys[3]));
    }
    let mut fails = std::mem::take(&mut pl.fails);
    let db = pl.sim.view().content.clone().unwrap();
    let hw = |r: &str| db.road(db.road_id(r).unwrap()).width / 2.0;
    let zones = &pl.sim.view().zones;
    for i in 0..3 {
        for j in 0..3 {
            let lo = v(xs[i] + hw(cross(i)), ys[j] + hw(long(j)));
            let hi = v(xs[i + 1] - hw(cross(i + 1)), ys[j + 1] - hw(long(j + 1)));
            let cols = ((hi.x - lo.x) / CELL).round() as u32;
            let rows = ((hi.y - lo.y) / CELL).round() as u32;
            for e in check_block(zones, pl.origin, lo, hi, cols, rows) {
                fails.push(format!("block ({i}, {j}) of {cols} x {rows}: {e}"));
            }
        }
    }
    fails
        .into_iter()
        .map(|f| format!("seed {seed}: {f}"))
        .collect()
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
