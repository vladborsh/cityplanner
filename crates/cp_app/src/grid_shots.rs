//! The grid layouts of the user's BUG-073 shots (T-255) and the U of BUG-081 (T-265), built the way the player built
//! them: every street through the road tool's own code ([`block_hover`], [`RoadTool::click`],
//! [`chain_commit`]: the quantizing, the joins, the snaps), a cross street started on the tick
//! the tool offers (an 8-cell line of the street's cells, [`cp_zone::cell_lines`]). The last street of each layout
//! is not built: the camera `grid-shotN` holds its ghost ([`ghost`]), from the click on its
//! start to the cursor at its end, as the shot shows it. Before/after proof for BUG-073.
//!
//! * shot 1: an L with a rounded corner, a street up from the bottom street's 16-cell tick to
//!   its own 8-cell tick; ghost: the cross street from the left street's 8-cell tick to that end;
//! * shot 2: an L with a rounded corner at 40 degrees; ghost: the bottom street extended on;
//! * shot 3: a street, two parallels off it 8 cells apart; ghost: closing the block from the
//!   upper street's end down to the lower one's;
//! * shot 4: a rounded corner (left street into the top street), the bottom street off the left
//!   street's 8-cell tick, longer than the block; ghost: closing from the top street's 16-cell
//!   tick down to the bottom street. Square to the map: the tool snaps a street's direction to
//!   the map axes, a turned frame would bend the streets.
//! * shot 5 (T-265): the U of BUG-081, drawn in one chain: up the left street, across the top for
//!   two blocks, down the right street; ghost: the street closing the block 8 rows under the top
//!   street's edge.

use bevy::prelude::*;
use bevy_ecs::message::Messages;
use cp_content::RoadDefId;
use cp_core::CommandOutcome;
use cp_core::curve::to_xz;
use cp_net::NetCommand;
use cp_sim::{SimCommand, SimRunner, SimView};
use cp_tools::road::chain_commit;
use cp_tools::zone_preview::block_hover;
use cp_tools::{GhostDemo, RoadMode, RoadTool};
use cp_zone::Mark;

/// `--scene grid-shots`.
pub const GRID_SHOTS: &str = "grid-shots";

/// The road the user drew with.
const ROAD: &str = "basic_2l";
const BLOCK: f32 = 64.0;
/// Radius of the rounded corners.
const BEND_R: f32 = 24.0;

/// Where layout `n` (1..=5) stands, and the turn of its frame in degrees.
pub fn origin(n: usize) -> (Vec2, f32) {
    let c = cp_terrain::MAP_SIZE / 2.0;
    let (dx, dz, deg) = match n {
        1 => (-450.0, 150.0, 0.0),
        2 => (150.0, 150.0, 40.0),
        3 => (-450.0, -350.0, 0.0),
        4 => (150.0, -350.0, 0.0),
        _ => (-450.0, -850.0, 0.0),
    };
    (Vec2::new(c + dx, c + dz), deg)
}

/// Where the camera `grid-shotN` looks: the middle of the layout, in the world.
pub fn focus(n: usize) -> Vec2 {
    let f = Frame::new(n);
    f.w(match n {
        1 => Vec2::new(110.0, 70.0),
        2 => Vec2::new(190.0, 10.0),
        3 => Vec2::new(110.0, 40.0),
        4 => Vec2::new(110.0, 60.0),
        _ => Vec2::new(76.0, 120.0),
    })
}

/// The cursors of the street each layout leaves unbuilt: the click on its start and the cursor
/// at its end, world points. Kept in the sim world (not saved) for [`ghost`].
#[derive(Resource, Clone, Debug, Default)]
pub struct GridShotCursors(pub Vec<(Vec2, Vec2)>);

/// A layout's frame: local `x` right, `y` up on the screen of the `grid-shotN` camera (world
/// `-z`), turned by the layout's angle about its origin.
#[derive(Clone, Copy)]
struct Frame {
    o: Vec2,
    rot: Vec2,
}

impl Frame {
    fn new(n: usize) -> Self {
        let (o, deg) = origin(n);
        let a = deg.to_radians();
        Self {
            o,
            rot: Vec2::new(a.cos(), a.sin()),
        }
    }
    fn wv(&self, l: Vec2) -> Vec2 {
        let l = Vec2::new(l.x, -l.y);
        Vec2::new(
            l.x * self.rot.x - l.y * self.rot.y,
            l.x * self.rot.y + l.y * self.rot.x,
        )
    }
    fn w(&self, l: Vec2) -> Vec2 {
        self.o + self.wv(l)
    }
    fn l(&self, w: Vec2) -> Vec2 {
        let d = w - self.o;
        let l = Vec2::new(
            d.x * self.rot.x + d.y * self.rot.y,
            -d.x * self.rot.y + d.y * self.rot.x,
        );
        Vec2::new(l.x, -l.y)
    }
}

/// A built street: its ends where the nodes stand, local.
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
}

/// The player's hand on the road tool in one layout.
struct Hand<'a> {
    sim: &'a mut SimRunner,
    f: Frame,
    road: RoadDefId,
    hw: f32,
    tool: RoadTool,
    rejected: Vec<String>,
}

impl Hand<'_> {
    fn start(&mut self, mode: RoadMode) {
        self.tool = RoadTool {
            mode,
            road: Some(self.road),
            ..Default::default()
        };
    }

    /// A click at the local point `l`, as the tool takes it; a completed piece is built.
    fn click(&mut self, l: Vec2) -> Option<Piece> {
        let p = self.f.w(l);
        let before = self.tool.clone();
        let view = self.sim.view();
        // The tool's own cursor, as a click takes it (T-264).
        let q = cp_tools::resolve_cursor(&view, &self.tool, p, false).map_or(p, |c| c.cursor);
        let req = self.tool.click(q)?;
        let plan = chain_commit(Some(&view), &mut self.tool, before, req, false)?;
        let (a, b) = (
            cp_tools::anchor_pos(&view, &plan.pieces[0].from)?,
            cp_tools::anchor_pos(&view, &plan.pieces.last()?.to)?,
        );
        drop(view);
        self.sim.push_command(
            SimCommand::Net(NetCommand::BuildPlan { plan }),
            self.sim.tick(),
        );
        self.sim.tick_once();
        self.sim.tick_once();
        self.rejected.extend(
            self.sim
                .world_mut()
                .resource_mut::<Messages<CommandOutcome>>()
                .drain()
                .filter_map(|o| o.result.err().map(|e| e.to_string())),
        );
        let view = self.sim.view();
        let node_at = |q: Vec2| {
            view.net
                .nodes()
                .map(|(_, n)| to_xz(n.pos))
                .min_by(|x, y| x.distance(q).total_cmp(&y.distance(q)))
                .filter(|n| n.distance(q) < 16.0)
                .unwrap_or(q)
        };
        Some(Piece {
            a: self.f.l(node_at(a)),
            b: self.f.l(node_at(b)),
        })
    }

    /// The tick nearest the local point `nominal` on a street (the centreline station of a cross
    /// street of this road whose near edge stands on an 8-cell line of the street's cells,
    /// [`cp_zone::cell_lines`], T-265), seen on the `side` (local) of it; the nominal point if
    /// the tool offers none there.
    fn tick(&self, nominal: Vec2, side: Vec2) -> Mark {
        let (p, sw) = (self.f.w(nominal), self.f.wv(side));
        let view = self.sim.view();
        let db = view.content.as_deref().expect("content");
        let roads = cp_zone::road_zoning(db);
        let ids = view.net.segments().map(|(id, _)| id);
        let mut best: Option<(f32, Mark)> = None;
        for line in cp_zone::cell_lines(&view.net, &roads, ids) {
            let m = line.mark;
            if !m.major || m.end || m.v.dot(sw) < 0.9 {
                continue;
            }
            let axis = m.axis + m.onward * self.hw;
            let r = axis - p;
            if r.dot(m.v).abs() > 3.0 {
                continue;
            }
            let d = r.perp_dot(m.v).abs();
            if d <= self.hw + 3.0 && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, Mark { axis, ..m }));
            }
        }
        best.map_or(
            Mark {
                axis: p,
                v: sw,
                onward: Vec2::new(sw.y, -sw.x),
                major: true,
                end: false,
                arc: 0.0,
            },
            |(_, m)| m,
        )
    }

    /// The `k`-th tick station from a street edge at `edge` (§7.1, BUG-073): the centreline of
    /// the `k`-th cross street of this road, 8 cells between the street edges.
    fn station(&self, edge: f32, k: f32) -> f32 {
        edge + k * BLOCK + (k - 1.0) * 2.0 * self.hw + self.hw
    }

    /// Where the tool puts a first click at the local cursor `l` (the snap), local.
    fn snapped(&self, l: Vec2) -> Vec2 {
        let tool = RoadTool {
            road: Some(self.road),
            ..Default::default()
        };
        self.f
            .l(block_hover(&self.sim.view(), &tool, self.f.w(l), false).0)
    }

    /// Where the player clicks a tick: in the band of cells, 2.5 m off the road edge.
    fn at_tick(&self, m: Mark) -> Vec2 {
        self.f.l(m.axis + m.v * (self.hw + 2.5))
    }

    /// A straight street between two local cursor points.
    fn street(&mut self, from: Vec2, to: Vec2) -> Option<Piece> {
        self.start(RoadMode::Straight);
        self.click(from);
        self.click(to)
    }

    /// From `top` (on the y axis) down to a rounded corner at the origin and on to `right` (on
    /// the x axis), one chain. Returns the left and the bottom straights.
    fn rounded_l(&mut self, top: Vec2, right: Vec2) -> Option<(Piece, Piece)> {
        self.start(RoadMode::Straight);
        self.click(top);
        let left = self.click(Vec2::new(0.0, BEND_R))?;
        self.tool.mode = RoadMode::Curve;
        self.click(Vec2::ZERO);
        self.click(Vec2::new(BEND_R, 0.0))?;
        self.tool.mode = RoadMode::Straight;
        let bottom = self.click(right)?;
        Some((left, bottom))
    }
}

const X: Vec2 = Vec2::X;
const Y: Vec2 = Vec2::Y;

fn shot1(h: &mut Hand) -> Option<(Vec2, Vec2)> {
    let hw = h.hw;
    let (left, bottom) = h.rounded_l(Vec2::new(0.0, 200.0), Vec2::new(216.0, 0.0))?;
    let m = h.tick(Vec2::new(h.station(left.x() + hw, 2.0), bottom.y()), Y);
    let at = h.at_tick(m);
    let s = h.snapped(at);
    let right = h.street(at, Vec2::new(s.x, h.station(bottom.y() + hw, 1.0)))?;
    let m = h.tick(Vec2::new(left.x(), h.station(bottom.y() + hw, 1.0)), X);
    Some((h.at_tick(m), right.b))
}

fn shot2(h: &mut Hand) -> Option<(Vec2, Vec2)> {
    let (_, bottom) = h.rounded_l(Vec2::new(0.0, 160.0), Vec2::new(96.0, 0.0))?;
    Some((bottom.b, Vec2::new(bottom.b.x + 160.0, bottom.y())))
}

fn shot3(h: &mut Hand) -> Option<(Vec2, Vec2)> {
    let hw = h.hw;
    let v = h.street(Vec2::new(0.0, -40.0), Vec2::new(0.0, 200.0))?;
    let lower = h.street(Vec2::new(v.x() + hw + 2.5, 0.0), Vec2::new(200.0, 0.0))?;
    let m = h.tick(Vec2::new(v.x(), h.station(lower.y() + hw, 1.0)), X);
    let at = h.at_tick(m);
    let s = h.snapped(at);
    let upper = h.street(at, Vec2::new(lower.b.x, s.y))?;
    Some((upper.b, lower.b))
}

fn shot4(h: &mut Hand) -> Option<(Vec2, Vec2)> {
    let hw = h.hw;
    // The left street up into a rounded corner and the top street on to the right.
    let top_y = 150.0;
    h.start(RoadMode::Straight);
    h.click(Vec2::new(0.0, -40.0));
    let left = h.click(Vec2::new(0.0, top_y - BEND_R))?;
    h.tool.mode = RoadMode::Curve;
    h.click(Vec2::new(0.0, top_y));
    h.click(Vec2::new(BEND_R, top_y))?;
    h.tool.mode = RoadMode::Straight;
    let top = h.click(Vec2::new(hw + 2.0 * BLOCK + 40.0, top_y))?;
    // The bottom street off the left street's 8-cell tick below the top street, past the block.
    let m = h.tick(Vec2::new(left.x(), -h.station(-(top.y() - hw), 1.0)), X);
    let at = h.at_tick(m);
    let s = h.snapped(at);
    let bottom = h.street(at, Vec2::new(240.0, s.y))?;
    // Closing from the top street's 16-cell tick down along its guide to the bottom street.
    let m = h.tick(Vec2::new(h.station(left.x() + hw, 2.0), top.y()), -Y);
    let at = h.at_tick(m);
    let s = h.snapped(at);
    Some((at, Vec2::new(s.x, bottom.y())))
}

fn shot5(h: &mut Hand) -> Option<(Vec2, Vec2)> {
    let hw = h.hw;
    let (right_x, top_y) = (2.0 * (BLOCK + 2.0 * hw), 3.0 * BLOCK);
    h.start(RoadMode::Straight);
    h.click(Vec2::ZERO);
    let left = h.click(Vec2::new(0.0, top_y))?;
    let top = h.click(Vec2::new(right_x, top_y))?;
    let right = h.click(Vec2::new(right_x, top_y - 2.0 * BLOCK))?;
    let y = top.y() - hw - BLOCK - hw;
    Some((Vec2::new(left.x(), y), Vec2::new(right.x(), y)))
}

/// Builds the five layouts and keeps the cursors of their last streets; the clock is held.
/// Returns the rejected commands.
pub fn build_grid_shots(sim: &mut SimRunner) -> Vec<String> {
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let db = sim.world().resource::<cp_content::SimContent>().0.clone();
    let road = db.road_id(ROAD).expect("basic_2l");
    let mut rejected = Vec::new();
    let mut cursors = Vec::new();
    for n in 1..=5 {
        let mut h = Hand {
            sim: &mut *sim,
            f: Frame::new(n),
            road,
            hw: db.road(road).width / 2.0,
            tool: RoadTool::default(),
            rejected: Vec::new(),
        };
        let last = match n {
            1 => shot1(&mut h),
            2 => shot2(&mut h),
            3 => shot3(&mut h),
            4 => shot4(&mut h),
            _ => shot5(&mut h),
        };
        let f = h.f;
        rejected.append(&mut h.rejected);
        match last {
            Some((a, b)) => cursors.push((f.w(a), f.w(b))),
            None => {
                rejected.push(format!("grid-shot{n}: a street was not built"));
                cursors.push((f.o, f.o));
            }
        }
    }
    sim.world_mut().insert_resource(GridShotCursors(cursors));
    sim.world_mut()
        .insert_resource(cp_core::time::SimSpeed::Pause);
    rejected
}

/// The ghost of layout `n`'s last street: the tool's click on its start, the cursor at its end.
pub fn ghost(view: &SimView, cursors: &GridShotCursors, n: usize) -> Option<GhostDemo> {
    let (from, to) = *cursors.0.get(n.checked_sub(1)?)?;
    let db = view.content.as_deref()?;
    let tool = RoadTool {
        road: Some(db.road_id(ROAD)?),
        ..Default::default()
    };
    Some(GhostDemo {
        road: ROAD.to_string(),
        from: block_hover(view, &tool, from, false).0,
        to,
    })
}

/// `grid-shotN` camera: the shot number.
pub fn shot_of_cam(cam: &str) -> Option<usize> {
    cam.strip_prefix("grid-shot")?
        .parse()
        .ok()
        .filter(|n| (1..=5).contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The five layouts build through the tool without a rejected command, and each camera
    /// gets a ghost of its last street: started on the street it leaves from, a street long.
    #[test]
    fn the_five_layouts_build_and_hold_a_ghost_each() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let terrain = cp_terrain::Terrain::load(&root.join("assets/maps/flatlands")).unwrap();
        let sim = crate::build_sim(
            terrain,
            cp_core::SimLimits::desktop(),
            8.0,
            false,
            Some(GRID_SHOTS),
        );
        let streets = sim.view().net.segments().count();
        assert!(streets >= 5 * 3, "{streets} streets");
        let cursors = sim.world().resource::<GridShotCursors>().clone();
        let view = sim.view();
        for n in 1..=5 {
            let g = ghost(&view, &cursors, n).expect("a ghost");
            let on_net = view.net.segments().any(|(_, s)| {
                let c = &s.curve;
                (0..=64).any(|i| to_xz(c.point(i as f32 / 64.0)).distance(g.from) < 12.0)
            });
            assert!(on_net, "shot {n}: the ghost starts at a street (its edge)");
            assert!(g.from.distance(g.to) > 50.0, "shot {n}: {g:?}");
            assert_eq!(shot_of_cam(&format!("grid-shot{n}")), Some(n));
        }
        // No command of the scene was refused (the scene logs them; rebuilt here to count).
        let terrain = cp_terrain::Terrain::load(&root.join("assets/maps/flatlands")).unwrap();
        let mut sim = crate::build_sim(terrain, cp_core::SimLimits::desktop(), 8.0, false, None);
        assert_eq!(build_grid_shots(&mut sim), Vec::<String>::new());
    }
}
