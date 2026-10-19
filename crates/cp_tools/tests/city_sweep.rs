//! T-275: roads by whole cells (`architecture/system-design.md` §6.8.3, the top rule). A player
//! builds a city of blocks with the real road tool ([`resolve_cursor`], [`RoadTool::click`],
//! [`chain_commit`], the ghost [`compute_ghost_alt`]) and a sloppy hand: every click lands up to
//! 3 m off the point meant and every stroke wobbles up to 2 degrees. The tool must still give:
//!
//! * every street square to the map (0.05 degrees);
//! * every block's cells exactly 8 x 16 whole cells on one lattice, nothing over, nothing missing;
//! * the label before each click == the whole cells the street gets after it, and the click ==
//!   the ghost.
//!
//! The city: a 3 x 3 grid of 8 x 16 blocks, long streets first (the top street, the left street,
//! the other long streets off the left street's ticks), then the cross streets off the top
//! street's ticks down across the long streets (X junctions inside, T junctions on the edge).
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
        let label: Option<u32> = ghost
            .label
            .as_deref()
            .and_then(|l| l.split(' ').next())
            .and_then(|n| n.parse().ok());
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
        let seen = self.cells_along(&ids);
        println!(
            "{} {name}: label {:?}, cells along {seen:?}, from {:?} to {:?}",
            self.road,
            ghost.label,
            pa - o,
            pb - o
        );
        if label != Some(seen[0].max(seen[1])) {
            self.fails.push(format!(
                "{name}: the label says {:?}, the street got {seen:?} cells",
                ghost.label
            ));
        }
        Some((pa - o, pb - o))
    }

    /// The valid cells in the first row along the street `ids` on each side (left, right of
    /// the street's own direction), whoever's cells they are: what the player sees along it.
    fn cells_along(&self, ids: &[SegmentId]) -> [u32; 2] {
        let view = self.sim.view();
        let cells = valid_centres(&view.zones);
        let mut out = [0u32; 2];
        for &id in ids {
            let s = view.net.segment(id).unwrap();
            let (p0, p3) = (
                cp_core::curve::to_xz(s.curve.p0),
                cp_core::curve::to_xz(s.curve.p3),
            );
            let len = p0.distance(p3);
            let u = (p3 - p0) / len;
            let n = Vec2::new(-u.y, u.x);
            for c in &cells {
                let rel = *c - p0;
                let (along, off) = (rel.dot(u), rel.dot(n));
                if !(0.0..len).contains(&along) {
                    continue;
                }
                if (self.hw..self.hw + CELL).contains(&off) {
                    out[0] += 1;
                } else if (-self.hw - CELL..-self.hw).contains(&off) {
                    out[1] += 1;
                }
            }
        }
        out
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

/// The 3 x 3 city of 8 x 16 blocks (16 cells along the long streets, 8 across), long streets
/// first. Returns what went wrong.
fn city_8x16(road: &'static str, seed: u64) -> Vec<String> {
    let mut pl = Player::new(road, seed);
    let w = 2.0 * pl.hw;
    // The axes of the streets: vertical at x = i * (128 + w), horizontal at y = j * (64 + w).
    let xs: Vec<f32> = (0..4).map(|i| i as f32 * (128.0 + w)).collect();
    let ys: Vec<f32> = (0..4).map(|j| j as f32 * (64.0 + w)).collect();
    let v = |x: f32, y: f32| Vec2::new(x, y);
    // The top street, the left street down from its start, the long streets off the left
    // street's ticks.
    if let Some((a, _)) = pl.stroke("top street", v(xs[0], ys[0]), v(xs[3], ys[0])) {
        pl.origin += a;
    }
    pl.stroke("left street", v(xs[0], ys[0]), v(xs[0], ys[3]));
    for (j, &y) in ys.iter().enumerate().skip(1) {
        pl.stroke(&format!("long street {j}"), v(xs[0], y), v(xs[3], y));
    }
    // The cross streets off the top street's 16-cell ticks, down across the long streets.
    for (i, &x) in xs.iter().enumerate().skip(1) {
        pl.stroke(&format!("cross street {i}"), v(x, ys[0]), v(x, ys[3]));
    }
    let mut fails = std::mem::take(&mut pl.fails);
    let zones = &pl.sim.view().zones;
    for i in 0..3 {
        for j in 0..3 {
            let lo = v(xs[i] + pl.hw, ys[j] + pl.hw);
            let hi = v(xs[i + 1] - pl.hw, ys[j + 1] - pl.hw);
            for e in check_block(zones, pl.origin, lo, hi, 16, 8) {
                fails.push(format!("block ({i}, {j}): {e}"));
            }
        }
    }
    fails
        .into_iter()
        .map(|f| format!("{road} seed {seed}: {f}"))
        .collect()
}

#[test]
fn a_city_of_8x16_blocks_is_whole_cells_with_a_sloppy_hand() {
    let mut fails = Vec::new();
    for road in ["basic_2l", "medium_4l"] {
        for seed in [1, 2, 3] {
            fails.extend(city_8x16(road, seed));
        }
    }
    assert!(
        fails.is_empty(),
        "{} failures:\n{}",
        fails.len(),
        fails.join("\n")
    );
}
