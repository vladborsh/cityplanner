//! Track and tunnels (T-161): old saves keep their road types, and a metro tunnel under a
//! built-up area leaves zones and buildings alone and survives a save.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_building::BuildingGrid;
use cp_content::ContentDb;
use cp_core::{SimLimits, Tick};
use cp_net::{Elevation, ElevationMode, NetCommand, NetGraph, RoadPath};
use cp_save::*;
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_zone::ZoneGrid;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

fn runner(content: &Arc<ContentDb>) -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(content));
    sim
}

fn fixture() -> Vec<u8> {
    std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v30-basic.sav")).unwrap()
}

#[test]
fn an_old_save_keeps_its_road_types_when_the_rail_types_join_the_content() {
    let content = content();
    let bytes = fixture();
    let (h, _) = read_header(&bytes).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    let old_names = l.data.def_names.roads.clone();
    assert!(old_names.len() < content.roads().count(), "new road types");
    let before: Vec<_> = l
        .data
        .net
        .segments()
        .map(|(id, s)| (id, old_names[s.road.index()].clone()))
        .collect();
    assert!(!before.is_empty());
    let warnings = l.data.remap_defs(&content);
    assert!(warnings.is_empty(), "{warnings:?}");
    for (id, name) in before {
        let seg = l.data.net.segment(id).unwrap();
        assert_eq!(content.road(seg.road).id, name);
    }
}

#[test]
fn a_metro_tunnel_under_the_city_changes_no_zone_and_no_building_and_saves() {
    let content = content();
    let hash = content.content_hash();
    let mut a = runner(&content);
    load_into_runner(&mut a, &fixture(), hash).unwrap();
    let pos: Vec2 = a
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .next()
        .expect("the fixture city has buildings")
        .1
        .pos;
    // The same city for the same ticks without the tunnel: what the sim does by itself.
    let mut control = runner(&content);
    load_into_runner(&mut control, &fixture(), hash).unwrap();
    for _ in 0..5 {
        control.tick_once();
    }
    let buildings = control.world().resource::<BuildingGrid>().clone();
    let metro = content.road_id("metro_track").unwrap();
    let depth = content.road(metro).tunnel.as_ref().unwrap().depth;
    a.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: metro,
            path: RoadPath::Straight {
                from: pos - Vec2::new(60.0, 0.0),
                to: pos + Vec2::new(60.0, 0.0),
            },
            elevation: ElevationMode::Tunnel(depth),
        }),
        Tick(a.tick().0 + 1),
    );
    for _ in 0..5 {
        a.tick_once();
    }
    let tunnels = a
        .world()
        .resource::<NetGraph>()
        .segments()
        .filter(|(_, s)| s.elevation == Elevation::Tunnel)
        .count();
    assert!(tunnels >= 2, "the tunnel was built: {tunnels}");
    // The grid keeps a fingerprint per segment (tunnels too, as non-ground); the blocks and
    // their cells are what zoning is made of.
    let blocks = |w: &bevy_ecs::world::World| {
        let z = w.resource::<ZoneGrid>();
        (
            format!("{:?}", z.blocks().collect::<Vec<_>>()),
            z.block_count(),
            z.valid_cells(),
        )
    };
    assert_eq!(
        blocks(a.world()),
        blocks(control.world()),
        "zones untouched"
    );
    assert!(
        a.world().resource::<BuildingGrid>() == &buildings,
        "buildings untouched"
    );

    // Saved and loaded, the city goes on identically.
    let bytes = save_runner(&a, hash, "flatlands").unwrap();
    let mut b = runner(&content);
    load_into_runner(&mut b, &bytes, hash).unwrap();
    assert_eq!(a.checksum(), b.checksum());
    for _ in 0..50 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum());
}
