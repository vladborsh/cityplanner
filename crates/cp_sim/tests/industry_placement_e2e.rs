//! T-208: the chain buildings stand only in an industrial area of their resource, with a Main
//! building, at the area level, and extractors on rich enough ground.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::BuildingCommand;
use cp_content::{CargoKind, ContentDb, ResourceKind};
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandOutcome};
use cp_district::{AreaId, AreaKind, DistrictCommand};
use cp_net::{Elevation, NetGraph};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{GRID, MapMeta, RES_GRID, ResourceMap, Terrain, TileOwnership};
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

/// Flat dry ground; forest and ore are rich in the columns left of x = 3136 m (pixels < 49),
/// nothing to the right.
fn terrain() -> Terrain {
    let meta = MapMeta {
        name: "flat".into(),
        sea_level: 0.0,
        start_tile: (1, 1),
        external_connections: vec![],
        entry_road: None,
        entry_rail: None,
    };
    let px = (0..RES_GRID * RES_GRID)
        .map(|i| {
            if i % RES_GRID < 49 {
                [0, 255, 255, 0]
            } else {
                [0; 4]
            }
        })
        .collect();
    Terrain::from_samples(meta, &vec![1000; GRID * GRID], &vec![0; GRID * GRID])
        .unwrap()
        .with_resources(ResourceMap::from_pixels(px).unwrap())
}

/// One 200 m road at z = 3000 on an owned tile, with the flat terrain.
fn world(c: &Arc<ContentDb>) -> SimRunner {
    let road = c.road_id("basic_2l").unwrap();
    let def = c.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(c));
    sim.set_terrain(Arc::new(terrain()));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3200.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance = cp_core::Cost::from_units(5.0e6);
    sim.tick_once();
    sim
}

fn run(sim: &mut SimRunner, cmd: SimCommand) -> Result<(), String> {
    sim.push_command(cmd, sim.tick());
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    match out.last().unwrap().result.clone() {
        Ok(_) => Ok(()),
        Err(CommandError::Rejected { reason, .. }) => Err(reason),
        Err(e) => panic!("{e:?}"),
    }
}

fn place(sim: &mut SimRunner, c: &ContentDb, id: &str, pos: Vec2) -> Result<(), String> {
    let def = c.building_id(id).unwrap();
    run(
        sim,
        SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 }),
    )
}

fn spot(sim: &SimRunner, col: u16) -> Vec2 {
    let zones = sim.world().resource::<ZoneGrid>();
    let (id, _) = zones.blocks().next().unwrap();
    zones
        .cell_footprint(ZoneCellRef {
            block: id,
            cell: col * 4 + 1,
        })
        .unwrap()
        .c
}

/// Creates an area of `kind` and paints the cells around the road (x 2880..3200, z 2816..3200).
fn area(sim: &mut SimRunner, kind: AreaKind) -> AreaId {
    run(
        sim,
        SimCommand::District(DistrictCommand::CreateArea {
            kind,
            name: "Area".into(),
        }),
    )
    .unwrap();
    let id = AreaId(sim.view().areas.iter().last().unwrap().0.0);
    let cells = (90..100)
        .flat_map(|c| (88..100).map(move |r| (c, r)))
        .collect();
    run(
        sim,
        SimCommand::District(DistrictCommand::PaintArea { cells, area: id }),
    )
    .unwrap();
    id
}

#[test]
fn a_chain_building_needs_its_area_a_main_building_the_level_and_rich_ground() {
    let c = content();
    let mut sim = world(&c);
    let (near_main, near_ext) = (spot(&sim, 1), spot(&sim, 7));

    // No area at all.
    let e = place(&mut sim, &c, "forest_extractor_s", near_ext).unwrap_err();
    assert!(e.contains("Forestry industry area"), "{e}");

    // An area of another resource.
    area(&mut sim, AreaKind::Industry(ResourceKind::Farming));
    let e = place(&mut sim, &c, "forest_extractor_s", near_ext).unwrap_err();
    assert!(e.contains("Forestry industry area"), "{e}");
    let e = place(&mut sim, &c, "forest_main", near_main).unwrap_err();
    assert!(e.contains("Forestry industry area"), "{e}");
    // The Farming main stands there; it has no richness rule.
    place(&mut sim, &c, "farm_main", near_main).unwrap();
}

#[test]
fn forest_chain_rules_in_a_forestry_area() {
    let c = content();
    let mut sim = world(&c);
    area(&mut sim, AreaKind::Industry(ResourceKind::Forestry));
    let (main_at, ext_at, poor_at) = (spot(&sim, 1), spot(&sim, 7), spot(&sim, 20));
    let (spot4, spot12) = (spot(&sim, 15), spot(&sim, 10));

    // In the area but no Main yet.
    let e = place(&mut sim, &c, "forest_extractor_s", ext_at).unwrap_err();
    assert!(e.contains("no main building"), "{e}");
    let e = place(&mut sim, &c, "forest_warehouse", ext_at).unwrap_err();
    assert!(e.contains("no main building"), "{e}");

    place(&mut sim, &c, "forest_main", main_at).unwrap();
    // A second Main is refused.
    let e = place(&mut sim, &c, "forest_main", spot12).unwrap_err();
    assert!(e.contains("main building already"), "{e}");

    // On the forest: fine. On bare ground: too poor.
    place(&mut sim, &c, "forest_extractor_s", ext_at).unwrap();
    let e = place(&mut sim, &c, "forest_extractor_s", poor_at).unwrap_err();
    assert!(e.contains("only 0%"), "{e}");
    // The other resource's building does not belong here.
    let e = place(&mut sim, &c, "ore_extractor_s", spot4).unwrap_err();
    assert!(e.contains("Ore industry area"), "{e}");

    // A building that needs level 2 in an area of level 1.
    let e = place(&mut sim, &c, "forest_extractor_l", spot4).unwrap_err();
    assert!(e.contains("needs area level 2 (it is 1)"), "{e}");
    // Level 2 opens it.
    let id = sim.view().areas.iter().next().unwrap().0;
    sim.world_mut()
        .resource_mut::<cp_district::Areas>()
        .get_mut(id)
        .unwrap()
        .level = 2;
    place(&mut sim, &c, "forest_extractor_l", spot4).unwrap();
}

#[test]
fn every_cargo_has_a_definition_and_the_chains_are_complete() {
    let c = content();
    for kind in CargoKind::ALL {
        assert!(c.cargo(kind).is_some(), "{kind:?}");
    }
    let n = c.buildings().filter(|(_, d)| d.industry.is_some()).count();
    assert_eq!(n, 24);
    for r in ["farm", "forest", "ore", "oil"] {
        for s in [
            "main",
            "extractor_s",
            "extractor_l",
            "processor_a",
            "processor_b",
            "warehouse",
        ] {
            assert!(c.building_id(&format!("{r}_{s}")).is_some(), "{r}_{s}");
        }
    }
}
