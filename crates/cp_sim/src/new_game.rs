//! What a new game starts with (`architecture/system-design.md` §5.1): the entry highway from
//! the map edge and the entry railway into the start tile. Only [`SimRunner::new_game`] calls it; loading a save,
//! scenes, benches and tests start from an empty network and keep their checksums.

use bevy_ecs::world::World;
use bevy_math::{Vec2, Vec3};
use cp_content::{RoadClass, RoadDefId, SimContent};
use cp_net::{Elevation, NetGraph, NodeKind, RoadPath, build_unowned};
use cp_terrain::SimTerrain;

use crate::runner::SimRunner;

impl SimRunner {
    /// Starts a new game on the attached map: builds its entry highway. Needs the terrain and
    /// the content; a map without `entry_road` (built in code) gets nothing. Call once, before
    /// the first tick.
    pub fn new_game(&mut self) -> Result<(), String> {
        self.world_mut().insert_resource(crate::Progress::START);
        // The treasury of a new game comes from the balance (BUG-061).
        let start = self
            .world()
            .get_resource::<SimContent>()
            .map(|c| cp_core::Cost::from_units(c.0.balance().start_money));
        if let Some(start) = start {
            self.world_mut().resource_mut::<cp_core::Treasury>().balance = start;
        }
        crate::progress::refresh_unlocks(self.world_mut());
        fill_deposits(self.world_mut());
        build_entry_road(self.world_mut())?;
        build_entry_rail(self.world_mut())
    }
}

/// Fills the ore and oil deposits from the attached map's resource map: `richness ×
/// balance.deposit_per_cell` per cell (T-205). A new game does it at the start; a save from
/// before v41 gets it on load. Needs the terrain and the content; without them nothing changes.
pub fn fill_deposits(world: &mut World) {
    let (Some(terrain), Some(content)) = (
        world.get_resource::<SimTerrain>().map(|t| t.0.clone()),
        world
            .get_resource::<cp_content::SimContent>()
            .map(|c| c.0.clone()),
    ) else {
        return;
    };
    world.insert_resource(cp_terrain::ResourceDeposits::from_map(
        terrain.resources(),
        content.balance().deposit_per_cell,
    ));
}

fn road_id(content: &cp_content::ContentDb, name: &str, what: &str) -> Result<RoadDefId, String> {
    content
        .road_id(name)
        .ok_or_else(|| format!("entry road: {what} road type `{name}` does not exist"))
}

/// Builds the entry highway of the attached map into the world's network (no ownership check,
/// no charge). Errors name the road piece that failed and why.
pub fn build_entry_road(world: &mut World) -> Result<(), String> {
    let content = world
        .get_resource::<SimContent>()
        .map(|c| c.0.clone())
        .ok_or("entry road: no content in the sim")?;
    let terrain = world
        .get_resource::<SimTerrain>()
        .map(|t| t.0.clone())
        .ok_or("entry road: no terrain in the sim")?;
    let Some(e) = terrain.meta().entry_road.clone() else {
        return Ok(());
    };
    let highway = road_id(&content, &e.highway, "highway")?;
    if content.road(highway).class != RoadClass::Highway {
        return Err(format!(
            "entry road: `{}` is not a Highway-class road",
            e.highway
        ));
    }
    let stub = road_id(&content, &e.stub, "stub")?;
    let (inbound, outbound) = e.carriageways();

    // The two ends at the map edge are Outside nodes; the pieces snap onto them.
    for end in [inbound[0], outbound[outbound.len() - 1]] {
        let y = terrain.height(end);
        world.resource_mut::<NetGraph>().add_node(
            Vec3::new(end.x, y, end.y),
            NodeKind::Outside,
            Elevation::Ground,
        );
    }
    let (inbound_run, inbound_merge) = smooth_merge(inbound, false);
    let (outbound_run, outbound_merge) = smooth_merge(outbound, true);
    for (what, road, path) in [
        ("inbound highway", highway, polyline(inbound_run)),
        ("inbound merge", highway, inbound_merge),
        ("outbound merge", highway, outbound_merge),
        ("outbound highway", highway, polyline(outbound_run)),
        ("stub", stub, polyline(vec![e.junction(), e.stub_end()])),
    ] {
        build_unowned(world, road, path).map_err(|err| format!("entry road, {what}: {err}"))?;
    }
    Ok(())
}

fn polyline(pts: Vec<Vec2>) -> RoadPath {
    if pts.len() == 2 {
        RoadPath::Straight {
            from: pts[0],
            to: pts[1],
        }
    } else {
        RoadPath::Free(pts)
    }
}

/// Longest straight piece of a carriageway's last run that the merge curve takes, metres.
const MERGE_LEAD: f32 = 40.0;

/// A carriageway (in travel direction; `outbound` starts at the junction, else it ends there)
/// split into its run along the centre line and the merge into the junction as one quadratic
/// curve (BUG-122): the curve leaves the run [`MERGE_LEAD`] metres (at most half its last leg)
/// before the corner where the polyline turned into the junction, with that corner as its
/// control point, so the carriageway bends smoothly instead of kinking there and still meets
/// the junction along the same leg.
fn smooth_merge(mut pts: Vec<Vec2>, outbound: bool) -> (Vec<Vec2>, RoadPath) {
    if outbound {
        pts.reverse();
    }
    let n = pts.len();
    let (j, k, prev) = (pts[n - 1], pts[n - 2], pts[n - 3]);
    let a = k + (prev - k).normalize_or_zero() * MERGE_LEAD.min(prev.distance(k) / 2.0);
    let mut run = pts[..n - 1].to_vec();
    run[n - 2] = a;
    let merge = if outbound {
        run.reverse();
        RoadPath::Curve {
            from: j,
            control: k,
            to: a,
        }
    } else {
        RoadPath::Curve {
            from: a,
            control: k,
            to: j,
        }
    };
    (run, merge)
}

/// Builds the entry railway of the attached map (T-166): an `Outside` node at the map edge and
/// the track from it to its dead end in the start tile. A map without `entry_rail` gets nothing.
pub fn build_entry_rail(world: &mut World) -> Result<(), String> {
    let content = world
        .get_resource::<SimContent>()
        .map(|c| c.0.clone())
        .ok_or("entry rail: no content in the sim")?;
    let terrain = world
        .get_resource::<SimTerrain>()
        .map(|t| t.0.clone())
        .ok_or("entry rail: no terrain in the sim")?;
    let Some(e) = terrain.meta().entry_rail.clone() else {
        return Ok(());
    };
    let road = content
        .road_id(&e.road)
        .ok_or_else(|| format!("entry rail: road type `{}` does not exist", e.road))?;
    if content.road(road).class != RoadClass::Rail {
        return Err(format!("entry rail: `{}` is not a Rail-class road", e.road));
    }
    let pts = e.points();
    let edge = pts[0];
    let y = terrain.height(edge);
    world.resource_mut::<NetGraph>().add_node(
        Vec3::new(edge.x, y, edge.y),
        NodeKind::Outside,
        Elevation::Ground,
    );
    let path = if pts.len() == 2 {
        RoadPath::Straight {
            from: pts[0],
            to: pts[1],
        }
    } else {
        RoadPath::Free(pts)
    };
    build_unowned(world, road, path).map_err(|err| format!("entry rail: {err}"))?;
    Ok(())
}
