//! T-166: the entry railway of a new game and the train station on it. The railway runs from
//! the map edge (its own `Outside` node) to a dead end in the start tile; a station whose
//! platform is joined to it is connected to the outside (derived), one without a track is not.

use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingCommand, BuildingGrid, BuildingId};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::treasury::Treasury;
use cp_core::{CommandOutcome, Cost};
use cp_net::{Elevation, EntryReach, NetGraph, NodeKind, RailReach, is_rail_segment};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_transit::{TransitNet, station_reaches_outside};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn db() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(format!("{ROOT}/../../assets/content")).unwrap())
}

fn map_dir(name: &str) -> std::path::PathBuf {
    Path::new(ROOT).join("../../assets/maps").join(name)
}

fn new_game(name: &str) -> (SimRunner, Arc<Terrain>) {
    let terrain = Arc::new(Terrain::load(&map_dir(name)).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::clone(&terrain));
    sim.set_content(db());
    sim.new_game().unwrap();
    sim.tick_once();
    (sim, terrain)
}

/// Both maps get a railway from a node on the map edge to a dead end in the start tile.
#[test]
fn a_new_game_builds_the_railway_from_the_edge_to_a_dead_end_in_the_start_tile() {
    for name in ["flatlands", "river_valley"] {
        let (sim, terrain) = new_game(name);
        let rail = terrain.meta().entry_rail.clone().expect(name);
        let net = sim.world().resource::<NetGraph>();
        let outside: Vec<_> = net
            .nodes()
            .filter(|(_, n)| n.kind == NodeKind::Outside && cp_net::node_is_rail(net, n))
            .collect();
        assert_eq!(outside.len(), 1, "{name}: one rail Outside node");
        let p = outside[0].1.pos;
        let edge = rail.points()[0];
        assert!(Vec2::new(p.x, p.z).distance(edge) < 0.5, "{name}");
        // The track ends in a dead end at the end of the path.
        let end = rail.dead_end();
        let dead = net
            .nodes()
            .find(|(_, n)| {
                n.kind == NodeKind::End && Vec2::new(n.pos.x, n.pos.z).distance(end) < 0.5
            })
            .unwrap_or_else(|| panic!("{name}: no dead end at {end}"));
        let (tx, tz) = terrain.meta().start_tile;
        let side = cp_terrain::TILE_SIZE;
        assert!(
            end.x > tx as f32 * side
                && end.x < (tx + 1) as f32 * side
                && end.y > tz as f32 * side
                && end.y < (tz + 1) as f32 * side
        );
        // The whole track is rail, and the rails reach the dead end from the edge.
        let reach = sim.world().resource::<RailReach>();
        assert!(reach.is_active());
        let segs = &net.node(dead.0).unwrap().segments;
        assert!(
            segs.iter()
                .all(|&s| is_rail_segment(net, s) && reach.reaches(s))
        );
        // The highway's reach stays on the roads.
        let roads = sim.world().resource::<EntryReach>();
        assert!(roads.is_active());
        assert!(segs.iter().all(|&s| !roads.reaches(s)), "{name}");
    }
}

/// The railway is not a way for new households, and the map hash does not know it.
#[test]
fn the_map_hash_does_not_change_and_a_map_without_a_railway_builds_none() {
    let dir = map_dir("flatlands");
    let ron = std::fs::read_to_string(dir.join("map.ron")).unwrap();
    let (h, w) = (
        std::fs::read(dir.join("height.png")).unwrap(),
        std::fs::read(dir.join("water.png")).unwrap(),
    );
    let cut = ron.find("    entry_rail:").unwrap();
    let without = format!("{}\n)", &ron[..cut]);
    let a = Terrain::from_bytes(&ron, &h, &w).unwrap();
    let b = Terrain::from_bytes(&without, &h, &w).unwrap();
    assert!(a.meta().entry_rail.is_some() && b.meta().entry_rail.is_none());
    assert_eq!(a.map_hash(), b.map_hash());

    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::new(b));
    sim.set_content(db());
    sim.new_game().unwrap();
    sim.tick_once();
    assert!(!sim.world().resource::<RailReach>().is_active());
}

fn place_station(sim: &mut SimRunner, def: cp_content::BuildingDefId, pos: Vec2) -> BuildingId {
    sim.push_command(
        SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 }),
        sim.tick(),
    );
    sim.tick_once();
    let outcomes = sim
        .world_mut()
        .resource_mut::<bevy_ecs::message::Messages<CommandOutcome>>()
        .drain()
        .collect::<Vec<_>>();
    let last = outcomes.last().expect("an outcome");
    assert!(last.result.is_ok(), "{:?}", last.result);
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .map(|(id, _)| id)
        .last()
        .unwrap()
}

/// A station whose platform joins the dead end is connected to the outside; one far from any
/// track is not.
#[test]
fn a_station_joined_to_the_railway_reaches_the_outside_and_one_without_track_does_not() {
    let db = db();
    let (mut sim, terrain) = new_game("flatlands");
    sim.world_mut().insert_resource(TileOwnership::only((2, 2)));
    sim.world_mut().resource_mut::<Treasury>().balance = Cost(1_000_000_000);
    let end = terrain.meta().entry_rail.as_ref().unwrap().dead_end();
    // A street beside the track gives the station its entrance.
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let y = |x: f32, z: f32| Vec3::new(x, terrain.height(Vec2::new(x, z)), z);
    for (x0, x1, z) in [
        (end.x - 150.0, end.x - 30.0, end.y + 22.0),
        (end.x - 1400.0, end.x - 1300.0, end.y - 678.0),
    ] {
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(y(x0, z), y(x1, z)),
                Elevation::Ground,
            )
            .unwrap();
    }
    sim.tick_once();
    let station = db.building_id("train_station").unwrap();
    // Its platform (128 m along x) ends on the dead end of the track.
    let joined = place_station(&mut sim, station, Vec2::new(end.x - 64.0, end.y));
    // Another one in the open, not touching any track.
    let alone = place_station(&mut sim, station, Vec2::new(end.x - 1350.0, end.y - 700.0));
    sim.tick_once();
    let transit = sim.world().resource::<TransitNet>();
    let rail = sim.world().resource::<RailReach>();
    assert!(station_reaches_outside(transit, rail, joined));
    assert!(!station_reaches_outside(transit, rail, alone));
}
