//! T-030 acceptance: service buildings are placed beside roads and demolished through the
//! command queue; invalid placements are rejected and change nothing.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingCommand, BuildingGrid, BuildingState, build_ticks};
use cp_content::{BuildingDefId, ContentDb};
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandLog, CommandOutcome};
use cp_net::{Elevation, NetGraph};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::TileOwnership;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

/// One 200 m road at z = 3000 on an owned tile; blocks built.
fn world(content: &Arc<ContentDb>) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(content));
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
    sim.tick_once();
    sim
}

fn run(sim: &mut SimRunner, cmd: BuildingCommand) -> Result<(), String> {
    sim.push_command(SimCommand::Building(cmd), sim.tick());
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

fn pump(c: &ContentDb) -> BuildingDefId {
    c.building_id("wind_turbine").unwrap()
}

/// Centre of a valid cell in row 1 of the first block, column `col`.
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

#[test]
fn place_then_demolish_round_trip() {
    let c = content();
    let mut sim = world(&c);
    let rev = sim.world().resource::<BuildingGrid>().revision();
    let zrev = sim.world().resource::<ZoneGrid>().revision();
    let at = spot(&sim, 5);
    run(
        &mut sim,
        BuildingCommand::Place {
            def: pump(&c),
            pos: at,
            rot: 0.0,
        },
    )
    .unwrap();
    let (id, cells) = {
        let g = sim.world().resource::<BuildingGrid>();
        assert_eq!(g.len(), 1);
        assert_ne!(g.revision(), rev);
        let (id, b) = g.iter().next().unwrap();
        assert_eq!(b.state, BuildingState::UnderConstruction);
        (id, b.lot.cells().collect::<Vec<_>>())
    };
    assert_eq!(cells.len(), 4);
    {
        let z = sim.world().resource::<ZoneGrid>();
        assert_ne!(z.revision(), zrev);
        assert!(cells.iter().all(|&x| z.is_occupied(x) && !z.is_valid(x)));
    }
    assert_eq!(sim.world().resource::<CommandLog<SimCommand>>().len(), 1);

    // Second building on the same spot: blocked, nothing changes.
    let err = run(
        &mut sim,
        BuildingCommand::Place {
            def: pump(&c),
            pos: at,
            rot: 0.0,
        },
    );
    assert!(err.unwrap_err().contains("blocked"));
    assert_eq!(sim.world().resource::<BuildingGrid>().len(), 1);

    // Finishes construction after its `build_hours`.
    let build = build_ticks(c.building(c.building_id("wind_turbine").unwrap()));
    for _ in 0..=build {
        sim.tick_once();
    }
    let g = sim.world().resource::<BuildingGrid>();
    assert_eq!(g.get(id).unwrap().state, BuildingState::Active);

    let rev = g.revision();
    let zrev = sim.world().resource::<ZoneGrid>().revision();
    run(&mut sim, BuildingCommand::Demolish { building: id }).unwrap();
    let g = sim.world().resource::<BuildingGrid>();
    assert!(g.is_empty() && g.revision() != rev);
    let z = sim.world().resource::<ZoneGrid>();
    assert_ne!(z.revision(), zrev);
    assert!(cells.iter().all(|&x| !z.is_occupied(x) && z.is_valid(x)));
    // Gone: demolishing again fails; the freed spot takes a building again.
    assert!(run(&mut sim, BuildingCommand::Demolish { building: id }).is_err());
    run(
        &mut sim,
        BuildingCommand::Place {
            def: pump(&c),
            pos: at,
            rot: 0.0,
        },
    )
    .unwrap();
}

#[test]
fn invalid_placements_are_rejected() {
    let c = content();
    let mut sim = world(&c);
    let place = |pos, def| BuildingCommand::Place { def, pos, rot: 0.0 };
    let s5 = spot(&sim, 5);
    // No road: far from any block.
    let e = run(&mut sim, place(Vec2::new(3100.0, 3300.0), pump(&c))).unwrap_err();
    assert!(e.contains("next to a road"), "{e}");
    // At the last column the lot would run past the end of the block.
    let last = sim
        .world()
        .resource::<ZoneGrid>()
        .blocks()
        .next()
        .unwrap()
        .1
        .cols
        - 1;
    let end = spot(&sim, last);
    let e = run(&mut sim, place(end, pump(&c))).unwrap_err();
    assert!(e.contains("fit"), "{e}");
    // Zoned buildings are not placeable by hand.
    let house = c.building_id("house_low_1").unwrap();
    let e = run(&mut sim, place(s5, house)).unwrap_err();
    assert!(e.contains("by hand"), "{e}");
    // Tile not owned.
    sim.world_mut().insert_resource(TileOwnership::none());
    let e = run(&mut sim, place(s5, pump(&c))).unwrap_err();
    assert!(e.contains("not owned"), "{e}");
    assert!(sim.world().resource::<BuildingGrid>().is_empty());
    assert!(sim.world().resource::<CommandLog<SimCommand>>().is_empty());
}

#[test]
fn overlapping_buildings_are_rejected_but_neighbours_fit() {
    let c = content();
    let mut sim = world(&c);
    let a = spot(&sim, 6);
    let (s7, s9) = (spot(&sim, 7), spot(&sim, 9));
    run(
        &mut sim,
        BuildingCommand::Place {
            def: pump(&c),
            pos: a,
            rot: 0.0,
        },
    )
    .unwrap();
    let e = run(
        &mut sim,
        BuildingCommand::Place {
            def: pump(&c),
            pos: s7,
            rot: 0.0,
        },
    )
    .unwrap_err();
    assert!(e.contains("blocked"), "{e}");
    run(
        &mut sim,
        BuildingCommand::Place {
            def: pump(&c),
            pos: s9,
            rot: 0.0,
        },
    )
    .unwrap();
    assert_eq!(sim.world().resource::<BuildingGrid>().len(), 2);
}

/// Flat dry land at 10 m with a lake (ground at 0 m, sea level 5 m) around x 3080..3140.
fn lake_terrain() -> cp_terrain::Terrain {
    use cp_terrain::{GRID, MapMeta, SPACING, Terrain};
    let mut height = vec![655u16; GRID * GRID]; // about 10 m
    for iz in 280..320 {
        for ix in 308..314 {
            height[iz * GRID + ix] = 0;
        }
    }
    let meta = MapMeta {
        name: "lake".into(),
        sea_level: 5.0,
        start_tile: (1, 1),
        external_connections: vec![],
    };
    let _ = SPACING;
    Terrain::from_samples(meta, &height, &vec![0u16; GRID * GRID]).unwrap()
}

#[test]
fn a_pump_needs_water_and_other_buildings_cannot_stand_in_it() {
    let c = content();
    let mut sim = world(&c);
    sim.world_mut()
        .insert_resource(cp_terrain::SimTerrain(Arc::new(lake_terrain())));
    let cols = sim
        .world()
        .resource::<ZoneGrid>()
        .blocks()
        .next()
        .unwrap()
        .1
        .cols;
    let (dry, wet) = (spot(&sim, 3), spot(&sim, 13));
    assert!(cols > 20);
    let pump = c.building_id("water_pump_station").unwrap();
    let turbine = c.building_id("wind_turbine").unwrap();
    let place = |pos, def| BuildingCommand::Place { def, pos, rot: 0.0 };
    let e = run(&mut sim, place(dry, pump)).unwrap_err();
    assert!(e.contains("on or beside water"), "{e}");
    let e = run(&mut sim, place(wet, turbine)).unwrap_err();
    assert!(e.contains("on water"), "{e}");
    run(&mut sim, place(wet, pump)).unwrap();
    run(&mut sim, place(dry, turbine)).unwrap();
}

#[test]
fn non_finite_positions_and_rotations_are_rejected() {
    let c = content();
    let mut sim = world(&c);
    let at = spot(&sim, 5);
    let def = c.building_id("wind_turbine").unwrap();
    for (pos, rot) in [
        (at, f32::NAN),
        (at, f32::INFINITY),
        (Vec2::new(f32::NAN, at.y), 0.0),
        (Vec2::new(at.x, f32::INFINITY), 0.0),
        (Vec2::new(1e30, at.y), 0.0),
    ] {
        let e = run(&mut sim, BuildingCommand::Place { def, pos, rot }).unwrap_err();
        assert!(e.contains("invalid"), "{e}");
    }
    assert!(sim.world().resource::<BuildingGrid>().is_empty());
}
