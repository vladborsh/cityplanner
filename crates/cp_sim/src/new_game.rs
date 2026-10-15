//! What a new game starts with (`architecture/system-design.md` §5.1): the entry highway from
//! the map edge into the start tile. Only [`SimRunner::new_game`] calls it; loading a save,
//! scenes, benches and tests start from an empty network and keep their checksums.

use bevy_ecs::world::World;
use bevy_math::Vec3;
use cp_content::{RoadClass, RoadDefId, SimContent};
use cp_net::{Elevation, NetGraph, NodeKind, RoadPath, build_unowned};
use cp_terrain::SimTerrain;

use crate::runner::SimRunner;

impl SimRunner {
    /// Starts a new game on the attached map: builds its entry highway. Needs the terrain and
    /// the content; a map without `entry_road` (built in code) gets nothing. Call once, before
    /// the first tick.
    pub fn new_game(&mut self) -> Result<(), String> {
        build_entry_road(self.world_mut())
    }
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
    for (what, road, pts) in [
        ("inbound highway", highway, inbound),
        ("outbound highway", highway, outbound),
        ("stub", stub, vec![e.junction(), e.stub_end()]),
    ] {
        let path = if pts.len() == 2 {
            RoadPath::Straight {
                from: pts[0],
                to: pts[1],
            }
        } else {
            RoadPath::Free(pts)
        };
        build_unowned(world, road, path).map_err(|err| format!("entry road, {what}: {err}"))?;
    }
    Ok(())
}
