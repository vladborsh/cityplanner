//! The city of the T-275 acceptance (`architecture/system-design.md` §6.8, model v8), built the
//! way the player builds it: every street through the road tool's own code
//! ([`RoadTool::click`], [`chain_commit`]: the sections, the joins, the snaps). Roads are whole
//! cells wide and a street ends a whole number of cells from where it starts (T-278; the sections
//! of 10 cells are guides).
//!
//! * shot 1: a city of 3 x 3 blocks, streets 2 sections (20 cells) apart one way and 1 section
//!   (10 cells) the other, long streets first (the top street, the left street, the long streets
//!   off its section marks), then the cross streets down across them; the camera `grid-shot1`
//!   holds the ghost of the last cross street, from the top street's end down to the bottom
//!   street's ([`ghost`]).
//! * shot 2 (T-268): the same city with its top-left corner drawn as a curve (the curve mode's
//!   three clicks: from the top street's first cross street, the bend at the corner, down to the
//!   first long street); the left street goes on from the curve's end.
//! * shot 3 (T-278): a city of mixed blocks, 18 x 8, 5 x 8 and 8 x 8 cells over 18 x 3, 5 x 3
//!   and 8 x 3; the ghost of its last cross street shows a tick at every cell, the 10th bold
//!   (`grid-shot3-ticks` close over it).
//!
//! (Until T-275 the scene held the layouts of the user's BUG-073 shots on the 8 m step; the
//! screenshots `docs/screenshots/t255-grid-shotN-*.png` are of those.)

use bevy::prelude::*;
use bevy_ecs::message::Messages;
use cp_content::RoadDefId;
use cp_core::CommandOutcome;
use cp_net::NetCommand;
use cp_sim::{SimCommand, SimRunner, SimView};
use cp_tools::road::chain_commit;
use cp_tools::{GhostDemo, RoadMode, RoadTool};

/// The layouts of the scene.
const LAYOUTS: usize = 3;

/// `--scene grid-shots`.
pub const GRID_SHOTS: &str = "grid-shots";

/// The road the player draws with.
const ROAD: &str = "basic_2l";

/// Where layout `n` stands (its first junction), in the world.
pub fn origin(n: usize) -> Vec2 {
    let c = cp_terrain::MAP_SIZE / 2.0;
    Vec2::new(c - 450.0 + (n as f32 - 1.0) * 900.0, c - 1800.0)
}

/// Where the camera `grid-shotN` looks: the middle of the layout, in the world.
pub fn focus(n: usize) -> Vec2 {
    origin(n)
        + if n == 3 {
            Vec2::new(148.0, 60.0)
        } else {
            Vec2::new(240.0, 120.0)
        }
}

/// The cursors of the street each layout leaves unbuilt: the click on its start and the cursor
/// at its end, world points. Kept in the sim world (not saved) for [`ghost`].
#[derive(Resource, Clone, Debug, Default)]
pub struct GridShotCursors(pub Vec<(Vec2, Vec2)>);

/// The player's hand on the road tool.
pub(crate) struct Hand<'a> {
    sim: &'a mut SimRunner,
    o: Vec2,
    road: RoadDefId,
    tool: RoadTool,
    pub(crate) rejected: Vec<String>,
}

impl<'a> Hand<'a> {
    /// A hand drawing `road` with points given from `o` (world).
    pub(crate) fn new(sim: &'a mut SimRunner, o: Vec2, road: RoadDefId) -> Self {
        Self {
            sim,
            o,
            road,
            tool: RoadTool::default(),
            rejected: Vec::new(),
        }
    }

    /// A click at the point `l` (from the origin, world axes), as the tool takes it; a completed
    /// piece is built.
    fn click(&mut self, l: Vec2) -> Option<()> {
        let p = self.o + l;
        let before = self.tool.clone();
        let view = self.sim.view();
        // The tool's own cursor, as a click takes it (T-264).
        let q = cp_tools::resolve_cursor(&view, &self.tool, p, false).map_or(p, |c| c.cursor);
        let req = self.tool.click(q)?;
        let plan = chain_commit(Some(&view), &mut self.tool, before, req, false)?;
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
        Some(())
    }

    /// A straight street between two cursor points.
    pub(crate) fn street(&mut self, from: Vec2, to: Vec2) -> Option<()> {
        self.tool = RoadTool {
            mode: RoadMode::Straight,
            road: Some(self.road),
            ..Default::default()
        };
        self.click(from);
        self.click(to)
    }

    /// A curve: its start, its bend and its end, cursor points.
    fn curve(&mut self, from: Vec2, bend: Vec2, to: Vec2) -> Option<()> {
        self.tool = RoadTool {
            mode: RoadMode::Curve,
            road: Some(self.road),
            ..Default::default()
        };
        self.click(from);
        self.click(bend);
        self.click(to)
    }
}

fn shot1(h: &mut Hand) -> Option<(Vec2, Vec2)> {
    let section = 10.0 * cp_net::build::ZONE_CELL;
    let x = |i: f32| i * 2.0 * section;
    let y = |j: f32| j * section;
    h.street(Vec2::new(x(0.0), y(0.0)), Vec2::new(x(3.0), y(0.0)))?;
    h.street(Vec2::new(x(0.0), y(0.0)), Vec2::new(x(0.0), y(3.0)))?;
    for j in 1..=3 {
        let j = j as f32;
        h.street(Vec2::new(x(0.0), y(j)), Vec2::new(x(3.0), y(j)))?;
    }
    for i in 1..=2 {
        let i = i as f32;
        h.street(Vec2::new(x(i), y(0.0)), Vec2::new(x(i), y(3.0)))?;
    }
    Some((Vec2::new(x(3.0), y(0.0)), Vec2::new(x(3.0), y(3.0))))
}

fn shot2(h: &mut Hand) -> Option<(Vec2, Vec2)> {
    let section = 10.0 * cp_net::build::ZONE_CELL;
    let x = |i: f32| i * 2.0 * section;
    let y = |j: f32| j * section;
    h.street(Vec2::new(x(1.0), y(0.0)), Vec2::new(x(3.0), y(0.0)))?;
    h.curve(
        Vec2::new(x(1.0), y(0.0)),
        Vec2::new(x(0.0), y(0.0)),
        Vec2::new(x(0.0), y(1.0)),
    )?;
    h.street(Vec2::new(x(0.0), y(1.0)), Vec2::new(x(0.0), y(3.0)))?;
    for j in 1..=3 {
        let j = j as f32;
        h.street(Vec2::new(x(0.0), y(j)), Vec2::new(x(3.0), y(j)))?;
    }
    for i in 1..=2 {
        let i = i as f32;
        h.street(Vec2::new(x(i), y(0.0)), Vec2::new(x(i), y(3.0)))?;
    }
    Some((Vec2::new(x(3.0), y(0.0)), Vec2::new(x(3.0), y(3.0))))
}

/// The centres of the mixed city's streets (cells from the first): blocks of `blocks` cells
/// between 2-cell roads.
fn centres(blocks: &[f32]) -> Vec<f32> {
    let cell = cp_net::build::ZONE_CELL;
    let mut out = vec![0.0];
    for b in blocks {
        out.push(out.last().unwrap() + (b + 2.0) * cell);
    }
    out
}

fn shot3(h: &mut Hand) -> Option<(Vec2, Vec2)> {
    let (xs, ys) = (centres(&[18.0, 5.0, 8.0]), centres(&[8.0, 3.0]));
    let v = Vec2::new;
    let (x1, y1) = (xs[3], ys[2]);
    h.street(v(0.0, 0.0), v(x1, 0.0))?;
    h.street(v(0.0, 0.0), v(0.0, y1))?;
    for &y in &ys[1..] {
        h.street(v(0.0, y), v(x1, y))?;
    }
    for &x in &xs[1..3] {
        h.street(v(x, 0.0), v(x, y1))?;
    }
    Some((v(x1, 0.0), v(x1, y1)))
}

/// Builds the layouts and keeps the cursors of their last streets; the clock is held. Returns
/// the rejected commands.
pub fn build_grid_shots(sim: &mut SimRunner) -> Vec<String> {
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let db = sim.world().resource::<cp_content::SimContent>().0.clone();
    let road = db.road_id(ROAD).expect("basic_2l");
    let mut rejected = Vec::new();
    let mut cursors = Vec::new();
    for n in 1..=LAYOUTS {
        let mut h = Hand {
            sim: &mut *sim,
            o: origin(n),
            road,
            tool: RoadTool::default(),
            rejected: Vec::new(),
        };
        let last = match n {
            1 => shot1(&mut h),
            2 => shot2(&mut h),
            _ => shot3(&mut h),
        };
        let o = h.o;
        rejected.append(&mut h.rejected);
        match last {
            Some((a, b)) => cursors.push((o + a, o + b)),
            None => {
                rejected.push(format!("grid-shot{n}: a street was not built"));
                cursors.push((o, o));
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
    view.content.as_deref()?.road_id(ROAD)?;
    Some(GhostDemo {
        road: ROAD.to_string(),
        from,
        to,
    })
}

/// The ends of layout `n`'s last street, the ghost (world), as its builder aims them.
fn last_street(n: usize) -> (Vec2, Vec2) {
    let o = origin(n);
    let (xs, ys) = if n == 3 {
        (centres(&[18.0, 5.0, 8.0]), centres(&[8.0, 3.0]))
    } else {
        (vec![0.0, 0.0, 0.0, 480.0], vec![0.0, 0.0, 0.0, 240.0])
    };
    let x = xs[3];
    (o + Vec2::new(x, 0.0), o + Vec2::new(x, *ys.last().unwrap()))
}

/// `grid-shotN` camera (also `grid-shotN-top` and `grid-shotN-corner`): the shot number.
pub fn shot_of_cam(cam: &str) -> Option<usize> {
    let rest = cam.strip_prefix("grid-shot")?;
    let n = rest
        .strip_suffix("-top")
        .or_else(|| rest.strip_suffix("-corner"))
        .or_else(|| rest.strip_suffix("-ticks"))
        .unwrap_or(rest);
    n.parse().ok().filter(|n| (1..=LAYOUTS).contains(n))
}

/// The cameras of the scene, `(position, look_at)`: `grid-shotN` obliquely from the south over
/// the whole city, `grid-shotN-top` straight down on it (the cells can be counted), and
/// `grid-shotN-corner` close over the L corner at its top-left (the cells inside it), and
/// `grid-shotN-ticks` close over the ghost of its last street (its cell ticks).
pub fn camera(name: &str) -> Option<(Vec3, Vec3)> {
    let n = shot_of_cam(name)?;
    let (f, o) = (focus(n), origin(n));
    Some(if name.ends_with("-top") {
        // A hair off the vertical, so the view has an up direction.
        (Vec3::new(f.x, 430.0, f.y + 1.0), Vec3::new(f.x, 0.0, f.y))
    } else if name.ends_with("-ticks") {
        let (a, b) = last_street(n);
        let c = (a + b) * 0.5;
        (
            Vec3::new(c.x - 45.0, 75.0, c.y + 25.0),
            Vec3::new(c.x, 0.0, c.y),
        )
    } else if name.ends_with("-corner") {
        let c = o + Vec2::splat(24.0);
        (Vec3::new(c.x, 95.0, c.y + 45.0), Vec3::new(c.x, 0.0, c.y))
    } else {
        (Vec3::new(f.x, 520.0, f.y + 260.0), Vec3::new(f.x, 0.0, f.y))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_core::curve::to_xz;

    /// The layouts build through the tool without a rejected command, and each camera gets a
    /// ghost of its last street: started on the street it leaves from, a street long.
    #[test]
    fn the_layouts_build_and_hold_a_ghost_each() {
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
        assert!(streets >= LAYOUTS * 3, "{streets} streets");
        let cursors = sim.world().resource::<GridShotCursors>().clone();
        let view = sim.view();
        for n in 1..=LAYOUTS {
            let g = ghost(&view, &cursors, n).expect("a ghost");
            let on_net = view.net.segments().any(|(_, s)| {
                let c = &s.curve;
                (0..=64).any(|i| to_xz(c.point(i as f32 / 64.0)).distance(g.from) < 12.0)
            });
            assert!(on_net, "shot {n}: the ghost starts at a street");
            assert!(g.from.distance(g.to) > 50.0, "shot {n}: {g:?}");
            assert_eq!(shot_of_cam(&format!("grid-shot{n}")), Some(n));
            assert_eq!(shot_of_cam(&format!("grid-shot{n}-top")), Some(n));
            assert!(camera(&format!("grid-shot{n}-corner")).is_some());
        }
        // No command of the scene was refused (the scene logs them; rebuilt here to count).
        let terrain = cp_terrain::Terrain::load(&root.join("assets/maps/flatlands")).unwrap();
        let mut sim = crate::build_sim(terrain, cp_core::SimLimits::desktop(), 8.0, false, None);
        assert_eq!(build_grid_shots(&mut sim), Vec::<String>::new());
    }
}
